use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::config::{self, Profile};
use crate::credentials;
use crate::oauth_refresh;
use crate::probe::{self, ProbeResult};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileState {
    Ok(ProbeResult),
    NeedsRelogin { reason: String },
    NotProMax,
    Error { message: String },
    Stale { last: ProbeResult, message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileUpdate {
    pub profile_id: String,
    pub profile_name: String,
    pub email: Option<String>,
    pub state: ProfileState,
    pub updated_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct UsageStore {
    pub updates: Mutex<Vec<ProfileUpdate>>,
}

pub type UsageStoreHandle = Arc<UsageStore>;

pub fn store() -> UsageStoreHandle {
    Arc::new(UsageStore::default())
}

pub fn spawn(app: AppHandle, store: UsageStoreHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let interval = config::load()
                .map(|c| c.poll_interval_s.max(10))
                .unwrap_or(60);

            tick(&app, &store).await;
            tokio::time::sleep(Duration::from_secs(interval)).await;
        }
    });
}

pub async fn tick(app: &AppHandle, store: &UsageStoreHandle) {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[poller] config load failed: {e:#}");
            return;
        }
    };

    let mut latest: Vec<ProfileUpdate> = vec![];
    for profile in cfg.profiles.iter().filter(|p| p.enabled) {
        let prior = store
            .updates
            .lock()
            .await
            .iter()
            .find(|u| u.profile_id == profile.id)
            .cloned();
        let update = poll_one(profile, prior).await;
        let _ = app.emit("usage-updated", &update);
        latest.push(update);

        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    *store.updates.lock().await = latest.clone();
    crate::tray::update_tray_title(app, &latest);
}

async fn poll_one(profile: &Profile, prior: Option<ProfileUpdate>) -> ProfileUpdate {
    let now = Utc::now();
    let prior_email = prior.as_ref().and_then(|p| p.email.clone());

    // Resolve email from .claude.json (more reliable than JWT decode for opaque tokens).
    let resolved_email = if profile.use_macos_keychain {
        credentials::find_active_email()
    } else {
        let dir = config::resolve_config_dir(profile);
        credentials::email_from_claude_json(&dir)
    }
    .or(prior_email.clone());

    let (state, email) = match load_token(profile).await {
        Ok(token) => {
            let state = run_probe(profile, token, prior.as_ref()).await;
            (state, resolved_email)
        }
        Err(e) => (
            ProfileState::NeedsRelogin { reason: e.to_string() },
            resolved_email,
        ),
    };

    ProfileUpdate {
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        email,
        state,
        updated_at: now,
    }
}

async fn load_token(profile: &Profile) -> Result<String> {
    #[cfg(target_os = "macos")]
    if profile.use_macos_keychain {
        return Ok(credentials::read_macos_keychain()?.access_token);
    }
    let dir = config::resolve_config_dir(profile);
    let creds = credentials::read(&dir).map_err(|_| {
        let path = credentials::credentials_path(&dir);
        if !path.exists() {
            anyhow::anyhow!(
                "login required — run in terminal:\nCLAUDE_CONFIG_DIR={} claude /login",
                dir.display()
            )
        } else {
            anyhow::anyhow!("cannot read {}", path.display())
        }
    })?;
    Ok(creds.access_token)
}

async fn run_probe(
    profile: &Profile,
    token: String,
    prior: Option<&ProfileUpdate>,
) -> ProfileState {
    match probe::run(&token).await {
        Ok(result) => ProfileState::Ok(result),
        Err(probe::ProbeError::NeedsRefresh) => attempt_refresh(profile).await,
        Err(probe::ProbeError::NotProMax) => ProfileState::NotProMax,
        Err(other) => stale_or_error(prior, other.to_string()),
    }
}

async fn attempt_refresh(profile: &Profile) -> ProfileState {
    // Keychain profiles can't do file-based refresh; require manual re-login.
    if profile.use_macos_keychain {
        return ProfileState::NeedsRelogin {
            reason: "token expired — run `claude /login` to refresh".to_string(),
        };
    }

    let dir = config::resolve_config_dir(profile);
    let current = match credentials::read(&dir) {
        Ok(c) => c,
        Err(e) => {
            return ProfileState::NeedsRelogin {
                reason: format!("cannot read credentials: {e}"),
            }
        }
    };

    match oauth_refresh::refresh(&current).await {
        Ok(updated) => {
            if let Err(e) = credentials::write(&dir, &updated) {
                return ProfileState::Error {
                    message: format!("refreshed but failed to persist: {e}"),
                };
            }
            match probe::run(&updated.access_token).await {
                Ok(result) => ProfileState::Ok(result),
                Err(probe::ProbeError::NotProMax) => ProfileState::NotProMax,
                Err(e) => ProfileState::Error {
                    message: e.to_string(),
                },
            }
        }
        Err(e) => ProfileState::NeedsRelogin {
            reason: e.to_string(),
        },
    }
}

fn stale_or_error(prior: Option<&ProfileUpdate>, message: String) -> ProfileState {
    if let Some(p) = prior {
        if let ProfileState::Ok(result) = &p.state {
            return ProfileState::Stale {
                last: result.clone(),
                message,
            };
        }
    }
    ProfileState::Error { message }
}


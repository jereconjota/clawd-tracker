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
    let dir = config::resolve_config_dir(profile);

    // macOS: Claude Code stores credentials in Keychain (one entry per
    // CLAUDE_CONFIG_DIR, keyed by sha256(abs_path)[:8]). Read that directly.
    #[cfg(target_os = "macos")]
    {
        let dir_opt = keychain_dir_for(profile, &dir);
        if let Ok(creds) = credentials::read_macos_keychain(dir_opt.as_deref()) {
            return Ok(creds.access_token);
        }
        // fall through to file (Ubuntu credentials synced to macOS, manual files, etc.)
    }

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

#[cfg(target_os = "macos")]
fn keychain_dir_for(profile: &Profile, resolved: &std::path::Path) -> Option<std::path::PathBuf> {
    if profile.config_dir.is_empty() {
        None
    } else {
        Some(resolved.to_path_buf())
    }
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
    let dir = config::resolve_config_dir(profile);

    // Read current credentials from wherever they live (Keychain on macOS,
    // file on Linux — try Keychain first on macOS, fall back to file).
    let current = match read_credentials(profile, &dir) {
        Ok(c) => c,
        Err(e) => {
            return ProfileState::NeedsRelogin {
                reason: format!("cannot read credentials: {e}"),
            }
        }
    };

    match oauth_refresh::refresh(&current).await {
        Ok(updated) => {
            if let Err(e) = persist_credentials(profile, &dir, &updated) {
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

fn read_credentials(
    profile: &Profile,
    dir: &std::path::Path,
) -> Result<credentials::OauthCredentials> {
    #[cfg(target_os = "macos")]
    {
        let dir_opt = keychain_dir_for(profile, dir);
        if let Ok(c) = credentials::read_macos_keychain(dir_opt.as_deref()) {
            return Ok(c);
        }
    }
    let _ = profile; // unused on non-macos
    credentials::read(dir)
}

fn persist_credentials(
    profile: &Profile,
    dir: &std::path::Path,
    creds: &credentials::OauthCredentials,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // If we read from Keychain, write back there. Otherwise (file-based
        // on macOS — rare, only if user synced from Ubuntu), update the file.
        let dir_opt = keychain_dir_for(profile, dir);
        if credentials::read_macos_keychain(dir_opt.as_deref()).is_ok() {
            return credentials::write_macos_keychain(dir_opt.as_deref(), creds);
        }
    }
    let _ = profile;
    credentials::write(dir, creds)
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


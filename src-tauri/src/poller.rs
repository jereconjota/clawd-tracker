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
    pub tray_label: Option<String>,
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

    let visible_ids: std::collections::HashSet<String> = cfg
        .profiles
        .iter()
        .filter(|p| p.show_in_tray)
        .map(|p| p.id.clone())
        .collect();

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
    crate::tray::update_tray_title(app, &latest, &visible_ids);
}

async fn poll_one(profile: &Profile, prior: Option<ProfileUpdate>) -> ProfileUpdate {
    let now = Utc::now();
    let prior_email = prior.as_ref().and_then(|p| p.email.clone());
    let resolved_email = resolve_email(profile).or(prior_email);

    let (state, email) = match load_credentials(profile) {
        Ok((creds, source)) => {
            let state = probe_with(creds, source, prior.as_ref()).await;
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
        tray_label: profile.tray_label.clone(),
        email,
        state,
        updated_at: now,
    }
}

fn resolve_email(profile: &Profile) -> Option<String> {
    if profile.config_dir.is_empty() {
        credentials::find_active_email()
    } else {
        credentials::email_from_claude_json(&config::resolve_config_dir(profile))
    }
}

#[derive(Debug, Clone)]
enum CredSource {
    #[cfg(target_os = "macos")]
    Keychain(Option<std::path::PathBuf>),
    File(std::path::PathBuf),
}

fn load_credentials(
    profile: &Profile,
) -> Result<(credentials::OauthCredentials, CredSource)> {
    let dir = config::resolve_config_dir(profile);

    #[cfg(target_os = "macos")]
    {
        let keychain_dir = if profile.config_dir.is_empty() {
            None
        } else {
            Some(dir.clone())
        };
        if let Ok(c) = credentials::read_macos_keychain(keychain_dir.as_deref()) {
            return Ok((c, CredSource::Keychain(keychain_dir)));
        }
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
    Ok((creds, CredSource::File(dir)))
}

fn persist_credentials(
    source: &CredSource,
    creds: &credentials::OauthCredentials,
) -> Result<()> {
    match source {
        #[cfg(target_os = "macos")]
        CredSource::Keychain(dir) => credentials::write_macos_keychain(dir.as_deref(), creds),
        CredSource::File(dir) => credentials::write(dir, creds),
    }
}

async fn probe_with(
    creds: credentials::OauthCredentials,
    source: CredSource,
    prior: Option<&ProfileUpdate>,
) -> ProfileState {
    match probe::run(&creds.access_token).await {
        Ok(result) => ProfileState::Ok(result),
        Err(probe::ProbeError::NeedsRefresh) => attempt_refresh(creds, source).await,
        Err(probe::ProbeError::NotProMax) => ProfileState::NotProMax,
        Err(other) => stale_or_error(prior, other.to_string()),
    }
}

async fn attempt_refresh(
    current: credentials::OauthCredentials,
    source: CredSource,
) -> ProfileState {
    match oauth_refresh::refresh(&current).await {
        Ok(updated) => {
            if let Err(e) = persist_credentials(&source, &updated) {
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
        // Carry the last known reading forward whether the prior poll was a
        // fresh `Ok` or already `Stale`. Without the `Stale` arm a transient
        // error would escalate Ok -> Stale -> Error, dropping the account from
        // the tray on the second failed poll.
        let last = match &p.state {
            ProfileState::Ok(result) => Some(result.clone()),
            ProfileState::Stale { last, .. } => Some(last.clone()),
            _ => None,
        };
        if let Some(last) = last {
            return ProfileState::Stale { last, message };
        }
    }
    ProfileState::Error { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_result(sess_pct: f32) -> ProbeResult {
        ProbeResult {
            sess_pct,
            sess_reset: None,
            week_pct: 0.0,
            week_reset: None,
            status: "allowed".into(),
            opus_pct: None,
            opus_reset: None,
        }
    }

    fn update(state: ProfileState) -> ProfileUpdate {
        ProfileUpdate {
            profile_id: "p".into(),
            profile_name: "p".into(),
            tray_label: None,
            email: None,
            state,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn ok_prior_becomes_stale() {
        let prior = update(ProfileState::Ok(probe_result(1.0)));
        match stale_or_error(Some(&prior), "boom".into()) {
            ProfileState::Stale { last, .. } => assert_eq!(last.sess_pct, 1.0),
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn stale_prior_stays_stale_not_error() {
        // The regression: a second failed poll used to escalate Stale -> Error,
        // which dropped the account from the tray. It must stay Stale and keep
        // the last reading.
        let prior = update(ProfileState::Stale {
            last: probe_result(1.0),
            message: "first failure".into(),
        });
        match stale_or_error(Some(&prior), "second failure".into()) {
            ProfileState::Stale { last, .. } => assert_eq!(last.sess_pct, 1.0),
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn no_prior_is_error() {
        assert!(matches!(
            stale_or_error(None, "boom".into()),
            ProfileState::Error { .. }
        ));
    }
}


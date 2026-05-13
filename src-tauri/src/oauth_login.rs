use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use reqwest::header;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::credentials::OauthCredentials;
use crate::poller::UsageStoreHandle;

// Canonical Claude Code OAuth flow (manual-paste / out-of-band).
// The console redirect_uri renders the code on a page so the user can copy it
// and paste back into the app — there is no localhost listener for this client.
const AUTH_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const SCOPES: &str = "org:create_api_key user:profile user:inference";
const SESSION_TTL_SECS: i64 = 600;

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("building reqwest client")
    })
}

#[derive(Debug, Serialize, Clone)]
pub struct OAuthDoneEvent {
    pub profile_id: String,
    pub success: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingSession {
    pub code_verifier: String,
    pub state: String,
    pub created_at: i64,
}

pub struct StartedLogin {
    pub auth_url: String,
    pub session: PendingSession,
}

pub fn start_login_session() -> Result<StartedLogin> {
    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);

    let hash = Sha256::digest(code_verifier.as_bytes());
    let code_challenge = URL_SAFE_NO_PAD.encode(hash);

    let mut state_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut state_bytes);
    let state = URL_SAFE_NO_PAD.encode(state_bytes);

    // Order/encoding matches the canonical Claude Code login flow.
    let auth_url = format!(
        "{}?code=true&client_id={}&response_type=code&redirect_uri={}\
         &scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        AUTH_URL,
        CLIENT_ID,
        urlencoding::encode(REDIRECT_URI),
        urlencoding::encode(SCOPES),
        code_challenge,
        state,
    );

    Ok(StartedLogin {
        auth_url,
        session: PendingSession {
            code_verifier,
            state,
            created_at: chrono::Utc::now().timestamp(),
        },
    })
}

pub async fn complete_with_pasted_code(
    app: AppHandle,
    profile_id: String,
    session: PendingSession,
    pasted: String,
    store: UsageStoreHandle,
) -> Result<(), String> {
    let result = do_complete(&app, &profile_id, &session, &pasted, &store).await;
    match &result {
        Ok(()) => {
            let _ = app.emit(
                "oauth-login-done",
                OAuthDoneEvent {
                    profile_id: profile_id.clone(),
                    success: true,
                    error: None,
                },
            );
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            let _ = app.emit(
                "oauth-login-done",
                OAuthDoneEvent {
                    profile_id: profile_id.clone(),
                    success: false,
                    error: Some(msg.clone()),
                },
            );
            Err(msg)
        }
    }
}

async fn do_complete(
    app: &AppHandle,
    profile_id: &str,
    session: &PendingSession,
    pasted: &str,
    store: &UsageStoreHandle,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    if now - session.created_at > SESSION_TTL_SECS {
        return Err(anyhow!("login session expired — start over"));
    }

    let (code, state_from_paste) = parse_pasted(pasted)?;

    if let Some(s) = state_from_paste {
        if s != session.state {
            return Err(anyhow!("state mismatch — possible CSRF, start over"));
        }
    }

    let creds = exchange_code(&code, &session.code_verifier, &session.state).await?;

    let cfg = crate::config::load()?;
    let profile = cfg
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| anyhow!("profile '{}' not found", profile_id))?;
    let dir = crate::config::resolve_config_dir(profile);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    crate::credentials::write(&dir, &creds)?;

    crate::poller::tick(app, store).await;
    Ok(())
}

/// Console renders the code as `<code>#<state>` (or sometimes `<code>&state=<state>`).
/// Strip URL noise and pull both halves.
fn parse_pasted(input: &str) -> Result<(String, Option<String>)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("paste the code from the browser page"));
    }
    let (code_part, state_part) = if let Some((c, rest)) = trimmed.split_once('#') {
        let state = rest
            .split('&')
            .find_map(|kv| kv.strip_prefix("state="))
            .map(|s| s.to_string())
            .or_else(|| Some(rest.to_string()))
            .filter(|s| !s.is_empty());
        (c.to_string(), state)
    } else if let Some((c, rest)) = trimmed.split_once('&') {
        let state = rest
            .split('&')
            .find_map(|kv| kv.strip_prefix("state="))
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        (c.to_string(), state)
    } else {
        (trimmed.to_string(), None)
    };
    if code_part.is_empty() {
        return Err(anyhow!("could not find code in pasted value"));
    }
    Ok((code_part, state_part))
}

#[derive(Debug, Serialize)]
struct CodeExchangeRequest<'a> {
    grant_type: &'a str,
    client_id: &'a str,
    code: &'a str,
    redirect_uri: &'a str,
    code_verifier: &'a str,
    state: &'a str,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", default)]
    rate_limit_tier: Option<String>,
}

async fn exchange_code(
    code: &str,
    code_verifier: &str,
    state: &str,
) -> Result<OauthCredentials> {
    let body = CodeExchangeRequest {
        grant_type: "authorization_code",
        client_id: CLIENT_ID,
        code,
        redirect_uri: REDIRECT_URI,
        code_verifier,
        state,
    };

    let resp = http_client()
        .post(TOKEN_URL)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json, text/plain, */*")
        .header(header::USER_AGENT, "claude-code/2.1.5")
        .header(header::REFERER, "https://claude.ai/")
        .header(header::ORIGIN, "https://claude.ai")
        .json(&body)
        .send()
        .await
        .context("exchanging authorization code")?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(anyhow!(
                "Anthropic rate-limited the OAuth endpoint (HTTP 429). Wait \
                 ~10 minutes before trying again — repeated login attempts \
                 trigger a temporary cooldown."
            ));
        }
        return Err(anyhow!("token exchange failed ({status}): {text}"));
    }

    let parsed: TokenResponse = serde_json::from_str(&text)
        .with_context(|| format!("parsing token response: {text}"))?;

    let expires_at = parsed
        .expires_in
        .map(|s| chrono::Utc::now().timestamp_millis() + s * 1000);

    let scopes: Vec<String> = parsed
        .scope
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();

    Ok(OauthCredentials {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token,
        expires_at,
        scopes,
        subscription_type: parsed.subscription_type,
        rate_limit_tier: parsed.rate_limit_tier,
    })
}

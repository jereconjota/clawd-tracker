use anyhow::{anyhow, Context, Result};
use reqwest::header;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

use crate::credentials::OauthCredentials;

const TOKEN_ENDPOINT: &str = "https://console.anthropic.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("building reqwest client")
    })
}

#[derive(Debug, Serialize)]
struct RefreshRequest<'a> {
    grant_type: &'a str,
    refresh_token: &'a str,
    client_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

pub async fn refresh(current: &OauthCredentials) -> Result<OauthCredentials> {
    let refresh_token = current
        .refresh_token
        .as_ref()
        .ok_or_else(|| anyhow!("no refresh_token stored — re-login required"))?;

    let req = RefreshRequest {
        grant_type: "refresh_token",
        refresh_token,
        client_id: CLIENT_ID,
    };

    let resp = http_client()
        .post(TOKEN_ENDPOINT)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ACCEPT, "application/json")
        .header(header::USER_AGENT, "claude-code/2.1.5")
        .json(&req)
        .send()
        .await
        .context("calling token endpoint")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(anyhow!("token endpoint returned {status}: {body}"));
    }

    let parsed: RefreshResponse = serde_json::from_str(&body)
        .with_context(|| format!("parsing token response: {body}"))?;

    let expires_at = parsed.expires_in.map(|s| {
        chrono::Utc::now().timestamp_millis() + (s * 1000)
    });

    Ok(OauthCredentials {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token.or_else(|| current.refresh_token.clone()),
        expires_at,
        scopes: current.scopes.clone(),
        subscription_type: current.subscription_type.clone(),
        rate_limit_tier: current.rate_limit_tier.clone(),
    })
}

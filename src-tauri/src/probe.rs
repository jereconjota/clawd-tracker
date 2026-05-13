use chrono::{DateTime, TimeZone, Utc};
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const PROBE_MODEL: &str = "claude-haiku-4-5-20251001";
const USER_AGENT: &str = "claude-code/2.1.5";

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("building reqwest client")
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub sess_pct: f32,
    pub sess_reset: Option<DateTime<Utc>>,
    pub week_pct: f32,
    pub week_reset: Option<DateTime<Utc>>,
    pub status: String,
    pub opus_pct: Option<f32>,
    pub opus_reset: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeError {
    NeedsRefresh,
    NotProMax,
    Network { message: String },
    Server { status: u16, body: String },
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::NeedsRefresh => write!(f, "needs refresh"),
            ProbeError::NotProMax => write!(
                f,
                "no unified-5h headers — account is not Pro/Max or scope is wrong"
            ),
            ProbeError::Network { message } => write!(f, "network: {message}"),
            ProbeError::Server { status, body } => write!(f, "server {status}: {body}"),
        }
    }
}

impl std::error::Error for ProbeError {}

pub async fn run(access_token: &str) -> Result<ProbeResult, ProbeError> {
    let body = serde_json::json!({
        "model": PROBE_MODEL,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}],
    });

    let resp = http_client()
        .post(ENDPOINT)
        .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(header::USER_AGENT, USER_AGENT)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| ProbeError::Network { message: e.to_string() })?;

    let status = resp.status();
    let headers = resp.headers().clone();

    if status == StatusCode::UNAUTHORIZED {
        return Err(ProbeError::NeedsRefresh);
    }

    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ProbeError::Server {
            status: status.as_u16(),
            body,
        });
    }

    let sess_pct = read_pct(&headers, "anthropic-ratelimit-unified-5h-utilization");
    let week_pct = read_pct(&headers, "anthropic-ratelimit-unified-7d-utilization");

    if sess_pct.is_none() && week_pct.is_none() {
        return Err(ProbeError::NotProMax);
    }

    let sess_reset = read_reset(&headers, "anthropic-ratelimit-unified-5h-reset");
    let week_reset = read_reset(&headers, "anthropic-ratelimit-unified-7d-reset");
    let opus_pct = read_pct(&headers, "anthropic-ratelimit-unified-7d-opus-utilization");
    let opus_reset = read_reset(&headers, "anthropic-ratelimit-unified-7d-opus-reset");
    let status_header = headers
        .get("anthropic-ratelimit-unified-5h-status")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    Ok(ProbeResult {
        sess_pct: sess_pct.unwrap_or(0.0),
        sess_reset,
        week_pct: week_pct.unwrap_or(0.0),
        week_reset,
        status: status_header,
        opus_pct,
        opus_reset,
    })
}

fn read_pct(headers: &header::HeaderMap, name: &str) -> Option<f32> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f32>().ok())
}

fn read_reset(headers: &header::HeaderMap, name: &str) -> Option<DateTime<Utc>> {
    let raw = headers.get(name)?.to_str().ok()?.trim().to_string();
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&raw) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(secs) = raw.parse::<i64>() {
        return Utc.timestamp_opt(secs, 0).single();
    }
    None
}


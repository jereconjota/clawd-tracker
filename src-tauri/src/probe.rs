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

    match classify(status) {
        Classification::NeedsRefresh => Err(ProbeError::NeedsRefresh),
        Classification::ServerError => {
            let body = resp.text().await.unwrap_or_default();
            Err(ProbeError::Server {
                status: status.as_u16(),
                body,
            })
        }
        Classification::Read { rate_limited } => build_result(&headers, rate_limited),
    }
}

/// What to do with a probe response based purely on its status code.
#[derive(Debug, PartialEq)]
enum Classification {
    NeedsRefresh,
    ServerError,
    Read { rate_limited: bool },
}

/// A 429 means the account is rate-limited — typically because a window has hit
/// 100%. The `anthropic-ratelimit-*` headers are still present on that response,
/// so we read them like a normal success instead of erroring out. That keeps
/// maxed accounts visible (at 100%) with their reset time, rather than dropping
/// them from the tray. Other non-success statuses (5xx, etc.) stay server errors.
fn classify(status: StatusCode) -> Classification {
    if status == StatusCode::UNAUTHORIZED {
        return Classification::NeedsRefresh;
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Classification::Read { rate_limited: true };
    }
    if !status.is_success() {
        return Classification::ServerError;
    }
    Classification::Read { rate_limited: false }
}

/// Build a [`ProbeResult`] from the rate-limit headers. `rate_limited` is true
/// when the response was a 429.
fn build_result(headers: &header::HeaderMap, rate_limited: bool) -> Result<ProbeResult, ProbeError> {
    let sess_pct = read_pct(headers, "anthropic-ratelimit-unified-5h-utilization");
    let week_pct = read_pct(headers, "anthropic-ratelimit-unified-7d-utilization");

    // Missing headers normally mean a non-Pro/Max account. On a 429 the limit
    // was definitely hit, so treat absent headers as "maxed" rather than as a
    // non-Pro/Max account.
    if sess_pct.is_none() && week_pct.is_none() && !rate_limited {
        return Err(ProbeError::NotProMax);
    }

    let sess_reset = read_reset(headers, "anthropic-ratelimit-unified-5h-reset");
    let week_reset = read_reset(headers, "anthropic-ratelimit-unified-7d-reset");
    let opus_pct = read_pct(headers, "anthropic-ratelimit-unified-7d-opus-utilization");
    let opus_reset = read_reset(headers, "anthropic-ratelimit-unified-7d-opus-reset");
    let status_header = headers
        .get("anthropic-ratelimit-unified-5h-status")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    // When rate-limited, a window with no utilization header is the one that's
    // maxed (the API omits the header for the exhausted window on some
    // responses), so fall back to 100% rather than 0%.
    let default_pct = if rate_limited { 1.0 } else { 0.0 };

    Ok(ProbeResult {
        sess_pct: sess_pct.unwrap_or(default_pct),
        sess_reset,
        week_pct: week_pct.unwrap_or(default_pct),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> header::HeaderMap {
        let mut h = header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                header::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn classify_routes_status_codes() {
        assert_eq!(classify(StatusCode::OK), Classification::Read { rate_limited: false });
        assert_eq!(classify(StatusCode::UNAUTHORIZED), Classification::NeedsRefresh);
        assert_eq!(classify(StatusCode::TOO_MANY_REQUESTS), Classification::Read { rate_limited: true });
        assert_eq!(classify(StatusCode::INTERNAL_SERVER_ERROR), Classification::ServerError);
        assert_eq!(classify(StatusCode::BAD_GATEWAY), Classification::ServerError);
    }

    #[test]
    fn maxed_account_429_reports_full_usage_and_reset() {
        // The exact scenario from the bug: account hit 100%, API returns 429 but
        // still includes the rate-limit headers.
        let h = headers(&[
            ("anthropic-ratelimit-unified-5h-utilization", "1"),
            ("anthropic-ratelimit-unified-5h-reset", "1717290000"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.42"),
        ]);
        let r = build_result(&h, true).expect("429 with headers should parse");
        assert_eq!(r.sess_pct, 1.0);
        assert_eq!(r.week_pct, 0.42);
        assert!(r.sess_reset.is_some(), "reset time should be parsed so the popover shows a countdown");
    }

    #[test]
    fn maxed_account_429_without_header_defaults_to_full() {
        // Some 429 responses omit the header for the exhausted window.
        let r = build_result(&headers(&[]), true).expect("429 should not be NotProMax");
        assert_eq!(r.sess_pct, 1.0);
        assert_eq!(r.week_pct, 1.0);
    }

    #[test]
    fn success_without_headers_is_not_pro_max() {
        // A normal 200 with no rate-limit headers is still a non-Pro/Max account.
        let err = build_result(&headers(&[]), false).unwrap_err();
        assert!(matches!(err, ProbeError::NotProMax));
    }

    #[test]
    fn normal_success_reads_actual_values() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.3"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.1"),
        ]);
        let r = build_result(&h, false).unwrap();
        assert_eq!(r.sess_pct, 0.3);
        assert_eq!(r.week_pct, 0.1);
    }
}


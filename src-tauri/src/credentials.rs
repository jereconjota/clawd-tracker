use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthCredentials {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(rename = "subscriptionType", default)]
    pub subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", default)]
    pub rate_limit_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: OauthCredentials,
}

pub fn expand(path: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(path).into_owned())
}

pub fn credentials_path(config_dir: &Path) -> PathBuf {
    config_dir.join(".credentials.json")
}

pub fn read(config_dir: &Path) -> Result<OauthCredentials> {
    let path = credentials_path(config_dir);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let parsed: CredentialsFile = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(parsed.claude_ai_oauth)
}

pub fn write(config_dir: &Path, creds: &OauthCredentials) -> Result<()> {
    let path = credentials_path(config_dir);
    // Merge into existing file so unrelated fields (mcpOAuth, etc.) are preserved.
    let mut root: serde_json::Value = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    root["claudeAiOauth"] = serde_json::to_value(creds)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming {}", path.display()))?;
    Ok(())
}

/// Build the Keychain service name for a given config_dir.
///
/// Claude Code stores one Keychain entry per `CLAUDE_CONFIG_DIR`:
/// - `Claude Code-credentials`              for the default `~/.claude`
/// - `Claude Code-credentials-<hash8>`      for any custom dir, where
///                                          `hash8 = sha256(abs_path)[:8]`
#[cfg(target_os = "macos")]
fn keychain_service(config_dir: Option<&Path>) -> String {
    match config_dir {
        Some(dir) => {
            let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            let hash = sha256_hex_prefix(abs.to_string_lossy().as_bytes(), 8);
            format!("Claude Code-credentials-{hash}")
        }
        None => "Claude Code-credentials".to_string(),
    }
}

#[cfg(target_os = "macos")]
fn sha256_hex_prefix(bytes: &[u8], n_chars: usize) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let full = digest.iter().fold(String::new(), |mut acc, b| {
        acc.push_str(&format!("{b:02x}"));
        acc
    });
    full.chars().take(n_chars).collect()
}

#[cfg(target_os = "macos")]
pub fn read_macos_keychain(config_dir: Option<&Path>) -> Result<OauthCredentials> {
    use std::process::Command;
    let service = keychain_service(config_dir);
    let output = Command::new("security")
        .args(["find-generic-password", "-s", &service, "-w"])
        .output()
        .context("invoking `security` to read Keychain")?;
    if !output.status.success() {
        return Err(anyhow!("Keychain entry '{service}' not found"));
    }
    let raw = String::from_utf8(output.stdout)?.trim().to_string();
    let parsed: CredentialsFile = serde_json::from_str(&raw)
        .context("parsing Keychain payload as Claude credentials JSON")?;
    Ok(parsed.claude_ai_oauth)
}

#[cfg(target_os = "macos")]
pub fn write_macos_keychain(config_dir: Option<&Path>, creds: &OauthCredentials) -> Result<()> {
    use std::process::Command;
    let service = keychain_service(config_dir);
    let user = std::env::var("USER").unwrap_or_else(|_| "claude".to_string());
    let payload = serde_json::to_string(&CredentialsFile {
        claude_ai_oauth: creds.clone(),
    })?;
    let status = Command::new("security")
        .args([
            "add-generic-password",
            "-U", // update if exists
            "-s",
            &service,
            "-a",
            &user,
            "-w",
            &payload,
        ])
        .status()
        .context("invoking `security` to write Keychain")?;
    if !status.success() {
        return Err(anyhow!("failed to update Keychain entry '{service}'"));
    }
    Ok(())
}

/// List config directories that have a corresponding Keychain entry on macOS,
/// regardless of whether `.credentials.json` exists. Returns the directory
/// paths under `~/.claude*` that have a slot in Keychain.
#[cfg(target_os = "macos")]
pub fn auto_detect_keychain_dirs() -> Vec<PathBuf> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let mut found = vec![];
    let entries = match std::fs::read_dir(&home) {
        Ok(e) => e,
        Err(_) => return found,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(".claude") {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if read_macos_keychain(Some(&path)).is_ok() {
            found.push(path);
        }
    }
    found
}

pub fn auto_detect_dirs() -> Vec<PathBuf> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    let mut found = vec![];
    let entries = match std::fs::read_dir(&home) {
        Ok(e) => e,
        Err(_) => return found,
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(".claude") {
            continue;
        }
        let path = entry.path();
        if path.is_dir() && credentials_path(&path).exists() {
            found.push(path);
        }
    }
    found
}

/// Read the email address from `<config_dir>/.claude.json` → oauthAccount.emailAddress.
pub fn email_from_claude_json(config_dir: &Path) -> Option<String> {
    let path = config_dir.join(".claude.json");
    let raw = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).ok()?;
    json.get("oauthAccount")?
        .get("emailAddress")?
        .as_str()
        .map(String::from)
}

/// Scan all ~/.claude* directories and return the first email found in any .claude.json.
/// Used for macOS Keychain profiles that don't have a specific config_dir.
pub fn find_active_email() -> Option<String> {
    let home = dirs::home_dir()?;
    let mut dirs_with_mtime: Vec<(std::path::PathBuf, std::time::SystemTime)> = vec![];
    for entry in std::fs::read_dir(&home).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(".claude") { continue; }
        let path = entry.path();
        if !path.is_dir() { continue; }
        let json_path = path.join(".claude.json");
        if let Ok(meta) = std::fs::metadata(&json_path) {
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            dirs_with_mtime.push((path, mtime));
        }
    }
    // Pick the most recently written .claude.json (most active account).
    dirs_with_mtime.sort_by(|a, b| b.1.cmp(&a.1));
    for (dir, _) in &dirs_with_mtime {
        if let Some(email) = email_from_claude_json(dir) {
            return Some(email);
        }
    }
    None
}

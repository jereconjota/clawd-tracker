use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub config_dir: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub show_in_tray: bool,
    #[serde(default)]
    pub use_macos_keychain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tray_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default = "default_interval")]
    pub poll_interval_s: u64,
    #[serde(default = "default_true")]
    pub autostart: bool,
}

fn default_true() -> bool {
    true
}

fn default_interval() -> u64 {
    60
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: vec![],
            poll_interval_s: default_interval(),
            autostart: true,
        }
    }
}

pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("no config dir found for this OS"))?;
    let dir = base.join("clawd-tracker");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.json"))
}

pub fn load() -> Result<Config> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(seed_default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let cfg: Config = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(cfg)
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path()?;
    let serialized = serde_json::to_string_pretty(cfg)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serialized)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

pub fn seed_default() -> Config {
    let mut cfg = Config::default();

    for dir in crate::credentials::auto_detect_dirs() {
        cfg.profiles.push(profile_for_dir(&dir));
    }

    #[cfg(target_os = "macos")]
    {
        for dir in crate::credentials::auto_detect_keychain_dirs() {
            let already = cfg
                .profiles
                .iter()
                .any(|p| crate::credentials::expand(&p.config_dir) == dir);
            if already {
                continue;
            }
            cfg.profiles.push(profile_for_dir(&dir));
        }

        // Default Keychain entry only if nothing else turned up; falls back to
        // the slot Claude Code uses when CLAUDE_CONFIG_DIR is unset.
        if cfg.profiles.is_empty() && crate::credentials::read_macos_keychain(None).is_ok() {
            let email = crate::credentials::find_active_email();
            let name = email.unwrap_or_else(|| "My Account".to_string());
            cfg.profiles.push(Profile {
                id: "keychain".to_string(),
                name,
                config_dir: String::new(),
                enabled: true,
                show_in_tray: true,
                use_macos_keychain: true,
                tray_label: None,
            });
        }
    }

    cfg
}

fn profile_for_dir(dir: &std::path::Path) -> Profile {
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "claude".into());
    let pretty = name.trim_start_matches('.').replace("claude-", "");
    let pretty = if pretty.is_empty() {
        "default".to_string()
    } else {
        pretty
    };
    Profile {
        id: name.clone(),
        name: pretty,
        config_dir: dir.to_string_lossy().to_string(),
        enabled: true,
        show_in_tray: true,
        use_macos_keychain: false,
        tray_label: None,
    }
}

pub fn resolve_config_dir(profile: &Profile) -> PathBuf {
    crate::credentials::expand(&profile.config_dir)
}

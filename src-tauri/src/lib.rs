mod config;
mod credentials;
mod oauth_login;
mod oauth_refresh;
mod poller;
mod probe;
mod tray;

use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct PendingOauthSessions(pub Mutex<HashMap<String, oauth_login::PendingSession>>);

#[derive(Debug, Clone, Serialize)]
pub struct StateSnapshot {
    pub updates: Vec<poller::ProfileUpdate>,
    pub config: config::Config,
}

#[tauri::command]
async fn get_snapshot(
    store: tauri::State<'_, poller::UsageStoreHandle>,
) -> Result<StateSnapshot, String> {
    let updates = store.updates.lock().await.clone();
    let config = config::load().map_err(|e| e.to_string())?;
    Ok(StateSnapshot { updates, config })
}

#[tauri::command]
async fn refresh_now(
    app: AppHandle,
    store: tauri::State<'_, poller::UsageStoreHandle>,
) -> Result<(), String> {
    let store = store.inner().clone();
    poller::tick(&app, &store).await;
    Ok(())
}

#[tauri::command]
async fn save_config(app: AppHandle, cfg: config::Config) -> Result<(), String> {
    config::save(&cfg).map_err(|e| e.to_string())?;
    let ids: Vec<String> = cfg.profiles.iter().map(|p| p.id.clone()).collect();
    let _ = app.emit("config-changed", ids);
    Ok(())
}

#[tauri::command]
async fn auto_detect() -> Result<Vec<String>, String> {
    Ok(credentials::auto_detect_dirs()
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect())
}

#[tauri::command]
async fn toggle_widget(app: AppHandle) {
    tray::toggle_widget(&app);
}

#[tauri::command]
async fn show_settings(app: AppHandle) {
    tray::show_window(&app, "settings");
}

#[tauri::command]
async fn persist_widget_pos(x: i32, y: i32) -> Result<(), String> {
    let mut cfg = config::load().map_err(|e| e.to_string())?;
    cfg.widget_pos = Some(config::WindowPos { x, y });
    config::save(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
async fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
async fn begin_oauth(
    profile_id: String,
    sessions: tauri::State<'_, PendingOauthSessions>,
) -> Result<String, String> {
    let cfg = config::load().map_err(|e| e.to_string())?;
    let profile = cfg
        .profiles
        .iter()
        .find(|p| p.id == profile_id)
        .ok_or_else(|| format!("profile '{}' not found", profile_id))?;
    if profile.config_dir.is_empty() {
        return Err("set a config_dir in Settings before logging in".into());
    }
    let started = oauth_login::start_login_session().map_err(|e| e.to_string())?;
    sessions
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .insert(profile_id, started.session);
    Ok(started.auth_url)
}

#[tauri::command]
async fn submit_oauth_code(
    app: AppHandle,
    profile_id: String,
    code: String,
    sessions: tauri::State<'_, PendingOauthSessions>,
    store: tauri::State<'_, poller::UsageStoreHandle>,
) -> Result<(), String> {
    let session = sessions
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&profile_id)
        .ok_or_else(|| "no pending login — click Login first".to_string())?;
    let store = store.inner().clone();
    oauth_login::complete_with_pasted_code(app, profile_id, session, code, store).await
}

#[tauri::command]
async fn fit_window_height(app: AppHandle, label: String, height: u32) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(&label) {
        win.set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: 320.0_f64,
            height: (height as f64).clamp(140.0, 700.0),
        }))
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            refresh_now,
            save_config,
            auto_detect,
            toggle_widget,
            show_settings,
            persist_widget_pos,
            quit_app,
            fit_window_height,
            begin_oauth,
            submit_oauth_code,
        ])
        .setup(|app| {
            let store = poller::store();
            app.manage(store.clone());
            app.manage(PendingOauthSessions::default());

            if let Err(e) = tray::build(&app.handle()) {
                eprintln!("tray init failed: {e:#}");
            }

            apply_initial_window_state(&app.handle());
            poller::spawn(app.handle().clone(), store);

            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running tauri app");
}

fn apply_initial_window_state(app: &AppHandle) {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(_) => return,
    };

    if let Some(widget) = app.get_webview_window("widget") {
        if let Some(pos) = &cfg.widget_pos {
            let _ = widget.set_position(tauri::PhysicalPosition { x: pos.x, y: pos.y });
        }
        if cfg.widget_visible {
            let _ = widget.show();
        }
    }
}

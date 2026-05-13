mod config;
mod credentials;
mod oauth_refresh;
mod poller;
mod probe;
mod tray;

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

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
    // Union of file-based and (on macOS) Keychain-based config dirs.
    let mut paths: Vec<std::path::PathBuf> = credentials::auto_detect_dirs();
    #[cfg(target_os = "macos")]
    for dir in credentials::auto_detect_keychain_dirs() {
        if !paths.iter().any(|p| p == &dir) {
            paths.push(dir);
        }
    }
    Ok(paths
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
        ])
        .setup(|app| {
            let store = poller::store();
            app.manage(store.clone());

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

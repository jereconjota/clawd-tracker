use anyhow::{anyhow, Result};
use tauri::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WebviewWindow,
};

use crate::config;
use crate::poller::{ProfileState, ProfileUpdate};
use std::collections::HashSet;

pub const TRAY_ID: &str = "main";

pub fn build(app: &AppHandle) -> Result<()> {
    let icon_bytes = include_bytes!("../icons/32x32.png");
    let icon = tauri::image::Image::from_bytes(icon_bytes)
        .map_err(|e| anyhow!("failed to load tray icon: {e}"))?;

    let menu = build_context_menu(app)?;

    let _tray = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .icon_as_template(false)
        .title("…")
        .tooltip("Clawd Tracker — Claude usage")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_context_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                toggle_popover_at(tray.app_handle(), position.x, position.y);
            }
        })
        .build(app)?;

    Ok(())
}

pub fn update_tray_title(app: &AppHandle, updates: &[ProfileUpdate]) {
    // Honor per-profile `show_in_tray` toggle. If config can't be read, fall
    // back to showing all profiles.
    let visible: Option<HashSet<String>> = config::load().ok().map(|cfg| {
        cfg.profiles
            .iter()
            .filter(|p| p.show_in_tray)
            .map(|p| p.id.clone())
            .collect()
    });

    let parts: Vec<String> = updates
        .iter()
        .filter(|u| visible.as_ref().map_or(true, |s| s.contains(&u.profile_id)))
        .filter_map(|u| match &u.state {
            ProfileState::Ok(r) => Some((r.sess_pct, r.week_pct)),
            ProfileState::Stale { last, .. } => Some((last.sess_pct, last.week_pct)),
            _ => None,
        })
        .map(|(sess, week)| {
            // Color reflects the worst of (5h, 7d) so a near-empty 5h with a
            // hot 7d still signals attention. The displayed number is the 5h
            // (the most immediate gauge).
            let worst = sess.max(week);
            let dot = if worst >= 0.85 {
                "🔴"
            } else if worst >= 0.60 {
                "🟡"
            } else {
                "🟢"
            };
            format!("{dot} {}%", (sess * 100.0).round() as i32)
        })
        .collect();

    let title = if parts.is_empty() {
        "—".to_string()
    } else {
        parts.join(" · ")
    };

    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_title(Some(&title));
    }
}

fn build_context_menu(app: &AppHandle) -> Result<Menu<tauri::Wry>> {
    let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &refresh,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    Ok(menu)
}

pub fn toggle_popover_at(app: &AppHandle, click_x: f64, click_y: f64) {
    if let Some(win) = app.get_webview_window("popover") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            position_popover(&win, Some(click_x), Some(click_y));
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

pub fn toggle_widget(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("widget") {
        let visible = win.is_visible().unwrap_or(false);
        if visible {
            let _ = win.hide();
        } else {
            let _ = win.show();
        }
        if let Ok(mut cfg) = crate::config::load() {
            cfg.widget_visible = !visible;
            let _ = crate::config::save(&cfg);
        }
    }
}

pub fn show_window(app: &AppHandle, label: &str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn position_popover(win: &WebviewWindow, click_x: Option<f64>, click_y: Option<f64>) {
    // y = 28 logical pts places the popup just below macOS menu bar (22-24pt tall) on all densities.
    const Y_BELOW_BAR: f64 = 28.0;
    let scale = win.scale_factor().unwrap_or(2.0);
    if let Some(px) = click_x {
        let _ = click_y; // y comes from menu-bar constant, not click
        let lx = px / scale;
        let x = (lx - 160.0).max(8.0);
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y: Y_BELOW_BAR }));
        return;
    }
    // Fallback: top-right area
    if let Ok(Some(monitor)) = win.primary_monitor() {
        let sz = monitor.size();
        let sc = monitor.scale_factor();
        let x = (sz.width as f64 / sc) - 320.0 - 24.0;
        let _ = win.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y: Y_BELOW_BAR }));
    }
}

fn handle_context_event(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
        "refresh" => {
            let store = app.state::<crate::poller::UsageStoreHandle>().inner().clone();
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                crate::poller::tick(&app_clone, &store).await;
            });
        }
        "settings" => show_window(app, "settings"),
        "quit" => app.exit(0),
        _ => {}
    }
}

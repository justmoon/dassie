//! The tray icon (spec/areas/desktop.md#lifecycle).
//!
//! The node keeps running when its window closes; the tray is how the owner
//! sees that, opens the wallet again, and quits.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::node::RunningNode;
use crate::{autostart, window};

/// How often the tray's status line is brought up to date.
const STATUS_EVERY: Duration = Duration::from_secs(2);

/// Makes the tray icon and keeps its status line current.
pub fn build(app: &AppHandle, node: &Arc<RunningNode>, data_dir: PathBuf) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", node.status().label(), false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open wallet", true, None::<&str>)?;
    let at_login = CheckMenuItem::with_id(
        app,
        "at-login",
        "Start at login",
        true,
        autostart::wanted(&data_dir),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit Dassie", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &at_login,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("dassie")
        .tooltip("Dassie")
        .menu(&menu)
        .show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    let for_menu = Arc::clone(node);
    tray.on_menu_event(move |app, event| match event.id().as_ref() {
        "open" => window::show(app, &for_menu),
        "at-login" => {
            let on = at_login.is_checked().unwrap_or(false);
            if let Err(error) = autostart::choose(&data_dir, on) {
                tracing::warn!(target: "dassie_desktop", %error, "could not change starting at login");
            }
        }
        "quit" => quit_app(app, &for_menu),
        _ => {}
    })
    .build(app)?;
    keep_current(status, Arc::clone(node));
    Ok(())
}

/// Brings the status line up to date until the node is asked to stop.
fn keep_current(status: MenuItem<tauri::Wry>, node: Arc<RunningNode>) {
    let spawned = std::thread::Builder::new()
        .name("dassie-tray".into())
        .spawn(move || {
            let mut shown = node.status();
            while !node.stopping() {
                std::thread::sleep(STATUS_EVERY);
                let now = node.status();
                if now != shown && status.set_text(now.label()).is_ok() {
                    shown = now;
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(target: "dassie_desktop", %error, "the tray's status stays as it is");
    }
}

/// Closes the window, stops the node and then the app, off the main thread
/// so that the desktop does not see the app hang while the node stops.
pub fn quit_app(app: &AppHandle, node: &Arc<RunningNode>) {
    if let Some(window) = app.get_webview_window(window::MAIN) {
        let _ = window.destroy();
    }
    let stopping = std::thread::Builder::new()
        .name("dassie-quit".into())
        .spawn({
            let (app, node) = (app.clone(), Arc::clone(node));
            move || {
                node.stop();
                app.exit(0);
            }
        });
    if stopping.is_err() {
        node.stop();
        app.exit(0);
    }
}

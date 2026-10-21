//! The Dassie desktop app (spec/areas/desktop.md): the owner's node, run
//! in this process as a periphery node, with its wallet in a window and a
//! tray icon that keeps it running.
//!
//! `dassie-desktop [--hidden]`: with `--hidden`, as the autostart entry
//! runs it, the app starts in the tray without a window. The node reads the
//! same environment as `dassie daemon` (`dassie_node::env`).

mod autostart;
mod node;
mod ports;
mod private;
mod signals;
mod tray;
mod trust;
mod window;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use dassie_node::env::{Env, Settings};
use dassie_node::log;
use tauri::{AppHandle, Manager, RunEvent};

use crate::node::{RunningNode, Status};

/// The app's state, once its node runs.
struct Desktop {
    node: Arc<RunningNode>,
}

fn main() -> ExitCode {
    // Before anything is written: the window's session cookie among it.
    private::umask();
    let hidden = std::env::args_os().skip(1).any(|arg| arg == "--hidden");
    let env = Env::from_process();
    match Settings::from_env(&env) {
        Ok(settings) => log::init(settings.log_filter),
        Err(error) => {
            eprintln!("dassie-desktop: {error}");
            return ExitCode::FAILURE;
        }
    }
    let built = tauri::Builder::default()
        // First, so that a second launch only shows this one's window and
        // never starts a node (spec/areas/desktop.md#one-node).
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(desktop) = app.try_state::<Desktop>() {
                window::show(app, &desktop.node);
            }
        }))
        .setup(move |app| {
            setup(app.handle(), &env, hidden);
            Ok(())
        })
        .build(tauri::generate_context!());
    let app = match built {
        Ok(app) => app,
        Err(error) => {
            eprintln!("dassie-desktop: {error}");
            return ExitCode::FAILURE;
        }
    };
    app.run(on_event);
    ExitCode::SUCCESS
}

/// Starts the node, the tray and, unless `hidden`, the window. A node that
/// cannot start is shown in the window, which then quits the app.
fn setup(app: &AppHandle, env: &Env, hidden: bool) {
    signals::watch(app);
    let started = data_dir(app).and_then(|dir| {
        let ports = ports::choose(&dir).map_err(|error| format!("choosing ports: {error}"))?;
        let ended = app.clone();
        let node = node::start(env, ports, move |error| {
            if let Some(error) = error {
                window::show_error(&ended, &error);
            }
        })?;
        Ok((dir, Arc::new(node)))
    });
    let (dir, node) = match started {
        Ok(started) => started,
        Err(error) => {
            tracing::error!(target: "dassie_desktop", %error, "the node could not start");
            window::show_error(app, &error);
            return;
        }
    };
    app.manage(Desktop {
        node: Arc::clone(&node),
    });
    if let Err(error) = autostart::refresh(&dir) {
        tracing::warn!(target: "dassie_desktop", %error, "could not update starting at login");
    }
    if let Err(error) = tray::build(app, &node, dir) {
        tracing::warn!(target: "dassie_desktop", %error, "no tray icon");
    }
    if !hidden {
        window::show(app, &node);
    }
}

/// The app's data directory, private: its ports, the owner's choices, and
/// the window's cookies, cache and storage (Tauri gives the web engine the
/// local data directory, the same one on Linux). The node's own data stays
/// where `dassie daemon` keeps it.
fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let paths = app.path();
    let dir = paths
        .app_data_dir()
        .map_err(|error| format!("finding the app's data directory: {error}"))?;
    let local = paths
        .app_local_data_dir()
        .map_err(|error| format!("finding the app's data directory: {error}"))?;
    for private in [&dir, &local] {
        private::directory(private)
            .map_err(|error| format!("securing {}: {error}", private.display()))?;
    }
    Ok(dir)
}

/// Keeps the app, and its node, running when its window closes, while the
/// node runs; stops the node when the app exits.
fn on_event(app: &AppHandle, event: RunEvent) {
    let running = || {
        app.try_state::<Desktop>()
            .is_some_and(|desktop| desktop.node.status() != Status::Stopped)
    };
    match event {
        RunEvent::ExitRequested {
            code: None, api, ..
        } if running() => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(desktop) = app.try_state::<Desktop>() {
                desktop.node.stop();
            }
        }
        _ => {}
    }
}

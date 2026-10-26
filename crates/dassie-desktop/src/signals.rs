//! SIGTERM and SIGINT (spec/areas/desktop.md#lifecycle).
//!
//! A session that ends sends the app SIGTERM. The first one quits the app,
//! which stops its node as SIGTERM stops `dassie daemon`; a second one,
//! while the node still stops, ends the process at once. The handlers live
//! on a thread of their own for as long as the app does, so they outlast a
//! node that stopped.

use std::process;

use tauri::AppHandle;
use tokio::signal::unix::{Signal, SignalKind, signal};

/// The exit code for a second signal, as a shell reports SIGTERM.
const FORCED_EXIT: i32 = 143;

/// Handles SIGTERM and SIGINT from now on.
pub fn watch(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("dassie-signals".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build();
            let Ok(runtime) = runtime else {
                tracing::warn!(target: dassie_node::log::target::DESKTOP, "SIGTERM and SIGINT end the app without stopping its node");
                return;
            };
            runtime.block_on(async move {
                let Some((mut terminate, mut interrupt)) = handlers() else {
                    tracing::warn!(target: dassie_node::log::target::DESKTOP, "SIGTERM and SIGINT end the app without stopping its node");
                    return;
                };
                next(&mut terminate, &mut interrupt).await;
                tracing::info!(target: dassie_node::log::target::DESKTOP, "quitting on a signal");
                app.exit(0);
                next(&mut terminate, &mut interrupt).await;
                process::exit(FORCED_EXIT);
            });
        });
    if let Err(error) = spawned {
        tracing::warn!(target: dassie_node::log::target::DESKTOP, %error, "SIGTERM and SIGINT end the app without stopping its node");
    }
}

/// SIGTERM and SIGINT, handled from now on.
fn handlers() -> Option<(Signal, Signal)> {
    Some((
        signal(SignalKind::terminate()).ok()?,
        signal(SignalKind::interrupt()).ok()?,
    ))
}

/// Until the next of either.
async fn next(terminate: &mut Signal, interrupt: &mut Signal) {
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
}

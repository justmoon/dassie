//! Quitting the app (spec/areas/desktop.md#lifecycle).
//!
//! Quitting stops the node, which first waits for its packets in flight,
//! so that their answers are not lost (spec/areas/node.md#shutdown). That
//! is usually a moment; a wait that lasts longer opens a small window with
//! the time left, where the owner may cancel the quit or quit at once. A
//! quit the owner chose drains in a way that can be taken back; one a
//! signal asked for, as when the session ends, cannot.
//!
//! The window is the app's own page, which gets no Tauri API: its buttons
//! are links to `action/…`, which the window takes and never loads, and
//! what it shows comes in its URL's fragment.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::node::RunningNode;
use crate::window;

/// The quitting window's label.
const WINDOW: &str = "quitting";
/// How long a quit waits before it shows its window: most drains end
/// sooner, and the window would only flash.
const SHOW_AFTER: Duration = Duration::from_secs(1);
/// How often the quit looks at the drain, and brings its window up to date.
const LOOK_EVERY: Duration = Duration::from_millis(200);

/// What the owner, or a signal, asks of a quit under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    /// Cancel the quit: the node takes packets again and runs on.
    Cancel,
    /// Quit at once, cutting the drain short.
    Now,
    /// A signal asked to quit: the quit can no longer be cancelled.
    Final,
}

/// The quit under way, if any, which its window and the signals reach.
#[derive(Default)]
pub struct Quitting(Mutex<Option<Sender<Ask>>>);

impl Quitting {
    fn under_way(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn ask(&self, ask: Ask) -> bool {
        let held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        held.as_ref().is_some_and(|quit| quit.send(ask).is_ok())
    }

    fn set(&self, quit: Option<Sender<Ask>>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = quit;
    }
}

/// Quits the app, once the node drained, off the main thread so that the
/// desktop does not see the app hang meanwhile. `signalled` when a signal
/// asked, which no one can take back. A quit already under way is shown,
/// or, for a signal, made final.
pub fn quit(app: &AppHandle, node: &Arc<RunningNode>, signalled: bool) {
    let Some(quitting) = app.try_state::<Quitting>() else {
        return;
    };
    if quitting.under_way() {
        if signalled {
            quitting.ask(Ask::Final);
        } else if let Some(window) = app.get_webview_window(WINDOW) {
            let _ = window.show();
            let _ = window.set_focus();
        }
        return;
    }
    if let Some(window) = app.get_webview_window(window::MAIN) {
        let _ = window.destroy();
    }
    let (asks, asked) = mpsc::channel();
    quitting.set(Some(asks));
    if signalled {
        node.begin_stop();
    } else {
        node.request_drain(true);
    }
    let spawned = std::thread::Builder::new()
        .name("dassie-quit".into())
        .spawn({
            let (app, node) = (app.clone(), Arc::clone(node));
            move || run(&app, &node, &asked, signalled)
        });
    if spawned.is_err() {
        node.stop();
        app.exit(0);
    }
}

/// Asks the quit under way to end at once, as a second signal does.
/// Whether one was under way.
pub fn now(app: &AppHandle) -> bool {
    app.try_state::<Quitting>()
        .is_some_and(|quitting| quitting.ask(Ask::Now))
}

/// Waits for the drain, showing it once it lasts, until it ends, the owner
/// cancels or quits at once.
fn run(app: &AppHandle, node: &RunningNode, asked: &mpsc::Receiver<Ask>, signalled: bool) {
    let started = Instant::now();
    let mut cancellable = !signalled;
    let mut shown = None;
    while !node.drained() {
        // A window the owner closed in a quit that cannot be cancelled
        // stays closed: the quit goes on without it.
        let dismissed = shown.is_some() && app.get_webview_window(WINDOW).is_none();
        if started.elapsed() >= SHOW_AFTER && !dismissed {
            let view = View {
                in_flight: node.in_flight(),
                until_ms: node.drain_until(),
                cancellable,
            };
            if shown != Some(view) {
                show(app, view);
                shown = Some(view);
            }
        }
        match asked.recv_timeout(LOOK_EVERY) {
            Ok(Ask::Cancel) if cancellable => {
                node.request_drain(false);
                tracing::info!(target: dassie_node::log::target::DESKTOP, "quitting cancelled");
                close(app);
                if let Some(quitting) = app.try_state::<Quitting>() {
                    quitting.set(None);
                }
                return;
            }
            Ok(Ask::Now) => {
                tracing::warn!(target: dassie_node::log::target::DESKTOP, "quitting at once: the answers to packets still in flight are lost");
                node.stop_now();
                break;
            }
            Ok(Ask::Final) if cancellable => {
                cancellable = false;
                node.begin_stop();
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    close(app);
    node.stop();
    app.exit(0);
}

/// What the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct View {
    in_flight: usize,
    /// When the last packet is answered at the latest, in ms since the
    /// epoch: the node's clock is the system's.
    until_ms: Option<u64>,
    cancellable: bool,
}

impl View {
    fn url(self) -> Option<Url> {
        let until = self
            .until_ms
            .map_or(String::new(), |until| until.to_string());
        let page = format!(
            "tauri://localhost/quitting.html#count={}&until={until}&cancel={}",
            self.in_flight,
            u8::from(self.cancellable),
        );
        Url::parse(&page).ok()
    }
}

/// Shows the window as `view` has it, making it if it is not open.
fn show(app: &AppHandle, view: View) {
    let Some(url) = view.url() else {
        return;
    };
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.navigate(url);
        return;
    }
    let built = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("quitting.html".into()))
        .title("Quitting Dassie")
        .inner_size(460.0, 380.0)
        .resizable(false)
        .center()
        .on_navigation({
            let app = app.clone();
            move |url| navigation(&app, url)
        })
        .build();
    match built {
        Ok(window) => {
            let _ = window.navigate(url);
            let app = app.clone();
            window.on_window_event(move |event| {
                // Closing it cancels the quit, as Cancel does; a quit that
                // cannot be cancelled goes on without it.
                if let WindowEvent::CloseRequested { .. } = event
                    && let Some(quitting) = app.try_state::<Quitting>()
                {
                    quitting.ask(Ask::Cancel);
                }
            });
        }
        Err(error) => {
            tracing::warn!(target: dassie_node::log::target::DESKTOP, %error, "could not open the quitting window");
        }
    }
}

/// Loads the window's own page; takes its buttons' links, and loads
/// nothing else.
fn navigation(app: &AppHandle, url: &Url) -> bool {
    let ours = url.scheme() == "tauri" && url.host_str() == Some("localhost");
    if !ours {
        return false;
    }
    let ask = match url.path() {
        "/quitting.html" => return true,
        "/action/cancel" => Ask::Cancel,
        "/action/now" => Ask::Now,
        _ => return false,
    };
    if let Some(quitting) = app.try_state::<Quitting>() {
        quitting.ask(ask);
    }
    false
}

fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.destroy();
    }
}

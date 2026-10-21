//! The wallet's window (spec/areas/desktop.md#the-window).
//!
//! The window loads the wallet from the node's own origin,
//! `https://localhost:<port>`, so the node's same-origin rule, its
//! `__Host-` session cookie and its CSP hold as in any browser. The node's
//! certificate there is self-signed; the window trusts exactly that
//! certificate, the one the node in this process serves, for `localhost`,
//! and nothing else it would not trust anyway. The page gets no Tauri API:
//! no capability names it.

use std::sync::{Arc, Weak};

use dassie_node::node::Node;
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use webkit2gtk::gio::TlsCertificate;
use webkit2gtk::gio::prelude::TlsCertificateExt;
use webkit2gtk::{WebContextExt, WebViewExt};

use crate::node::RunningNode;
use crate::trust::{self, NODE_HOST, Navigation};

/// The window's label.
pub const MAIN: &str = "main";

/// Shows the wallet's window, making it if it is closed.
pub fn show(app: &AppHandle, node: &Arc<RunningNode>) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    if let Err(error) = open(app, node) {
        tracing::error!(target: "dassie_desktop", %error, "could not open the window");
    }
}

/// Shows the app's own page with why the node could not start.
pub fn show_error(app: &AppHandle, error: &str) {
    let mut url = String::from("index.html#error=");
    url.extend(url::form_urlencoded::byte_serialize(error.as_bytes()));
    let window = match app.get_webview_window(MAIN) {
        Some(window) => window,
        None => match builder(app, 0).build() {
            Ok(window) => window,
            Err(error) => {
                tracing::error!(target: "dassie_desktop", %error, "could not open the window");
                return;
            }
        },
    };
    if let Ok(page) = Url::parse(&format!("tauri://localhost/{url}")) {
        let _ = window.navigate(page);
    }
    let _ = window.show();
}

fn open(app: &AppHandle, node: &Arc<RunningNode>) -> tauri::Result<()> {
    let window = builder(app, node.https_port).build()?;
    if let Some(pem) = node.certificate_pem() {
        trust_node_certificate(&window, &pem, node.https_port, node.weak())?;
    }
    if let Some(url) = node.start_url() {
        window.navigate(url)?;
    }
    Ok(())
}

/// The window, on the app's own page until it navigates to the node.
fn builder(app: &AppHandle, https_port: u16) -> WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Dassie")
        .inner_size(1100.0, 780.0)
        .min_inner_size(400.0, 500.0)
        .on_navigation(move |url| match trust::navigation(url, https_port) {
            Navigation::Allow => true,
            Navigation::External => {
                if let Err(error) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
                    tracing::warn!(target: "dassie_desktop", %error, "could not open a link");
                }
                false
            }
            Navigation::Refuse => false,
        })
}

/// Trusts the node's certificate for [`NODE_HOST`] before the first load,
/// and, should the node have renewed it since, the certificate it serves
/// then: only if WebKit's is byte for byte the one the node in this process
/// serves on its origin.
fn trust_node_certificate(
    window: &WebviewWindow,
    pem: &str,
    https_port: u16,
    node: Weak<Node>,
) -> tauri::Result<()> {
    let pem = pem.to_owned();
    window.with_webview(move |platform| {
        let view = platform.inner();
        let Some(context) = view.context() else {
            return;
        };
        if let Ok(certificate) = TlsCertificate::from_pem(&pem) {
            context.allow_tls_certificate_for_host(&certificate, NODE_HOST);
        }
        view.connect_load_failed_with_tls_errors(move |view, uri, presented, _errors| {
            let on_origin = Url::parse(uri).is_ok_and(|url| trust::is_node_origin(&url, https_port));
            let served = node
                .upgrade()
                .map(|node| node.certificate().chain_pem())
                .and_then(|pem| TlsCertificate::from_pem(&pem).ok());
            let same = served.is_some_and(|served| presented.is_same(&served));
            if !(on_origin && same) {
                tracing::warn!(target: "dassie_desktop", uri, "refused a certificate that is not the node's");
                return false;
            }
            if let Some(context) = view.context() {
                context.allow_tls_certificate_for_host(presented, NODE_HOST);
                view.load_uri(uri);
            }
            true
        });
    })
}

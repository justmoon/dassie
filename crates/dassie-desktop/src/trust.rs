//! What the window loads (spec/areas/desktop.md#the-window).
//!
//! The window shows the app's own page and the node's origin,
//! `https://localhost:<https port>`, where the wallet is served, and
//! nothing else: a link elsewhere opens in the owner's browser, which is
//! where a page the owner did not choose belongs. The certificate it trusts
//! for that origin is the node's own, checked in `window.rs`.

use tauri::Url;

/// The one host the window trusts the node's certificate for.
pub const NODE_HOST: &str = "localhost";

/// What the window does with a navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Navigation {
    /// Load it in the window.
    Allow,
    /// Open it in the owner's browser instead.
    External,
    /// Load it nowhere.
    Refuse,
}

/// Whether `url` is on the node's origin.
#[must_use]
pub fn is_node_origin(url: &Url, https_port: u16) -> bool {
    url.scheme() == "https"
        && url.host_str() == Some(NODE_HOST)
        && url.port_or_known_default() == Some(https_port)
}

/// Whether `url` is the app's own page, which Tauri serves on Linux from
/// `tauri://localhost`.
#[must_use]
pub fn is_app_page(url: &Url) -> bool {
    url.scheme() == "tauri" && url.host_str() == Some("localhost")
}

/// What the window does when asked to load `url`.
#[must_use]
pub fn navigation(url: &Url, https_port: u16) -> Navigation {
    if is_node_origin(url, https_port) || is_app_page(url) || url.as_str() == "about:blank" {
        return Navigation::Allow;
    }
    let local = match url.host() {
        Some(url::Host::Domain(name)) => name == NODE_HOST || name.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified(),
        None => true,
    };
    // The owner's browser gets public pages only, never one that could
    // reach a service on this machine or its network for the wallet.
    if url.scheme() == "https" && !local {
        Navigation::External
    } else {
        Navigation::Refuse
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(url: &str) -> Navigation {
        navigation(&Url::parse(url).unwrap(), 41_443)
    }

    #[test]
    fn the_nodes_origin_and_the_apps_page_load() {
        assert_eq!(at("https://localhost:41443/"), Navigation::Allow);
        assert_eq!(at("https://localhost:41443/setup/ab12"), Navigation::Allow);
        assert_eq!(
            at("tauri://localhost/index.html#error=x"),
            Navigation::Allow
        );
        assert_eq!(at("about:blank"), Navigation::Allow);
    }

    #[test]
    fn other_local_origins_are_refused() {
        assert_eq!(at("https://localhost:8443/"), Navigation::Refuse);
        assert_eq!(at("http://localhost:41443/"), Navigation::Refuse);
        assert_eq!(at("https://127.0.0.1:41443/"), Navigation::Refuse);
        assert_eq!(at("https://192.168.1.1/"), Navigation::Refuse);
        assert_eq!(at("https://[::1]/"), Navigation::Refuse);
        assert_eq!(at("https://admin.localhost/"), Navigation::Refuse);
        assert_eq!(at("file:///etc/passwd"), Navigation::Refuse);
        assert_eq!(at("data:text/html,hi"), Navigation::Refuse);
    }

    #[test]
    fn public_pages_open_in_the_browser() {
        assert_eq!(at("https://testnet.xrpl.org/"), Navigation::External);
        assert_eq!(at("https://dassie.land/"), Navigation::External);
        assert_eq!(at("http://example.com/"), Navigation::Refuse);
    }
}

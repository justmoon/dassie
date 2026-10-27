//! The port the node's HTTPS listener takes on loopback
//! (spec/areas/desktop.md#sockets).
//!
//! The wallet's origin is `https://localhost:<port>`, and the window's
//! storage belongs to that origin, so the app keeps the same port from one
//! start to the next: it remembers the one it chose, and chooses a new one
//! only when it is taken. The node has no HTTP listener, so there is no
//! other port to keep.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;

/// The file in the app's data directory that remembers the port.
pub const PORTS_FILE: &str = "ports";

/// The port remembered in `text`: `<https>`. A file that still names the
/// HTTP port first, `<http> <https>`, keeps its HTTPS port, so the wallet's
/// origin stays what it was.
#[must_use]
pub fn parse(text: &str) -> Option<u16> {
    let words: Vec<&str> = text.split_whitespace().take(3).collect();
    let https = match words.as_slice() {
        [https] | [_, https] => https.parse::<u16>().ok()?,
        _ => return None,
    };
    (https != 0).then_some(https)
}

/// `port` as [`parse`] reads it.
#[must_use]
pub fn format(port: u16) -> String {
    format!("{port}\n")
}

/// The remembered port if it is free on loopback, else one the system
/// hands out now; remembered for the next start.
pub fn choose(data_dir: &Path) -> io::Result<u16> {
    let path = data_dir.join(PORTS_FILE);
    let text = fs::read_to_string(&path).ok();
    let remembered = text.as_deref().and_then(parse);
    let port = match remembered {
        Some(port) if free(port) => port,
        _ => fresh()?,
    };
    if text.as_deref() != Some(format(port).as_str()) {
        fs::create_dir_all(data_dir)?;
        fs::write(&path, format(port))?;
    }
    Ok(port)
}

/// Whether `port` can be bound on loopback now.
fn free(port: u16) -> bool {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).is_ok()
}

/// A port the system hands out on loopback. The node binds it a moment
/// later; should another program take it in between, the node's start
/// fails and the next start chooses again.
fn fresh() -> io::Result<u16> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
    Ok(listener.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remembered_port_reads_back() {
        assert_eq!(parse(&format(40_443)), Some(40_443));
        // HTTPS of a file that names HTTP first.
        assert_eq!(parse("40080 40443\n"), Some(40_443));
    }

    #[test]
    fn unusable_text_remembers_nothing() {
        for text in [
            "",
            "0",
            "80 0",
            "80 443 8443",
            "70000",
            "80 70000",
            "a",
            "a b",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_port_is_kept_while_free() {
        let dir = tempfile::tempdir().unwrap();
        let first = choose(dir.path()).unwrap();
        assert_eq!(choose(dir.path()).unwrap(), first);
        let text = fs::read_to_string(dir.path().join(PORTS_FILE)).unwrap();
        assert_eq!(text, format(first));
        // Taken: a new one, remembered in its place.
        let _taken = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, first)));
        let second = choose(dir.path()).unwrap();
        assert_ne!(second, first);
        let text = fs::read_to_string(dir.path().join(PORTS_FILE)).unwrap();
        assert_eq!(parse(&text), Some(second));
    }

    // A file of two ports is written anew with the HTTPS port alone, which
    // the node keeps.
    #[test]
    fn a_file_of_two_ports_keeps_its_https_port() {
        let dir = tempfile::tempdir().unwrap();
        let https = fresh().unwrap();
        fs::write(dir.path().join(PORTS_FILE), format!("1 {https}\n")).unwrap();
        assert_eq!(choose(dir.path()).unwrap(), https);
        let text = fs::read_to_string(dir.path().join(PORTS_FILE)).unwrap();
        assert_eq!(text, format(https));
    }
}

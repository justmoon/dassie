//! The ports the node's listeners take on loopback
//! (spec/areas/desktop.md#sockets).
//!
//! The wallet's origin is `https://localhost:<port>`, and the window's
//! storage belongs to that origin, so the app keeps the same ports from one
//! start to the next: it remembers the pair it chose, and chooses a new one
//! only when one of them is taken.

use std::fs;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;

use dassie_node_core::config::Ports;

/// The file in the app's data directory that remembers the ports.
pub const PORTS_FILE: &str = "ports";

/// The ports remembered in `text`: `<http> <https>`.
#[must_use]
pub fn parse(text: &str) -> Option<Ports> {
    let mut words = text.split_whitespace();
    let http = words.next()?.parse::<u32>().ok()?;
    let https = words.next()?.parse::<u32>().ok()?;
    if words.next().is_some() {
        return None;
    }
    Ports::new(http, https).ok()
}

/// `ports` as [`parse`] reads them.
#[must_use]
pub fn format(ports: Ports) -> String {
    format!("{} {}\n", ports.http(), ports.https())
}

/// The remembered ports if both are free on loopback, else two the system
/// hands out now; remembered for the next start.
pub fn choose(data_dir: &Path) -> io::Result<Ports> {
    let path = data_dir.join(PORTS_FILE);
    let remembered = fs::read_to_string(&path).ok().as_deref().and_then(parse);
    let ports = match remembered {
        Some(ports) if free(ports) => ports,
        _ => fresh()?,
    };
    if remembered != Some(ports) {
        fs::create_dir_all(data_dir)?;
        fs::write(&path, format(ports))?;
    }
    Ok(ports)
}

/// Whether both ports can be bound on loopback now.
fn free(ports: Ports) -> bool {
    let bind = |port: u16| TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    bind(ports.http()).is_ok() && bind(ports.https()).is_ok()
}

/// Two ports the system hands out on loopback. The node binds them a moment
/// later; should another program take one in between, the node's start
/// fails and the next start chooses again.
fn fresh() -> io::Result<Ports> {
    let any = || TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
    let (http, https) = (any()?, any()?);
    let port = |listener: &TcpListener| listener.local_addr().map(|a| u32::from(a.port()));
    Ports::new(port(&http)?, port(&https)?).map_err(|error| io::Error::other(format!("{error:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_ports_read_back() {
        let ports = Ports::new(40_080, 40_443).unwrap();
        assert_eq!(parse(&format(ports)), Some(ports));
    }

    #[test]
    fn unusable_text_remembers_nothing() {
        for text in ["", "80", "80 80", "0 443", "80 443 8443", "80 70000", "a b"] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn ports_are_kept_while_free() {
        let dir = tempfile::tempdir().unwrap();
        let first = choose(dir.path()).unwrap();
        assert_eq!(choose(dir.path()).unwrap(), first);
        // One taken: a new pair, remembered in its place.
        let _taken = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, first.https())));
        let second = choose(dir.path()).unwrap();
        assert_ne!(second, first);
        let text = fs::read_to_string(dir.path().join(PORTS_FILE)).unwrap();
        assert_eq!(parse(&text), Some(second));
    }
}

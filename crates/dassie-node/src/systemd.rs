//! systemd: socket activation and readiness.
//!
//! Under systemd the daemon gets its sockets from the socket units, by name
//! (spec/areas/node.md#runtimes): four it needs, QUIC's UDP socket among
//! them since QUIC is the peer protocol's only transport, and the rate
//! feed's UDP socket, which it can go without: its unit is only wanted, and
//! its port may be taken, so the node runs without a feed. The original did
//! not check that `LISTEN_PID` names this process, and reported readiness by
//! running `systemd-notify`. Here both are done in the process: the PID is
//! checked, and `READY=1` and `STOPPING=1` go straight to `NOTIFY_SOCKET`.
//!
//! Taking the sockets must happen before any other thread exists, since it
//! removes `LISTEN_*` from the environment: before the tokio runtime starts.

use std::fmt;
use std::io;
use std::net::{TcpListener, UdpSocket};
#[cfg(target_os = "linux")]
use std::os::linux::net::SocketAddrExt;
#[cfg(target_os = "linux")]
use std::os::unix::net::SocketAddr;
use std::os::unix::net::{UnixDatagram, UnixListener};
use std::path::PathBuf;

use listenfd::ListenFd;

use crate::env::{Env, EnvError};
use crate::log::target;

/// The HTTP socket's name.
pub const HTTP_SOCKET: &str = "dassie-http.socket";
/// The HTTPS socket's name.
pub const HTTPS_SOCKET: &str = "dassie-https.socket";
/// The IPC socket's name.
pub const IPC_SOCKET: &str = "dassie-ipc.socket";
/// The QUIC socket's name.
pub const QUIC_SOCKET: &str = "dassie-quic.socket";
/// The rate feed's socket's name.
pub const FEED_SOCKET: &str = "dassie-feed.socket";

/// The sockets systemd passed in.
#[derive(Debug)]
pub struct ActivatedSockets {
    /// For ACME challenges and redirects.
    pub http: TcpListener,
    /// For everything else.
    pub https: TcpListener,
    /// For the CLI.
    pub ipc: UnixListener,
    /// For the peer protocol over QUIC, its only transport.
    pub quic: UdpSocket,
    /// For the rate feed, if systemd passed it: `dassie.service` only wants
    /// its unit, which fails when its port is taken.
    pub feed: Option<UdpSocket>,
}

/// Socket activation failed.
#[derive(Debug)]
pub enum ActivationError {
    /// A variable is missing or malformed.
    Env(EnvError),
    /// The sockets are meant for another process.
    NotForThisProcess {
        /// `LISTEN_PID`.
        listen_pid: String,
    },
    /// A socket of this name was not passed.
    Missing(&'static str),
    /// Adopting a socket failed.
    Io(io::Error),
}

impl fmt::Display for ActivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(error) => error.fmt(f),
            Self::NotForThisProcess { listen_pid } => write!(
                f,
                "LISTEN_PID is {listen_pid}, not this process ({}): the sockets are not ours",
                std::process::id()
            ),
            Self::Missing(name) => write!(f, "systemd passed no socket named {name}"),
            Self::Io(error) => write!(f, "adopting a socket: {error}"),
        }
    }
}

impl std::error::Error for ActivationError {}

/// Whether systemd passed sockets to this process.
#[must_use]
pub fn is_activated(env: &Env) -> bool {
    let pid = env.get("LISTEN_PID").ok().flatten();
    pid == Some(std::process::id().to_string().as_str())
        && matches!(env.get("LISTEN_FDS"), Ok(Some(_)))
}

/// Takes the sockets. Call it on the main thread before starting any other
/// (see the module documentation).
pub fn take_sockets(env: &Env) -> Result<ActivatedSockets, ActivationError> {
    let required = |name: &'static str| {
        env.get(name)
            .map_err(ActivationError::Env)?
            .ok_or(ActivationError::Env(EnvError {
                variable: name,
                problem: "is not set".into(),
            }))
    };
    let listen_pid = required("LISTEN_PID")?;
    if listen_pid != std::process::id().to_string() {
        return Err(ActivationError::NotForThisProcess {
            listen_pid: listen_pid.into(),
        });
    }
    let count: usize = required("LISTEN_FDS")?.parse().map_err(|_| {
        ActivationError::Env(EnvError {
            variable: "LISTEN_FDS",
            problem: "is not a number".into(),
        })
    })?;
    let names: Vec<&str> = required("LISTEN_FDNAMES")?.split(':').collect();
    if names.len() != count {
        return Err(ActivationError::Env(EnvError {
            variable: "LISTEN_FDNAMES",
            problem: format!("names {} sockets, but LISTEN_FDS is {count}", names.len()),
        }));
    }
    let mut fds = ListenFd::from_env();
    let (mut http, mut https, mut ipc) = (None, None, None);
    let (mut quic, mut feed) = (None, None);
    for (index, name) in names.iter().enumerate() {
        match *name {
            HTTP_SOCKET => http = fds.take_tcp_listener(index).map_err(ActivationError::Io)?,
            HTTPS_SOCKET => https = fds.take_tcp_listener(index).map_err(ActivationError::Io)?,
            IPC_SOCKET => ipc = fds.take_unix_listener(index).map_err(ActivationError::Io)?,
            QUIC_SOCKET => quic = fds.take_udp_socket(index).map_err(ActivationError::Io)?,
            FEED_SOCKET => feed = fds.take_udp_socket(index).map_err(ActivationError::Io)?,
            other => {
                tracing::warn!(target: target::SYSTEMD, name = other, "ignoring an unknown socket")
            }
        }
    }
    Ok(ActivatedSockets {
        http: http.ok_or(ActivationError::Missing(HTTP_SOCKET))?,
        https: https.ok_or(ActivationError::Missing(HTTPS_SOCKET))?,
        ipc: ipc.ok_or(ActivationError::Missing(IPC_SOCKET))?,
        quic: quic.ok_or(ActivationError::Missing(QUIC_SOCKET))?,
        feed,
    })
}

/// Where readiness goes: `NOTIFY_SOCKET`.
#[derive(Clone, Debug)]
pub enum Notifier {
    /// A socket file.
    Path(PathBuf),
    /// An abstract socket (`@name`), which only Linux has.
    Abstract(Vec<u8>),
}

impl Notifier {
    /// The notifier `NOTIFY_SOCKET` names, if it is set.
    pub fn from_env(env: &Env) -> Result<Option<Self>, EnvError> {
        let Some(address) = env.get("NOTIFY_SOCKET")? else {
            return Ok(None);
        };
        if let Some(name) = address.strip_prefix('@').filter(|name| !name.is_empty()) {
            return Ok(Some(Self::Abstract(name.as_bytes().to_vec())));
        }
        if address.starts_with('/') && address.len() >= 2 {
            return Ok(Some(Self::Path(address.into())));
        }
        Err(EnvError {
            variable: "NOTIFY_SOCKET",
            problem: "must be a path or an @abstract name".into(),
        })
    }

    /// Sends `state`, e.g. `READY=1`.
    pub fn notify(&self, state: &str) -> io::Result<()> {
        let socket = UnixDatagram::unbound()?;
        let sent = match self {
            Self::Path(path) => socket.send_to(state.as_bytes(), path)?,
            Self::Abstract(name) => send_to_abstract(&socket, state, name)?,
        };
        if sent == state.len() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short write to NOTIFY_SOCKET",
            ))
        }
    }
}

/// Sends `state` to the abstract socket `name`.
#[cfg(target_os = "linux")]
fn send_to_abstract(socket: &UnixDatagram, state: &str, name: &[u8]) -> io::Result<usize> {
    socket.send_to_addr(state.as_bytes(), &SocketAddr::from_abstract_name(name)?)
}

/// Off Linux there are no abstract sockets, so nothing can listen on one.
/// The error is reported like any other failed notification, and the daemon
/// runs on.
#[cfg(not(target_os = "linux"))]
fn send_to_abstract(_: &UnixDatagram, _: &str, _: &[u8]) -> io::Result<usize> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "abstract sockets exist only on Linux",
    ))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn sockets_for_another_process_are_refused() {
        let env = Env::from_pairs([
            ("LISTEN_PID", "1"),
            ("LISTEN_FDS", "3"),
            ("LISTEN_FDNAMES", "a:b:c"),
        ]);
        assert!(!is_activated(&env));
        assert!(matches!(
            take_sockets(&env),
            Err(ActivationError::NotForThisProcess { .. })
        ));
    }

    #[test]
    fn names_must_match_the_count() {
        let pid = std::process::id().to_string();
        let env = Env::from_pairs([
            ("LISTEN_PID", pid.as_str()),
            ("LISTEN_FDS", "3"),
            ("LISTEN_FDNAMES", "a:b"),
        ]);
        assert!(is_activated(&env));
        assert!(matches!(take_sockets(&env), Err(ActivationError::Env(_))));
    }

    #[test]
    fn notifies_a_path_socket() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("notify");
        let receiver = UnixDatagram::bind(&path).unwrap();
        let env = Env::from_pairs([("NOTIFY_SOCKET", path.to_str().unwrap())]);
        Notifier::from_env(&env)
            .unwrap()
            .unwrap()
            .notify("READY=1")
            .unwrap();
        let mut buffer = [0; 16];
        let length = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"READY=1");
        assert!(Notifier::from_env(&Env::from_pairs([("NOTIFY_SOCKET", "x")])).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn notifies_an_abstract_socket() {
        let name = format!("dassie-test-{}", std::process::id());
        let receiver =
            UnixDatagram::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap())
                .unwrap();
        let address = format!("@{name}");
        let env = Env::from_pairs([("NOTIFY_SOCKET", address.as_str())]);
        Notifier::from_env(&env)
            .unwrap()
            .unwrap()
            .notify("STOPPING=1")
            .unwrap();
        let mut buffer = [0; 16];
        let length = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..length], b"STOPPING=1");
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn an_abstract_socket_is_unsupported() {
        let env = Env::from_pairs([("NOTIFY_SOCKET", "@dassie-test")]);
        let error = Notifier::from_env(&env)
            .unwrap()
            .unwrap()
            .notify("READY=1")
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }
}

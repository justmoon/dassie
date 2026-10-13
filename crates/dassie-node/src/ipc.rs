//! The IPC socket: the RPC contract for the `dassie` CLI.
//!
//! Every connection is the owner's: access is the socket file's
//! permissions (spec 2 §11). Under systemd the socket unit sets them; in the
//! standalone runtime the daemon makes the socket `0660` itself, which the
//! original did not (RT-9). Messages are frames with a 4-byte big-endian
//! length prefix.

use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use futures_util::StreamExt;
use std::os::unix::net::UnixListener as StdUnixListener;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::log::target;
use crate::node::Node;
use crate::rpc::{Caller, MAX_MESSAGE_BYTES, connection};

/// Most CLI connections at once.
pub const MAX_CONNECTIONS: usize = 64;

/// The socket's framing.
#[must_use]
pub fn codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .length_field_length(4)
        .big_endian()
        .max_frame_length(MAX_MESSAGE_BYTES)
        .new_codec()
}

/// A socket bound at a temporary name, not yet where clients look for it.
#[derive(Debug)]
pub struct Unpublished {
    temporary: PathBuf,
    path: PathBuf,
}

impl Unpublished {
    /// Renames the socket into place: from now on clients reach it.
    pub fn publish(self) -> io::Result<PathBuf> {
        fs::rename(&self.temporary, &self.path)?;
        Ok(self.path)
    }

    /// Removes the socket file, for a daemon that did not start.
    pub fn discard(self) {
        let _ = fs::remove_file(&self.temporary);
    }
}

/// Binds the socket for `path`, mode 0660, at a temporary name.
///
/// A daemon that answers on `path` already is left alone, and this one
/// refuses to start. The socket is renamed into place by
/// [`Unpublished::publish`] once the daemon is ready for clients, replacing
/// a stale file; so it is never reachable with looser permissions, nor
/// before the daemon answers, nor left behind by a daemon that failed.
pub fn bind_unpublished(path: &Path) -> io::Result<(StdUnixListener, Unpublished)> {
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("another daemon is listening on {}", path.display()),
        ));
    }
    let temporary = temporary_path(path);
    let _ = fs::remove_file(&temporary);
    let listener = StdUnixListener::bind(&temporary)?;
    fs::set_permissions(&temporary, Permissions::from_mode(0o660))?;
    Ok((
        listener,
        Unpublished {
            temporary,
            path: path.to_path_buf(),
        },
    ))
}

/// [`bind_unpublished`], published at once.
pub fn bind(path: &Path) -> io::Result<StdUnixListener> {
    let (listener, unpublished) = bind_unpublished(path)?;
    unpublished.publish()?;
    Ok(listener)
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(name)
}

/// Accepts CLI connections until `shutdown`, then drains them.
pub async fn serve(node: Arc<Node>, listener: Arc<UnixListener>, shutdown: CancellationToken) {
    let tasks = TaskTracker::new();
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = shutdown.cancelled() => break,
        };
        let stream = match accepted {
            Ok((stream, _)) => stream,
            Err(error) => {
                tracing::warn!(target: target::IPC, %error, "accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            tracing::warn!(target: target::IPC, "too many IPC connections; closing");
            continue;
        };
        let node = Arc::clone(&node);
        let shutdown = shutdown.clone();
        tasks.spawn(async move {
            let _permit = permit;
            connection(node, stream, shutdown).await;
        });
    }
    tasks.close();
    let _ = tokio::time::timeout(crate::http::DRAIN_TIMEOUT, tasks.wait()).await;
}

async fn connection(node: Arc<Node>, stream: UnixStream, shutdown: CancellationToken) {
    tracing::debug!(target: target::IPC, "IPC connection");
    let (sink, stream) = Framed::new(stream, codec()).split();
    let incoming = stream.map(|frame| frame.map(BytesMut::freeze));
    connection::serve(node, Caller::Owner, incoming, sink, shutdown).await;
}

//! The CLI's side of the IPC socket.
//!
//! One call at a time, which is all the CLI needs. The original's client had
//! no error handler on its socket, so a stopped daemon or a missing
//! permission crashed it with a stack trace (RT-9); here each case gets a
//! sentence that says what to do.

use std::io;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use dassie_node::env::Env;
use dassie_node::ipc;
use dassie_node::rpc::proto::{
    self, client_message, request, response, server_message, stream_item,
};
use futures_util::{SinkExt, StreamExt};
use prost::Message;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// A call failed, for a reason fit to show the user.
#[derive(Debug)]
pub struct CallError {
    /// The contract's code, if the node answered.
    pub code: Option<proto::ErrorCode>,
    /// What to tell the user.
    pub message: String,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl CallError {
    fn transport(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }
}

/// A connection to the daemon.
pub struct Client {
    framed: Framed<UnixStream, LengthDelimitedCodec>,
    next_call: u32,
}

/// The socket paths to try, in order: `DASSIE_IPC_SOCKET_PATH` alone if set,
/// otherwise systemd's, then the standalone runtime's.
fn socket_paths(env: &Env) -> Vec<PathBuf> {
    if let Some(path) = env.path("DASSIE_IPC_SOCKET_PATH") {
        return vec![path];
    }
    let mut paths = vec![PathBuf::from("/run/dassie.sock")];
    if let Some(runtime) = env.path("XDG_RUNTIME_DIR") {
        paths.push(runtime.join("dassie.sock"));
    }
    paths
}

fn explain(path: &Path, error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => format!(
            "The Dassie daemon is not running (nothing listens on {}). Start it with \
             `systemctl start dassie` or `dassie daemon`.",
            path.display()
        ),
        io::ErrorKind::PermissionDenied => format!(
            "You may not use the Dassie daemon's socket {}. Run this as a member of the \
             dassie-users group (log in again after being added), or as root.",
            path.display()
        ),
        _ => format!(
            "Could not reach the Dassie daemon at {}: {error}",
            path.display()
        ),
    }
}

impl Client {
    /// Connects to the first socket that answers.
    pub async fn connect(env: &Env) -> Result<Self, CallError> {
        let mut last = None;
        for path in socket_paths(env) {
            match UnixStream::connect(&path).await {
                Ok(stream) => {
                    return Ok(Self {
                        framed: Framed::new(stream, ipc::codec()),
                        next_call: 0,
                    });
                }
                Err(error) => {
                    // A later path is more interesting only if this one is
                    // simply absent.
                    let absent = error.kind() == io::ErrorKind::NotFound;
                    if last.is_none() || !absent {
                        last = Some(explain(&path, &error));
                    }
                }
            }
        }
        Err(CallError::transport(
            last.unwrap_or_else(|| "No socket path to try".into()),
        ))
    }

    async fn send(&mut self, method: request::Method) -> Result<u32, CallError> {
        let call_id = self.next_call;
        self.next_call = self.next_call.wrapping_add(1);
        let message = proto::ClientMessage {
            call_id,
            kind: Some(client_message::Kind::Request(proto::Request {
                method: Some(method),
            })),
        };
        self.framed
            .send(Bytes::from(message.encode_to_vec()))
            .await
            .map_err(|error| {
                CallError::transport(format!("Lost the connection to the daemon: {error}"))
            })?;
        Ok(call_id)
    }

    async fn receive(&mut self, call_id: u32) -> Result<server_message::Kind, CallError> {
        loop {
            let frame = self
                .framed
                .next()
                .await
                .ok_or_else(|| CallError::transport("The daemon closed the connection"))?
                .map_err(|error| {
                    CallError::transport(format!("Lost the connection to the daemon: {error}"))
                })?;
            let message = proto::ServerMessage::decode(frame.freeze()).map_err(|_| {
                CallError::transport("The daemon sent something this CLI does not understand")
            })?;
            if message.call_id == call_id
                && let Some(kind) = message.kind
            {
                return Ok(kind);
            }
        }
    }

    /// Calls a unary method.
    pub async fn call(&mut self, method: request::Method) -> Result<response::Method, CallError> {
        let call_id = self.send(method).await?;
        match self.receive(call_id).await? {
            server_message::Kind::Response(proto::Response {
                method: Some(method),
            }) => Ok(method),
            server_message::Kind::Error(error) => Err(node_error(error)),
            _ => Err(CallError::transport("The daemon answered out of turn")),
        }
    }

    /// Calls a streaming method, handing each item to `on_item`.
    pub async fn stream(
        &mut self,
        method: request::Method,
        mut on_item: impl FnMut(stream_item::Method),
    ) -> Result<(), CallError> {
        let call_id = self.send(method).await?;
        loop {
            match self.receive(call_id).await? {
                server_message::Kind::Item(proto::StreamItem { method: Some(item) }) => {
                    on_item(item)
                }
                server_message::Kind::End(_) => return Ok(()),
                server_message::Kind::Error(error) => return Err(node_error(error)),
                _ => return Err(CallError::transport("The daemon answered out of turn")),
            }
        }
    }
}

fn node_error(error: proto::Error) -> CallError {
    CallError {
        code: proto::ErrorCode::try_from(error.code).ok(),
        message: error.message,
    }
}

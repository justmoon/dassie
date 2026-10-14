//! The settlement schemes' connections to their ledgers (spec 7 §4.5):
//! one WebSocket per scheme that follows a ledger, opened and closed when
//! the core says, and fed back to it with the connection's epoch, so a
//! message from a connection it has replaced is dropped. A connection that
//! drops is made again, backing off from 1 s to 60 s.
//!
//! `wss://` connects with the node's TLS trust; `ws://` only where private
//! addresses are allowed (dev and tests). Names resolve as for the node's
//! requests: public addresses only, unless private ones are allowed.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_util::sync::CancellationToken;

use super::{Command, LedgerEvent};
use crate::log::target;
use crate::peer_client::{PeerClient, WebRequest};

/// Most bytes in one message from a ledger.
const MAX_MESSAGE: usize = 1 << 20;
/// Messages waiting to go out on one connection; beyond, they are dropped,
/// and the scheme, hearing nothing, connects anew.
const QUEUE: usize = 64;
/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Bounds on asking a faucet.
const FAUCET_TIMEOUT_MS: u64 = 30_000;
const FAUCET_MAX_BYTES: usize = 64 * 1024;

struct Link {
    epoch: u64,
    outgoing: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
}

/// The open connections, by scheme.
#[derive(Default)]
pub(super) struct Links {
    by_scheme: BTreeMap<String, Link>,
}

impl Links {
    /// Opens `scheme`'s connection numbered `epoch` to `url`, replacing any
    /// it had. It ends with the engine's generation.
    pub(super) fn connect(
        &mut self,
        client: Option<PeerClient>,
        commands: mpsc::Sender<Command>,
        generation: &CancellationToken,
        scheme: String,
        epoch: u64,
        url: String,
    ) {
        self.close(&scheme, None);
        let (outgoing, queue) = mpsc::channel(QUEUE);
        let cancel = generation.child_token();
        let task = Task {
            client,
            commands,
            scheme: scheme.clone(),
            epoch,
            url,
        };
        tokio::spawn(task.run(queue, cancel.clone()));
        self.by_scheme.insert(
            scheme,
            Link {
                epoch,
                outgoing,
                cancel,
            },
        );
    }

    /// Closes `scheme`'s connection, if it is `epoch` or any.
    pub(super) fn close(&mut self, scheme: &str, epoch: Option<u64>) {
        let matches = self
            .by_scheme
            .get(scheme)
            .is_some_and(|link| epoch.is_none_or(|epoch| link.epoch == epoch));
        if matches && let Some(link) = self.by_scheme.remove(scheme) {
            link.cancel.cancel();
        }
    }

    /// Sends a message on `scheme`'s connection `epoch`.
    pub(super) fn send(&self, scheme: &str, epoch: u64, message: Vec<u8>) {
        let Some(link) = self
            .by_scheme
            .get(scheme)
            .filter(|link| link.epoch == epoch)
        else {
            return;
        };
        if link.outgoing.try_send(message).is_err() {
            tracing::warn!(target: target::SETTLEMENT, scheme, "a ledger connection is not keeping up: a message was dropped");
        }
    }
}

/// Asks a faucet for `scheme`, and tells the core how it went.
pub(super) fn fund(
    client: Option<PeerClient>,
    commands: mpsc::Sender<Command>,
    scheme: String,
    epoch: u64,
    url: String,
    body: Vec<u8>,
) {
    tokio::spawn(async move {
        let funded = match client {
            Some(client) => {
                let request = WebRequest {
                    method: http::Method::POST,
                    url,
                    headers: vec![(http::header::CONTENT_TYPE, "application/json".into())],
                    body: Some(body),
                    timeout_ms: FAUCET_TIMEOUT_MS,
                    max_response_bytes: FAUCET_MAX_BYTES,
                };
                match client.web(request).await {
                    Ok(response) => (200..300).contains(&response.status),
                    Err(error) => {
                        tracing::warn!(target: target::SETTLEMENT, %scheme, ?error, "the faucet did not answer");
                        false
                    }
                }
            }
            None => false,
        };
        tracing::info!(target: target::SETTLEMENT, %scheme, funded, "asked the faucet");
        report(&commands, scheme, epoch, LedgerEvent::Funded(funded)).await;
    });
}

/// Feeds an event to the core.
async fn report(commands: &mpsc::Sender<Command>, scheme: String, epoch: u64, event: LedgerEvent) {
    let call: super::Call = Box::new(move |core| {
        core.ledger_event(&scheme, epoch, &event);
        Box::new(|| {})
    });
    let _ = commands.send(Command::Call(call)).await;
}

struct Task {
    client: Option<PeerClient>,
    commands: mpsc::Sender<Command>,
    scheme: String,
    epoch: u64,
    url: String,
}

impl Task {
    async fn run(self, mut queue: mpsc::Receiver<Vec<u8>>, cancel: CancellationToken) {
        let mut backoff = BACKOFF_MIN;
        loop {
            let connected = tokio::select! {
                () = cancel.cancelled() => return,
                result = tokio::time::timeout(CONNECT_TIMEOUT, self.connect()) => {
                    result.unwrap_or_else(|_| Err("timed out".into()))
                }
            };
            match connected {
                Ok(socket) => {
                    backoff = BACKOFF_MIN;
                    tracing::info!(target: target::SETTLEMENT, scheme = %self.scheme, url = %self.url, "connected to the ledger");
                    self.event(LedgerEvent::Connected).await;
                    self.serve_any(socket, &mut queue, &cancel).await;
                    self.event(LedgerEvent::Disconnected).await;
                    if cancel.is_cancelled() {
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(target: target::SETTLEMENT, scheme = %self.scheme, url = %self.url, %error, "cannot connect to the ledger");
                }
            }
            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }

    async fn event(&self, event: LedgerEvent) {
        report(&self.commands, self.scheme.clone(), self.epoch, event).await;
    }

    async fn serve<S>(
        &self,
        mut socket: tokio_tungstenite::WebSocketStream<S>,
        queue: &mut mpsc::Receiver<Vec<u8>>,
        cancel: &CancellationToken,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        // What was queued for an earlier connection goes nowhere: the
        // scheme asks again once connected.
        while queue.try_recv().is_ok() {}
        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    let _ = socket.close(None).await;
                    return;
                }
                message = queue.recv() => {
                    let Some(message) = message else { return };
                    let text = String::from_utf8_lossy(&message).into_owned();
                    if socket.send(Message::text(text)).await.is_err() {
                        return;
                    }
                }
                incoming = socket.next() => {
                    match incoming {
                        Some(Ok(Message::Text(text))) => {
                            self.event(LedgerEvent::Message(text.as_bytes().to_vec())).await;
                        }
                        Some(Ok(Message::Binary(bytes))) => {
                            self.event(LedgerEvent::Message(bytes.to_vec())).await;
                        }
                        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                        Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                    }
                }
            }
        }
    }

    async fn connect(&self) -> Result<Socket, String> {
        let client = self.client.as_ref().ok_or("no client")?;
        let (secure, rest) = if let Some(rest) = self.url.strip_prefix("wss://") {
            (true, rest)
        } else if let Some(rest) = self.url.strip_prefix("ws://") {
            if !client.allows_private() {
                return Err("ws:// is for dev and tests only".into());
            }
            (false, rest)
        } else {
            return Err("not a WebSocket URL".into());
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() || authority.contains('@') {
            return Err("not a WebSocket URL".into());
        }
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || host.starts_with('[') => {
                (host, port.parse::<u16>().map_err(|e| e.to_string())?)
            }
            _ => (authority, if secure { 443 } else { 80 }),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let addresses = client
            .resolve(host, port)
            .await
            .map_err(|e| e.to_string())?;
        let tcp = TcpStream::connect(addresses.as_slice())
            .await
            .map_err(|e| e.to_string())?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        if secure {
            let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
            let tls = tokio_rustls::TlsConnector::from(client.tls())
                .connect(name, tcp)
                .await
                .map_err(|e| e.to_string())?;
            let (socket, _) =
                tokio_tungstenite::client_async_with_config(self.url.as_str(), tls, Some(config))
                    .await
                    .map_err(|e| e.to_string())?;
            Ok(Socket::Tls(Box::new(socket)))
        } else {
            let (socket, _) =
                tokio_tungstenite::client_async_with_config(self.url.as_str(), tcp, Some(config))
                    .await
                    .map_err(|e| e.to_string())?;
            Ok(Socket::Plain(Box::new(socket)))
        }
    }
}

enum Socket {
    Tls(Box<tokio_tungstenite::WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>>),
    Plain(Box<tokio_tungstenite::WebSocketStream<TcpStream>>),
}

impl Task {
    async fn serve_any(
        &self,
        socket: Socket,
        queue: &mut mpsc::Receiver<Vec<u8>>,
        cancel: &CancellationToken,
    ) {
        match socket {
            Socket::Tls(socket) => self.serve(*socket, queue, cancel).await,
            Socket::Plain(socket) => self.serve(*socket, queue, cancel).await,
        }
    }
}

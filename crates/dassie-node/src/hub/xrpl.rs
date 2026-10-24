//! A hub's escrow on the validator's own XRPL server: one WebSocket per
//! hub, carrying out what its [`Watcher`] decides, and made again when it
//! drops, backing off from 1 s to 60 s.
//!
//! `wss://` connects with the node's TLS trust; `ws://` only where private
//! addresses are allowed (dev and tests). Names resolve as for the node's
//! requests: public addresses only, unless private ones are allowed.

use std::sync::Arc;
use std::time::Duration;

use dassie_hub::HubId;
use dassie_hub::operation::ValidatorOperation;
use dassie_xrpl::AccountId;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_util::sync::CancellationToken;

use super::driver::hex;
use super::escrow::{Out, Watcher};
use super::service::EscrowLinks;
use crate::clock::Clock;
use crate::log::target;
use crate::peer_client::PeerClient;
use crate::store::Store;

/// Most bytes in one message from the server.
const MAX_MESSAGE: usize = 1 << 20;
/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// The most attestations waiting for room in the validator's queue.
const MAX_WAITING_ATTESTATIONS: usize = 256;

/// How often the watcher's timers are looked at.
const TICK: Duration = Duration::from_secs(1);

/// Follows one hub's escrow until shutdown.
pub struct Follow {
    /// Where the cursor is kept.
    pub store: Arc<Store>,
    /// The hub.
    pub hub: HubId,
    /// The XRPL server.
    pub url: String,
    /// Resolves the server's name and trusts its certificate.
    pub client: Option<PeerClient>,
    /// The node's clock.
    pub clock: Arc<dyn Clock>,
    /// What it shares with the hub's validator.
    pub links: EscrowLinks,
    /// Attestations the validator's queue had no room for yet, in order.
    pub retry: std::collections::VecDeque<(ValidatorOperation, Option<[u8; 32]>)>,
}

impl Follow {
    /// Connects, follows, and connects again, until `shutdown`.
    pub async fn run(mut self, shutdown: CancellationToken) {
        let rules = self.links.view.borrow().rules.clone();
        let escrow = AccountId(rules.terms.escrow);
        let cursor = match self.store.hub_escrow(self.hub.0.as_bytes()) {
            Ok(cursor) => cursor,
            Err(error) => {
                tracing::error!(target: target::HUB, %error, "cannot read how far the escrow was followed; starting from its beginning");
                None
            }
        };
        let mut watcher = Watcher::new(escrow, rules.terms.network, cursor);
        let mut backoff = BACKOFF_MIN;
        loop {
            let connected = tokio::select! {
                () = shutdown.cancelled() => return,
                result = tokio::time::timeout(CONNECT_TIMEOUT, connect(self.client.as_ref(), &self.url)) => {
                    result.unwrap_or_else(|_| Err("timed out".into()))
                }
            };
            match connected {
                Ok(socket) => {
                    backoff = BACKOFF_MIN;
                    tracing::info!(target: target::HUB, hub = %hex(self.hub.0.as_bytes()), url = %self.url, "following the hub's escrow");
                    watcher.connected();
                    match socket {
                        Socket::Tls(socket) => self.serve(*socket, &mut watcher, &shutdown).await,
                        Socket::Plain(socket) => self.serve(*socket, &mut watcher, &shutdown).await,
                    }
                    watcher.disconnected();
                    self.links.connected.send_replace(false);
                }
                Err(error) => {
                    tracing::warn!(target: target::HUB, url = %self.url, %error, "cannot connect to the XRPL server to follow the hub's escrow");
                }
            }
            if !self.wait(&mut watcher, backoff, &shutdown).await {
                return;
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }

    /// Waits `backoff` before connecting again, queueing what the validator
    /// submits meanwhile; false at shutdown.
    async fn wait(
        &mut self,
        watcher: &mut Watcher,
        backoff: Duration,
        shutdown: &CancellationToken,
    ) -> bool {
        let sleep = tokio::time::sleep(backoff);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return false,
                () = &mut sleep => return true,
                blob = self.links.submits.recv() => match blob {
                    Some(blob) => watcher.submit(blob),
                    None => return false,
                },
            }
        }
    }

    async fn serve<S>(
        &mut self,
        mut socket: WebSocketStream<S>,
        watcher: &mut Watcher,
        shutdown: &CancellationToken,
    ) where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut tick = tokio::time::interval(TICK);
        loop {
            if !self.flush(&mut socket, watcher).await {
                return;
            }
            let now = self.clock.now_ms();
            tokio::select! {
                () = shutdown.cancelled() => {
                    let _ = socket.close(None).await;
                    return;
                }
                incoming = socket.next() => match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let view = self.links.view.borrow().clone();
                        watcher.message(now, text.as_bytes(), &view);
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        let view = self.links.view.borrow().clone();
                        watcher.message(now, &bytes, &view);
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                },
                blob = self.links.submits.recv() => match blob {
                    Some(blob) => watcher.submit(blob),
                    None => return,
                },
                _ = tick.tick() => {
                    let view = self.links.view.borrow().clone();
                    watcher.tick(now, &view);
                }
                changed = self.links.view.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let view = self.links.view.borrow_and_update().clone();
                    watcher.tick(now, &view);
                }
            }
        }
    }

    /// Hands the attestations waiting to the validator, in order, while
    /// its queue has room; each it took the watcher hears of.
    fn hand_over(&mut self, watcher: &mut Watcher) {
        while let Some((operation, transaction)) = self.retry.pop_front() {
            match self.links.attest.try_send(operation) {
                Ok(()) => watcher.taken(transaction),
                Err(tokio::sync::mpsc::error::TrySendError::Full(operation)) => {
                    self.retry.push_front((operation, transaction));
                    return;
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
    }

    /// Carries out what the watcher decided; false once the socket failed.
    async fn flush<S>(&mut self, socket: &mut WebSocketStream<S>, watcher: &mut Watcher) -> bool
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.hand_over(watcher);
        while let Some(out) = watcher.poll() {
            match out {
                Out::Send(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    if socket.send(Message::text(text)).await.is_err() {
                        return false;
                    }
                }
                Out::Attest(operation, transaction) => {
                    // Past the bound the oldest goes: its transaction is
                    // never taken, so the cursor stays before it, and a
                    // restart reads it again.
                    if self.retry.len() >= MAX_WAITING_ATTESTATIONS
                        && let Some((_, dropped)) = self.retry.pop_front()
                    {
                        watcher.dropped(dropped);
                    }
                    self.retry.push_back((operation, transaction));
                    self.hand_over(watcher);
                }
                Out::Cursor(cursor) => {
                    if let Err(error) = self.store.set_hub_escrow(self.hub.0.as_bytes(), &cursor) {
                        tracing::warn!(target: target::HUB, %error, "cannot record how far the escrow was followed");
                    }
                }
                Out::Problem(problem) => {
                    tracing::warn!(target: target::HUB, hub = %hex(self.hub.0.as_bytes()), "{problem}");
                }
            }
        }
        self.links.connected.send_replace(watcher.following());
        true
    }
}

enum Socket {
    Tls(Box<WebSocketStream<tokio_rustls::client::TlsStream<TcpStream>>>),
    Plain(Box<WebSocketStream<TcpStream>>),
}

/// A WebSocket to `url`, as the node connects to its ledgers.
async fn connect(client: Option<&PeerClient>, url: &str) -> Result<Socket, String> {
    let client = client.ok_or("no peer client")?;
    let (secure, rest) = if let Some(rest) = url.strip_prefix("wss://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("ws://") {
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
    let (host, port) = host_and_port(authority, if secure { 443 } else { 80 })?;
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
        let (socket, _) = tokio_tungstenite::client_async_with_config(url, tls, Some(config))
            .await
            .map_err(|e| e.to_string())?;
        Ok(Socket::Tls(Box::new(socket)))
    } else {
        let (socket, _) = tokio_tungstenite::client_async_with_config(url, tcp, Some(config))
            .await
            .map_err(|e| e.to_string())?;
        Ok(Socket::Plain(Box::new(socket)))
    }
}

/// The host and port of a URL's authority: `host`, `host:port`, `[v6]` or
/// `[v6]:port`.
fn host_and_port(authority: &str, default: u16) -> Result<(&str, u16), String> {
    let port = |text: &str| text.parse::<u16>().map_err(|e| e.to_string());
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or("an unclosed IPv6 address")?;
        return match after.strip_prefix(':') {
            Some(number) => Ok((host, port(number)?)),
            None if after.is_empty() => Ok((host, default)),
            None => Err("not a WebSocket URL".into()),
        };
    }
    match authority.rsplit_once(':') {
        Some((host, number)) => Ok((host, port(number)?)),
        None => Ok((authority, default)),
    }
}

/// How long a request to the XRPL server waits for its answer.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// The most messages read while waiting for one answer: those the server
/// pushes unasked are skipped.
const MAX_SKIPPED: usize = 1_024;

/// What a request to the XRPL server came back with: its result, or the
/// server's error.
pub(super) type Answer = Result<serde_json::Value, dassie_xrpl::api::ApiError>;

/// A WebSocket to an XRPL server that only asks and reads the answers: how
/// a sponsor sets up its hub's escrow (spec/areas/hubs.md#founding-a-hub).
pub(super) struct Session {
    socket: Socket,
    next_id: u64,
}

impl Session {
    /// Connects to `url`, as the node connects to its ledgers.
    pub(super) async fn connect(client: Option<&PeerClient>, url: &str) -> Result<Self, String> {
        let socket = tokio::time::timeout(CONNECT_TIMEOUT, connect(client, url))
            .await
            .map_err(|_| "timed out".to_string())??;
        Ok(Self { socket, next_id: 1 })
    }

    /// Sends the request `build` makes under a fresh ID, and reads until
    /// its answer.
    pub(super) async fn call(
        &mut self,
        build: impl FnOnce(u64) -> Vec<u8>,
    ) -> Result<Answer, String> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        let text = String::from_utf8_lossy(&build(id)).into_owned();
        let exchange = async {
            match &mut self.socket {
                Socket::Tls(socket) => answer_of(socket.as_mut(), id, text).await,
                Socket::Plain(socket) => answer_of(socket.as_mut(), id, text).await,
            }
        };
        tokio::time::timeout(CALL_TIMEOUT, exchange)
            .await
            .map_err(|_| "the XRPL server did not answer".to_string())?
    }
}

async fn answer_of<S>(
    socket: &mut WebSocketStream<S>,
    id: u64,
    text: String,
) -> Result<Answer, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    socket
        .send(Message::text(text))
        .await
        .map_err(|error| error.to_string())?;
    for _ in 0..MAX_SKIPPED {
        let bytes = match socket.next().await {
            Some(Ok(Message::Text(text))) => text.as_bytes().to_vec(),
            Some(Ok(Message::Binary(bytes))) => bytes.to_vec(),
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
            Some(Ok(Message::Close(_)) | Err(_)) | None => {
                return Err("the XRPL server closed the connection".into());
            }
        };
        if let dassie_xrpl::api::Message::Response {
            id: answered,
            outcome,
        } = dassie_xrpl::api::parse(&bytes)
            && answered == id
        {
            return Ok(outcome);
        }
    }
    Err("the XRPL server did not answer".into())
}

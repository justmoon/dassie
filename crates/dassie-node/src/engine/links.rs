//! The external assets' connections to their ledgers
//! (spec/areas/settlement.md#assets-that-follow-a-ledger): one WebSocket per
//! external asset that follows a ledger, however many schemes settle
//! through it, opened and closed when the core says, and
//! fed back to it with the connection's epoch, so a message from a connection
//! it has replaced is dropped. A connection that drops is made again, backing
//! off from 1 s to 60 s.
//!
//! `wss://` connects with the node's TLS trust; `ws://` only where private
//! addresses are allowed (dev and tests). Names resolve as for the node's
//! requests: public addresses only, unless private ones are allowed.
//!
//! Beside the connections, what an asset asks of the web: a faucet's
//! funding, and documents it fetches, such as its validator lists, over
//! HTTPS, following no redirect (spec/designs/xrpl-validation.md#9-io).

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

/// Most bytes in one message from a ledger: the longest answer an XRPL
/// account reads, a proof server's proofs and validations
/// ([`dassie_xrpl::api::MAX_ANSWER_BYTES`]). A server whose message is
/// longer is cut off, and the asset connects anew.
const MAX_LEDGER_MESSAGE: usize = dassie_xrpl::api::MAX_ANSWER_BYTES;
/// Messages waiting to go out on one connection; beyond, they are dropped,
/// and the asset, hearing nothing, connects anew.
const QUEUE: usize = 64;
/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Bounds on asking a faucet.
const FAUCET_TIMEOUT_MS: u64 = 30_000;
const FAUCET_MAX_BYTES: usize = 64 * 1024;
/// How long a fetch may take: within the account's own bound
/// ([`dassie_xrpl::account::FETCH_TIMEOUT_MS`]), so its answer is still
/// taken.
const FETCH_TIMEOUT_MS: u64 = dassie_xrpl::account::FETCH_TIMEOUT_MS / 2;

struct Link {
    epoch: u64,
    outgoing: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
}

/// The open connections, by asset.
#[derive(Default)]
pub(super) struct Links {
    by_asset: BTreeMap<String, Link>,
}

impl Links {
    /// Opens `asset`'s connection numbered `epoch` to `url`, replacing any
    /// it had. It ends with the engine's generation.
    pub(super) fn connect(
        &mut self,
        client: Option<PeerClient>,
        commands: mpsc::Sender<Command>,
        generation: &CancellationToken,
        asset: String,
        epoch: u64,
        url: String,
    ) {
        self.close(&asset, None);
        let (outgoing, queue) = mpsc::channel(QUEUE);
        let cancel = generation.child_token();
        let task = Task {
            client,
            commands,
            asset: asset.clone(),
            epoch,
            url,
        };
        tokio::spawn(task.run(queue, cancel.clone()));
        self.by_asset.insert(
            asset,
            Link {
                epoch,
                outgoing,
                cancel,
            },
        );
    }

    /// Closes `asset`'s connection, if it is `epoch` or any.
    pub(super) fn close(&mut self, asset: &str, epoch: Option<u64>) {
        let matches = self
            .by_asset
            .get(asset)
            .is_some_and(|link| epoch.is_none_or(|epoch| link.epoch == epoch));
        if matches && let Some(link) = self.by_asset.remove(asset) {
            link.cancel.cancel();
        }
    }

    /// Sends a message on `asset`'s connection `epoch`.
    pub(super) fn send(&self, asset: &str, epoch: u64, message: Vec<u8>) {
        let Some(link) = self.by_asset.get(asset).filter(|link| link.epoch == epoch) else {
            return;
        };
        if link.outgoing.try_send(message).is_err() {
            tracing::warn!(target: target::SETTLEMENT, asset, "a ledger connection is not keeping up: a message was dropped");
        }
    }
}

/// Asks a faucet for `asset`, and tells the core how it went.
pub(super) fn fund(
    client: Option<PeerClient>,
    commands: mpsc::Sender<Command>,
    asset: String,
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
                        tracing::warn!(target: target::SETTLEMENT, %asset, ?error, "the faucet did not answer");
                        false
                    }
                }
            }
            None => false,
        };
        tracing::info!(target: target::SETTLEMENT, %asset, funded, "asked the faucet");
        report(&commands, asset, epoch, LedgerEvent::Funded(funded)).await;
    });
}

/// A document an asset fetches.
pub(super) struct Fetch {
    /// The asset.
    pub asset: String,
    /// Its connection, which the answer is for.
    pub epoch: u64,
    /// What to fetch: `https://`.
    pub url: String,
    /// The longest body taken.
    pub max_bytes: usize,
}

/// Fetches a document for an asset by an HTTPS GET, and tells the core what
/// came: the body of a successful answer, or nothing. A redirect is no
/// answer: the client follows none, so a site is the address that serves
/// it.
pub(super) fn fetch(client: Option<PeerClient>, commands: mpsc::Sender<Command>, fetch: Fetch) {
    let Fetch {
        asset,
        epoch,
        url,
        max_bytes,
    } = fetch;
    tokio::spawn(async move {
        let body = match client {
            Some(client) => {
                let request = WebRequest {
                    method: http::Method::GET,
                    url: url.clone(),
                    headers: Vec::new(),
                    body: None,
                    timeout_ms: FETCH_TIMEOUT_MS,
                    max_response_bytes: max_bytes,
                };
                match client.web(request).await {
                    Ok(response) if (200..300).contains(&response.status) => Some(response.body),
                    Ok(response) => {
                        tracing::warn!(target: target::SETTLEMENT, %asset, %url, status = response.status, "a fetch was refused");
                        None
                    }
                    Err(error) => {
                        tracing::warn!(target: target::SETTLEMENT, %asset, %url, ?error, "a fetch failed");
                        None
                    }
                }
            }
            None => None,
        };
        tracing::debug!(target: target::SETTLEMENT, %asset, %url, fetched = body.is_some(), "fetched");
        report(&commands, asset, epoch, LedgerEvent::Fetched { url, body }).await;
    });
}

/// A message for one validator of a settlement hub.
pub(super) struct HubSend {
    /// The hub account it is for.
    pub asset: String,
    /// The account's connection, which the answer is for.
    pub epoch: u64,
    /// The hub.
    pub hub: dassie_hub::HubId,
    /// The validator's place in the hub's rules.
    pub endpoint: usize,
    /// The validator's node key, which the connection is pinned to.
    pub node: dassie_identity::PublicKey,
    /// Where it is reached: `host:port`.
    pub address: String,
    /// The message.
    pub message: Vec<u8>,
}

/// How long one exchange with a validator may take: the account's interval
/// for sending an operation again (`dassie_hub::member::account::RESUBMIT_MS`). Its
/// polls come more often than that, and a later answer is superseded by
/// the next one's.
const HUB_EXCHANGE_TIMEOUT: Duration =
    Duration::from_millis(dassie_hub::member::account::RESUBMIT_MS);

/// Sends a hub account's message (a hub `Message::Request`) to one
/// validator on a task of its own, over the hub's stream
/// (spec/areas/peer-protocol.md#the-hubs-stream): QUIC pinned to its node
/// key. Each message it sends
/// back goes to the core as from that validator. What does not arrive, or
/// finds the account with as many exchanges in flight with that validator
/// as it may have (`MAX_EXCHANGES_PER_VALIDATOR`), the account sends again.
pub(super) fn hub_send(
    client: Option<crate::hub::HubClient>,
    hubs: &crate::hub::Hubs,
    commands: mpsc::Sender<Command>,
    send: HubSend,
) {
    let Some(client) = client else {
        tracing::warn!(target: target::SETTLEMENT, asset = %send.asset, endpoint = send.endpoint, "no peer client, so no hub validator is reached");
        return;
    };
    let Some(permit) = hubs.exchange_slot(&send.asset, send.endpoint) else {
        tracing::debug!(target: target::SETTLEMENT, asset = %send.asset, endpoint = send.endpoint, "a hub exchange still in flight: the account asks again");
        return;
    };
    tokio::spawn(async move {
        let _permit = permit;
        let HubSend {
            asset,
            epoch,
            hub,
            endpoint,
            node,
            address,
            message,
        } = send;
        let to = node.node_id();
        let exchanged = client
            .exchange(to, &address, &hub, &message, HUB_EXCHANGE_TIMEOUT)
            .await;
        match exchanged {
            Ok(messages) => {
                for message in messages {
                    let event = LedgerEvent::From { endpoint, message };
                    report(&commands, asset.clone(), epoch, event).await;
                }
            }
            Err(error) => {
                tracing::debug!(target: target::SETTLEMENT, %asset, endpoint, %error, "a hub validator did not answer");
            }
        }
    });
}

/// Feeds an event to the core.
async fn report(commands: &mpsc::Sender<Command>, asset: String, epoch: u64, event: LedgerEvent) {
    let call: super::Call = Box::new(move |core| {
        core.ledger_event(&asset, epoch, &event);
        Box::new(|| {})
    });
    let _ = commands.send(Command::Call(call)).await;
}

struct Task {
    client: Option<PeerClient>,
    commands: mpsc::Sender<Command>,
    asset: String,
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
                    tracing::info!(target: target::SETTLEMENT, asset = %self.asset, url = %self.url, "connected to the ledger");
                    // What was queued for an earlier connection goes
                    // nowhere, before the asset hears it is connected and
                    // queues what it asks first.
                    while queue.try_recv().is_ok() {}
                    self.event(LedgerEvent::Connected).await;
                    self.serve_any(socket, &mut queue, &cancel).await;
                    self.event(LedgerEvent::Disconnected).await;
                    if cancel.is_cancelled() {
                        return;
                    }
                }
                Err(error) => {
                    tracing::warn!(target: target::SETTLEMENT, asset = %self.asset, url = %self.url, %error, "cannot connect to the ledger");
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
        report(&self.commands, self.asset.clone(), self.epoch, event).await;
    }

    async fn serve<S>(
        &self,
        mut socket: tokio_tungstenite::WebSocketStream<S>,
        queue: &mut mpsc::Receiver<Vec<u8>>,
        cancel: &CancellationToken,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
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
        let (host, port) = host_and_port(authority, if secure { 443 } else { 80 })?;
        let addresses = client
            .resolve(host, port)
            .await
            .map_err(|e| e.to_string())?;
        let tcp = TcpStream::connect(addresses.as_slice())
            .await
            .map_err(|e| e.to_string())?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_LEDGER_MESSAGE))
            .max_frame_size(Some(MAX_LEDGER_MESSAGE));
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

#[cfg(test)]
mod tests {
    use super::host_and_port;

    #[test]
    fn hosts_and_ports_are_read_as_written() {
        assert_eq!(host_and_port("a.example", 443), Ok(("a.example", 443)));
        assert_eq!(
            host_and_port("a.example:51233", 443),
            Ok(("a.example", 51233))
        );
        assert_eq!(host_and_port("[::1]", 80), Ok(("::1", 80)));
        assert_eq!(host_and_port("[::1]:6006", 80), Ok(("::1", 6006)));
        assert!(host_and_port("[::1", 80).is_err());
        assert!(host_and_port("a.example:x", 80).is_err());
    }
}

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
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::{Command, LedgerEvent};
use crate::log::target;
use crate::outbound::{OutboundClient, WebRequest, WebSocket};

/// Most bytes in one message from a ledger: the longest answer an XRPL
/// account reads, a proof server's proofs and validations
/// ([`dassie_xrpl::api::MAX_ANSWER_BYTES`]). A server whose message is
/// longer is cut off, and the asset connects anew.
const MAX_LEDGER_MESSAGE: usize = dassie_xrpl::api::MAX_ANSWER_BYTES;
/// Most bytes of a ledger's messages one link holds on their way to the
/// core: two of the longest, so the core can work on one while the next
/// waits. A link that holds as much reads nothing more from its server
/// until the core has taken some, and the server, its socket full, waits
/// too. Without it, a hostile or broken server could fill the engine's
/// whole command queue with its longest messages while the core is
/// busy. Beside these, a link holds at most the one message it is reading
/// ([`MAX_LEDGER_MESSAGE`]).
const MAX_HELD_BYTES: usize = 2 * MAX_LEDGER_MESSAGE;
const _: () = assert!(MAX_LEDGER_MESSAGE <= MAX_HELD_BYTES);
const _: () = assert!(MAX_HELD_BYTES <= u32::MAX as usize);
const _: () = assert!(MAX_HELD_BYTES <= Semaphore::MAX_PERMITS);
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
        client: Option<OutboundClient>,
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
            held: Arc::new(Semaphore::new(MAX_HELD_BYTES)),
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
    client: Option<OutboundClient>,
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
pub(super) fn fetch(client: Option<OutboundClient>, commands: mpsc::Sender<Command>, fetch: Fetch) {
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
    /// The validator's identity, whose node ID the connection is pinned to.
    pub node: dassie_identity::NodeIdentity,
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
    client: crate::hub::HubClient,
    hubs: &crate::hub::Hubs,
    commands: mpsc::Sender<Command>,
    send: HubSend,
) {
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
    let _ = commands.send(ledger_call(asset, epoch, event, None)).await;
}

/// The command that feeds an event to the core, holding `held` (its share
/// of a link's [`MAX_HELD_BYTES`]) until the core has taken it, or the
/// command is dropped.
fn ledger_call(
    asset: String,
    epoch: u64,
    event: LedgerEvent,
    held: Option<OwnedSemaphorePermit>,
) -> Command {
    Command::Call(Box::new(move |core| {
        core.ledger_event(&asset, epoch, &event);
        drop(held);
        Box::new(|| {})
    }))
}

struct Task {
    client: Option<OutboundClient>,
    commands: mpsc::Sender<Command>,
    asset: String,
    epoch: u64,
    url: String,
    /// What is left of [`MAX_HELD_BYTES`]: a message waiting for the core
    /// holds its length.
    held: Arc<Semaphore>,
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
                    self.serve(socket, &mut queue, &cancel).await;
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
                            if !self.deliver(text.as_bytes(), cancel).await {
                                let _ = socket.close(None).await;
                                return;
                            }
                        }
                        Some(Ok(Message::Binary(bytes))) => {
                            if !self.deliver(&bytes, cancel).await {
                                let _ = socket.close(None).await;
                                return;
                            }
                        }
                        Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                        Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                    }
                }
            }
        }
    }

    /// Feeds a message from the ledger to the core once the link has room
    /// to hold it ([`MAX_HELD_BYTES`]); until then the link reads nothing
    /// more. False if the link ends meanwhile, or the message could never
    /// be held, and the connection is done.
    async fn deliver(&self, message: &[u8], cancel: &CancellationToken) -> bool {
        if message.len() > MAX_HELD_BYTES {
            return false;
        }
        let Ok(bytes) = u32::try_from(message.len()) else {
            return false;
        };
        let permit = tokio::select! {
            () = cancel.cancelled() => return false,
            permit = Arc::clone(&self.held).acquire_many_owned(bytes) => {
                let Ok(permit) = permit else { return false };
                permit
            }
        };
        let event = LedgerEvent::Message(message.to_vec());
        let command = ledger_call(self.asset.clone(), self.epoch, event, Some(permit));
        tokio::select! {
            () = cancel.cancelled() => false,
            sent = self.commands.send(command) => sent.is_ok(),
        }
    }

    async fn connect(&self) -> Result<WebSocket, String> {
        let client = self.client.as_ref().ok_or("no client")?;
        client.websocket(&self.url, MAX_LEDGER_MESSAGE).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use futures_util::SinkExt;
    use tokio::sync::{Semaphore, mpsc};
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::{Role, WebSocketConfig};
    use tokio_util::sync::CancellationToken;

    use super::{MAX_HELD_BYTES, MAX_LEDGER_MESSAGE, Task};

    /// Each of the flood's messages.
    const FLOOD_MESSAGE: usize = 1 << 20;
    /// As many as the link may hold.
    const FLOOD_HELD: usize = MAX_HELD_BYTES / FLOOD_MESSAGE;
    const _: () = assert!(FLOOD_HELD * FLOOD_MESSAGE == MAX_HELD_BYTES);

    /// Waits, a bounded while, until `receiver` holds `count` commands.
    async fn queued(receiver: &mpsc::Receiver<super::Command>, count: usize) {
        for _ in 0..1000 {
            assert!(receiver.len() <= count, "the link holds too much");
            if receiver.len() == count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the link never delivered {count} messages");
    }

    #[tokio::test]
    async fn a_flooding_server_never_makes_a_link_hold_more_than_the_bound() {
        let (node_side, server_side) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let mut socket =
                WebSocketStream::from_raw_socket(server_side, Role::Server, None).await;
            let flood = "x".repeat(FLOOD_MESSAGE);
            while socket.send(Message::text(flood.clone())).await.is_ok() {}
        });
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_LEDGER_MESSAGE))
            .max_frame_size(Some(MAX_LEDGER_MESSAGE));
        let socket = WebSocketStream::from_raw_socket(node_side, Role::Client, Some(config)).await;
        // The core is stalled: nothing takes the commands.
        let (commands, mut receiver) = mpsc::channel(super::super::QUEUE);
        let held = Arc::new(Semaphore::new(MAX_HELD_BYTES));
        let task = Task {
            client: None,
            commands,
            asset: "XRP".into(),
            epoch: 1,
            url: "ws://ledger.test".into(),
            held: Arc::clone(&held),
        };
        let cancel = CancellationToken::new();
        let (_outgoing, mut queue) = mpsc::channel(1);
        let serving = tokio::spawn({
            let cancel = cancel.clone();
            async move { task.serve(socket, &mut queue, &cancel).await }
        });

        queued(&receiver, FLOOD_HELD).await;
        // However long the server keeps on, the link reads no more.
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert_eq!(receiver.len(), FLOOD_HELD);
            assert_eq!(held.available_permits(), 0);
        }

        // Once the core takes them, the link reads on, as far as the bound.
        while receiver.try_recv().is_ok() {}
        queued(&receiver, FLOOD_HELD).await;

        cancel.cancel();
        assert!(
            tokio::time::timeout(Duration::from_secs(10), serving)
                .await
                .is_ok_and(|joined| joined.is_ok())
        );
        server.abort();
    }
}

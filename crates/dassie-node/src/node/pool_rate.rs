//! XRP's rate from mainnet's XRP/RLUSD pool (spec/areas/xrpl.md#xrps-rate):
//! while the owner's rate sources name the pool, one task follows mainnet's
//! validated ledgers on a WebSocket to the source's server, with no key,
//! and carries out what its reader ([`PoolRate`]) asks. The rate fetcher
//! takes the reader's rate as that source's answer.
//!
//! The WebSocket is made again when it drops or the reader asks for a new
//! one, backing off from 1 s to 60 s, as a hub's link to its server is; the
//! validator list publishers' sites are fetched over HTTPS with the node's
//! client. `wss://` connects with the node's TLS trust; `ws://` only where
//! private addresses are allowed (dev and tests).

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dassie_connector::RATE_PRECISION;
use dassie_xrpl::pool_rate::{Out, PoolRate, Timings};
use dassie_xrpl::unl::Secp256k1Signatures;
use dassie_xrpl::validator_list::{MAX_LIST_BYTES, UnlConfig};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::clock::{Clock, clock_moved};
use crate::log::target;
use crate::outbound::{OutboundClient, WebRequest, WebSocket};

/// Most bytes in one message from the server.
const MAX_MESSAGE: usize = 1 << 20;
/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// How often the reader's timers are looked at on the system clock; a clock
/// moved by hand has them looked at whenever it moves.
const TICK: Duration = Duration::from_secs(1);
/// How long a fetch of a validator list may take: well within the time the
/// verifier gives it, so its answer still counts.
const FETCH_TIMEOUT_MS: u64 = dassie_xrpl::lists::FETCH_TIMEOUT_MS / 2;
/// The most fetches whose answers wait for the reader: one per publisher.
const MAX_FETCHED: usize = dassie_xrpl::validator_list::MAX_PUBLISHERS;
/// The oldest the reader's rate may be for the rate fetcher to take it:
/// the reader tells it every [`TICK`], so an older one is of a task gone.
const MAX_TOLD_AGE_MS: u64 = 10_000;

/// A fetch's site and the body that came, if one did.
type Fetched = (String, Option<Vec<u8>>);

/// The rate the reader last told, in XRP per dollar at the connector's
/// precision, if it had one, and when it told it.
pub type Told = Arc<Mutex<(Option<u128>, u64)>>;

/// The task following the pool, while a rate source names it.
pub struct Follower {
    url: String,
    stop: CancellationToken,
    told: Told,
}

impl Follower {
    /// Starts following the pool on the server at `url`, until `shutdown`
    /// or until it is dropped.
    pub fn start(
        url: &str,
        client: Option<OutboundClient>,
        clock: Arc<dyn Clock>,
        (timings, unl): (Timings, Option<&'static UnlConfig>),
        shutdown: &CancellationToken,
    ) -> Self {
        let stop = shutdown.child_token();
        let told: Told = Arc::new(Mutex::new((None, 0)));
        let config = unl.cloned().unwrap_or_else(UnlConfig::mainnet);
        let reader = PoolRate::new(
            dassie_xrpl::network::MAINNET.id,
            config,
            Box::new(Secp256k1Signatures),
            timings,
            RATE_PRECISION,
        );
        let task = Task {
            url: url.to_owned(),
            client,
            clock,
            reader,
            told: Arc::clone(&told),
        };
        let until = stop.clone();
        tokio::spawn(async move { task.run(until).await });
        tracing::info!(target: target::ENGINE, %url, "following mainnet's XRP/RLUSD pool for XRP's rate");
        Self {
            url: url.to_owned(),
            stop,
            told,
        }
    }

    /// The server it follows the pool on.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// XRP's rate as the reader tells it at `now_ms`, if it has one.
    #[must_use]
    pub fn rate(&self, now_ms: u64) -> Option<u128> {
        let (rate, told_ms) = *self.told.lock().unwrap_or_else(PoisonError::into_inner);
        rate.filter(|_| now_ms.saturating_sub(told_ms) <= MAX_TOLD_AGE_MS)
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// The reader and its connection.
struct Task {
    url: String,
    client: Option<OutboundClient>,
    clock: Arc<dyn Clock>,
    reader: PoolRate,
    told: Told,
}

impl Task {
    /// Follows the pool, connecting and connecting again, until `stop`.
    async fn run(mut self, stop: CancellationToken) {
        let (fetches, mut fetched) = mpsc::channel(MAX_FETCHED);
        let mut backoff = BACKOFF_MIN;
        loop {
            let connected = tokio::select! {
                () = stop.cancelled() => return,
                result = tokio::time::timeout(CONNECT_TIMEOUT, connect(self.client.as_ref(), &self.url)) => {
                    result.unwrap_or_else(|_| Err("timed out".into()))
                }
            };
            match connected {
                Ok(socket) => {
                    backoff = BACKOFF_MIN;
                    self.reader.connected(self.clock.now_ms());
                    self.serve(socket, &fetches, &mut fetched, &stop).await;
                    self.reader.disconnected();
                }
                Err(error) => {
                    tracing::warn!(target: target::ENGINE, url = %self.url, %error, "cannot connect to the XRPL server for XRP's rate");
                }
            }
            if !self.back_off(backoff, &stop).await {
                return;
            }
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    }

    /// Waits `backoff` before connecting again, telling the rate fetcher
    /// the reader's rate all along: the readings it holds still count
    /// until their ledgers leave the window. False once stopped.
    async fn back_off(&mut self, backoff: Duration, stop: &CancellationToken) -> bool {
        let sleep = tokio::time::sleep(backoff);
        tokio::pin!(sleep);
        let mut tick = tokio::time::interval(TICK);
        let mut moves = self.clock.subscribe();
        loop {
            self.tell();
            tokio::select! {
                () = stop.cancelled() => return false,
                () = &mut sleep => return true,
                _ = tick.tick() => {}
                () = clock_moved(&mut moves) => {}
            }
        }
    }

    async fn serve(
        &mut self,
        mut socket: WebSocket,
        fetches: &mpsc::Sender<Fetched>,
        fetched: &mut mpsc::Receiver<Fetched>,
        stop: &CancellationToken,
    ) {
        let clock = Arc::clone(&self.clock);
        let mut tick = tokio::time::interval(TICK);
        let mut moves = clock.subscribe();
        loop {
            if !self.flush(&mut socket, fetches).await {
                let _ = socket.close(None).await;
                return;
            }
            self.tell();
            tokio::select! {
                () = stop.cancelled() => {
                    let _ = socket.close(None).await;
                    return;
                }
                incoming = socket.next() => match incoming {
                    Some(Ok(Message::Text(text))) => self.reader.message(clock.now_ms(), text.as_bytes()),
                    Some(Ok(Message::Binary(bytes))) => self.reader.message(clock.now_ms(), &bytes),
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                },
                Some((url, body)) = fetched.recv() => {
                    self.reader.fetched(clock.now_ms(), &url, body.as_deref());
                }
                _ = tick.tick() => self.reader.tick(clock.now_ms()),
                () = clock_moved(&mut moves) => self.reader.tick(clock.now_ms()),
            }
        }
    }

    /// Tells the rate fetcher the reader's rate now.
    fn tell(&self) {
        let now_ms = self.clock.now_ms();
        let rate = self.reader.rate(now_ms);
        *self.told.lock().unwrap_or_else(PoisonError::into_inner) = (rate, now_ms);
    }

    /// Carries out what the reader decided; false once the socket failed or
    /// the reader asked for a new connection.
    async fn flush(&mut self, socket: &mut WebSocket, fetches: &mpsc::Sender<Fetched>) -> bool {
        // Once the connection is to end, or failed, what was to go on it is
        // dropped, but fetches still go.
        let mut alive = true;
        while let Some(out) = self.reader.poll() {
            match out {
                Out::Send(_) | Out::Reconnect if !alive => {}
                Out::Send(bytes) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    alive = socket.send(Message::text(text)).await.is_ok();
                }
                Out::Fetch(url) => fetch(self.client.clone(), fetches.clone(), url),
                Out::Reconnect => {
                    if let Some(waiting) = self.reader.waiting() {
                        tracing::warn!(target: target::ENGINE, url = %self.url, ?waiting, "the XRPL server for XRP's rate did not prove what it was asked: connecting anew");
                    }
                    alive = false;
                }
            }
        }
        alive
    }
}

/// Fetches a validator list publisher's site by an HTTPS GET, and hands
/// what came to the reader: the body of a successful answer, or nothing.
/// A redirect is no answer: the client follows none.
fn fetch(client: Option<OutboundClient>, fetched: mpsc::Sender<Fetched>, url: String) {
    tokio::spawn(async move {
        let body = match client {
            Some(client) => {
                let request = WebRequest {
                    method: http::Method::GET,
                    url: url.clone(),
                    headers: Vec::new(),
                    body: None,
                    timeout_ms: FETCH_TIMEOUT_MS,
                    max_response_bytes: MAX_LIST_BYTES,
                };
                match client.web(request).await {
                    Ok(response) if (200..300).contains(&response.status) => Some(response.body),
                    Ok(response) => {
                        tracing::warn!(target: target::ENGINE, %url, status = response.status, "a validator list fetch was refused");
                        None
                    }
                    Err(error) => {
                        tracing::warn!(target: target::ENGINE, %url, ?error, "a validator list fetch failed");
                        None
                    }
                }
            }
            None => None,
        };
        // A reader gone, or one with fetches enough waiting, needs no more.
        let _ = fetched.try_send((url, body));
    });
}

/// A WebSocket to `url`, as the node connects to its ledgers.
async fn connect(client: Option<&OutboundClient>, url: &str) -> Result<WebSocket, String> {
    let client = client.ok_or("no client for other servers")?;
    client.websocket(url, MAX_MESSAGE).await
}

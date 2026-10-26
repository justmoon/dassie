//! The node's rate feed (spec/areas/rate-feed.md#the-node-side).
//!
//! A core node publishes what its connector converts at: the price table
//! the engine builds, as a signed datagram it makes once and sends to every
//! subscriber on a UDP socket of its own, with no retries. The decisions are
//! `dassie-feed`'s: who is subscribed, what to answer, when to publish. The
//! one thing decided here, what a feed says, is a pure function of the
//! table ([`Content::of`]); [`Publisher`] holds the rest without IO, and
//! [`run`] carries it out against the socket.
//!
//! The service runs only on a core node: one with a URL, at whose host
//! subscribers find the feed, and a key to sign with. It answers every
//! datagram it takes, or drops it, counted and never logged, so a flood
//! costs no more than its own bytes and a MAC each.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_connector::PriceTable;
use dassie_connector::pricing::MAX_PRICED_LEDGERS;
use dassie_feed::{
    CookieSecret, Decision, FEED_MIN_INTERVAL_MS, Feed, Ledger, Limits, MAX_DATAGRAM_BYTES,
    MAX_LEDGERS, Poll, Rate, Refused, Schedule, SubscriberTable,
};
use dassie_identity::NodeKey;
use dassie_ledger::LedgerId;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::engine::{Engine, Versions};
use crate::node::{Node, lock};

/// The UDP port a node takes its feed's subscribers on, unless told
/// another (spec/areas/rate-feed.md#the-socket).
pub const DEFAULT_FEED_PORT: u16 = 7768;

/// The longest one publish spends sending: half the least time between two,
/// so that a socket whose buffer stays full never holds up the next.
/// Pacing IO, so on the monotonic clock.
const PUBLISH_TIME: Duration = Duration::from_millis(FEED_MIN_INTERVAL_MS / 2);

/// The longest the service waits for the engine's price table.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

// Every ledger the connector prices fits in a feed, so a feed never leaves
// one out for want of room: every ledger of the node's realm, settlement
// hubs' included. A realm with more needs a new feed version, since the
// bound is part of the wire format.
const _: () = assert!(MAX_PRICED_LEDGERS <= MAX_LEDGERS);

/// What the feed has done since the node started, counted rather than
/// logged, and how many addresses it sends to now. The owner reads them in
/// `GetStatus`, each on its own and without a lock
/// (spec/areas/rate-feed.md#what-the-owner-sees).
#[derive(Debug, Default)]
pub struct FeedStats {
    /// Addresses in the subscriber table, as the service last left it.
    pub subscribers: AtomicU64,
    /// Datagrams taken.
    pub received: AtomicU64,
    /// Challenges sent.
    pub challenged: AtomicU64,
    /// Addresses subscribed.
    pub subscribed: AtomicU64,
    /// Subscriptions renewed.
    pub renewed: AtomicU64,
    /// Proven addresses refused because the table was full.
    pub refused_full: AtomicU64,
    /// Proven addresses refused because their client had its share.
    pub refused_client: AtomicU64,
    /// Datagrams that got no answer: not a Subscribe, broken, or from an
    /// address a feed cannot go to.
    pub ignored: AtomicU64,
    /// Subscriptions that lapsed.
    pub expired: AtomicU64,
    /// Feeds made and signed.
    pub published: AtomicU64,
    /// Datagrams sent.
    pub sent: AtomicU64,
    /// Datagrams the socket refused.
    pub send_failed: AtomicU64,
    /// Feeds not sent because a publish ran out of time.
    pub unsent: AtomicU64,
}

fn count(counter: &AtomicU64, by: u64) {
    counter.fetch_add(by, Ordering::Relaxed);
}

/// Notes how many addresses `table` holds, for the owner.
fn note_subscribers(stats: &FeedStats, table: &SubscriberTable) {
    let subscribers = u64::try_from(table.len()).unwrap_or(u64::MAX);
    stats.subscribers.store(subscribers, Ordering::Relaxed);
}

/// Whether `node` serves its feed: it has the feed's socket, a URL, at
/// whose host subscribers find the feed, and a key to sign with.
#[must_use]
pub fn serving(node: &Node) -> bool {
    node.feed().is_some() && node.own_info().url.is_some() && node.node_key().is_some()
}

/// The feed's socket, and what outlives a restart of its service.
#[derive(Debug)]
pub struct FeedEndpoint {
    socket: Arc<UdpSocket>,
    stats: FeedStats,
    /// The schedule, kept so that a restarted service's sequence grows on.
    schedule: Mutex<Schedule>,
}

impl FeedEndpoint {
    /// The endpoint on `socket`. Inside the async runtime.
    ///
    /// # Errors
    ///
    /// The socket cannot be made non-blocking or taken by the runtime.
    pub fn new(socket: std::net::UdpSocket) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket: Arc::new(UdpSocket::from_std(socket)?),
            stats: FeedStats::default(),
            schedule: Mutex::new(Schedule::new()),
        })
    }

    /// Where it takes datagrams.
    ///
    /// # Errors
    ///
    /// The socket's address cannot be read.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// What it has done.
    #[must_use]
    pub const fn stats(&self) -> &FeedStats {
        &self.stats
    }
}

/// What a feed says besides who made it and when: the fee, the ledgers and
/// the rates. Feeds that say the same differ only in their sequence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Says {
    fee_ppm: u32,
    /// Each with its largest packet, in ascending order of their IDs.
    ledgers: Vec<(LedgerId, u64)>,
    /// In ascending order of their pair; indexes into `ledgers`.
    rates: Vec<Rate>,
}

/// What to publish, from one price table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    says: Says,
    /// Until when the table's rates hold: by then the engine builds it
    /// again, and a feed made later from this one names no rates.
    valid_until_ms: u64,
}

impl Content {
    /// What `table` says at `now_ms`, every ledger's largest packet
    /// `max_packet_amount`: its fee, its ledgers in ascending order of
    /// their IDs, and each price it has as a rate. A pair without a price
    /// is left out. A table whose rates aged out says no ledgers.
    #[must_use]
    pub fn of(table: &PriceTable, max_packet_amount: u64, now_ms: u64) -> Self {
        let fee_ppm = table.charges().fee_ppm;
        let valid_until_ms = table.valid_until_ms();
        if valid_until_ms <= now_ms {
            return Self {
                says: Says {
                    fee_ppm,
                    ..Says::default()
                },
                valid_until_ms: u64::MAX,
            };
        }
        let mut ledgers: Vec<(LedgerId, u64)> = table
            .ledgers()
            .map(|entry| (entry.ledger, max_packet_amount))
            .take(MAX_LEDGERS)
            .collect();
        ledgers.sort_unstable_by_key(|(ledger, _)| ledger.as_str());
        let index = |ledger: LedgerId| {
            let position = ledgers.iter().position(|(id, _)| *id == ledger)?;
            u8::try_from(position).ok()
        };
        let mut rates: Vec<Rate> = table
            .prices()
            .filter(|price| price.rate_e15 > 0)
            .filter_map(|price| {
                Some(Rate {
                    from: index(price.from)?,
                    to: index(price.to)?,
                    rate_e15: price.rate_e15,
                })
            })
            .collect();
        rates.sort_unstable();
        Self {
            says: Says {
                fee_ppm,
                ledgers,
                rates,
            },
            valid_until_ms,
        }
    }

    /// The feed of `key` with `sequence_ms`, valid until `valid_until_ms`
    /// or until the table's rates age out, whichever is first. Rates that
    /// age out by `sequence_ms` are not said.
    fn feed(&self, key: &NodeKey, sequence_ms: u64, valid_until_ms: u64) -> Feed<'static> {
        let fresh = self.valid_until_ms > sequence_ms;
        let (says, valid_until_ms) = if fresh {
            (&self.says, valid_until_ms.min(self.valid_until_ms))
        } else {
            (&Says::default(), valid_until_ms)
        };
        Feed {
            public_key: *key.public_key().as_bytes(),
            sequence_ms,
            valid_until_ms,
            fee_ppm: self.says.fee_ppm,
            ledgers: says
                .ledgers
                .iter()
                .map(|(ledger, max_packet_amount)| Ledger {
                    id: ledger.as_str(),
                    max_packet_amount: *max_packet_amount,
                })
                .collect(),
            rates: says.rates.clone(),
        }
    }
}

/// What to do about a datagram: the table's decision, and what to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// What the table decided.
    pub decision: Decision,
    /// A Challenge to send back, with this cookie.
    pub challenge: Option<dassie_feed::Cookie>,
    /// Whether to send the latest feed back: the address is newly
    /// subscribed, and there is one that still counts.
    pub latest: bool,
    /// Subscriptions that lapsed and were dropped to make room.
    pub expired: u64,
}

/// A feed to send to each of its recipients.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publish {
    /// The signed datagram.
    pub datagram: Arc<[u8]>,
    /// Where it goes, at most one per subscriber.
    pub recipients: Vec<SocketAddr>,
}

/// The feed without IO: the subscriber table, the schedule and the content
/// to publish, all decided by their inputs and `now_ms`.
#[derive(Debug)]
pub struct Publisher {
    table: SubscriberTable,
    schedule: Schedule,
    content: Option<Content>,
    /// Whether what the content says changed since the schedule last heard.
    changed: bool,
    /// The last feed made, and until when it counts.
    latest: Option<(Arc<[u8]>, u64)>,
}

impl Publisher {
    /// A publisher whose cookies are made under `secret`, going on from
    /// `schedule`.
    #[must_use]
    pub fn new(limits: Limits, secret: CookieSecret, schedule: Schedule) -> Self {
        Self {
            table: SubscriberTable::new(limits, secret),
            schedule,
            content: None,
            changed: false,
            latest: None,
        }
    }

    /// What to publish from now on; `None` while there is nothing to, as
    /// while the engine is down. Only a change in what it says counts as
    /// one for the schedule.
    pub fn set_content(&mut self, content: Option<Content>) {
        let says = |content: &Option<Content>| content.as_ref().map(|content| content.says.clone());
        self.changed |= says(&self.content) != says(&content);
        if content.is_none() {
            self.latest = None;
        }
        self.content = content;
    }

    /// What to do about `datagram`, which came from `from` at `now_ms`.
    pub fn receive(&mut self, from: SocketAddr, datagram: &[u8], now_ms: u64) -> Answer {
        // A full table drops what lapsed to make room, so a Subscribe can
        // expire entries too.
        let expired_before = self.table.stats().expired;
        let decision = self.table.handle(from, datagram, now_ms);
        let expired = self.table.stats().expired.saturating_sub(expired_before);
        let (challenge, latest) = match decision {
            Decision::Challenge(cookie) => (Some(cookie), false),
            Decision::Subscribed { new, refresh } => {
                (refresh, new && self.latest(now_ms).is_some())
            }
            Decision::Refused(_) | Decision::Ignored(_) => (None, false),
        };
        Answer {
            decision,
            challenge,
            latest,
            expired,
        }
    }

    /// The last feed made, if it still counts at `now_ms`.
    #[must_use]
    pub fn latest(&self, now_ms: u64) -> Option<&Arc<[u8]>> {
        self.latest
            .as_ref()
            .filter(|(_, valid_until_ms)| *valid_until_ms > now_ms)
            .map(|(datagram, _)| datagram)
    }

    /// Whether to publish at `now_ms`, signing with `key`: the feed and its
    /// recipients, or nothing yet, with the next time to poll. Polling
    /// costs a look at the schedule; only a publish, at most once per
    /// least interval, goes through the table, and drops the subscriptions
    /// that lapsed.
    pub fn poll(&mut self, now_ms: u64, key: &NodeKey) -> Polled {
        let Some(content) = &self.content else {
            return Polled {
                publish: None,
                expired: 0,
                next_ms: None,
            };
        };
        let changed = std::mem::take(&mut self.changed);
        let (sequence_ms, valid_until_ms, next_ms) = match self.schedule.poll(now_ms, changed) {
            Poll::Wait { next_ms } => {
                return Polled {
                    publish: None,
                    expired: 0,
                    next_ms: Some(next_ms),
                };
            }
            Poll::Publish {
                sequence_ms,
                valid_until_ms,
                next_ms,
            } => (sequence_ms, valid_until_ms, next_ms),
        };
        let feed = content.feed(key, sequence_ms, valid_until_ms);
        let expired = self.table.expire(now_ms);
        // The feed is built to the rules and is the key's, so this holds;
        // if it did not, nothing goes out, and the heartbeat tries again.
        let publish = dassie_feed::sign_feed(key, &feed).ok().map(|datagram| {
            assert!(datagram.len() <= MAX_DATAGRAM_BYTES);
            let datagram: Arc<[u8]> = datagram.into();
            self.latest = Some((Arc::clone(&datagram), feed.valid_until_ms));
            Publish {
                datagram,
                recipients: self.table.recipients(now_ms).collect(),
            }
        });
        Polled {
            publish,
            expired,
            next_ms: Some(next_ms),
        }
    }

    /// The schedule, to keep across a restart.
    #[must_use]
    pub const fn schedule(&self) -> Schedule {
        self.schedule
    }

    /// The subscriber table.
    #[must_use]
    pub const fn table(&self) -> &SubscriberTable {
        &self.table
    }
}

/// What a poll decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Polled {
    /// A feed to send, if one is due.
    pub publish: Option<Publish>,
    /// Subscriptions that lapsed.
    pub expired: usize,
    /// When to poll again, at the latest, while there is content.
    pub next_ms: Option<u64>,
}

/// Runs the feed on the node's feed socket, if it has one, until
/// `shutdown`.
pub async fn run(node: Arc<Node>, shutdown: CancellationToken) {
    let Some(endpoint) = node.feed().cloned() else {
        return shutdown.cancelled().await;
    };
    // Drawn once per start, kept in memory only: a restart voids every
    // cookie (spec/areas/rate-feed.md#the-cookie).
    let secret = node.with_rng(|rng| {
        let mut bytes = [0u8; 32];
        rand_core::Rng::fill_bytes(rng, &mut bytes);
        CookieSecret::from_bytes(bytes)
    });
    let schedule = *lock(&endpoint.schedule);
    let publisher = Publisher::new(node.policies().feed, secret, schedule);
    let mut service = Service::new(node, endpoint, publisher);
    service.run(&shutdown).await;
}

/// The service's state between wakes.
struct Service {
    node: Arc<Node>,
    endpoint: Arc<FeedEndpoint>,
    publisher: Publisher,
    engine: Option<Engine>,
    versions: Option<watch::Receiver<Versions>>,
    /// The engine's count of price-table changes, as last seen.
    seen_prices: u64,
    /// Whether the price table changed since it was last fetched.
    dirty: bool,
    /// When the table may be fetched again: at most once per least
    /// interval between feeds.
    fetch_after_ms: u64,
    /// When to poll the publisher next.
    next_poll_ms: Option<u64>,
}

impl Service {
    fn new(node: Arc<Node>, endpoint: Arc<FeedEndpoint>, publisher: Publisher) -> Self {
        // A service that starts again starts with an empty table.
        note_subscribers(&endpoint.stats, publisher.table());
        Self {
            node,
            endpoint,
            publisher,
            engine: None,
            versions: None,
            seen_prices: 0,
            dirty: true,
            fetch_after_ms: 0,
            next_poll_ms: None,
        }
    }

    async fn run(&mut self, shutdown: &CancellationToken) {
        let node = Arc::clone(&self.node);
        let socket = Arc::clone(&self.endpoint.socket);
        let mut config = node.subscribe_config();
        let mut identity = node.identity();
        let mut engines = node.engines();
        let mut wake = node.clock().subscribe();
        // One more byte than a datagram may have, so a longer one is seen
        // as too long rather than cut to size.
        let mut buffer = vec![0u8; MAX_DATAGRAM_BYTES + 1];
        self.take_engine(engines.borrow_and_update().clone());
        // Whether the node serves changes only with its config or its key,
        // so it is looked at then, not once per datagram.
        let mut key = self.serving_key();
        loop {
            self.step(key.as_deref()).await;
            let sleep = self.sleep(key.is_some());
            // The socket comes last: under a flood it is always ready, and
            // must not keep the timer and the node's changes from being
            // heard.
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return,
                () = sleep => {}
                () = clock_moved(&mut wake) => {}
                changed = config.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    key = self.serving_key();
                }
                changed = identity.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    key = self.serving_key();
                }
                changed = engines.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    self.take_engine(engines.borrow_and_update().clone());
                }
                () = versions_changed(&mut self.versions) => self.note_versions(),
                received = socket.recv_from(&mut buffer), if key.is_some() => {
                    if !self.received(received, &buffer) {
                        return;
                    }
                }
            }
        }
    }

    /// The node key, while the node serves a feed: it has a URL, at whose
    /// host subscribers find it, and a key to sign with.
    fn serving_key(&self) -> Option<Arc<NodeKey>> {
        self.node.node_key().filter(|_| serving(&self.node))
    }

    /// Follows a new engine, or none.
    fn take_engine(&mut self, engine: Option<Engine>) {
        self.versions = engine.as_ref().map(Engine::versions);
        self.seen_prices = self
            .versions
            .as_mut()
            .map_or(0, |versions| versions.borrow_and_update().prices);
        if engine.is_none() {
            self.publisher.set_content(None);
        }
        self.engine = engine;
        self.dirty = true;
        self.fetch_after_ms = 0;
    }

    /// Notes whether the engine's change counts say the price table
    /// changed.
    fn note_versions(&mut self) {
        let Some(versions) = &mut self.versions else {
            return;
        };
        let prices = versions.borrow_and_update().prices;
        if prices != self.seen_prices {
            self.seen_prices = prices;
            self.dirty = true;
        }
    }

    /// While the node serves: fetches the price table if it changed and may
    /// be fetched, then publishes if a feed is due, and keeps the schedule.
    /// A node that does not serve never asks the engine; the table is
    /// fetched once it does.
    async fn step(&mut self, key: Option<&NodeKey>) {
        let Some(key) = key else {
            self.next_poll_ms = None;
            return;
        };
        let now_ms = self.node.now_ms();
        if self.dirty && now_ms >= self.fetch_after_ms {
            self.fetch(now_ms).await;
        }
        let now_ms = self.node.now_ms();
        let polled = self.publisher.poll(now_ms, key);
        *lock(&self.endpoint.schedule) = self.publisher.schedule();
        let stats = &self.endpoint.stats;
        count(
            &stats.expired,
            u64::try_from(polled.expired).unwrap_or(u64::MAX),
        );
        note_subscribers(stats, self.publisher.table());
        self.next_poll_ms = polled.next_ms;
        if let Some(publish) = polled.publish {
            count(&stats.published, 1);
            self.send(&publish).await;
        }
    }

    /// Takes the engine's price table as the content to publish.
    async fn fetch(&mut self, now_ms: u64) {
        self.dirty = false;
        self.fetch_after_ms = now_ms.saturating_add(FEED_MIN_INTERVAL_MS);
        let Some(engine) = &self.engine else {
            self.publisher.set_content(None);
            return;
        };
        match tokio::time::timeout(FETCH_TIMEOUT, engine.price_table()).await {
            Ok(Ok(table)) => {
                let max_packet_amount = self.node.policies().connector.max_packet_amount;
                let content = Content::of(&table, max_packet_amount, now_ms);
                self.publisher.set_content(Some(content));
            }
            // The engine is stopping or busy: try again after the interval.
            Ok(Err(_)) | Err(_) => self.dirty = true,
        }
    }

    /// Sends `publish` to each recipient once, within [`PUBLISH_TIME`].
    async fn send(&self, publish: &Publish) {
        let stats = &self.endpoint.stats;
        let socket = &self.endpoint.socket;
        let mut sent = 0u64;
        let sending = async {
            for recipient in &publish.recipients {
                match socket.send_to(&publish.datagram, recipient).await {
                    Ok(_) => count(&stats.sent, 1),
                    Err(_) => count(&stats.send_failed, 1),
                }
                sent += 1;
            }
        };
        if tokio::time::timeout(PUBLISH_TIME, sending).await.is_err() {
            let recipients = u64::try_from(publish.recipients.len()).unwrap_or(u64::MAX);
            let left = recipients.saturating_sub(sent);
            count(&stats.unsent, left);
        }
    }

    /// Handles what the socket received; `false` if it failed for good.
    fn received(&mut self, received: io::Result<(usize, SocketAddr)>, buffer: &[u8]) -> bool {
        let stats = &self.endpoint.stats;
        let (length, from) = match received {
            Ok(received) => received,
            // An ICMP error for an earlier datagram, on some systems.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused
                ) =>
            {
                return true;
            }
            Err(_) => return false,
        };
        count(&stats.received, 1);
        let datagram = buffer.get(..length).unwrap_or_default();
        let now_ms = self.node.now_ms();
        let answer = self.publisher.receive(from, datagram, now_ms);
        let counter = match answer.decision {
            Decision::Challenge(_) => &stats.challenged,
            Decision::Subscribed { new: true, .. } => &stats.subscribed,
            Decision::Subscribed { new: false, .. } => &stats.renewed,
            Decision::Refused(Refused::Full) => &stats.refused_full,
            Decision::Refused(Refused::ClientFull) => &stats.refused_client,
            Decision::Ignored(_) => &stats.ignored,
        };
        count(counter, 1);
        count(&stats.expired, answer.expired);
        note_subscribers(stats, self.publisher.table());
        let socket = &self.endpoint.socket;
        if let Some(cookie) = answer.challenge {
            let challenge = dassie_feed::encode_challenge(&cookie);
            try_send(socket, stats, &challenge, from);
        }
        if answer.latest
            && let Some(latest) = self.publisher.latest(now_ms)
        {
            try_send(socket, stats, latest, from);
        }
        true
    }

    /// Until the next poll is due, or, while the node `serving`, the table
    /// may be fetched again.
    fn sleep(&self, serving: bool) -> impl std::future::Future<Output = ()> + use<> {
        let fetch = (serving && self.dirty).then_some(self.fetch_after_ms);
        let deadline = [self.next_poll_ms, fetch].into_iter().flatten().min();
        let delay = deadline.map(|deadline| deadline.saturating_sub(self.node.now_ms()).max(1));
        async move {
            match delay {
                Some(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                None => std::future::pending().await,
            }
        }
    }
}

/// Sends one datagram at once, or counts that it could not be.
fn try_send(socket: &UdpSocket, stats: &FeedStats, datagram: &[u8], to: SocketAddr) {
    match socket.try_send_to(datagram, to) {
        Ok(_) => count(&stats.sent, 1),
        Err(_) => count(&stats.send_failed, 1),
    }
}

/// Resolves when the engine's change counts move, which the caller reads,
/// or the engine stopped; never without an engine.
async fn versions_changed(versions: &mut Option<watch::Receiver<Versions>>) {
    let Some(receiver) = versions else {
        return std::future::pending().await;
    };
    if receiver.changed().await.is_err() {
        *versions = None;
    }
}

/// Resolves when a clock that can be moved by hand moves, so tests need not
/// wait for real time to pass. Never resolves for the system clock.
async fn clock_moved(wake: &mut Option<watch::Receiver<u64>>) {
    match wake {
        Some(receiver) => {
            if receiver.changed().await.is_err() {
                *wake = None;
            }
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

    use dassie_connector::{Charges, Holding, Inventory, Pricing, RATE_PRECISION, Rates};
    use dassie_feed::{
        FEED_HEARTBEAT_MS, FEED_VALIDITY_MS, Message, SUBSCRIPTION_TTL_MS, encode_subscribe,
    };
    use dassie_ledger::{AccountId, Realm};

    use super::*;

    const NOW: u64 = 1_800_000_000_000;
    const AGE: u64 = 3_600_000;
    const USD: LedgerId = LedgerId::StubUsd;
    const XRP: LedgerId = LedgerId::XrplTestnetXrp;
    const MAX_PACKET: u64 = 5_000_000_000;

    fn key() -> NodeKey {
        NodeKey::from_bytes(&[7; 32])
    }

    fn held(ledger: LedgerId, posted: u64) -> Inventory {
        Inventory {
            ledger,
            owner: AccountId(0),
            holding: Holding {
                posted: u128::from(posted),
                ..Holding::NONE
            },
        }
    }

    /// Dollars and XRP at 2 XRP a dollar, learned at `as_of_ms`, the owner
    /// holding `usd` and `xrp`, with a fee of 1000 ppm.
    fn table(usd: u64, xrp: u64, as_of_ms: u64) -> PriceTable {
        let mut rates = Rates::new();
        rates.set_at("USD", 10u128.pow(RATE_PRECISION), as_of_ms);
        rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), as_of_ms);
        let pricing = Pricing {
            charges: Charges {
                fee_ppm: 1_000,
                spread_ppm: 2_000,
            },
            rebalance_ppm: 50_000,
        };
        // The table names XRP first, so that the feed's order is its own.
        PriceTable::build(&rates, &[held(XRP, xrp), held(USD, usd)], pricing, NOW, AGE)
    }

    fn content(usd: u64, xrp: u64) -> Content {
        Content::of(&table(usd, xrp, NOW), MAX_PACKET, NOW)
    }

    fn address(port: u16) -> SocketAddr {
        SocketAddr::from(([203, 0, 113, 7], port))
    }

    /// What `datagram` says, as a subscriber reads it, verified.
    fn read(datagram: &[u8]) -> Feed<'_> {
        let Ok(Message::Feed(signed)) = dassie_feed::decode(datagram) else {
            panic!("not a feed");
        };
        signed.verify().unwrap();
        signed.feed
    }

    // A feed says the table's prices, by ledger ID, with the fee and each
    // ledger's largest packet (spec/areas/rate-feed.md#what-a-feed-says).
    #[test]
    fn content_says_the_tables_prices_in_the_feeds_order() {
        let table = table(10_000_000_000, 20_000_000_000, NOW);
        let content = Content::of(&table, MAX_PACKET, NOW);
        let feed = content.feed(&key(), NOW, NOW + FEED_VALIDITY_MS);
        feed.check().unwrap();
        assert_eq!(feed.fee_ppm, 1_000);
        let ids: Vec<&str> = feed.ledgers.iter().map(|ledger| ledger.id).collect();
        assert_eq!(ids, ["stub+usd", "xrpl-testnet+xrp"]);
        assert!(
            feed.ledgers
                .iter()
                .all(|ledger| ledger.max_packet_amount == MAX_PACKET)
        );
        assert_eq!(feed.rates.len(), 2);
        for (from, to) in [(USD, XRP), (XRP, USD)] {
            let price = table.price(from, to).unwrap().rate_e15;
            assert_eq!(feed.rate(from.as_str(), to.as_str()), Some(price));
        }
    }

    // A settlement hub's ledger is named as any other, by its ID, with
    // the table's prices to and from it (spec/areas/rate-feed.md#what-a-feed-says).
    #[test]
    fn a_hub_ledger_is_named_as_any_other() {
        const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
        let mut rates = Rates::new();
        rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
        rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
        let inventories = [
            held(HUB, 20_000_000_000),
            held(XRP, 30_000_000_000),
            held(USD, 10_000_000_000),
        ];
        let table = PriceTable::build(&rates, &inventories, Pricing::NONE, NOW, AGE);
        let feed = Content::of(&table, MAX_PACKET, NOW).feed(&key(), NOW, NOW + 1);
        feed.check().unwrap();
        let mut expected = [HUB, XRP, USD].map(LedgerId::as_str);
        expected.sort_unstable();
        let ids: Vec<&str> = feed.ledgers.iter().map(|ledger| ledger.id).collect();
        assert_eq!(ids, expected);
        assert_eq!(feed.rates.len(), 6);
        for price in table.prices() {
            let said = feed.rate(price.from.as_str(), price.to.as_str());
            assert_eq!(said, Some(price.rate_e15));
        }
        assert!(feed.rate(HUB.as_str(), USD.as_str()).is_some());
        assert!(feed.rate(XRP.as_str(), HUB.as_str()).is_some());
    }

    // A node runs the ledgers of its realm, hubs' included, and a feed
    // names all of them at once: every ID fits the feed's alphabet and
    // size, and their number its bound (spec/areas/rate-feed.md#bounds).
    #[test]
    fn a_feed_names_every_ledger_of_a_realm() {
        let mut rates = Rates::new();
        rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
        rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
        for realm in [Realm::Test, Realm::Live] {
            let inventories: Vec<Inventory> = LedgerId::ALL
                .into_iter()
                .filter(|ledger| ledger.realm() == realm)
                .map(|ledger| held(ledger, 10_000_000_000))
                .collect();
            let table = PriceTable::build(&rates, &inventories, Pricing::NONE, NOW, AGE);
            let feed = Content::of(&table, MAX_PACKET, NOW).feed(&key(), NOW, NOW + 1);
            feed.check().unwrap();
            assert_eq!(feed.ledgers.len(), inventories.len());
            let pairs = inventories.len() * (inventories.len() - 1);
            assert_eq!(feed.rates.len(), pairs);
        }
    }

    // Out of a ledger the owner holds nothing on, nothing converts: the
    // pair is left out, the other kept.
    #[test]
    fn a_pair_without_a_price_is_left_out() {
        let table = table(10_000_000_000, 0, NOW);
        assert!(table.price(XRP, USD).is_some() && table.price(USD, XRP).is_none());
        let feed = Content::of(&table, MAX_PACKET, NOW).feed(&key(), NOW, NOW + 1);
        assert_eq!(feed.rate("stub+usd", "xrpl-testnet+xrp"), None);
        assert!(feed.rate("xrpl-testnet+xrp", "stub+usd").is_some());
    }

    // A feed never outlives the rates it says: it ends when they age out,
    // and one made after says none.
    #[test]
    fn a_feed_ends_with_the_tables_rates() {
        let as_of_ms = NOW - AGE + 10_000;
        let table = table(10_000_000_000, 20_000_000_000, as_of_ms);
        let ends_ms = table.valid_until_ms();
        assert!(ends_ms < NOW + FEED_VALIDITY_MS);
        let content = Content::of(&table, MAX_PACKET, NOW);
        let feed = content.feed(&key(), NOW, NOW + FEED_VALIDITY_MS);
        assert_eq!(feed.valid_until_ms, ends_ms);
        assert_eq!(feed.rates.len(), 2);
        let later = content.feed(&key(), ends_ms, ends_ms + FEED_VALIDITY_MS);
        assert!(later.ledgers.is_empty() && later.rates.is_empty());
        assert_eq!(later.valid_until_ms, ends_ms + FEED_VALIDITY_MS);
        // A table whose rates aged out says no ledgers either.
        let stale = Content::of(&table, MAX_PACKET, ends_ms);
        assert!(stale.feed(&key(), ends_ms, ends_ms + 1).ledgers.is_empty());
    }

    fn publisher() -> Publisher {
        let secret = CookieSecret::from_bytes([3; 32]);
        Publisher::new(Limits::DEFAULT, secret, Schedule::new())
    }

    /// Subscribes `from` at `now_ms`: challenged, then subscribed.
    fn subscribe(publisher: &mut Publisher, from: SocketAddr, now_ms: u64) -> Answer {
        let answer = publisher.receive(from, &encode_subscribe(None), now_ms);
        let Some(cookie) = answer.challenge else {
            panic!("not challenged: {answer:?}");
        };
        publisher.receive(from, &encode_subscribe(Some(&cookie)), now_ms)
    }

    // Nothing is published before there is something to say; then at
    // once, to every subscriber, and a newcomer gets the latest feed at
    // once. Content that says the same waits for the heartbeat; a change
    // goes out after the least interval.
    #[test]
    fn the_publisher_publishes_what_changed_and_heartbeats() {
        let mut publisher = publisher();
        let (a, b) = (address(4000), address(4001));
        assert!(!subscribe(&mut publisher, a, NOW).latest);
        assert_eq!(publisher.poll(NOW, &key()).publish, None);
        publisher.set_content(Some(content(10_000_000_000, 20_000_000_000)));
        let first = publisher.poll(NOW, &key()).publish.unwrap();
        assert_eq!(first.recipients, [a]);
        assert_eq!(read(&first.datagram).sequence_ms, NOW);
        assert!(subscribe(&mut publisher, b, NOW + 1).latest);
        assert_eq!(publisher.latest(NOW + 1), Some(&first.datagram));
        // The same again is no change.
        publisher.set_content(Some(content(10_000_000_000, 20_000_000_000)));
        let polled = publisher.poll(NOW + FEED_MIN_INTERVAL_MS, &key());
        assert_eq!(polled.publish, None);
        assert_eq!(polled.next_ms, Some(NOW + FEED_HEARTBEAT_MS));
        let at = NOW + FEED_HEARTBEAT_MS;
        let beat = publisher.poll(at, &key()).publish.unwrap();
        assert_eq!(beat.recipients, [a, b]);
        // A change waits out the least interval, then goes.
        publisher.set_content(Some(content(30_000_000_000, 2_000_000_000)));
        assert_eq!(publisher.poll(at + 1, &key()).publish, None);
        let changed = publisher.poll(at + FEED_MIN_INTERVAL_MS, &key());
        let changed = changed.publish.unwrap();
        assert!(read(&changed.datagram).sequence_ms > read(&beat.datagram).sequence_ms);
        assert_ne!(read(&changed.datagram).rates, read(&beat.datagram).rates);
    }

    // Without content, as while the engine is down, nothing is published
    // and no latest feed is handed out; subscriptions lapse after their
    // lifetime, and a lapsed subscriber gets no feed.
    #[test]
    fn subscribers_lapse_and_nothing_goes_out_without_content() {
        let mut publisher = publisher();
        let a = address(4000);
        publisher.set_content(Some(content(10_000_000_000, 20_000_000_000)));
        subscribe(&mut publisher, a, NOW);
        assert_eq!(publisher.poll(NOW, &key()).publish.unwrap().recipients, [a]);
        let later = NOW + SUBSCRIPTION_TTL_MS;
        let polled = publisher.poll(later, &key());
        assert_eq!(polled.publish.unwrap().recipients, []);
        assert_eq!(polled.expired, 1);
        publisher.set_content(None);
        assert_eq!(publisher.latest(later), None);
        let polled = publisher.poll(later + FEED_HEARTBEAT_MS, &key());
        assert_eq!((polled.publish, polled.next_ms), (None, None));
    }

    // A full table drops what lapsed to take a newcomer: the answer says
    // so, for the owner's count of lapsed subscriptions.
    #[test]
    fn an_answer_counts_what_lapsed_to_make_room() {
        let secret = CookieSecret::from_bytes([3; 32]);
        let limits = Limits {
            max_subscribers: 1,
            ..Limits::DEFAULT
        };
        let mut publisher = Publisher::new(limits, secret, Schedule::new());
        assert_eq!(subscribe(&mut publisher, address(4000), NOW).expired, 0);
        let later = NOW + SUBSCRIPTION_TTL_MS;
        let answer = subscribe(&mut publisher, address(4001), later);
        assert!(matches!(
            answer.decision,
            Decision::Subscribed { new: true, .. }
        ));
        assert_eq!(answer.expired, 1);
        assert_eq!(publisher.table().len(), 1);
    }

    // A restarted service goes on from the schedule it kept: its sequence
    // grows on, even with the clock gone back.
    #[test]
    fn the_sequence_grows_across_a_restart() {
        let mut publisher = publisher();
        publisher.set_content(Some(content(10_000_000_000, 20_000_000_000)));
        let first = publisher.poll(NOW, &key()).publish.unwrap();
        let secret = CookieSecret::from_bytes([4; 32]);
        let mut again = Publisher::new(Limits::DEFAULT, secret, publisher.schedule());
        again.set_content(Some(content(10_000_000_000, 20_000_000_000)));
        let back = NOW - 60_000;
        assert_eq!(again.poll(back, &key()).publish, None);
        let next = again.poll(back + FEED_MIN_INTERVAL_MS, &key());
        let next = next.publish.unwrap();
        assert!(read(&next.datagram).sequence_ms > read(&first.datagram).sequence_ms);
    }
}

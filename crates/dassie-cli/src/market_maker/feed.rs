//! The target's feed, over UDP (spec/areas/rate-feed.md#subscribing).
//!
//! [`FeedClient`] decides, from the datagrams that come and the time it is
//! told, what to send and which feed is current; [`run`] carries that out
//! on a socket, and publishes each feed it takes. It takes datagrams only
//! from the target's address, a feed only verified (ZIP-215) under the
//! target's key and newer than the last it took, and says a feed is
//! current only until it expires, so a stale or replayed rate never
//! decides a trade.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dassie_feed::{
    FeedReceiver, MAX_DATAGRAM_BYTES, RENEW_INTERVAL_MS, Received, Rejected, SUBSCRIBE_BYTES,
};
use dassie_identity::NodeId;
use dassie_node::clock::Clock;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::TARGET;

/// How long the socket rests after it failed to receive, so a socket that
/// keeps failing does not spin.
const RECEIVE_ERROR_PAUSE_MS: u64 = 1_000;

/// A feed, owned: what the market maker decides from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedView {
    /// When the target made it, in ms.
    pub sequence_ms: u64,
    /// When it stops counting.
    pub valid_until_ms: u64,
    /// The target's fee, in parts per million.
    pub fee_ppm: u32,
    /// Its rates: from, to, and out per in × 10^15.
    pub rates: Vec<(String, String, u64)>,
}

impl FeedView {
    fn of(feed: &dassie_feed::Feed<'_>) -> Self {
        let id = |index: u8| {
            feed.ledgers
                .get(usize::from(index))
                .map_or_else(String::new, |ledger| ledger.id.to_owned())
        };
        Self {
            sequence_ms: feed.sequence_ms,
            valid_until_ms: feed.valid_until_ms,
            fee_ppm: feed.fee_ppm,
            rates: feed
                .rates
                .iter()
                .map(|rate| (id(rate.from), id(rate.to), rate.rate_e15))
                .collect(),
        }
    }

    /// The rate from ledger `from` to `to`, if the target converts.
    #[must_use]
    pub fn rate(&self, from: &str, to: &str) -> Option<u64> {
        self.rates
            .iter()
            .find(|(a, b, _)| a == from && b == to)
            .map(|(_, _, rate)| *rate)
    }

    /// Whether it still counts at `now_ms`.
    #[must_use]
    pub const fn is_current(&self, now_ms: u64) -> bool {
        now_ms < self.valid_until_ms
    }
}

/// What a datagram did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A feed was taken: the current one now.
    Feed,
    /// The target handed out a cookie: subscribe again at once.
    Challenge,
    /// It came from another address than the target's.
    Foreign,
    /// It was refused.
    Rejected(Rejected),
}

/// A subscription to the target's feed, without IO.
#[derive(Debug)]
pub struct FeedClient {
    target: SocketAddr,
    public_key: [u8; 32],
    receiver: FeedReceiver,
    latest: Option<FeedView>,
    next_subscribe_ms: u64,
}

impl FeedClient {
    /// A subscription to the feed of the node with `node_id` and
    /// `public_key`, at `target`; it subscribes first at `now_ms`. The
    /// caller has checked that the ID is the key's.
    #[must_use]
    pub fn new(target: SocketAddr, node_id: NodeId, public_key: [u8; 32], now_ms: u64) -> Self {
        Self {
            target,
            public_key,
            receiver: FeedReceiver::new(node_id),
            latest: None,
            next_subscribe_ms: now_ms,
        }
    }

    /// The target's address.
    #[must_use]
    pub const fn target(&self) -> SocketAddr {
        self.target
    }

    /// When the next Subscribe is due.
    #[must_use]
    pub const fn next_subscribe_ms(&self) -> u64 {
        self.next_subscribe_ms
    }

    /// The Subscribe to send at `now_ms`, if one is due; the next is due a
    /// renewal later.
    pub fn due(&mut self, now_ms: u64) -> Option<[u8; SUBSCRIBE_BYTES]> {
        if now_ms < self.next_subscribe_ms {
            return None;
        }
        self.next_subscribe_ms = now_ms.saturating_add(RENEW_INTERVAL_MS);
        Some(self.receiver.subscribe())
    }

    /// Takes `datagram`, which came from `from` at `now_ms`.
    pub fn receive(&mut self, from: SocketAddr, datagram: &[u8], now_ms: u64) -> Event {
        if !same_address(from, self.target) {
            return Event::Foreign;
        }
        match self.receiver.receive(datagram, now_ms) {
            Ok(Received::Challenge) => {
                self.next_subscribe_ms = now_ms;
                Event::Challenge
            }
            Ok(Received::Feed(feed)) => {
                // The receiver took it from the node subscribed to, whose
                // ID is the key's hash, so the key is the target's.
                assert!(
                    feed.public_key == self.public_key,
                    "a feed of the target's ID is signed by its key"
                );
                self.latest = Some(FeedView::of(&feed));
                Event::Feed
            }
            Err(rejected) => Event::Rejected(rejected),
        }
    }

    /// The newest feed taken, if it still counts at `now_ms`.
    #[must_use]
    pub fn current(&self, now_ms: u64) -> Option<&FeedView> {
        self.latest.as_ref().filter(|feed| feed.is_current(now_ms))
    }
}

/// Whether two addresses are the same, an IPv4 address mapped into IPv6
/// being the IPv4 address.
fn same_address(a: SocketAddr, b: SocketAddr) -> bool {
    a.ip().to_canonical() == b.ip().to_canonical() && a.port() == b.port()
}

/// Keeps `client`'s subscription on `socket` until `cancel`, and publishes
/// each feed it takes to `feeds`.
pub async fn run(
    socket: UdpSocket,
    mut client: FeedClient,
    clock: Arc<dyn Clock>,
    feeds: watch::Sender<Option<FeedView>>,
    cancel: CancellationToken,
) {
    // One more byte than a datagram may have, so a longer one is seen and
    // refused rather than cut.
    let mut buffer = vec![0u8; MAX_DATAGRAM_BYTES + 1];
    loop {
        let now_ms = clock.now_ms();
        if let Some(subscribe) = client.due(now_ms)
            && let Err(error) = socket.send_to(&subscribe, client.target()).await
        {
            tracing::warn!(target: TARGET, %error, "could not send a subscribe to the target's feed");
        }
        let wait_ms = client.next_subscribe_ms().saturating_sub(now_ms);
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_millis(wait_ms)) => {}
            received = socket.recv_from(&mut buffer) => match received {
                Ok((length, from)) => {
                    let datagram = buffer.get(..length).unwrap_or_default();
                    let now_ms = clock.now_ms();
                    match client.receive(from, datagram, now_ms) {
                        Event::Feed => {
                            feeds.send_replace(client.current(now_ms).cloned());
                        }
                        Event::Challenge => {}
                        Event::Foreign => {
                            tracing::debug!(target: TARGET, %from, "dropped a datagram from another address");
                        }
                        Event::Rejected(Rejected::Old) => {}
                        Event::Rejected(rejected) => {
                            tracing::debug!(target: TARGET, ?rejected, "dropped a datagram from the target");
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(target: TARGET, %error, "the feed's socket failed");
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        () = tokio::time::sleep(Duration::from_millis(RECEIVE_ERROR_PAUSE_MS)) => {}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dassie_feed::{
        Decision, Feed, InvalidFeed, Ledger, Limits, Rate, SubscriberTable, assemble_feed,
        encode_challenge, sign_feed,
    };
    use dassie_identity::NodeKey;
    use dassie_node::clock::ManualClock;

    use super::*;

    const NOW_MS: u64 = 1_790_000_000_000;
    const XRP: &str = "xrpl-testnet+xrp";
    const DSUSD: &str = "xrpl-testnet-dsusd+usd";

    fn key() -> NodeKey {
        NodeKey::from_bytes(&[3; 32])
    }

    /// A feed of `key`'s made at `sequence_ms`, valid for a minute.
    fn feed(key: &NodeKey, sequence_ms: u64, rate: u64) -> Feed<'static> {
        let public_key = key.public_key();
        Feed {
            node_id: public_key.node_id(),
            public_key: *public_key.as_bytes(),
            sequence_ms,
            valid_until_ms: sequence_ms + 60_000,
            fee_ppm: 1_000,
            ledgers: vec![
                Ledger {
                    id: XRP,
                    max_packet_amount: 1_000_000_000,
                },
                Ledger {
                    id: DSUSD,
                    max_packet_amount: 1_000_000_000,
                },
            ],
            rates: vec![
                Rate {
                    from: 0,
                    to: 1,
                    rate_e15: rate,
                },
                Rate {
                    from: 1,
                    to: 0,
                    rate_e15: 2_000_000_000_000_000,
                },
            ],
        }
    }

    fn signed(feed: &Feed<'_>) -> Vec<u8> {
        sign_feed(&key(), feed).unwrap()
    }

    fn client(target: SocketAddr) -> FeedClient {
        let public_key = key().public_key();
        FeedClient::new(target, public_key.node_id(), *public_key.as_bytes(), NOW_MS)
    }

    const TARGET_ADDRESS: SocketAddr = SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)),
        7768,
    );

    #[test]
    fn it_subscribes_renews_and_answers_a_challenge_at_once() {
        let mut client = client(TARGET_ADDRESS);
        let first = client.due(NOW_MS).unwrap();
        assert_eq!(first, dassie_feed::encode_subscribe(None));
        assert_eq!(client.due(NOW_MS + 1), None);
        let cookie = dassie_feed::Cookie::from_bytes([9; 16]);
        assert_eq!(
            client.receive(TARGET_ADDRESS, &encode_challenge(&cookie), NOW_MS + 5),
            Event::Challenge
        );
        assert_eq!(
            client.due(NOW_MS + 5),
            Some(dassie_feed::encode_subscribe(Some(&cookie)))
        );
        assert_eq!(client.next_subscribe_ms(), NOW_MS + 5 + RENEW_INTERVAL_MS);
        assert!(client.due(NOW_MS + 5 + RENEW_INTERVAL_MS).is_some());
    }

    #[test]
    fn only_the_newest_verified_unexpired_feed_counts() {
        let mut client = client(TARGET_ADDRESS);
        let rate = 510_000_000_000_000;
        assert_eq!(
            client.receive(TARGET_ADDRESS, &signed(&feed(&key(), NOW_MS, rate)), NOW_MS),
            Event::Feed
        );
        let current = client.current(NOW_MS).unwrap();
        assert_eq!(current.rate(XRP, DSUSD), Some(rate));
        assert_eq!(current.rate(DSUSD, XRP), Some(2_000_000_000_000_000));
        assert_eq!(current.rate(XRP, "stub+usd"), None);
        assert_eq!(current.fee_ppm, 1_000);

        // An older one, or the same again, sets nothing back.
        let old = signed(&feed(&key(), NOW_MS - 1, 1));
        assert_eq!(
            client.receive(TARGET_ADDRESS, &old, NOW_MS),
            Event::Rejected(Rejected::Old)
        );
        // Another node's, or one signed by another key, is dropped.
        let other = NodeKey::from_bytes(&[4; 32]);
        let foreign = sign_feed(&other, &feed(&other, NOW_MS + 1, 1)).unwrap();
        assert_eq!(
            client.receive(TARGET_ADDRESS, &foreign, NOW_MS),
            Event::Rejected(Rejected::OtherNode)
        );
        let forged = feed(&key(), NOW_MS + 1, 1);
        let forged = assemble_feed(&forged.signed_bytes().unwrap(), &other.sign(b"anything"));
        assert_eq!(
            client.receive(TARGET_ADDRESS, &forged, NOW_MS),
            Event::Rejected(Rejected::Invalid(InvalidFeed::Signature))
        );
        // One that expired is dropped.
        let expired = signed(&feed(&key(), NOW_MS + 2, 1));
        assert_eq!(
            client.receive(TARGET_ADDRESS, &expired, NOW_MS + 70_000),
            Event::Rejected(Rejected::Expired)
        );
        // From another address, even the target's own feed is dropped.
        let elsewhere = SocketAddr::new(TARGET_ADDRESS.ip(), 7769);
        let newer = signed(&feed(&key(), NOW_MS + 3, 1));
        assert_eq!(client.receive(elsewhere, &newer, NOW_MS), Event::Foreign);
        assert_eq!(client.receive(TARGET_ADDRESS, b"junk", NOW_MS), {
            Event::Rejected(Rejected::Wire(dassie_feed::decode(b"junk").err().unwrap()))
        });
        assert_eq!(client.current(NOW_MS).unwrap().rate(XRP, DSUSD), Some(rate));
        // The feed lapses with its validity.
        assert!(client.current(NOW_MS + 59_999).is_some());
        assert_eq!(client.current(NOW_MS + 60_000), None);
    }

    /// A feed served from a UDP socket: the target's side, with a real
    /// subscriber table.
    struct Server {
        socket: UdpSocket,
        table: SubscriberTable,
    }

    impl Server {
        async fn bind() -> Self {
            Self {
                socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
                table: SubscriberTable::new(
                    Limits::DEFAULT,
                    dassie_feed::CookieSecret::from_bytes([5; 32]),
                ),
            }
        }

        /// Takes one Subscribe and answers it as the node would.
        async fn answer(&mut self) -> (SocketAddr, Decision) {
            let mut buffer = [0u8; 2_000];
            let (length, from) = self.socket.recv_from(&mut buffer).await.unwrap();
            let decision = self.table.handle(from, &buffer[..length], NOW_MS);
            if let Decision::Challenge(cookie) = &decision {
                self.socket
                    .send_to(&encode_challenge(cookie), from)
                    .await
                    .unwrap();
            }
            (from, decision)
        }
    }

    #[tokio::test]
    async fn it_subscribes_over_udp_and_takes_only_good_feeds() {
        let mut server = Server::bind().await;
        let target = server.socket.local_addr().unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let clock = Arc::new(ManualClock::new(NOW_MS));
        let (sender, mut feeds) = watch::channel(None);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(socket, client(target), clock, sender, cancel.clone()));

        // Challenged first, then subscribed with the cookie.
        let (_, decision) = server.answer().await;
        assert!(matches!(decision, Decision::Challenge(_)));
        let (subscriber, decision) = server.answer().await;
        assert_eq!(
            decision,
            Decision::Subscribed {
                new: true,
                refresh: None
            }
        );

        // Wrongly signed, another node's, expired and malformed feeds
        // are dropped; then a good one is taken.
        let other = NodeKey::from_bytes(&[4; 32]);
        let forged = feed(&key(), NOW_MS, 1);
        let forged = assemble_feed(&forged.signed_bytes().unwrap(), &other.sign(b"x"));
        let foreign = sign_feed(&other, &feed(&other, NOW_MS, 1)).unwrap();
        let mut expired = feed(&key(), NOW_MS - 120_000, 1);
        expired.valid_until_ms = NOW_MS;
        let good = signed(&feed(&key(), NOW_MS, 7));
        for datagram in [forged, foreign, signed(&expired), vec![1, 2, 3], good] {
            server.socket.send_to(&datagram, subscriber).await.unwrap();
        }
        feeds.changed().await.unwrap();
        let taken = feeds.borrow_and_update().clone().unwrap();
        assert_eq!(
            (taken.sequence_ms, taken.rate(XRP, DSUSD)),
            (NOW_MS, Some(7))
        );

        // A stale one, and a newer one from another socket, are dropped;
        // the next good one is taken.
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        stranger
            .send_to(&signed(&feed(&key(), NOW_MS + 2, 9)), subscriber)
            .await
            .unwrap();
        let stale = signed(&feed(&key(), NOW_MS - 1, 8));
        server.socket.send_to(&stale, subscriber).await.unwrap();
        let next = signed(&feed(&key(), NOW_MS + 1, 10));
        server.socket.send_to(&next, subscriber).await.unwrap();
        feeds.changed().await.unwrap();
        let taken = feeds.borrow_and_update().clone().unwrap();
        assert_eq!(
            (taken.sequence_ms, taken.rate(XRP, DSUSD)),
            (NOW_MS + 1, Some(10))
        );

        cancel.cancel();
        task.await.unwrap();
    }
}

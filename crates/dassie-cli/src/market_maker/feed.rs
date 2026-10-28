//! A target's snapshot, over UDP (spec/areas/rate-feed.md#requests). The
//! market maker asks each target for its snapshot with a padded request,
//! every [`POLL_INTERVAL_MS`], each on a socket of its own and checked
//! against that target's address and key alone, so one target's snapshot
//! never sets another's quotes.
//!
//! [`SnapshotClient`] decides, from the datagrams that come and the time it
//! is told, when to ask and which snapshot is current; [`run`] carries that
//! out on a socket, and publishes each snapshot it takes. It takes
//! datagrams only from the target's address, a snapshot only verified
//! (ZIP-215) under the target's key and newer than the last it took
//! (`dassie_feed::SnapshotReader`), and says a snapshot is current only
//! until it expires, so stale or replayed quotes never decide a trade.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dassie_feed::{
    FEED_MIN_INTERVAL_MS, MAX_DATAGRAM_BYTES, REQUEST_BYTES, Snapshot, SnapshotReader,
    SnapshotRejected, encode_request,
};
use dassie_identity::NodeIdentity;
use dassie_node::clock::Clock;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::TARGET;

/// How often it asks a target for its snapshot: a few times a decision, so
/// that a lost answer costs a few seconds, and a target that changes its
/// quotes at most once a second is not asked more often than that.
pub const POLL_INTERVAL_MS: u64 = 5 * FEED_MIN_INTERVAL_MS;

/// How long the socket rests after it failed to receive, so a socket that
/// keeps failing does not spin.
const RECEIVE_ERROR_PAUSE_MS: u64 = 1_000;

/// What a datagram did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A snapshot was taken: the current one now.
    Snapshot,
    /// It came from another address than the target's.
    Foreign,
    /// It was refused.
    Rejected(SnapshotRejected),
}

/// Asking one target for its snapshot, without IO.
#[derive(Debug)]
pub struct SnapshotClient {
    target: SocketAddr,
    reader: SnapshotReader,
    latest: Option<Arc<Snapshot>>,
    next_ask_ms: u64,
}

impl SnapshotClient {
    /// Asking the node of identity `node`, at `target`; it asks first at
    /// `now_ms`. The caller has checked that the node's ID is its keys'.
    #[must_use]
    pub fn new(target: SocketAddr, node: NodeIdentity, now_ms: u64) -> Self {
        Self {
            target,
            reader: SnapshotReader::new(node.key()),
            latest: None,
            next_ask_ms: now_ms,
        }
    }

    /// The target's address.
    #[must_use]
    pub const fn target(&self) -> SocketAddr {
        self.target
    }

    /// When the next request is due.
    #[must_use]
    pub const fn next_ask_ms(&self) -> u64 {
        self.next_ask_ms
    }

    /// The request to send at `now_ms`, if one is due; the next is due a
    /// poll later.
    pub fn due(&mut self, now_ms: u64) -> Option<[u8; REQUEST_BYTES]> {
        if now_ms < self.next_ask_ms {
            return None;
        }
        self.next_ask_ms = now_ms.saturating_add(POLL_INTERVAL_MS);
        Some(encode_request())
    }

    /// Takes `datagram`, which came from `from` at `now_ms`.
    pub fn receive(&mut self, from: SocketAddr, datagram: &[u8], now_ms: u64) -> Event {
        if !same_address(from, self.target) {
            return Event::Foreign;
        }
        match self.reader.receive(datagram, now_ms) {
            Ok(snapshot) => {
                self.latest = Some(Arc::new(snapshot));
                Event::Snapshot
            }
            Err(rejected) => Event::Rejected(rejected),
        }
    }

    /// The newest snapshot taken, if it still counts at `now_ms`.
    #[must_use]
    pub fn current(&self, now_ms: u64) -> Option<&Arc<Snapshot>> {
        self.latest
            .as_ref()
            .filter(|snapshot| now_ms < snapshot.valid_until_ms)
    }
}

/// Whether two addresses are the same, an IPv4 address mapped into IPv6
/// being the IPv4 address.
fn same_address(a: SocketAddr, b: SocketAddr) -> bool {
    a.ip().to_canonical() == b.ip().to_canonical() && a.port() == b.port()
}

/// Asks `client`'s target on `socket` until `cancel`, and publishes each
/// snapshot it takes to `snapshots`.
pub async fn run(
    socket: UdpSocket,
    mut client: SnapshotClient,
    clock: Arc<dyn Clock>,
    snapshots: watch::Sender<Option<Arc<Snapshot>>>,
    cancel: CancellationToken,
) {
    // One more byte than a datagram may have, so a longer one is seen and
    // refused rather than cut.
    let mut buffer = vec![0u8; MAX_DATAGRAM_BYTES + 1];
    loop {
        let now_ms = clock.now_ms();
        if let Some(request) = client.due(now_ms)
            && let Err(error) = socket.send_to(&request, client.target()).await
        {
            tracing::warn!(target: TARGET, feed = %client.target(), %error, "could not ask a target for its snapshot");
        }
        let wait_ms = client.next_ask_ms().saturating_sub(now_ms);
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_millis(wait_ms)) => {}
            received = socket.recv_from(&mut buffer) => match received {
                Ok((length, from)) => {
                    let datagram = buffer.get(..length).unwrap_or_default();
                    let now_ms = clock.now_ms();
                    match client.receive(from, datagram, now_ms) {
                        Event::Snapshot => {
                            snapshots.send_replace(client.current(now_ms).cloned());
                        }
                        Event::Foreign => {
                            tracing::debug!(target: TARGET, feed = %client.target(), %from, "dropped a datagram from another address");
                        }
                        // The same snapshot again, as the target answers
                        // until it makes a new one.
                        Event::Rejected(SnapshotRejected::Old) => {}
                        Event::Rejected(rejected) => {
                            tracing::debug!(target: TARGET, feed = %client.target(), ?rejected, "dropped a datagram from the target");
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(target: TARGET, feed = %client.target(), %error, "the snapshot's socket failed");
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
    use dassie_feed::{InvalidSnapshot, Ladder, MethodQuotes, Quote, method_tag, sign_snapshot};
    use dassie_identity::NodeKey;
    use dassie_node::clock::ManualClock;

    use super::*;

    const NOW_MS: u64 = 1_790_000_000_000;
    const XRP: &str = "hub-xrpl-testnet-xrp";
    const DSUSD: &str = "hub-xrpl-testnet-dsusd";

    fn key() -> NodeKey {
        NodeKey::from_bytes(&[3; 32])
    }

    /// An identity of `public_key`'s: the snapshot is checked against its
    /// key alone, so any SLH-DSA key will do, without generating one.
    fn identity(public_key: &dassie_identity::PublicKey) -> NodeIdentity {
        NodeIdentity::new(
            *public_key,
            dassie_identity::SlhDsaPublicKey::from_bytes([3; 32]),
        )
    }

    /// A snapshot made at `sequence_ms`, valid for a minute, whose XRP bid
    /// is `bid` millionths.
    fn snapshot(sequence_ms: u64, bid: u64) -> Snapshot {
        let ladder = Ladder::floor([1_000_000_000_000; dassie_feed::LEVELS]);
        let method = |id: &str, bid: Quote| MethodQuotes {
            tag: method_tag(id),
            bid,
            ask: Quote::ceil(1, 1),
            bid_ladder: ladder,
            ask_ladder: ladder,
        };
        let mut methods = vec![
            method(XRP, Quote::floor(u128::from(bid), 1_000_000)),
            method(DSUSD, Quote::floor(1, 1)),
        ];
        methods.sort_by_key(|method| method.tag);
        Snapshot {
            sequence_ms,
            valid_until_ms: sequence_ms + 60_000,
            grid_bp: [0, 1, 2, 3, 4, 5, 6, 7],
            methods,
        }
    }

    fn signed(key: &NodeKey, snapshot: &Snapshot) -> Vec<u8> {
        sign_snapshot(key, snapshot).unwrap()
    }

    fn client(target: SocketAddr) -> SnapshotClient {
        let public_key = key().public_key();
        SnapshotClient::new(target, identity(&public_key), NOW_MS)
    }

    /// What 1000 units of XRP get of DSUSD by `snapshot`.
    fn xrp_out(snapshot: &Snapshot) -> Option<u64> {
        snapshot.swap_out(method_tag(XRP), method_tag(DSUSD), 1_000)
    }

    const TARGET_ADDRESS: SocketAddr = SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(192, 0, 2, 1)),
        7768,
    );

    #[test]
    fn it_asks_at_once_then_every_poll() {
        let mut client = client(TARGET_ADDRESS);
        assert_eq!(client.due(NOW_MS), Some(encode_request()));
        assert_eq!(client.due(NOW_MS + 1), None);
        assert_eq!(client.next_ask_ms(), NOW_MS + POLL_INTERVAL_MS);
        assert!(client.due(NOW_MS + POLL_INTERVAL_MS).is_some());
    }

    #[test]
    fn only_the_newest_verified_unexpired_snapshot_counts() {
        let mut client = client(TARGET_ADDRESS);
        let good = snapshot(NOW_MS, 510_000);
        assert_eq!(
            client.receive(TARGET_ADDRESS, &signed(&key(), &good), NOW_MS),
            Event::Snapshot
        );
        assert_eq!(
            xrp_out(client.current(NOW_MS).unwrap()),
            Some(509),
            "1000 at 0.51, less the rounding of the quote"
        );

        // An older one, or the same again, sets nothing back.
        let old = signed(&key(), &snapshot(NOW_MS - 1, 1_000_000));
        assert_eq!(
            client.receive(TARGET_ADDRESS, &old, NOW_MS),
            Event::Rejected(SnapshotRejected::Old)
        );
        // One signed by another key is dropped.
        let other = NodeKey::from_bytes(&[4; 32]);
        let foreign = signed(&other, &snapshot(NOW_MS + 1, 1_000_000));
        assert_eq!(
            client.receive(TARGET_ADDRESS, &foreign, NOW_MS),
            Event::Rejected(SnapshotRejected::Invalid(InvalidSnapshot::Signature))
        );
        // One that expired is dropped.
        let expired = signed(&key(), &snapshot(NOW_MS + 2, 1_000_000));
        assert_eq!(
            client.receive(TARGET_ADDRESS, &expired, NOW_MS + 70_000),
            Event::Rejected(SnapshotRejected::Expired)
        );
        // From another address, even the target's own is dropped.
        let elsewhere = SocketAddr::new(TARGET_ADDRESS.ip(), 7769);
        let newer = signed(&key(), &snapshot(NOW_MS + 3, 1_000_000));
        assert_eq!(client.receive(elsewhere, &newer, NOW_MS), Event::Foreign);
        assert!(matches!(
            client.receive(TARGET_ADDRESS, b"junk", NOW_MS),
            Event::Rejected(SnapshotRejected::Wire(_))
        ));
        assert_eq!(xrp_out(client.current(NOW_MS).unwrap()), Some(509));
        // The snapshot lapses with its validity.
        assert!(client.current(NOW_MS + 59_999).is_some());
        assert!(client.current(NOW_MS + 60_000).is_none());
    }

    /// A target's endpoint on a UDP socket.
    struct Server {
        socket: UdpSocket,
    }

    impl Server {
        async fn bind() -> Self {
            Self {
                socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            }
        }

        /// Takes one request, and where it came from.
        async fn request(&self) -> SocketAddr {
            let mut buffer = [0u8; 2_000];
            let (length, from) = self.socket.recv_from(&mut buffer).await.unwrap();
            assert_eq!(&buffer[..length], &encode_request()[..]);
            from
        }
    }

    /// Asks `server` with a client of `key`'s snapshots, as the market
    /// maker follows one target: the task, its snapshots, and the address
    /// the server sees it at.
    async fn follow(
        server: &Server,
        key: &NodeKey,
        clock: &Arc<ManualClock>,
        cancel: &CancellationToken,
    ) -> (
        tokio::task::JoinHandle<()>,
        watch::Receiver<Option<Arc<Snapshot>>>,
        SocketAddr,
    ) {
        let public_key = key.public_key();
        let client = SnapshotClient::new(
            server.socket.local_addr().unwrap(),
            identity(&public_key),
            NOW_MS,
        );
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (sender, snapshots) = watch::channel(None);
        let clock: Arc<dyn Clock> = Arc::clone(clock) as Arc<dyn Clock>;
        let task = tokio::spawn(run(socket, client, clock, sender, cancel.clone()));
        let asker = server.request().await;
        (task, snapshots, asker)
    }

    #[tokio::test]
    async fn it_asks_over_udp_and_takes_only_good_snapshots() {
        let server = Server::bind().await;
        let clock = Arc::new(ManualClock::new(NOW_MS));
        let cancel = CancellationToken::new();
        let (task, mut snapshots, asker) = follow(&server, &key(), &clock, &cancel).await;

        // Another node's, expired and malformed datagrams are dropped; then
        // a good one is taken.
        let other = NodeKey::from_bytes(&[4; 32]);
        let foreign = signed(&other, &snapshot(NOW_MS, 1));
        let mut expired = snapshot(NOW_MS - 120_000, 1);
        expired.valid_until_ms = NOW_MS;
        let good = signed(&key(), &snapshot(NOW_MS, 7_000_000));
        for datagram in [foreign, signed(&key(), &expired), vec![1, 2, 3], good] {
            server.socket.send_to(&datagram, asker).await.unwrap();
        }
        snapshots.changed().await.unwrap();
        let taken = snapshots.borrow_and_update().clone().unwrap();
        assert_eq!(taken.sequence_ms, NOW_MS);
        assert_eq!(xrp_out(&taken), Some(7_000));

        // A newer one from another socket, and a stale one, are dropped;
        // the next good one is taken.
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let newer = signed(&key(), &snapshot(NOW_MS + 2, 9_000_000));
        stranger.send_to(&newer, asker).await.unwrap();
        let stale = signed(&key(), &snapshot(NOW_MS - 1, 8_000_000));
        server.socket.send_to(&stale, asker).await.unwrap();
        let next = signed(&key(), &snapshot(NOW_MS + 1, 10_000_000));
        server.socket.send_to(&next, asker).await.unwrap();
        snapshots.changed().await.unwrap();
        let taken = snapshots.borrow_and_update().clone().unwrap();
        assert_eq!(
            (taken.sequence_ms, xrp_out(&taken)),
            (NOW_MS + 1, Some(10_000))
        );

        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn it_follows_several_targets_each_by_its_own_key() {
        let (key_a, key_b) = (key(), NodeKey::from_bytes(&[6; 32]));
        let a = Server::bind().await;
        let b = Server::bind().await;
        let clock = Arc::new(ManualClock::new(NOW_MS));
        let cancel = CancellationToken::new();
        let (task_a, mut snapshots_a, at_a) = follow(&a, &key_a, &clock, &cancel).await;
        let (task_b, mut snapshots_b, at_b) = follow(&b, &key_b, &clock, &cancel).await;
        assert_ne!(at_a, at_b, "a socket per target");

        // Each takes its own target's snapshot.
        a.socket
            .send_to(&signed(&key_a, &snapshot(NOW_MS, 7_000_000)), at_a)
            .await
            .unwrap();
        b.socket
            .send_to(&signed(&key_b, &snapshot(NOW_MS, 11_000_000)), at_b)
            .await
            .unwrap();
        snapshots_a.changed().await.unwrap();
        snapshots_b.changed().await.unwrap();
        let out = |snapshots: &mut watch::Receiver<Option<Arc<Snapshot>>>| {
            let snapshot = snapshots.borrow_and_update().clone().unwrap();
            (snapshot.sequence_ms, xrp_out(&snapshot))
        };
        assert_eq!(out(&mut snapshots_a), (NOW_MS, Some(7_000)));
        assert_eq!(out(&mut snapshots_b), (NOW_MS, Some(11_000)));

        // To b's asker, each newer than what follows: a's snapshot from a's
        // address, a's from b's address, and b's own expired. Had any been
        // taken, b's next, older, would not be.
        a.socket
            .send_to(&signed(&key_a, &snapshot(NOW_MS + 9, 90_000_000)), at_b)
            .await
            .unwrap();
        b.socket
            .send_to(&signed(&key_a, &snapshot(NOW_MS + 8, 80_000_000)), at_b)
            .await
            .unwrap();
        clock.advance(30_000);
        let mut expired = snapshot(NOW_MS + 7, 70_000_000);
        expired.valid_until_ms = NOW_MS + 20_000;
        b.socket
            .send_to(&signed(&key_b, &expired), at_b)
            .await
            .unwrap();
        b.socket
            .send_to(&signed(&key_b, &snapshot(NOW_MS + 1, 12_000_000)), at_b)
            .await
            .unwrap();
        snapshots_b.changed().await.unwrap();
        assert_eq!(out(&mut snapshots_b), (NOW_MS + 1, Some(12_000)));
        // a's own client took none of it.
        assert!(!snapshots_a.has_changed().unwrap());

        cancel.cancel();
        task_a.await.unwrap();
        task_b.await.unwrap();
    }
}

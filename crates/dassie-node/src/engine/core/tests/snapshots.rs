//! Snapshots over the peer protocol in the node
//! (spec/decisions/peer-swaps.md,
//! spec/areas/network.md#snapshot-subscriptions): a core node pushes its
//! snapshot to a subscribed downlink at once and then on the rate feed's
//! schedule, refuses while its owner offers no swaps, and subscribes to a
//! peering's snapshots only while its owner lets it swap and the peering
//! spans two ledgers.

use std::sync::Arc;

use dassie_feed::{FEED_HEARTBEAT_MS, FEED_MIN_INTERVAL_MS};
use dassie_identity::IdentityKey;
use dassie_node_core::config::{LiveRealm, Realm};
use dassie_oer::{Decode, Encode};
use dassie_peer::wire::{Content, Kind, RefusalReason, SubscriptionResponse};
use dassie_peer::{Method, Peering, PeeringHooks, RequestId, Response};

use super::super::{Core, Output, Quoting, SnapshotSource};
use super::{NOW, STUB_TWO, TRUSTING, add_stub, core_taking_downlinks, scheme_id};
use crate::store::{PeerSettings, Store};

/// A snapshot of its own bytes, which a test replaces to change it.
struct Fixed(Option<Arc<[u8]>>);

impl SnapshotSource for Fixed {
    fn current(&mut self, _: u64, _: &Quoting<'_>) -> Option<Arc<[u8]>> {
        self.0.clone()
    }
}

fn snapshot(byte: u8) -> Arc<[u8]> {
    Arc::from(vec![byte; 200])
}

/// A core node on the stub with unlimited trust, with a downlink peered on
/// it, at `NOW + 1 000`.
fn with_downlink() -> (Core, Arc<IdentityKey>) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_taking_downlinks(store, Realm::Test, LiveRealm::Refused, NOW);
    add_stub(&mut core, TRUSTING);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let downlink = Arc::new(IdentityKey::from_bytes(&[11; 32]));
    let at = NOW + 1_000;
    core.begin_step(at);
    let body = super::stub_downlink_request(&mut core, &downlink, at);
    core.peer_inbound(dassie_peer::InboundId(1), downlink.identity(), &body, None);
    core.finish_step(&mut outputs);
    let id = downlink.node_id();
    assert!(core.peer.is_downlink(&id));
    assert!(core.peer.active_peers().any(|(peer, _)| *peer == id));
    (core, downlink)
}

/// `from` sends `content` to `core` at `at`: the reply, and every output
/// of the step.
fn send(
    core: &mut Core,
    from: &IdentityKey,
    content: &Content<'_>,
    at: u64,
) -> (dassie_peer::Reply, Vec<Output>) {
    core.begin_step(at);
    core.peer_inbound(
        dassie_peer::InboundId(at),
        from.identity(),
        &content.to_vec(),
        None,
    );
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let reply = outputs
        .iter()
        .find_map(|output| match output {
            Output::PeerReply { reply, .. } => Some(reply.clone()),
            _ => None,
        })
        .unwrap();
    (reply, outputs)
}

/// `downlink` subscribes to `core`'s snapshots at `at`: the answer, and
/// every output of the step.
fn subscribe(
    core: &mut Core,
    downlink: &IdentityKey,
    at: u64,
) -> (SubscriptionResponse, Vec<Output>) {
    let (reply, outputs) = send(
        core,
        downlink,
        &Content::Subscription { subscribe: true },
        at,
    );
    assert_eq!(reply.status, 200);
    (
        SubscriptionResponse::decode_exact(&reply.body).unwrap(),
        outputs,
    )
}

/// The snapshots pushed in `outputs`, each with its request.
fn pushes(outputs: &[Output]) -> Vec<(RequestId, Vec<u8>)> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::PeerRequest(request) if request.kind == Kind::Snapshot => {
                match Content::decode_exact(&request.body).unwrap() {
                    Content::Snapshot(bytes) => Some((request.id, bytes.to_vec())),
                    other => panic!("not a snapshot: {other:?}"),
                }
            }
            _ => None,
        })
        .collect()
}

/// A step of `core` at `at`: its outputs.
fn step(core: &mut Core, at: u64) -> Vec<Output> {
    core.begin_step(at);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    outputs
}

/// The subscriber's answer to a push: `status`.
fn answer(core: &mut Core, id: RequestId, status: u16) {
    let response = Response {
        status,
        body: Vec::new(),
    };
    core.peer_response(id, Ok(response));
}

// spec/decisions/peer-swaps.md: a subscriber gets the snapshot at once,
// then each the node makes, on the feed's schedule: at most once per
// interval while it changes, a heartbeat otherwise. A subscriber that
// answers a push with 409 takes our snapshots no more, and nothing is due
// once none is left.
#[test]
fn a_subscriber_gets_the_snapshot_at_once_then_on_the_feeds_schedule() {
    let (mut core, downlink) = with_downlink();
    core.set_snapshot_source(Box::new(Fixed(Some(snapshot(1)))));
    step(&mut core, NOW + 1_500);
    assert_eq!(core.peer.own_snapshot(), Some(&[1; 200][..]));
    assert_eq!(
        core.snapshot_deadline(),
        None,
        "nothing due without subscribers"
    );

    let start = NOW + 2_000;
    let (answered, outputs) = subscribe(&mut core, &downlink, start);
    assert_eq!(answered, SubscriptionResponse::Accepted);
    let pushed = pushes(&outputs);
    assert_eq!(pushed.len(), 1, "once, as it subscribed");
    assert_eq!(pushed[0].1, [1; 200]);
    assert_eq!(core.snapshot_deadline(), Some(start + FEED_HEARTBEAT_MS));
    answer(&mut core, pushed[0].0, 200);

    // A change within the interval waits for its end.
    core.set_snapshot_source(Box::new(Fixed(Some(snapshot(2)))));
    assert!(pushes(&step(&mut core, start + 100)).is_empty());
    assert_eq!(core.snapshot_deadline(), Some(start + FEED_MIN_INTERVAL_MS));
    let changed = start + FEED_MIN_INTERVAL_MS;
    let pushed = pushes(&step(&mut core, changed));
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].1, [2; 200]);
    answer(&mut core, pushed[0].0, 200);

    // Unchanged, the heartbeat.
    assert!(pushes(&step(&mut core, changed + FEED_HEARTBEAT_MS - 1)).is_empty());
    let pushed = pushes(&step(&mut core, changed + FEED_HEARTBEAT_MS));
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].1, [2; 200]);

    // Not subscribed after all, as after a restart of its own.
    answer(&mut core, pushed[0].0, 409);
    assert!(!core.peer.has_subscribers());
    step(&mut core, changed + FEED_HEARTBEAT_MS + 1);
    assert_eq!(core.snapshot_deadline(), None);
    core.check_invariants();
}

// spec/decisions/peer-swaps.md: a node whose owner offers no swaps refuses
// a subscription, saying so, and has no snapshot to give; once the owner
// turns them off, its subscribers are forgotten within a check.
#[test]
fn a_node_that_offers_no_swaps_refuses_and_forgets_its_subscribers() {
    let (mut core, downlink) = with_downlink();
    core.set_snapshot_source(Box::new(Fixed(Some(snapshot(1)))));
    let off = PeerSettings {
        offer_swaps: false,
        ..PeerSettings::DEFAULT
    };
    core.set_peer_settings(off).unwrap();
    assert!(!core.connector.offers_swaps(), "nor does it take swaps");
    let at = NOW + 2_000;
    let (answered, outputs) = subscribe(&mut core, &downlink, at);
    assert_eq!(
        answered,
        SubscriptionResponse::Refused(RefusalReason::NoSwaps)
    );
    assert!(pushes(&outputs).is_empty());
    assert_eq!(core.peer.own_snapshot(), None);

    core.set_peer_settings(PeerSettings::DEFAULT).unwrap();
    assert!(core.connector.offers_swaps());
    step(&mut core, at + 1);
    let (answered, outputs) = subscribe(&mut core, &downlink, at + 2);
    assert_eq!(answered, SubscriptionResponse::Accepted);
    assert_eq!(pushes(&outputs).len(), 1);

    core.set_peer_settings(off).unwrap();
    let mut pushed = 0;
    for second in 1..=10 {
        pushed += pushes(&step(&mut core, at + second * 1_000)).len();
    }
    assert!(!core.peer.has_subscribers());
    assert_eq!(pushed, 0, "nothing to push");
    core.check_invariants();
}

// spec/decisions/peer-swaps.md: a downlink is a periphery node, which
// offers no swaps: the node never subscribes to it, and takes no snapshot
// from it.
#[test]
fn a_downlink_is_never_subscribed_to() {
    let (mut core, downlink) = with_downlink();
    let (reply, _) = send(
        &mut core,
        &downlink,
        &Content::Snapshot(&[9; 10]),
        NOW + 2_000,
    );
    assert_eq!(reply.status, 409);
    let mut asked = 0;
    for second in 3..=20 {
        asked += step(&mut core, NOW + second * 1_000)
            .iter()
            .filter(|output| {
                matches!(output, Output::PeerRequest(request) if request.kind == Kind::Subscription)
            })
            .count();
    }
    assert_eq!(asked, 0);
    assert!(core.peer_snapshots().next().is_none());
    assert_eq!(core.peer_snapshot(&downlink.node_id()), None);
}

// spec/decisions/peer-swaps.md: the node subscribes to a peering's
// snapshots only while its owner lets it swap and the peering's methods are
// on two ledgers or more, of schemes that run; it offers swaps as its owner
// says.
#[test]
fn snapshots_are_wanted_of_peerings_on_two_ledgers_while_swapping() {
    let (mut core, _) = with_downlink();
    core.add_scheme(STUB_TWO, r#"{"commitments":4}"#.into())
        .unwrap();
    let method = |id: &str| Method::new(scheme_id(id), Vec::new(), 0);
    let one = Peering::new(method("stub"), NOW, false);
    let mut two = one.clone();
    two.methods.push(method(STUB_TWO));
    let mut unrun = one.clone();
    unrun.methods.push(method("stub-three"));
    let peer = IdentityKey::from_bytes(&[12; 32]).node_id();
    let wants = |core: &mut Core, peering: &Peering| {
        core.with_peer(|_, hooks, _| (hooks.wants_snapshots(peer, peering), hooks.offers_swaps()))
    };
    assert_eq!(wants(&mut core, &two), (true, true));
    assert_eq!(wants(&mut core, &one), (false, true));
    assert_eq!(wants(&mut core, &unrun), (false, true), "a scheme not run");
    core.set_peer_settings(PeerSettings {
        swapping: false,
        ..PeerSettings::DEFAULT
    })
    .unwrap();
    assert_eq!(wants(&mut core, &two), (false, true));
    core.set_peer_settings(PeerSettings {
        offer_swaps: false,
        ..PeerSettings::DEFAULT
    })
    .unwrap();
    assert_eq!(wants(&mut core, &two), (true, false));
}

/// A snapshot of `key`'s made at `sequence_ms`, counting for a minute.
fn peer_snapshot(key: &IdentityKey, sequence_ms: u64) -> Vec<u8> {
    let snapshot = dassie_feed::Snapshot {
        sequence_ms,
        valid_until_ms: sequence_ms + 60_000,
        grid_bp: [1, 3, 10, 30, 100, 300, 1_000, 3_000],
        methods: Vec::new(),
    };
    dassie_feed::sign_snapshot(key, &snapshot).unwrap()
}

// spec/decisions/peer-swaps.md: a peer's snapshot is kept only if it
// decodes, verifies under the key its connection proved, is newer than
// the one held and has not expired, as a reader takes one.
#[test]
fn a_peers_snapshot_is_taken_only_verified_newer_and_unexpired() {
    let (mut core, _) = with_downlink();
    let peer = IdentityKey::from_bytes(&[12; 32]);
    let key = peer.public_key();
    let id = peer.node_id();
    let at = NOW + 2_000;
    core.begin_step(at);
    let first = peer_snapshot(&peer, at - 10);
    let newer = peer_snapshot(&peer, at - 5);
    let foreign = peer_snapshot(&IdentityKey::from_bytes(&[13; 32]), at);
    let expired = peer_snapshot(&peer, at - 60_000);
    let takes = |core: &mut Core, bytes: &[u8], held: Option<&[u8]>| {
        core.with_peer(|_, hooks, _| hooks.takes_snapshot(id, &key, bytes, held))
    };
    assert!(takes(&mut core, &first, None));
    assert!(takes(&mut core, &newer, Some(&first)));
    assert!(!takes(&mut core, &first, Some(&newer)), "older");
    assert!(!takes(&mut core, &first, Some(&first)), "the same");
    assert!(!takes(&mut core, &foreign, None), "another node's");
    assert!(!takes(&mut core, &expired, None), "expired");
    assert!(!takes(&mut core, &[2, 1, 0], None), "broken");
    let request = dassie_feed::encode_request();
    assert!(!takes(&mut core, &request, None), "a request");
}

// spec/areas/rate-feed.md#the-node-side: a core node that offers swaps has
// a snapshot, made by the engine's source and signed with its key; one
// whose owner offers none has none. Its link state names the port it
// answers requests on only while it offers swaps.
#[test]
fn a_core_node_that_offers_swaps_has_a_snapshot_and_names_its_port() {
    let (mut core, _) = with_downlink();
    let key = Arc::new(IdentityKey::from_bytes(&[7; 32]));
    core.set_snapshot_source(Box::new(super::super::OwnSnapshots::new(Arc::clone(&key))));
    let at = NOW + 2_000;
    step(&mut core, at);
    let bytes = core.own_snapshot().unwrap().clone();
    let mut reader = dassie_feed::SnapshotReader::new(key.public_key());
    assert_eq!(reader.receive(&bytes, at).unwrap().sequence_ms, at);
    assert_eq!(core.peer.own_snapshot(), Some(&bytes[..]));
    assert!(core.snapshot_deadline().is_some(), "the heartbeat");

    let info = core.peer.own_info().clone();
    core.set_own_info(dassie_peer::OwnInfo {
        feed_port: Some(7768),
        ..info
    });
    assert_eq!(core.peer.own_info().feed_port, Some(7768));
    let off = PeerSettings {
        offer_swaps: false,
        ..PeerSettings::DEFAULT
    };
    core.set_peer_settings(off).unwrap();
    step(&mut core, at + 1);
    assert_eq!(core.own_snapshot(), None);
    assert_eq!(core.peer.own_snapshot(), None);
    assert_eq!(core.peer.own_info().feed_port, None);
    core.set_peer_settings(PeerSettings::DEFAULT).unwrap();
    step(&mut core, at + 2);
    assert!(core.own_snapshot().is_some());
    assert_eq!(core.peer.own_info().feed_port, Some(7768));
    core.check_invariants();
}

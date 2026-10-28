//! Our settlements on several methods with one peer, which share the
//! pair's settlement channel, one message at a time
//! (spec/areas/settlement.md#outgoing-settlements): one that executes while
//! a message on another method is still on its way to the peer. On one
//! engine core whose channel with the peer is in its database, running the
//! stub with commitments, whose messages book what they state, and the
//! second and third stubs, which pretend to be two more hubs and here send
//! bare notices that book nothing, as hubs' do.

use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_ledger::{AccountName, AccountPath, LedgerId, Name, Reference, TransferSpec};
use dassie_peer::wire::{Endpoint, Kind};
use dassie_peer::{Channel, Outgoing, PeerBatch, Response, StoredChannel};

use super::super::Core;
use super::super::settlement::Settling;
use super::{NOW, STUB_THREE, STUB_TWO, StubTwo, add_stub, core_on, scheme_id, stub_two_bare};
use crate::engine::Output;
use crate::node::Policies;
use crate::store::{LedgerBatch, Store};

/// The config of each stub with commitments here.
const COMMITMENTS: &str = r#"{"commitments":4}"#;

/// What each settlement here moves.
const AMOUNT: u64 = 400;

/// The peer.
fn peer() -> NodeId {
    NodeKey::from_bytes(&[3; 32]).node_id()
}

/// A database holding our settlement channel with [`peer`], with our
/// message on the method on `busy` on its way, the first we sent it, if
/// one is named.
fn store_with_channel(busy: Option<&str>) -> Arc<Store> {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let key = NodeKey::from_bytes(&[3; 32]);
    let outbox = busy.map(|scheme| Outgoing {
        sequence: 1,
        scheme: scheme_id(scheme),
        amount: 0,
        data: Vec::new(),
        attempts: 0,
    });
    let channel = StoredChannel {
        node: key.node_id(),
        channel: Channel {
            key: key.public_key(),
            endpoint: Endpoint::of_url("https://peer.test"),
            settled_in: 0,
            settled_out: u64::from(busy.is_some()),
            outbox,
            paid_us: false,
        },
    };
    let batch = LedgerBatch {
        peer: PeerBatch {
            channels: vec![channel],
            ..PeerBatch::default()
        },
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    store
}

/// A core on `store` that runs the stub with commitments and the second
/// and third stubs.
fn node(store: &Arc<Store>, seed: u64) -> Core {
    let mut core = core_on(Arc::clone(store), seed);
    add_stub(&mut core, COMMITMENTS);
    for scheme in [STUB_TWO, STUB_THREE] {
        core.add_scheme(scheme, COMMITMENTS.into()).unwrap();
    }
    core.flush();
    core
}

/// The ledger of `scheme`.
fn ledger_of(scheme: &str) -> LedgerId {
    StubTwo::of(scheme).map_or(LedgerId::StubUsd, |(ledger, _)| ledger)
}

/// Our settlement of [`AMOUNT`] to [`peer`] on `scheme`, prepared by the
/// scheme and booked pending, to execute in the next step, as the node
/// makes one when the peer asks it to settle.
fn settle(core: &mut Core, scheme: &str) {
    let ledger = ledger_of(scheme);
    let name = Name::new(peer().as_str()).unwrap();
    let path = AccountPath::new(ledger, AccountName::AssetsInterledger(name));
    let interledger = match core.ledger.find(&path) {
        Some(account) => account,
        None => core.ledger.open_peer(NOW, ledger, name, 1_000).unwrap().0,
    };
    let settlement = core.account(ledger, AccountName::AssetsSettlement).unwrap();
    let scheme = Name::new(scheme).unwrap();
    let prepared = {
        let Core { schemes, rng, .. } = &mut *core;
        schemes
            .get_mut(&scheme)
            .unwrap()
            .scheme
            .prepare_settlement(rng.as_mut(), None, name, AMOUNT, &[])
            .unwrap()
    };
    assert_eq!(prepared.amount, AMOUNT);
    let spec = TransferSpec {
        debit: interledger,
        credit: settlement,
        amount: AMOUNT,
        pending: true,
        reference: Reference::Settlement {
            scheme,
            id: prepared.id,
        },
    };
    let transfer = core.ledger.create(NOW, &spec).unwrap();
    core.settle.settling.insert(
        (scheme, prepared.id),
        Settling {
            peer: peer(),
            transfer,
            amount: AMOUNT,
            message: prepared.message,
        },
    );
    core.settle.to_execute.push((scheme, prepared.id));
    core.flush();
}

/// What the books hold in settlement on `scheme`'s ledger, and what the
/// scheme holds: the two are one once nothing of it is on its way.
fn held(core: &Core, scheme: &str) -> (i128, u64) {
    let ledger = ledger_of(scheme);
    let account = core
        .ledger
        .find(&AccountPath::new(ledger, AccountName::AssetsSettlement))
        .unwrap();
    let booked = -core.ledger.account(account).unwrap().balance().net_credit();
    let view = core
        .ledger_views()
        .into_iter()
        .find(|view| view.id == ledger)
        .unwrap();
    (booked, view.on_ledger_balance)
}

/// Runs a step at `now`, and answers our settlement message to the peer
/// that went in it, if one did, as the peer would once it applied it.
/// Returns that message's scheme and sequence.
fn step_and_acknowledge(core: &mut Core, now: u64) -> Option<(String, u64)> {
    let mut outputs = Vec::new();
    core.begin_step(now);
    core.finish_step(&mut outputs);
    let id = outputs.into_iter().find_map(|output| match output {
        Output::PeerRequest(request)
            if request.to == peer() && request.kind == Kind::Settlement =>
        {
            Some(request.id)
        }
        _ => None,
    })?;
    let went = on_its_way(core);
    core.begin_step(now);
    core.peer_response(
        id,
        Ok(Response {
            status: 200,
            body: Vec::new(),
        }),
    );
    core.finish_step(&mut Vec::new());
    assert_eq!(on_its_way(core), None, "not acknowledged");
    went
}

/// The scheme of our message on its way to the peer, and its sequence.
fn on_its_way(core: &Core) -> Option<(String, u64)> {
    let outbox = core.peer.channel(&peer())?.outbox.as_ref()?;
    Some((outbox.scheme.to_string(), outbox.sequence))
}

// spec/areas/settlement.md#outgoing-settlements,
// spec/areas/hubs.md#in-the-node: a node in two hubs has a notice on one
// hub's method on its way to the peer, unacknowledged, when its settlement
// over the other hub executes, and its scheme forgets it as it finalizes
// it. The settlement is posted then, and only its notice waits. A restart
// before that notice went finds nothing of it pending, which the scheme,
// having forgotten it, would cancel though it was paid: the books hold
// what the scheme holds, the settlement booked, and the message on its way
// is still the one before.
#[test]
fn a_settlement_finalized_behind_another_message_survives_a_restart() {
    stub_two_bare(true);
    let store = store_with_channel(Some(STUB_TWO));
    let before = {
        let mut core = node(&store, 5);
        assert!(core.peer.settling(&peer()));
        let before = held(&core, STUB_THREE);
        assert_eq!(before.0, i128::from(before.1));
        settle(&mut core, STUB_THREE);
        core.begin_step(NOW + 1);
        core.finish_step(&mut Vec::new());
        assert_eq!(held(&core, STUB_THREE).1, before.1 - AMOUNT, "not paid");
        before
    };
    // The crash: what is not on disk is gone.
    let core = core_on(Arc::clone(&store), 6);
    assert_eq!(core.ledger.pending_transfers().count(), 0);
    let after = held(&core, STUB_THREE);
    assert_eq!(after.1, before.1 - AMOUNT, "the scheme forgot it paid");
    assert_eq!(
        after.0,
        i128::from(after.1),
        "the books cancelled a settlement the scheme paid"
    );
    assert_eq!(on_its_way(&core), Some((STUB_TWO.into(), 1)));
}

// spec/areas/settlement.md#outgoing-settlements: two settlements, over two
// hubs, execute behind a message on a third method still on its way: both
// are booked at once, and their notices follow it in the order they
// executed, one at a time, each once the one before it was acknowledged.
#[test]
fn notices_behind_another_message_follow_it_in_order() {
    stub_two_bare(true);
    let store = store_with_channel(Some("stub"));
    let mut core = node(&store, 7);
    settle(&mut core, STUB_TWO);
    settle(&mut core, STUB_THREE);
    let interval = Policies::DEFAULT.settlement_interval_ms;
    let mut now = NOW + 1;
    assert_eq!(
        step_and_acknowledge(&mut core, now),
        Some(("stub".into(), 1)),
        "the message on its way did not go"
    );
    assert_eq!(core.ledger.pending_transfers().count(), 0, "not booked");
    for scheme in [STUB_TWO, STUB_THREE] {
        let (booked, holds) = held(&core, scheme);
        assert_eq!(booked, i128::from(holds), "on {scheme}");
    }
    let mut sent = Vec::new();
    while !core.settle.notices.is_empty() {
        assert!(sent.len() < 2, "a notice went twice");
        now += interval;
        let went = step_and_acknowledge(&mut core, now);
        assert!(went.is_some(), "a notice did not go");
        sent.extend(went);
    }
    assert_eq!(sent, [(STUB_TWO.into(), 2), (STUB_THREE.into(), 3)]);
}

// spec/areas/settlement.md#outgoing-settlements: on a scheme whose messages
// book what they state, a settlement is posted with its message, in one
// batch, so the message must find the channel free. The node settles on
// such a scheme with a peer only while no settlement of ours to it is
// pending on any method, and on no other method with it until that one is
// posted. A settlement on a scheme whose messages book nothing holds up
// only such a one: over the other hubs, settling goes on.
#[test]
fn a_settlement_whose_message_books_waits_for_the_channel() {
    stub_two_bare(true);
    let (stub, two, three) = (
        scheme_id("stub"),
        scheme_id(STUB_TWO),
        scheme_id(STUB_THREE),
    );
    let mut core = node(&store_with_channel(None), 9);
    assert!(
        [stub, two, three]
            .iter()
            .all(|on| core.can_settle(&peer(), *on))
    );
    settle(&mut core, "stub");
    assert!(
        [two, three].iter().all(|on| !core.can_settle(&peer(), *on)),
        "settles beside a settlement whose message books"
    );
    let mut core = node(&store_with_channel(None), 10);
    settle(&mut core, STUB_THREE);
    assert!(
        !core.can_settle(&peer(), stub),
        "settles where the message books beside another settlement"
    );
    assert!(core.can_settle(&peer(), two), "one hub held up another");
}

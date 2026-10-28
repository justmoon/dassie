//! Our settlements on several methods with one peer, which share the
//! pair's settlement channel, one message at a time
//! (spec/areas/settlement.md#outgoing-settlements): one that executes while
//! a message on another method is still on its way to the peer. On one
//! engine core whose channel with the peer is in its database, running the
//! stub with commitments, whose messages book what they state, and the
//! second and third stubs, which pretend to be two more hubs and here send
//! and take settlement messages as the hub's scheme does: bare notices
//! that book nothing, as `HubScheme::BOOKS_MESSAGES` says, which a test
//! checks against the real scheme.

use std::collections::BTreeMap;
use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_ledger::{AccountName, AccountPath, LedgerId, Name, Reference, TransferSpec};
use dassie_peer::wire::{Endpoint, Kind};
use dassie_peer::{Channel, Outgoing, PeerBatch, Response, StoredChannel};

use super::super::settlement::Settling;
use super::super::trust::raise_trust;
use super::super::{Core, Event, SettlementEvent};
use super::{
    NOW, STUB_THREE, STUB_TWO, StubTwo, add_stub, core_on, hub_config, scheme_id, stub_two_as_hub,
};
use crate::engine::Output;
use crate::node::Policies;
use crate::sim::checks::{check_delivered, check_held, check_ledgers};
use crate::sim::{START, Settling as Run, Sim, key};
use crate::store::{LedgerBatch, Store};

/// The config of each stub with commitments here.
const COMMITMENTS: &str = r#"{"commitments":4}"#;

/// What each settlement here moves.
const AMOUNT: u64 = 400;

/// The peer.
fn peer() -> NodeId {
    NodeKey::from_bytes(&[3; 32])
        .identity(&dassie_identity::TableKeygen)
        .node_id()
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
        node: key.identity(&dassie_identity::TableKeygen).node_id(),
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
    settle_to(core, scheme, peer(), NOW);
    core.flush();
}

/// Our settlement of [`AMOUNT`] to `to` on `scheme` at `now`, as
/// [`settle`] makes one, within a step.
fn settle_to(core: &mut Core, scheme: &str, to: NodeId, now: u64) {
    let ledger = ledger_of(scheme);
    let name = Name::new(to.as_str()).unwrap();
    let path = AccountPath::new(ledger, AccountName::AssetsInterledger(name));
    let interledger = match core.ledger.find(&path) {
        Some(account) => account,
        None => core.ledger.open_peer(now, ledger, name, 1_000).unwrap().0,
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
    let transfer = core.ledger.create(now, &spec).unwrap();
    core.settle.settling.insert(
        (scheme, prepared.id),
        Settling {
            peer: to,
            transfer,
            amount: AMOUNT,
        },
    );
    core.settle.to_execute.push((scheme, prepared.id));
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
    acknowledge(core, outputs, now)
}

/// Answers our settlement message to the peer in `outputs`, if one went,
/// at `now`, as the peer would once it applied it. Returns that message's
/// scheme and sequence.
fn acknowledge(core: &mut Core, outputs: Vec<Output>, now: u64) -> Option<(String, u64)> {
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
    stub_two_as_hub(true);
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
    stub_two_as_hub(true);
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
        // Posted, its notice waiting: on its way, which is the answer to a
        // request to settle on its method.
        let on = scheme_id(scheme);
        assert_eq!(core.settlement_requested(peer(), on), AMOUNT, "on {scheme}");
    }
    assert_eq!(core.ledger.pending_transfers().count(), 0, "settled again");
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
    stub_two_as_hub(true);
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

// spec/areas/hubs.md#in-the-node, spec/areas/settlement.md#incoming-settlements:
// the pretend hubs here send and take settlement messages as the real hub
// scheme does, which a core running it reports: its messages book nothing.
// So a peer's notice over a hub is taken whatever amount it states, even
// one the books could never take, and books nothing: the hub says what
// arrived.
#[test]
fn the_pretend_hubs_messages_are_the_hub_schemes() {
    stub_two_as_hub(true);
    let books = |core: &Core, scheme: &str| {
        core.schemes[&Name::new(scheme).unwrap()]
            .scheme
            .books_messages()
    };
    // The second stub serves the XRP hub's ledger, which the hub scheme
    // books on: one core runs them, another the real scheme.
    let pretending = node(&store_with_channel(None), 11);
    let mut core = core_on(Arc::new(Store::open_in_memory().unwrap()), 12);
    let hub = dassie_hub::member::SCHEME_ID;
    core.add_scheme(hub, hub_config(600, 300)).unwrap();
    assert!(!books(&core, hub), "a hub's message books");
    for scheme in [STUB_TWO, STUB_THREE] {
        assert_eq!(books(&pretending, scheme), books(&core, hub), "on {scheme}");
    }
    assert!(books(&pretending, "stub"));
    core.begin_step(NOW + 1);
    let transfers = core.ledger.next_transfer_id();
    for amount in [0, AMOUNT, dassie_ledger::MAX_TOTAL + 1, u64::MAX] {
        core.incoming_settlement(peer(), scheme_id(hub), amount, &[])
            .unwrap();
    }
    assert_eq!(core.ledger.next_transfer_id(), transfers, "booked");
    core.finish_step(&mut Vec::new());
}

// spec/areas/settlement.md#outgoing-settlements: a settlement whose message
// books that finds the channel busy all the same, as one pending from
// before a restart can when its scheme finalizes it beside another
// method's message from then, does not stop the node. It is posted, and reported; its message waits, and goes in
// order, stating what it booked, once the one before it arrived. A crash
// before then restarts cleanly, with nothing pending.
#[test]
fn a_settlement_whose_message_books_behind_another_message_does_not_stop_the_node() {
    stub_two_as_hub(true);
    let store = store_with_channel(Some(STUB_TWO));
    let mut core = node(&store, 12);
    settle(&mut core, "stub");
    let mut outputs = Vec::new();
    core.begin_step(NOW + 1);
    core.finish_step(&mut outputs);
    assert!(outputs.iter().any(|output| matches!(
        output,
        Output::Event(Event::Settlement(SettlementEvent::MessageWaits { scheme, .. }))
            if scheme.as_str() == "stub"
    )));
    assert_eq!(core.ledger.pending_transfers().count(), 0, "not posted");
    let (booked, holds) = held(&core, "stub");
    assert_eq!(booked, i128::from(holds));
    assert_eq!(core.settle.notices.len(), 1);
    // The crash, before the message went: what is not on disk is gone.
    let restarted = core_on(Arc::clone(&store), 13);
    assert_eq!(restarted.ledger.pending_transfers().count(), 0);
    assert!(restarted.settle.notices.is_empty());
    assert_eq!(on_its_way(&restarted), Some((STUB_TWO.into(), 1)));
    restarted.check_invariants();
    drop(restarted);
    // Without one, it goes once the one before it arrived.
    let mut now = NOW + 1;
    assert_eq!(
        acknowledge(&mut core, outputs, now),
        Some((STUB_TWO.into(), 1))
    );
    now += Policies::DEFAULT.settlement_interval_ms;
    core.begin_step(now);
    core.finish_step(&mut Vec::new());
    let outbox = core.peer.channel(&peer()).unwrap().outbox.as_ref().unwrap();
    assert_eq!(
        (outbox.scheme.as_str(), outbox.sequence, outbox.amount),
        ("stub", 2, AMOUNT)
    );
    assert!(core.settle.notices.is_empty());
}

// spec/areas/testing.md#the-simulations: the checks of a run over the stub
// with commitments count a settlement whose message waits behind another
// as on its way. The payer's stub paid it and the payee's has not taken it,
// so the stubs' total counts it, and it is not delivered until it went and
// the peer applied it: while the message before it is on its way, and once
// that one arrived and the channel is free but it still waits to go.
#[test]
fn a_settlement_message_that_waits_is_on_its_way_until_delivered() {
    let mut sim = Sim::with(1, 2, 1, BTreeMap::new(), Run::StubHub(1));
    sim.loss_per_mille = 0;
    let busy = |sim: &Sim| {
        (0..2).find(|&index| {
            sim.core(index)
                .network()
                .channels
                .iter()
                .any(|(_, channel)| channel.outbox.is_some())
        })
    };
    assert!(
        sim.run_until_or(START + 60_000, |sim| busy(sim).is_some()),
        "no settlement went"
    );
    let from = busy(&sim).unwrap();
    let to = key(1 - from).node_id();
    let now = sim.now;
    // Another settlement to the same peer, which funds our hold toward it
    // as the node's does: the credit raised by it first, so the books take
    // it, then following the holds.
    sim.step(from, |core| {
        let name = Name::new(to.as_str()).unwrap();
        raise_trust(&mut core.ledger, now, LedgerId::StubUsd, name, AMOUNT).unwrap();
        settle_to(core, "stub", to, now);
        core.sync_credit_with(Name::new("stub").unwrap(), to);
    });
    sim.step(from, |_| {});
    let total = 2 * Run::deposit(LedgerId::StubUsd);
    let stubs = |sim: &Sim| -> i128 {
        (0..2)
            .map(|index| check_held(sim, index, LedgerId::StubUsd).unwrap())
            .sum()
    };
    let waiting = |sim: &Sim| !sim.core(from).waiting_notices().is_empty();
    let behind = |sim: &Sim| sim.core(from).peer.channel(&to).unwrap().outbox.is_some();
    // Behind the message before it.
    assert!(waiting(&sim) && behind(&sim), "the message did not wait");
    assert!(stubs(&sim) < total);
    assert_eq!(check_ledgers(&sim), Ok(()));
    assert!(check_delivered(&sim).is_err());
    // Every channel free, the message still waiting.
    assert!(sim.run_until_or(sim.now + 10_000, |sim| busy(sim).is_none()));
    assert!(waiting(&sim), "the message went with the acknowledgement");
    assert!(stubs(&sim) < total);
    assert_eq!(check_ledgers(&sim), Ok(()));
    assert_eq!(
        check_delivered(&sim),
        Err(format!(
            "node {from}'s settlement to {to} on stub waits to go"
        ))
    );
    // Delivered.
    let delivered = |sim: &Sim| check_delivered(sim).is_ok();
    assert!(sim.run_until_or(sim.now + 60_000, delivered));
    assert_eq!(stubs(&sim), total);
    assert_eq!(check_ledgers(&sim), Ok(()));
}

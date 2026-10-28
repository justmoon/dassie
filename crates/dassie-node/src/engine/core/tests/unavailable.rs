//! Settlement methods that take nothing new (spec/decisions/settlement-methods.md,
//! spec/areas/network.md#settlement-methods): one being wound down, which
//! the regular settlement check asks nothing of, its one request being the
//! wind-down's; and one on an owner book, which would settle directly,
//! which no method may be, as a stored peering is made to hold here. A
//! method whose scheme can no longer receive, as one over a hub that
//! failed, is dropped as `winding.rs` tests.

use std::collections::BTreeMap;

use dassie_ledger::{LedgerId, Name};

use super::{STUB_TWO, scheme_id};
use crate::sim::{START, Settling, Sim, key};

/// The peer protocol's settings, as a node runs with them: a silent peer is
/// kept a day, so a method that cannot receive is dropped after a day.
const PEER: dassie_peer::Config = crate::node::Policies::DEFAULT.peer;

/// How often the node asks a peer short of room to settle.
const ASK_EVERY_MS: u64 = crate::node::Policies::DEFAULT.settlement_request_every_ms;

/// The schemes of node `index`'s methods with node `other`, in order.
fn methods(sim: &Sim, index: usize, other: usize) -> Vec<String> {
    sim.core(index)
        .peer
        .peering_of(&key(other).node_id())
        .map(|peering| peering.schemes().map(|scheme| scheme.to_string()).collect())
        .unwrap_or_default()
}

/// When node 0 last asked node 1 to settle on the second stub in its
/// regular check, if it did.
fn asked_on_two(sim: &Sim) -> Option<u64> {
    let other = key(1).node_id();
    sim.core(0)
        .settle
        .asked
        .get(&(other, scheme_id(STUB_TWO)))
        .copied()
}

// spec/decisions/settlement-methods.md: a method being wound down is asked
// to settle once, by the wind-down; the regular check, which asks a peer
// short of room every minute, asks nothing more on it, while the wind-down
// waits for an answer that does not come.
#[test]
fn a_method_wound_down_is_asked_only_by_its_wind_down() {
    let mut sim = Sim::with(61, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    for index in 0..2 {
        sim.step(index, |core| {
            core.add_scheme(STUB_TWO, r#"{"commitments":4}"#.into())
                .unwrap();
        });
    }
    // Node 1 never settles with node 0 on the second stub, so node 0's
    // room there stays short, and it asks again at every check.
    sim.step(1, |core| {
        core.settle
            .backoff
            .insert((zero, scheme_id(STUB_TWO)), (u64::MAX, 1));
    });
    let both = |sim: &Sim| methods(sim, 0, 1).len() == 2 && methods(sim, 1, 0).len() == 2;
    assert!(
        sim.run_until_or(START + 600_000, |sim| both(sim)
            && asked_on_two(sim).is_some()),
        "the pair did not peer on both stubs, or node 0 never asked"
    );
    let first = asked_on_two(&sim).unwrap();
    assert!(
        sim.run_until_or(sim.now + 2 * ASK_EVERY_MS, |sim| {
            asked_on_two(sim).is_some_and(|at| at > first)
        }),
        "node 0 did not ask again: the check below would show nothing"
    );
    // Node 0 winds the method down; its request to settle reaches node 1
    // no more, so the wind-down waits for the answer.
    sim.cut.insert((0, 1));
    sim.step(0, |core| {
        let now = core.now_ms();
        assert!(core.peer.wind_down(now, &one, scheme_id(STUB_TWO)));
    });
    let start = sim.now;
    // Within the wind-down's wait for the answer, the regular check comes
    // round at least once.
    const _: () = assert!(PEER.depeer_wait_ms > ASK_EVERY_MS + 2_000);
    sim.run_until(start + PEER.depeer_wait_ms - 1_000);
    assert!(
        sim.core(0).peer.method_winding(&one, scheme_id(STUB_TWO)),
        "no longer wound down: the check below would show nothing"
    );
    // Each request to settle that never reached node 1 is asked again at
    // the next check, so the regular check would ask every few seconds:
    // only the wind-down asks, once. The stub has room, and is not asked.
    let winding = sim
        .trace
        .iter()
        .position(|line| line.contains(" 0 PeerNode(MethodWinding {"))
        .expect("never wound down");
    let asks = sim.trace[winding..]
        .iter()
        .filter(|line| line.ends_with(" 0 asks 1 to settle"))
        .count();
    assert_eq!(
        asks, 1,
        "the regular check asked on a method being wound down"
    );
    // Once the pair hears each other again, the method is dropped within
    // its bound, and the peering goes on, on the stub.
    sim.cut.clear();
    assert!(
        sim.run_until_or(sim.now + 2 * PEER.depeer_wait_ms, |sim| {
            methods(sim, 0, 1) == ["stub"]
        }),
        "the method was not dropped within its bound"
    );
}

/// A scheme that would settle directly, an owner book's stand-in: it
/// peers with nobody, and has no ledger to follow, so it is always in step
/// (spec/areas/settlement.md#owner-books).
const OWNER_BOOK: &str = super::XRP_HUB;

/// Two nodes peered over the stub with commitments, each also running
/// [`OWNER_BOOK`].
fn pair_with_owner_books(seed: u64) -> Sim {
    let mut sim = Sim::with(seed, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    for index in 0..2 {
        sim.step(index, |core| {
            core.add_scheme(OWNER_BOOK, String::new()).unwrap();
        });
    }
    let peered = |sim: &Sim| (0..2).all(|index| methods(sim, index, 1 - index) == ["stub"]);
    assert!(
        sim.run_until_or(START + 600_000, peered),
        "the pair did not peer"
    );
    sim
}

/// Gives node `index`'s stored peering with the other node a method on the
/// owner book first, which no method may be, and the peer's accounts on
/// its ledger with nothing extended: a node that took it for a method
/// would find the peer short of room there, and ask it to settle. The node
/// is down after.
fn store_owner_book_method(sim: &mut Sim, index: usize) {
    let other = key(1 - index).node_id();
    let peer = Name::new(other.as_str()).unwrap();
    sim.step(index, |core| {
        let now = core.now_ms();
        core.ledger
            .open_peer(now, LedgerId::HubXrplTestnetXrp, peer, 0)
            .unwrap();
    });
    sim.crash(index);
    let store = &sim.nodes[index].store;
    let stored = store.load_peer().unwrap();
    let mut node = stored
        .nodes
        .into_iter()
        .find(|node| node.id == other)
        .unwrap();
    let peering = node.peering.as_mut().unwrap();
    let mut method = peering.first().clone();
    method.scheme = scheme_id(OWNER_BOOK);
    method.state = Vec::new();
    method.credit_out = 0;
    peering.methods.insert(0, method);
    let mut batch = crate::store::LedgerBatch::default();
    batch.peer.nodes.push(node);
    store.commit_ledger(&batch).unwrap();
}

/// Node `index`'s engine core, started from its database at the network's
/// time, as the simulation starts it, and not stepped: the core and its peer
/// node as they load, before any timer runs.
fn started(sim: &Sim, index: usize) -> crate::engine::Core {
    let parts = crate::engine::CoreParts {
        store: std::sync::Arc::clone(&sim.nodes[index].store),
        key: key(index),
        realm: dassie_node_core::config::Realm::Test,
        live: dassie_node_core::config::LiveRealm::Refused,
        policies: crate::node::Policies::DEFAULT,
        rng: Box::new(dassie_kit::sim::SeededRng::new(1)),
        peer_rng: Box::new(dassie_kit::sim::SeededRng::new(2)),
        own_info: dassie_peer::OwnInfo {
            url: None,
            alias: String::new(),
            feed_port: None,
        },
        bootstrap: Vec::new(),
        uplinks: Vec::new(),
        dev_core: Vec::new(),
    };
    crate::engine::Core::start(parts, sim.now).unwrap()
}

/// Node `index`, started from its database, before its first review: the
/// method on [`OWNER_BOOK`] is there, neither a packet's method nor one
/// asked on, at the regular check that asks a peer short of room to
/// settle.
fn takes_nothing_at_start(sim: &Sim, index: usize) {
    let (book, other) = (scheme_id(OWNER_BOOK), key(1 - index).node_id());
    let mut core = started(sim, index);
    let schemes: Vec<String> = core
        .peer
        .peering_of(&other)
        .unwrap()
        .schemes()
        .map(|scheme| scheme.to_string())
        .collect();
    assert_eq!(schemes, [OWNER_BOOK, "stub"], "node {index}");
    assert!(!core.method_available(&other, book), "node {index}");
    assert!(core.method_available(&other, scheme_id("stub")));
    core.settle.at_ms = 0;
    core.settlement_timers();
    assert!(
        !core.settle.asked.contains_key(&(other, book)),
        "node {index} asked on the owner book"
    );
}

// spec/areas/settlement.md#owner-books: a method is only over a hub or of
// unlimited trust. A stored peering is made to hold one on an owner book,
// its first, and one on the stub with commitments. The owner book would
// settle directly: its method carries no packet and its peer is never asked to
// settle on it, though the node extends the peer nothing there; each node
// drops it at its first review, at its start, without collecting, and the
// peering carries on over the stub, which a payment then crosses. What the
// method's accounts hold stays on them.
#[test]
fn a_method_that_would_settle_directly_takes_nothing_and_is_dropped() {
    let mut sim = pair_with_owner_books(63);
    for index in 0..2 {
        store_owner_book_method(&mut sim, index);
        takes_nothing_at_start(&sim, index);
    }
    let book = scheme_id(OWNER_BOOK);
    let mark = sim.trace.len();
    for index in 0..2 {
        sim.start(index);
    }
    let over_stub = |sim: &Sim| methods(sim, 0, 1) == ["stub"] && methods(sim, 1, 0) == ["stub"];
    let asked = |sim: &Sim| {
        (0..2).any(|index| {
            let other = key(1 - index).node_id();
            sim.core(index).settle.asked.contains_key(&(other, book))
        })
    };
    assert!(
        sim.run_until_or(sim.now + 120_000, |sim| asked(sim) || over_stub(sim)),
        "the method on the owner book was not dropped"
    );
    assert!(!asked(&sim), "asked on the owner book");
    let ours = format!("scheme: {book:?}, by_us: true");
    assert!(
        sim.trace[mark..]
            .iter()
            .any(|line| line.contains("PeerNode(MethodDropped {") && line.contains(&ours)),
        "not dropped by the node"
    );
    let winding = sim.trace[mark..]
        .iter()
        .any(|line| line.contains("PeerNode(MethodWinding {"));
    assert!(!winding, "collected on a method that settles nothing");
    // The peering carries on over the stub, and does not take the owner
    // book back.
    let amount = 3 * crate::node::Policies::DEFAULT.connector.max_packet_amount;
    assert!(sim.pay(0, 1, amount, None, Some(amount)));
    assert!(
        !sim.run_until_or(sim.now + 300_000, |sim| asked(sim) || !over_stub(sim)),
        "the owner book came back"
    );
    assert!(
        sim.payments[0].done,
        "the payment over the stub did not finish"
    );
    for index in 0..2 {
        let other = key(1 - index).node_id();
        let balances = sim.balances_on(index, LedgerId::HubXrplTestnetXrp);
        assert_eq!(
            balances.get(&format!("assets/interledger/{other}")),
            Some(&0),
            "node {index}: a packet moved on the owner book"
        );
    }
}

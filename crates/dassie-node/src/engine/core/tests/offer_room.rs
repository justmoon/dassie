//! The room an offer keeps for a core peer's accounts
//! (spec/decisions/method-offers.md, spec/decisions/account-budgets.md),
//! in the node's own books: a core peer's two accounts on a ledger stay for
//! good once open, so a method is offered only where the core peers' share
//! keeps a pair for it, and two nodes short of room offer the same one.
//! The cases come from Niq Dudfield's audit of open simulation failures.

use std::collections::BTreeMap;

use dassie_ledger::{LedgerId, Name};

use super::super::Core;
use super::super::peers::{has_accounts, peer_accounts, peer_budget, room_for_core_peer};
use super::{STUB_THREE, STUB_TWO, StubTwo};
use crate::sim::{Settling, Sim, key};

/// Whether the node's core share has room for two more pairs or more.
fn room_for_two(core: &Core) -> bool {
    let ledger = &core.ledger;
    let budget = peer_budget(ledger.config().max_accounts, false);
    let taken = peer_accounts(ledger).saturating_sub(2 * core.downlinks.opened.len());
    taken + 4 <= budget
}

/// Opens accounts on the stub's ledger for made-up core peers, a few
/// hundred a step, until the node's core share has room for exactly one
/// more pair.
fn leave_one_core_pair(sim: &mut Sim, index: usize, seed: u8) {
    let mut count = 0u32;
    while room_for_two(sim.core(index)) {
        sim.step(index, |core| {
            let now = core.now_ms();
            for _ in 0..200 {
                if !room_for_two(core) {
                    break;
                }
                let mut bytes = [0u8; 24];
                bytes[..4].copy_from_slice(&count.to_be_bytes());
                bytes[4] = 0xfe;
                bytes[5] = seed;
                let node = dassie_identity::NodeId::from_bytes(&bytes);
                let name = Name::new(node.as_str()).unwrap();
                core.ledger
                    .open_peer(now, LedgerId::StubUsd, name, 1)
                    .unwrap();
                count += 1;
            }
        });
    }
    let core = sim.core(index);
    assert!(room_for_core_peer(&core.ledger, &core.downlinks.opened));
}

/// The schemes of node `index`'s methods with the other node, sorted.
fn methods(sim: &Sim, index: usize) -> Vec<String> {
    let other = key(1 - index).node_id();
    let mut schemes: Vec<String> = sim
        .core(index)
        .peer
        .peering_of(&other)
        .map(|peering| peering.schemes().map(|scheme| scheme.to_string()).collect())
        .unwrap_or_default();
    schemes.sort_unstable();
    schemes
}

/// Whether node `index` holds accounts for the other on the ledger of
/// `scheme`, one of the two further stubs.
fn opened(sim: &Sim, index: usize, scheme: &str) -> bool {
    let (ledger, _) = StubTwo::of(scheme).unwrap();
    let name = Name::new(key(1 - index).node_id().as_str()).unwrap();
    has_accounts(&sim.core(index).ledger, ledger, name)
}

// Two nodes peered on the stub, each with room for one more core peer's
// pair, both take up two further hubs at once. Each offers only the one its
// room covers, the lowest scheme ID, so both offer the same one: they agree
// it, and neither opens accounts for the other on the hub left out. Each
// adopting in its own order would leave a pair each and no method.
#[test]
fn two_nodes_at_their_last_pair_agree_on_one_hub_and_leave_no_pair() {
    let mut sim = Sim::with(301, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(sim.now + 600_000, |sim| {
            (0..2).all(|index| methods(sim, index) == ["stub"])
        }),
        "the pair did not peer"
    );
    for index in 0..2 {
        leave_one_core_pair(&mut sim, index, u8::try_from(index).unwrap());
    }
    for index in 0..2 {
        sim.step(index, |core| {
            for scheme in [STUB_TWO, STUB_THREE] {
                core.add_scheme(scheme, r#"{"commitments":4}"#.into())
                    .unwrap();
            }
        });
    }
    let agreed = |sim: &Sim| (0..2).all(|index| methods(sim, index) == ["stub", STUB_THREE]);
    assert!(
        sim.run_until_or(sim.now + 600_000, agreed),
        "they never agreed on the lowest: {:?} and {:?}",
        methods(&sim, 0),
        methods(&sim, 1)
    );
    sim.run_until(sim.now + 60_000);
    for index in 0..2 {
        assert_eq!(methods(&sim, index), ["stub", STUB_THREE]);
        assert!(opened(&sim, index, STUB_THREE));
        assert!(
            !opened(&sim, index, STUB_TWO),
            "node {index} opened accounts on the hub left out"
        );
    }
}

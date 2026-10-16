#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use dassie_identity::NodeKey;
use dassie_ilp::{Timestamp, ildcp};
use dassie_node_core::config::Realm;
use dassie_node_core::testing::SeededRng;
use dassie_oer::Encode;

use super::{Core, CoreParts, Outbound, Output};
use crate::node::Policies;
use crate::store::Store;

const NOW: u64 = 1_800_000_000_000;

fn core(seed: u64) -> Core {
    core_on(Arc::new(Store::open_in_memory().unwrap()), seed)
}

fn core_on(store: Arc<Store>, seed: u64) -> Core {
    let parts = CoreParts {
        store,
        key: Arc::new(NodeKey::from_bytes(&[7; 32])),
        realm: Realm::Test,
        policies: Policies::DEFAULT,
        rng: Box::new(SeededRng::new(seed)),
        peer_rng: Box::new(SeededRng::new(seed + 1)),
        own_info: dassie_peer::OwnInfo {
            url: None,
            alias: String::new(),
        },
        bootstrap: Vec::new(),
    };
    Core::start(parts, NOW).unwrap()
}

/// A short session: a scheme, a token, a client that asks for its address.
fn session(seed: u64) -> Vec<Output> {
    let mut core = core(seed);
    let mut outputs = Vec::new();
    core.add_scheme("stub", String::new()).unwrap();
    let token = core
        .create_token("wallet".into(), "stub+usd", None)
        .unwrap();
    let link = core.btp_connect(&token.secret).unwrap().link;
    core.finish_step(&mut outputs);
    core.begin_step(NOW + 5);
    let request = ildcp::request(Timestamp::from_unix_ms(NOW)).to_vec();
    core.btp_packet(link, 1, &request, false);
    core.finish_step(&mut outputs);
    outputs
}

// The core reads no clock and no OS randomness: the same seed and inputs
// give the same outputs, which a simulation relies on (D16).
#[test]
fn the_same_seed_gives_the_same_steps() {
    let first = session(3);
    assert_eq!(first, session(3));
    let answered = first.iter().any(|output| {
        matches!(
            output,
            Output::Btp {
                message: Outbound::Response { request_id: 1, .. },
                ..
            }
        )
    });
    assert!(answered, "{first:?}");
    assert_ne!(first, session(4), "the client's address comes from the RNG");
}

// Time never goes back within a core, whatever its driver says.
#[test]
fn time_only_moves_forward() {
    let mut core = core(1);
    core.begin_step(NOW + 100);
    core.begin_step(NOW);
    assert_eq!(core.now_ms(), NOW + 100);
}

// ACC-16: a settlement whose pending transfer reached the disk, but which
// a crash stopped before it executed, is handed to its scheme at the next
// start. The stub never executed it, so its money never moved: it is
// voided, and what we owe the peer is as before.
#[test]
fn a_settlement_cut_short_by_a_crash_is_reconciled() {
    use dassie_ledger::{AccountName, AccountPath, LedgerId, Name, Reference, TransferSpec};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let peer = Name::new("dpeer").unwrap();
    let path = AccountPath::new(LedgerId::StubUsd, AccountName::AssetsInterledger(peer));
    {
        let mut core = core_on(Arc::clone(&store), 5);
        core.add_scheme("stub", String::new()).unwrap();
        let (interledger, _) = core
            .ledger
            .open_peer(NOW, LedgerId::StubUsd, peer, 1_000)
            .unwrap();
        let settlement = core
            .account(LedgerId::StubUsd, AccountName::AssetsSettlement)
            .unwrap();
        let id = Name::new("0123456789abcdef0123456789abcdef").unwrap();
        let spec = TransferSpec {
            debit: interledger,
            credit: settlement,
            amount: 400,
            pending: true,
            reference: Reference::Settlement {
                scheme: Name::new("stub").unwrap(),
                id,
            },
        };
        core.ledger.create(NOW, &spec).unwrap();
        // On disk, and then the crash: never executed.
        core.flush();
    }
    assert_eq!(store.load_ledger().unwrap().pending.len(), 1);
    let core = core_on(Arc::clone(&store), 6);
    assert_eq!(core.ledger.pending_transfers().count(), 0);
    assert_eq!(store.load_ledger().unwrap().pending.len(), 0);
    let account = core.ledger.find(&path).unwrap();
    assert_eq!(
        dassie_settlement::policy::position(core.ledger.account(account).unwrap().balance()),
        1_000
    );
    assert_eq!(core.ledger_views()[0].on_ledger_balance, 100_000_000_000);
}

// What keeps a ledger from being removed is what the wallet shows, and
// what removing it refuses with.
#[test]
fn removal_blockers_match_the_refusal() {
    use super::RemovalBlocker;

    let mut core = core(2);
    let view = core.add_scheme("stub", String::new()).unwrap();
    assert!(view.removal_blockers.is_empty());
    core.create_token("a".into(), "stub+usd", None).unwrap();
    core.create_token("b".into(), "stub+usd", None).unwrap();
    let blockers = core.ledger_views()[0].removal_blockers.clone();
    assert_eq!(blockers, [RemovalBlocker::BtpTokens(2)]);
    let refusal = core.remove_scheme("stub").unwrap_err();
    assert_eq!(refusal.to_string(), blockers[0].message());
}

// Spec 7 §4.5: a token's cap is kept across a restart, and changing it
// is persisted with the step.
#[test]
fn token_caps_survive_a_restart() {
    use dassie_connector::SpendingCap;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let cap = SpendingCap {
        amount: 5_000,
        window_ms: 60_000,
    };
    let id = {
        let mut core = core_on(Arc::clone(&store), 3);
        core.add_scheme("stub", String::new()).unwrap();
        let token = core
            .create_token("capped".into(), "stub+usd", Some(cap))
            .unwrap();
        assert_eq!(
            (token.view.cap, token.view.cap_available),
            (Some(cap), 5_000)
        );
        let other = core.create_token("free".into(), "stub+usd", None).unwrap();
        let half = SpendingCap {
            amount: 2_500,
            ..cap
        };
        core.set_token_cap(other.view.id, Some(half)).unwrap();
        core.set_token_cap(other.view.id, None).unwrap();
        let invalid = SpendingCap { amount: 0, ..cap };
        assert!(core.set_token_cap(token.view.id, Some(invalid)).is_err());
        core.flush();
        token.view.id
    };
    let core = core_on(store, 4);
    let views = core.token_views();
    assert_eq!(views[0].id, id);
    assert_eq!((views[0].cap, views[0].cap_available), (Some(cap), 5_000));
    assert_eq!(views[1].cap, None);
}

// Live views fetch again when the state they show changed, once it is on
// disk.
#[test]
fn steps_report_what_they_changed() {
    let mut core = core(5);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    core.take_changes();
    core.finish_step(&mut outputs);
    assert_eq!(core.take_changes(), super::Changes::default());
    core.add_scheme("stub", String::new()).unwrap();
    core.finish_step(&mut outputs);
    assert!(core.take_changes().ledger);
}

/// A peer whose peering ended, owing us `owed` of the 1 000 we extend,
/// and with `lost` booked as its refusals' losses and `reversed` as ones
/// our receiver took back, all on disk.
fn costly_peer(core: &mut Core, owed: u64, lost: u64, reversed: u64) -> dassie_identity::NodeId {
    use dassie_ledger::{AccountName, LedgerId, Name, Reference, TraceId, TransferSpec};

    let node = NodeKey::from_bytes(&[9; 32]).node_id();
    let name = Name::new(node.as_str()).unwrap();
    core.add_scheme("stub", String::new()).unwrap();
    let (interledger, _) = core
        .ledger
        .open_peer(NOW, LedgerId::StubUsd, name, 1_000)
        .unwrap();
    let owner = core
        .account(LedgerId::StubUsd, AccountName::EquityOwner)
        .unwrap();
    let losses = core
        .account(LedgerId::StubUsd, AccountName::ExpensesLosses)
        .unwrap();
    let trace = TraceId {
        epoch: 1,
        sequence: 1,
    };
    for (debit, credit, amount, reference) in [
        (interledger, owner, owed, Reference::Packet(trace)),
        (losses, interledger, lost, Reference::Refusal(trace)),
        (owner, interledger, reversed, Reference::Refusal(trace)),
    ] {
        if amount > 0 {
            let spec = TransferSpec {
                debit,
                credit,
                amount,
                pending: false,
                reference,
            };
            core.ledger.create(NOW, &spec).unwrap();
        }
    }
    core.flush();
    node
}

// D18: a peer's refusals count as their losses, never what our receiver
// took back. A peering that ended without the peer settling stays owed on
// the books, but is not overdue: the peer may never have heard it ended.
#[test]
fn a_peer_costs_its_losses_and_not_an_ended_peerings_debt() {
    let mut core = core(8);
    let node = costly_peer(&mut core, 800, 30, 40);
    core.update_synced();
    let none = std::collections::BTreeSet::new();
    let cost = core.peer_cost(node, &none).unwrap();
    assert_eq!((cost.refused, cost.overdue), (30, 0));
    // The floor: a hundredth of a unit.
    assert_eq!(cost.grace, 10_000_000);
    core.begin_step(NOW + 10 * Policies::DEFAULT.grace.settle_grace_ms);
    let cost = core.peer_cost(node, &none).unwrap();
    assert_eq!((cost.refused, cost.overdue), (30, 0));
}

// D18: a peer past its limit is banned, with what it cost, and the owner
// lifting the ban starts its window anew.
#[test]
fn a_ban_keeps_what_the_peer_cost_and_an_unban_forgets_it() {
    use crate::store::{BanReason, PeerSettings};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 10);
    // A tiny share, so the floor decides: a hundredth of a unit.
    core.set_peer_settings(PeerSettings {
        grace_share_ppm: 1,
        ..PeerSettings::DEFAULT
    })
    .unwrap();
    let node = costly_peer(&mut core, 0, 20_000_000, 0);
    core.update_synced();
    let none = std::collections::BTreeSet::new();
    let cost = core.peer_cost(node, &none).unwrap();
    assert!(cost.over());
    assert!(core.ban_for(node, cost));
    core.flush();
    assert!(core.peer.is_banned(&node));
    let record = core.ban_record(&node).unwrap();
    assert_eq!(record.reason, BanReason::GraceLimit);
    assert_eq!(record.cost, Some(cost));
    assert_eq!(store.ban_records().unwrap(), [record]);

    core.begin_step(NOW + 1_000);
    core.unban_node(&node).unwrap();
    core.flush();
    assert_eq!(core.peer_cost(node, &none).unwrap().refused, 0);
    assert_eq!(
        store.ban_records().unwrap()[0].unbanned_at_ms,
        Some(NOW + 1_000)
    );
    // A restart keeps that.
    drop(core);
    let core = core_on(store, 11);
    assert_eq!(
        core.ban_record(&node).unwrap().unbanned_at_ms,
        Some(NOW + 1_000)
    );
}

// Found in review: lifting a ban the node has no record of, from before
// it kept them, still starts the peer's window anew.
#[test]
fn an_unban_without_a_record_makes_one() {
    use crate::store::BanReason;

    let mut core = core(14);
    let node = NodeKey::from_bytes(&[9; 32]).node_id();
    core.peer.ban(NOW, node).unwrap();
    core.unban_node(&node).unwrap();
    let record = core.ban_record(&node).unwrap();
    assert_eq!(record.reason, BanReason::Owner);
    assert_eq!(record.unbanned_at_ms, Some(NOW));
}

// Found in review: the owner banning a node the node banned already keeps
// the record of why.
#[test]
fn an_owner_ban_keeps_an_automatic_bans_record() {
    use crate::store::{BanReason, PeerSettings};

    let mut core = core(15);
    core.set_peer_settings(PeerSettings {
        grace_share_ppm: 1,
        ..PeerSettings::DEFAULT
    })
    .unwrap();
    let node = costly_peer(&mut core, 0, 20_000_000, 0);
    core.update_synced();
    let cost = core
        .peer_cost(node, &std::collections::BTreeSet::new())
        .unwrap();
    core.ban_for(node, cost);
    core.ban_node(node).unwrap();
    assert_eq!(
        core.ban_record(&node).unwrap().reason,
        BanReason::GraceLimit
    );
}

// The owner's share is checked and persisted.
#[test]
fn peer_settings_are_checked_and_kept() {
    use crate::store::PeerSettings;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 12);
    assert_eq!(core.peer_settings(), PeerSettings::DEFAULT);
    assert!(
        core.set_peer_settings(PeerSettings {
            grace_share_ppm: 0,
            ..PeerSettings::DEFAULT
        })
        .is_err()
    );
    for refused in [
        PeerSettings {
            grace_share_ppm: 10_000_001,
            ..PeerSettings::DEFAULT
        },
        PeerSettings {
            connector_fee_ppm: 100_001,
            ..PeerSettings::DEFAULT
        },
        PeerSettings {
            fx_spread_ppm: 100_001,
            ..PeerSettings::DEFAULT
        },
    ] {
        assert!(core.set_peer_settings(refused).is_err(), "{refused:?}");
    }
    let settings = PeerSettings {
        grace_share_ppm: 750_000,
        connector_fee_ppm: 1_000,
        fx_spread_ppm: 2_000,
    };
    core.set_peer_settings(settings).unwrap();
    assert_eq!(core.connector.charges(), settings.charges());
    core.flush();
    drop(core);
    // The connector charges what was kept.
    let core = core_on(store, 13);
    assert_eq!(core.peer_settings(), settings);
    assert_eq!(core.connector.charges(), settings.charges());
}

// D16: a broken invariant stops the core before its batch is written, so
// its successor starts from the last state that held.
#[test]
fn a_broken_invariant_never_reaches_the_disk() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 6);
    core.add_scheme("stub", String::new()).unwrap();
    core.flush();
    let payment = core
        .create_incoming_payment(super::NewIncoming {
            ledger: dassie_ledger::LedgerId::StubUsd,
            wallet_address: "https://node.example/.well-known/pay".into(),
            client: None,
            incoming_amount: Some(10),
            expires_at_ms: None,
            metadata: None,
        })
        .unwrap();
    // A bug: more payments open than the bound allows.
    core.incoming_policy.max_open_owner = 0;
    let flushed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| core.flush()));
    assert!(flushed.is_err());
    assert_eq!(store.incoming_payment(payment.id).unwrap(), None);
}

// In release, the sweep runs at most once a second of engine time, and
// always at the commit that ends a restore.
#[test]
fn sweeps_follow_the_engine_clock() {
    let mut core = core(7);
    assert_eq!(core.sweeps.count, 1);
    core.sweeps.every_ms = 1_000;
    core.sweeps.at_ms = NOW;
    let mut outputs = Vec::new();
    for (at, sweeps) in [(NOW, 2), (NOW + 10, 2), (NOW + 999, 2), (NOW + 1_000, 3)] {
        core.begin_step(at);
        core.add_scheme("stub", String::new()).ok();
        core.finish_step(&mut outputs);
        assert_eq!(core.sweeps.count, sweeps, "at {at}");
    }
}

// D16: the core reports events and logs nothing itself, so a simulation
// sees all it says. Clippy does not see `tracing`'s macros under the
// core's `deny`, so its sources are read for them.
#[test]
fn the_core_does_not_log() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/engine/core");
    let mut read = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name == "tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("tracing"), "{} logs", path.display());
        read += 1;
    }
    assert!(read > 10, "the core's sources were not found");
}

// A probe the connector answers at once, to an address nobody routes, ends
// in the same step without a rate: its packets' answers are taken in as
// they come, without the sending loop calling itself again (D16).
#[test]
fn a_probe_to_nowhere_ends_at_once() {
    let mut core = core(8);
    core.add_scheme("stub", String::new()).unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let ledger = dassie_ledger::LedgerId::StubUsd;
    core.start_probe(1, ledger, "test.nowhere.receiver", &[3; 32])
        .unwrap();
    core.finish_step(&mut outputs);
    assert!(
        outputs
            .iter()
            .any(|output| matches!(output, Output::ProbeDone { id: 1, rate: None })),
        "{outputs:?}"
    );
}

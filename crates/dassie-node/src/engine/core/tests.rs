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

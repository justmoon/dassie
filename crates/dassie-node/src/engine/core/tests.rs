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

/// The node's first XRPL testnet account, which the XRP Ledger scheme
/// settles through: its connection is the account's.
const XRPL_ACCOUNT: &str = "xrpl-testnet-account/0";

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
            quic: false,
            feed_port: None,
            hub: None,
        },
        bootstrap: Vec::new(),
        uplinks: Vec::new(),
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
// give the same outputs, which a simulation relies on
// (spec/decisions/tigerstyle.md).
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

// A scheme's effects are booked all or none
// (spec/areas/settlement.md#the-scheme-interface): when the books refuse
// one, the ones before it are not booked either, and the scheme stops, so
// its checkpoint stays with what the books hold. Stopped, it is fed
// nothing more.
#[test]
fn a_refused_effect_books_none_of_its_batch() {
    use dassie_ledger::{AccountName, LedgerId, MAX_TOTAL, Name};
    use dassie_settlement::{Effect, LedgerStatus};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 9);
    core.add_scheme("stub", String::new()).unwrap();
    core.flush();
    let stub = Name::new("stub").unwrap();
    let settlement = core
        .account(LedgerId::StubUsd, AccountName::AssetsSettlement)
        .unwrap();
    let posted =
        |store: &Store| store.load_ledger().unwrap().accounts[settlement.0 as usize].debits_posted;
    let before = posted(&store);
    let checkpoints = store.checkpoints().unwrap();
    let effects = [
        Effect::Deposit { amount: 5 },
        Effect::Deposit { amount: MAX_TOTAL },
    ];
    let result = core.apply_effects(stub, LedgerId::StubUsd, &effects);
    assert!(result.is_err(), "the second deposit overflows the account");
    core.flush();
    assert_eq!(posted(&store), before, "the first deposit is not booked");
    assert_eq!(store.checkpoints().unwrap(), checkpoints);
    assert!(matches!(
        core.ledger_views()[0].status,
        LedgerStatus::Failed(_)
    ));
    assert!(core.stub_deposit(1).is_err());
    assert_eq!(posted(&store), before);
}

// A stopped scheme is not peered on, as before its first sync
// (spec/decisions/synced-before-peering.md): a peer's settlement on it would
// be refused until a restart. It leaves what the node offers, asks for and
// accepts; a restart brings it back
// (spec/areas/network.md#asking-and-answering).
#[test]
fn a_stopped_scheme_is_not_peered_on() {
    use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
    use dassie_peer::wire::SchemeId;
    use dassie_settlement::Effect;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 9);
    core.add_scheme("stub", String::new()).unwrap();
    let stub = SchemeId::parse("stub").unwrap();
    assert_eq!(core.peer.schemes(), [stub]);
    let effects = [Effect::Deposit { amount: MAX_TOTAL }];
    let stopped = core.apply_effects(Name::new("stub").unwrap(), LedgerId::StubUsd, &effects);
    assert!(stopped.is_err(), "the deposit overflows the account");
    assert!(core.peer.schemes().is_empty());
    core.flush();
    drop(core);
    let core = core_on(Arc::clone(&store), 10);
    assert_eq!(core.peer.schemes(), [stub]);
}

/// Carries the XRP Ledger scheme's requests to `ledger` and the answers
/// back, until it has nothing more to say.
fn follow_xrpl(core: &mut Core, ledger: &mut dassie_xrpl::fake::FakeLedger) {
    follow_xrpl_outputs(core, ledger);
}

/// [`follow_xrpl`], returning every output of the steps.
fn follow_xrpl_outputs(core: &mut Core, ledger: &mut dassie_xrpl::fake::FakeLedger) -> Vec<Output> {
    use super::LedgerEvent;

    let mut all = Vec::new();
    for _ in 0..50 {
        let mut outputs = Vec::new();
        core.finish_step(&mut outputs);
        let events: Vec<(u64, LedgerEvent)> = outputs
            .iter()
            .filter_map(|output| match output {
                Output::LedgerConnect { epoch, .. } => Some((*epoch, LedgerEvent::Connected)),
                Output::LedgerSend { epoch, message, .. } => {
                    Some((*epoch, LedgerEvent::Message(ledger.request(1, message))))
                }
                _ => None,
            })
            .collect();
        all.extend(outputs);
        if events.is_empty() {
            return all;
        }
        for (epoch, event) in &events {
            core.ledger_event(XRPL_ACCOUNT, *epoch, event);
        }
    }
    panic!("the scheme kept talking");
}

// What the books refused, a scheme that follows a ledger reads again after
// a restart, from its checkpoint: none of it was booked, so none of it is
// booked twice. Here the XRP Ledger scheme catches up on two deposits made
// while the node was down, in one batch, and the settlement account has
// room left for the first only.
#[test]
fn a_refused_batch_read_again_is_never_booked_twice() {
    use dassie_ledger::{AccountName, LedgerId, MAX_TOTAL, Reference, TransferSpec};
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::fake::FakeLedger;
    use dassie_xrpl::{UNITS_PER_DROP, Wallet};

    const SCHEME: &str = "xrpl-testnet";
    const XRP: u64 = 1_000_000;
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32])
        .settlement_entropy(SCHEME, "seed-testnet")
        .unwrap();
    let account = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = FakeLedger::new();
    ledger.fund(account);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let settlement = {
        let mut core = core_on(Arc::clone(&store), 9);
        core.add_scheme(SCHEME, config.into()).unwrap();
        follow_xrpl(&mut core, &mut ledger);
        assert_eq!(core.ledger_views()[0].status, LedgerStatus::Synced);
        let ledger_id = LedgerId::XrplTestnetXrp;
        let settlement = core
            .account(ledger_id, AccountName::AssetsSettlement)
            .unwrap();
        let suspense = core
            .account(ledger_id, AccountName::EquitySuspense)
            .unwrap();
        // Through the settlement account and back, leaving it room for
        // 10 XRP more of deposits over its life.
        let debits = core
            .ledger
            .account(settlement)
            .unwrap()
            .balance()
            .debits_posted;
        let filler = MAX_TOTAL - u64::try_from(debits).unwrap() - 10 * XRP * UNITS_PER_DROP;
        for (debit, credit) in [(settlement, suspense), (suspense, settlement)] {
            let spec = TransferSpec {
                debit,
                credit,
                amount: filler,
                pending: false,
                reference: Reference::Reconciliation,
            };
            core.ledger.create(NOW, &spec).unwrap();
        }
        core.flush();
        settlement
    };
    ledger.disconnect(1);
    ledger.deposit(account, 5 * XRP);
    ledger.close();
    ledger.deposit(account, 20 * XRP);
    ledger.close();
    let posted =
        |store: &Store| store.load_ledger().unwrap().accounts[settlement.0 as usize].debits_posted;
    let balance = |store: &Store| store.checkpoints().unwrap()[0].1.balance;
    let before = (posted(&store), balance(&store));
    for seed in [10, 11] {
        let mut core = core_on(Arc::clone(&store), seed);
        follow_xrpl(&mut core, &mut ledger);
        ledger.disconnect(1);
        assert!(matches!(
            core.ledger_views()[0].status,
            LedgerStatus::Failed(_)
        ));
        assert_eq!((posted(&store), balance(&store)), before);
    }
}

// A change of the XRP Ledger's reserve is reconciled as the reserve's, and
// reported as information: the books follow what is above the reserve. Any
// other reconciliation stays a warning.
#[test]
fn a_reserve_change_is_reconciled_as_information() {
    use dassie_ledger::{AccountName, LedgerId, Name};
    use dassie_settlement::ReconcileCause;
    use dassie_xrpl::UNITS_PER_DROP;
    use dassie_xrpl::fake::{FakeLedger, RESERVE_BASE, RESERVE_INC};

    use super::{Event, LedgerEvent, SettlementEvent, Severity};

    const SCHEME: &str = "xrpl-testnet";
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32])
        .settlement_entropy(SCHEME, "seed-testnet")
        .unwrap();
    let account = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = FakeLedger::new();
    ledger.fund(account);
    ledger.close();
    let mut core = core(9);
    core.add_scheme(SCHEME, config.into()).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let suspense = core
        .account(LedgerId::XrplTestnetXrp, AccountName::EquitySuspense)
        .unwrap();
    let held = |core: &Core| {
        core.ledger
            .account(suspense)
            .unwrap()
            .balance()
            .debits_posted
    };
    let before = held(&core);
    ledger.set_reserve(2 * RESERVE_BASE, RESERVE_INC);
    let epoch = core.assets[&dassie_xrpl::account::FIRST].epoch;
    for (_, message) in ledger.close() {
        core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Message(message));
    }
    let reconciled: Vec<(Event, Severity)> = follow_xrpl_outputs(&mut core, &mut ledger)
        .into_iter()
        .filter_map(|output| match output {
            Output::Event(event @ Event::Settlement(SettlementEvent::Reconciled { .. })) => {
                let severity = event.severity();
                Some((event, severity))
            }
            _ => None,
        })
        .collect();
    let amount = RESERVE_BASE * UNITS_PER_DROP;
    assert_eq!(
        reconciled,
        [(
            Event::Settlement(SettlementEvent::Reconciled {
                scheme: Name::new(SCHEME).unwrap(),
                amount,
                gain: false,
                cause: ReconcileCause::Reserve,
            }),
            Severity::Info
        )]
    );
    // Booked as any reconciliation: through suspense.
    assert_eq!(held(&core), before + u128::from(amount));
    let unexplained = Event::Settlement(SettlementEvent::Reconciled {
        scheme: Name::new(SCHEME).unwrap(),
        amount,
        gain: true,
        cause: ReconcileCause::Unexplained,
    });
    assert_eq!(unexplained.severity(), Severity::Warn);
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

// A token's cap is kept across a restart, and changing it is persisted with
// the step (spec/areas/connector.md#spending-caps).
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

/// The messages a step's outputs carry to a BTP link, in order.
fn messages_to(outputs: &[Output], link: u64) -> Vec<&Outbound> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::Btp { link: to, message } if *to == link => Some(message),
            _ => None,
        })
        .collect()
}

// Revoking a token lifts its cap at once, while its links close only at the
// end of the step: a Prepare on one in between is refused, not booked
// without the cap (spec/areas/connector.md#spending-caps). The link still
// closes cleanly.
#[test]
fn a_revoked_tokens_link_takes_no_prepare_before_it_closes() {
    use dassie_connector::{EndpointId, SpendingCap};
    use dassie_ilp::{Address, ErrorCode, Prepare, Response};
    use dassie_ledger::{LedgerId, Name};
    use dassie_oer::Decode;

    let mut core = core(81);
    core.add_scheme("stub", String::new()).unwrap();
    let cap = SpendingCap {
        amount: 5_000,
        window_ms: 60_000,
    };
    let token = core
        .create_token("capped".into(), "stub+usd", Some(cap))
        .unwrap();
    let link = core.btp_connect(&token.secret).unwrap().link;
    // A peer to pay, reached directly: this checks booking, not transport.
    let (account, _) = core
        .ledger
        .open_peer(NOW, LedgerId::StubUsd, Name::new("other").unwrap(), 0)
        .unwrap();
    let peer = EndpointId::Peer(999);
    core.connector
        .add_peer(peer, "test.das.other", account, LedgerId::StubUsd, 0)
        .unwrap();
    core.connector
        .set_routes([("test.das.other", &[peer][..])])
        .unwrap();
    let packet = Prepare {
        amount: 6_000,
        expires_at: Timestamp::from_unix_ms(NOW + 20_000),
        execution_condition: [7; 32],
        destination: Address::parse("test.das.other").unwrap(),
        data: &[],
    }
    .to_vec();
    core.btp_packet(link, 1, &packet, false);
    assert_eq!(core.connector.pending_count(), 0, "the cap refuses it");

    core.revoke_token(token.view.id).unwrap();
    core.btp_packet(link, 2, &packet, false);
    assert_eq!(core.connector.pending_count(), 0, "nothing is booked");
    core.check_invariants();

    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let to_link = messages_to(&outputs, link);
    let Some(Outbound::Response {
        request_id: 2,
        packet: answer,
    }) = to_link.iter().rev().nth(1)
    else {
        panic!("no answer to the second Prepare: {to_link:?}");
    };
    let Response::Reject(reject) = Response::decode_exact(answer).unwrap() else {
        panic!("not a Reject");
    };
    assert_eq!(reject.code, ErrorCode::F00_BAD_REQUEST);
    assert_eq!(to_link.last(), Some(&&Outbound::Close));

    // The link is gone: what still arrives on it is dropped, and the next
    // sweep holds.
    core.begin_step(NOW + 1);
    core.btp_packet(link, 3, &packet, false);
    let mut later = Vec::new();
    core.finish_step(&mut later);
    assert!(messages_to(&later, link).is_empty(), "{later:?}");
    assert_eq!(core.connector.pending_count(), 0);
    core.check_invariants();
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
    // The rate feed follows the prices: new charges move them, and a table
    // built again that says the same moves nothing.
    let settings = crate::store::PeerSettings {
        connector_fee_ppm: 1_000,
        ..crate::store::PeerSettings::DEFAULT
    };
    core.set_peer_settings(settings).unwrap();
    core.finish_step(&mut outputs);
    assert!(core.take_changes().prices);
    core.set_peer_settings(settings).unwrap();
    core.finish_step(&mut outputs);
    assert!(!core.take_changes().prices);
}

// The owner's fee, spread and rebalancing are checked and persisted.
#[test]
fn peer_settings_are_checked_and_kept() {
    use crate::store::PeerSettings;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 12);
    assert_eq!(core.peer_settings(), PeerSettings::DEFAULT);
    for refused in [
        PeerSettings {
            connector_fee_ppm: 100_001,
            ..PeerSettings::DEFAULT
        },
        PeerSettings {
            fx_spread_ppm: 100_001,
            ..PeerSettings::DEFAULT
        },
        PeerSettings {
            rebalance_ppm: 100_001,
            ..PeerSettings::DEFAULT
        },
    ] {
        assert!(core.set_peer_settings(refused).is_err(), "{refused:?}");
    }
    let settings = PeerSettings {
        connector_fee_ppm: 1_000,
        fx_spread_ppm: 2_000,
        rebalance_ppm: 30_000,
    };
    core.set_peer_settings(settings).unwrap();
    core.flush();
    assert_eq!(core.price_table().pricing(), settings.pricing());
    drop(core);
    // The connector charges what was kept.
    let core = core_on(store, 13);
    assert_eq!(core.peer_settings(), settings);
    assert_eq!(core.price_table().pricing(), settings.pricing());
}

// spec/decisions/random-seed-simulations.md: a commit that fails stops the
// core before it hands over anything of the step, here the answer to a
// client, and its successor starts from the commit before.
#[cfg(feature = "testing")]
#[test]
fn a_failed_commit_hands_nothing_over() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 8);
    let mut outputs = Vec::new();
    core.add_scheme("stub", String::new()).unwrap();
    let token = core
        .create_token("wallet".into(), "stub+usd", None)
        .unwrap();
    let link = core.btp_connect(&token.secret).unwrap().link;
    core.finish_step(&mut outputs);
    outputs.clear();
    let failed = store.fail_commit(crate::store::Aim::Any, 0);
    core.begin_step(NOW + 5);
    let payment = core
        .create_incoming_payment(super::NewIncoming {
            ledger: dassie_ledger::LedgerId::StubUsd,
            wallet_address: "https://node.example/.well-known/pay".into(),
            client: None,
            incoming_amount: Some(10),
            expires_at_ms: None,
            metadata: None,
            uplink: None,
        })
        .unwrap();
    let request = ildcp::request(Timestamp::from_unix_ms(NOW)).to_vec();
    core.btp_packet(link, 1, &request, false);
    let stepped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        core.finish_step(&mut outputs);
    }));
    assert!(stepped.is_err() && failed.held().is_some());
    assert!(outputs.is_empty(), "handed over {outputs:?}");
    drop(core);
    assert_eq!(store.incoming_payment(payment.id).unwrap(), None);
    let core = core_on(store, 9);
    assert_eq!(core.active_schemes(), ["stub"]);
}

// spec/decisions/tigerstyle.md: a broken invariant stops the core before its
// batch is written, so its successor starts from the last state that held.
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
            uplink: None,
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

// spec/decisions/tigerstyle.md: the core reports events and logs nothing
// itself, so a simulation sees all it says. Clippy does not see `tracing`'s
// macros under the core's `deny`, so its sources are read for them.
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
// they come, without the sending loop calling itself again
// (spec/decisions/tigerstyle.md).
#[test]
fn a_probe_to_nowhere_ends_at_once() {
    let mut core = core(8);
    core.add_scheme("stub", String::new()).unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let ledger = dassie_ledger::LedgerId::StubUsd;
    core.start_probe(
        1,
        ledger,
        "test.nowhere.receiver",
        &[3; 32],
        super::Prober::Owner,
    )
    .unwrap();
    core.finish_step(&mut outputs);
    assert!(
        outputs.iter().any(|output| matches!(
            output,
            Output::ProbeDone {
                id: 1,
                measured: None
            }
        )),
        "{outputs:?}"
    );
}

/// A core whose probes stay in progress until its step ends, as they go
/// to its own receiver, whose answers come after the step's sending; and
/// that receiver's address.
fn probing_core(seed: u64) -> (Core, String) {
    let mut core = core(seed);
    core.add_scheme("stub", String::new()).unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    core.begin_step(NOW + 1);
    let receiver = core
        .incoming_credentials(
            crate::store::PaymentId([1; 16]),
            dassie_ledger::LedgerId::StubUsd,
        )
        .unwrap()
        .destination;
    (core, receiver)
}

/// A stranger, the `n`th, from the `on`th IPv4 /24.
fn stranger(n: u8, on: u8) -> super::Prober {
    use std::net::{IpAddr, Ipv4Addr};
    let network = IpAddr::V4(Ipv4Addr::new(192, 0, on, 1));
    super::Prober::Stranger {
        client: format!("jwk:stranger-{n}"),
        network: Some(dassie_node_core::rate_limit::ClientKey::network_of(network)),
    }
}

fn exhausted(result: Result<(), super::EngineError>) {
    assert!(
        matches!(result, Err(super::EngineError::Exhausted(_))),
        "{result:?}"
    );
}

// Clients' quotes need no owner, so strangers can start probes: they never
// take the owner's slots, and no one client or network holds more than its
// share (spec/decisions/owners-share.md).
#[test]
fn strangers_probes_leave_the_owner_a_slot() {
    use super::Prober;
    use super::probes::{MAX_PROBES, MAX_PROBES_PER_CLIENT, OWNER_PROBES};

    let (mut core, receiver) = probing_core(9);
    let mut id = 0;
    let mut start = |core: &mut Core, by: Prober| {
        id += 1;
        core.start_probe(
            id,
            dassie_ledger::LedgerId::StubUsd,
            &receiver,
            &[3; 32],
            by,
        )
    };
    // One client, or one network, takes only its share.
    for _ in 0..MAX_PROBES_PER_CLIENT {
        start(&mut core, stranger(0, 0)).unwrap();
    }
    exhausted(start(&mut core, stranger(0, 1)));
    exhausted(start(&mut core, stranger(1, 0)));
    // Strangers fill all but the owner's slots.
    let mut n = 1;
    while core.spending.probes.len() < MAX_PROBES - OWNER_PROBES {
        start(&mut core, stranger(n, n)).unwrap();
        n += 1;
    }
    exhausted(start(&mut core, stranger(n, n)));
    // The owner's Send still measures, until every slot is taken.
    for _ in 0..OWNER_PROBES {
        start(&mut core, Prober::Owner).unwrap();
    }
    exhausted(start(&mut core, Prober::Owner));
    core.check_invariants();
}

// An app the owner approved shares a cloud's network with strangers: their
// probes there never hold it to the network's share, nor to a share of
// its own; only the clients' slots bound it, as before
// (spec/decisions/owners-share.md).
#[test]
fn strangers_on_its_network_never_stop_an_approved_apps_probes() {
    use super::Prober;
    use super::probes::{MAX_PROBES, MAX_PROBES_PER_CLIENT, OWNER_PROBES};

    let (mut core, receiver) = probing_core(10);
    let mut id = 0;
    let mut start = |core: &mut Core, by: Prober| {
        id += 1;
        core.start_probe(
            id,
            dassie_ledger::LedgerId::StubUsd,
            &receiver,
            &[3; 32],
            by,
        )
    };
    for n in 0..MAX_PROBES_PER_CLIENT {
        start(&mut core, stranger(u8::try_from(n).unwrap(), 0)).unwrap();
    }
    exhausted(start(&mut core, stranger(9, 0)));
    while core.spending.probes.len() < MAX_PROBES - OWNER_PROBES {
        start(&mut core, Prober::Approved).unwrap();
    }
    exhausted(start(&mut core, Prober::Approved));
    start(&mut core, Prober::Owner).unwrap();
    core.check_invariants();
}

// A scheme that stops by itself, here on a server off the XRP Ledger testnet,
// has its connection closed, so the driver does not make it again for ever
// (spec/areas/settlement.md#the-scheme-interface). Removed and added again,
// it connects anew.
#[test]
fn a_stopped_scheme_closes_its_connection() {
    use super::LedgerEvent;
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::fake::FakeLedger;

    const SCHEME: &str = "xrpl-testnet";
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let connected = |outputs: &[Output]| {
        outputs.iter().find_map(|output| match output {
            Output::LedgerConnect { epoch, .. } => Some(*epoch),
            _ => None,
        })
    };
    let mut core = core(9);
    core.add_scheme(SCHEME, config.into()).unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let epoch = connected(&outputs).unwrap();
    let mut ledger = FakeLedger::new();
    ledger.set_network_id(Some(0));
    core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Connected);
    outputs.clear();
    core.finish_step(&mut outputs);
    let [Output::LedgerSend { message, .. }] = outputs.as_slice() else {
        panic!("{outputs:?}");
    };
    let reply = ledger.request(1, message);
    core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Message(reply));
    outputs.clear();
    core.finish_step(&mut outputs);
    assert!(
        outputs
            .iter()
            .any(|output| matches!(output, Output::LedgerClose { epoch: e, .. } if *e == epoch)),
        "{outputs:?}"
    );
    assert!(matches!(
        core.ledger_views()[0].status,
        LedgerStatus::Failed(_)
    ));
    // What the closed connection still says is dropped.
    core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Disconnected);
    core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Connected);
    outputs.clear();
    core.finish_step(&mut outputs);
    assert!(
        !outputs.iter().any(|output| matches!(
            output,
            Output::LedgerSend { .. } | Output::LedgerConnect { .. }
        )),
        "{outputs:?}"
    );
    core.remove_scheme(SCHEME).unwrap();
    core.add_scheme(SCHEME, config.into()).unwrap();
    outputs.clear();
    core.finish_step(&mut outputs);
    assert!(connected(&outputs).is_some_and(|again| again != epoch));
    assert_eq!(core.ledger_views()[0].status, LedgerStatus::Connecting);
}

// spec/decisions/earned-trust.md: a node that once accepted a peer's request
// extended it nothing, on a trust account that stays when the peering ends.
// Peering again on its own request, it is the junior, and extends at least
// the default credit: it posts the difference, rather than keep the senior's
// nothing.
#[test]
fn a_junior_extends_at_least_the_default_credit() {
    use dassie_ledger::{AccountName, AccountPath, LedgerId, Name};
    use dassie_peer::PeeringHooks;

    let mut core = core(7);
    core.add_scheme("stub", String::new()).unwrap();
    let peer = NodeKey::from_bytes(&[9; 32]).node_id();
    let name = Name::new(peer.as_str()).unwrap();
    core.ledger
        .open_peer(NOW, LedgerId::StubUsd, name, core.senior_credit)
        .unwrap();
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    let mut hooks = super::peers::Hooks {
        schemes: &mut core.schemes,
        ledger: &mut core.ledger,
        credit: core.peer_credit,
        senior_credit: core.senior_credit,
        connector: &core.connector,
        endpoints: &core.peer_endpoints,
        assets: &core.assets,
        store: &core.store,
    };
    let agreed = hooks
        .finalize(NOW, peer, scheme, 1, &[], &[], false)
        .unwrap();
    assert_eq!(agreed.credit_out, Policies::DEFAULT.peer_credit);
    let contra = core
        .ledger
        .find(&AccountPath::new(
            LedgerId::StubUsd,
            AccountName::ContraTrust(name),
        ))
        .unwrap();
    let trust = dassie_settlement::policy::trust(core.ledger.account(contra).unwrap().balance());
    assert_eq!(trust, Policies::DEFAULT.peer_credit);
}

// spec/decisions/ended-for-room.md: a peering we ended because the peer's
// room stayed short is formed again later on our own request. We are the
// junior now, but the peer gets no more credit than it had as our junior: the
// default would be a new line for a peer that never settled.
#[test]
fn a_junior_keeps_the_lower_credit_of_a_peer_it_ended_for_room() {
    use dassie_ledger::{AccountName, AccountPath, LedgerId, Name};
    use dassie_peer::PeeringHooks;

    let mut core = core(7);
    core.add_scheme("stub", String::new()).unwrap();
    let peer = NodeKey::from_bytes(&[9; 32]).node_id();
    let name = Name::new(peer.as_str()).unwrap();
    core.ledger
        .open_peer(NOW, LedgerId::StubUsd, name, core.senior_credit)
        .unwrap();
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    let mut hooks = super::peers::Hooks {
        schemes: &mut core.schemes,
        ledger: &mut core.ledger,
        credit: core.peer_credit,
        senior_credit: core.senior_credit,
        connector: &core.connector,
        endpoints: &core.peer_endpoints,
        assets: &core.assets,
        store: &core.store,
    };
    let agreed = hooks
        .finalize(NOW, peer, scheme, 1, &[], &[], true)
        .unwrap();
    assert_eq!(agreed.credit_out, core.senior_credit);
    assert!(core.senior_credit < Policies::DEFAULT.peer_credit);
    let contra = core
        .ledger
        .find(&AccountPath::new(
            LedgerId::StubUsd,
            AccountName::ContraTrust(name),
        ))
        .unwrap();
    let trust = dassie_settlement::policy::trust(core.ledger.account(contra).unwrap().balance());
    assert_eq!(trust, core.senior_credit);
}

// spec/decisions/closing-channels.md: the books let a former peer's channel
// close only with nothing of it pending, and every scheme it has accounts
// with running and seeing its ledger, so a settlement it made before the
// close is booked as its own.
#[test]
fn the_books_hold_a_channel_open_while_anything_of_it_is_pending() {
    use dassie_ledger::{AccountName, AccountPath, LedgerId, Name, Reference, TransferSpec};
    use dassie_peer::PeeringHooks;

    let mut core = core(7);
    core.add_scheme("stub", String::new()).unwrap();
    let peer = NodeKey::from_bytes(&[9; 32]).node_id();
    let name = Name::new(peer.as_str()).unwrap();
    core.ledger
        .open_peer(NOW, LedgerId::StubUsd, name, core.peer_credit)
        .unwrap();
    assert!(core.with_peer(|_, hooks, _| hooks.closable(peer)));
    let find = |core: &Core, name| {
        core.ledger
            .find(&AccountPath::new(LedgerId::StubUsd, name))
            .unwrap()
    };
    let spec = TransferSpec {
        debit: find(&core, AccountName::AssetsInterledger(name)),
        credit: find(&core, AccountName::AssetsSettlement),
        amount: 1,
        pending: true,
        reference: Reference::Settlement {
            scheme: Name::new("stub").unwrap(),
            id: Name::new("s1").unwrap(),
        },
    };
    let pending = core.ledger.create(NOW, &spec).unwrap();
    assert!(
        !core.with_peer(|_, hooks, _| hooks.closable(peer)),
        "pending"
    );
    core.ledger.void(NOW, pending).unwrap();
    assert!(core.with_peer(|_, hooks, _| hooks.closable(peer)));
    // Accounts on a ledger no scheme follows: a late settlement there
    // could not be told from a deposit.
    core.ledger
        .open_peer(NOW, LedgerId::XrplTestnetXrp, name, core.peer_credit)
        .unwrap();
    assert!(
        !core.with_peer(|_, hooks, _| hooks.closable(peer)),
        "unseen"
    );
}

// Found by the simulation (spec/decisions/random-seed-simulations.md): a
// sender crashed before the last Fulfill of a payment reached it, and its
// receiver restarted after the payment completed. Restarted, the receiver
// forgot the payment and refused the sender's new connections, so the sender
// never learned that the money had arrived, and failed the payment. A
// restarted receiver still tells a sender that reconnects what its connection
// delivered, and that no more fits.
#[test]
fn a_restarted_receiver_tells_a_reconnecting_sender_what_got_there() {
    use dassie_ledger::LedgerId;
    use dassie_stream::Admission;

    use super::incoming::{NewIncoming, admission, connection_tag};
    use crate::store::{ConnectionKey, IncomingPaid};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let key = ConnectionKey([8; 16]);
    let id = {
        let mut core = core_on(Arc::clone(&store), 5);
        core.add_scheme("stub", String::new()).unwrap();
        let payment = core
            .create_incoming_payment(NewIncoming {
                ledger: LedgerId::StubUsd,
                wallet_address: "https://a.example/pay".into(),
                client: None,
                incoming_amount: Some(100),
                expires_at_ms: None,
                metadata: None,
                uplink: None,
            })
            .unwrap();
        core.incoming_received(IncomingPaid {
            payment: payment.id,
            connection: key,
            amount: 100,
        });
        core.flush();
        payment.id
    };
    let core = core_on(store, 6);
    let tag = connection_tag(id, key);
    let (admitted, _) = admission(
        &core.incoming,
        &core.finished_incoming,
        LedgerId::StubUsd,
        NOW,
        &tag,
    );
    assert_eq!(
        admitted,
        Admission::Accept {
            money_limit: 0,
            received: 100,
            tolerance: 0,
            exhausted: dassie_stream::ErrorCode::NO_ERROR,
        }
    );
}

// An incoming payment its creator completes before it received all it
// asked for, or that asked for no amount, tells its senders so with
// another close code than a full one, before a restart and after: a
// sender then says the receiver stopped the payment, rather than that it
// takes no more.
#[test]
fn a_payment_completed_short_tells_its_senders_so() {
    use dassie_ledger::LedgerId;
    use dassie_stream::{Admission, ErrorCode};

    use super::incoming::{NewIncoming, admission, connection_tag};
    use crate::store::{ConnectionKey, IncomingPaid};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let key = ConnectionKey([8; 16]);
    let short = |core: &Core, id| {
        admission(
            &core.incoming,
            &core.finished_incoming,
            LedgerId::StubUsd,
            NOW,
            &connection_tag(id, key),
        )
        .0
    };
    let ids = {
        let mut core = core_on(Arc::clone(&store), 5);
        core.add_scheme("stub", String::new()).unwrap();
        let ids = [Some(100), None].map(|incoming_amount| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: LedgerId::StubUsd,
                    wallet_address: "https://a.example/pay".into(),
                    client: None,
                    incoming_amount,
                    expires_at_ms: None,
                    metadata: None,
                    uplink: None,
                })
                .unwrap();
            core.incoming_received(IncomingPaid {
                payment: payment.id,
                connection: key,
                amount: 30,
            });
            core.complete_incoming_payment(payment.id).unwrap();
            payment.id
        });
        core.flush();
        for id in ids {
            assert_eq!(
                short(&core, id),
                Admission::Accept {
                    money_limit: 0,
                    received: 30,
                    tolerance: 0,
                    exhausted: ErrorCode::APPLICATION_ERROR,
                }
            );
        }
        ids
    };
    let core = core_on(store, 6);
    for id in ids {
        assert_eq!(
            short(&core, id),
            Admission::Accept {
                money_limit: 0,
                received: 30,
                tolerance: 0,
                exhausted: ErrorCode::APPLICATION_ERROR,
            }
        );
    }
}

/// What the receiver on `stub+usd` admits for the connection `key` of the
/// incoming payment `id`.
fn admitted(
    core: &Core,
    id: crate::store::PaymentId,
    key: crate::store::ConnectionKey,
) -> dassie_stream::Admission {
    let tag = super::incoming::connection_tag(id, key);
    let ledger = dassie_ledger::LedgerId::StubUsd;
    super::incoming::admission(&core.incoming, &core.finished_incoming, ledger, NOW, &tag).0
}

// An incoming payment counts as paid in full within its tolerance, under
// its amount or over it (spec/decisions/paid-within-a-tolerance.md): it
// completes, tells its senders it is full, says so again after a restart,
// and shows what it really received.
#[test]
fn a_payment_within_its_tolerance_is_paid_in_full() {
    use dassie_ledger::LedgerId;
    use dassie_stream::{Admission, ErrorCode};

    use super::incoming::NewIncoming;
    use crate::store::{ConnectionKey, IncomingPaid};

    // A dollar, in billionths: a ten-thousandth of a dollar either way.
    const DOLLAR: u64 = 1_000_000_000;
    let store = Arc::new(Store::open_in_memory().unwrap());
    let key = ConnectionKey([8; 16]);
    let full = |received| Admission::Accept {
        money_limit: 0,
        received,
        tolerance: 0,
        exhausted: ErrorCode::NO_ERROR,
    };
    let paid = [DOLLAR - 100_000, DOLLAR + 100_000];
    let ids = {
        let mut core = core_on(Arc::clone(&store), 5);
        core.add_scheme("stub", String::new()).unwrap();
        let ids = paid.map(|amount| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: LedgerId::StubUsd,
                    wallet_address: "https://a.example/pay".into(),
                    client: None,
                    incoming_amount: Some(DOLLAR),
                    expires_at_ms: None,
                    metadata: None,
                    uplink: None,
                })
                .unwrap();
            let open = Admission::Accept {
                money_limit: DOLLAR,
                received: 0,
                tolerance: 100_000,
                exhausted: ErrorCode::NO_ERROR,
            };
            assert_eq!(admitted(&core, payment.id, key), open);
            core.incoming_received(IncomingPaid {
                payment: payment.id,
                connection: key,
                amount,
            });
            core.check_invariants();
            payment.id
        });
        core.flush();
        for (id, amount) in ids.into_iter().zip(paid) {
            assert_eq!(admitted(&core, id, key), full(amount));
        }
        ids
    };
    let core = core_on(Arc::clone(&store), 6);
    for (id, amount) in ids.into_iter().zip(paid) {
        assert_eq!(admitted(&core, id, key), full(amount));
        let row = store.incoming_payment(id).unwrap().unwrap();
        assert_eq!(row.received_amount, amount);
        assert!(row.completed_at_ms.is_some());
    }
}

/// The `n`th connection key.
fn connection_key(n: usize) -> crate::store::ConnectionKey {
    crate::store::ConnectionKey([u8::try_from(n).unwrap(); 16])
}

/// An incoming payment of 1 000 000 whose every place is taken by a
/// connection that paid one unit, the first a moment before the others.
fn payment_paid_by_every_connection(core: &mut Core) -> crate::store::PaymentId {
    use super::incoming::{MAX_CONNECTIONS_PER_PAYMENT, NewIncoming};
    use crate::store::IncomingPaid;

    core.add_scheme("stub", String::new()).unwrap();
    let payment = core
        .create_incoming_payment(NewIncoming {
            ledger: dassie_ledger::LedgerId::StubUsd,
            wallet_address: "https://a.example/pay".into(),
            client: None,
            incoming_amount: Some(1_000_000),
            expires_at_ms: None,
            metadata: None,
            uplink: None,
        })
        .unwrap();
    for n in 1..=MAX_CONNECTIONS_PER_PAYMENT {
        core.begin_step(NOW + u64::from(n > 1));
        core.incoming_received(IncomingPaid {
            payment: payment.id,
            connection: connection_key(n),
            amount: 1,
        });
    }
    core.flush();
    payment.id
}

// A new payer that takes the place of an idle connection that paid only
// dust is counted, and the connection it replaced is forgotten on disk
// too, so a restart reads back the payment's places as they were
// (spec/decisions/owners-share.md).
#[test]
fn a_replaced_connection_is_forgotten_across_a_restart() {
    use dassie_ilp::ErrorCode;
    use dassie_ledger::LedgerId;
    use dassie_stream::Admission;

    use super::incoming::{IDLE_CONNECTION_MS, admission, connection_tag};
    use crate::store::{ConnectionKey, IncomingPaid};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let newcomer = ConnectionKey([0xee; 16]);
    let id = {
        let mut core = core_on(Arc::clone(&store), 5);
        let id = payment_paid_by_every_connection(&mut core);
        core.begin_step(NOW + 1 + IDLE_CONNECTION_MS);
        let tag = connection_tag(id, newcomer);
        let (admitted, paid) = admission(
            &core.incoming,
            &core.finished_incoming,
            LedgerId::StubUsd,
            core.now_ms(),
            &tag,
        );
        assert!(matches!(admitted, Admission::Accept { .. }), "{admitted:?}");
        assert_eq!(paid, Some((id, newcomer)));
        core.incoming_received(IncomingPaid {
            payment: id,
            connection: newcomer,
            amount: 5,
        });
        core.check_invariants();
        core.flush();
        id
    };
    let core = core_on(store, 6);
    let at = |connection| {
        admission(
            &core.incoming,
            &core.finished_incoming,
            LedgerId::StubUsd,
            NOW,
            &connection_tag(id, connection),
        )
        .0
    };
    assert_eq!(
        at(newcomer),
        Admission::Accept {
            money_limit: 1_000_000 - 37,
            received: 5,
            tolerance: 1_000,
            exhausted: dassie_stream::ErrorCode::NO_ERROR,
        }
    );
    assert_eq!(
        at(connection_key(2)),
        Admission::Accept {
            money_limit: 1_000_000 - 37,
            received: 1,
            tolerance: 1_000,
            exhausted: dassie_stream::ErrorCode::NO_ERROR,
        }
    );
    // The one replaced is a stranger now, and every place is taken again.
    assert_eq!(
        at(connection_key(1)),
        Admission::Refuse(ErrorCode::F99_APPLICATION_ERROR)
    );
}

/// A periphery node's peering request to a core node at `https://core.test`
/// that takes QUIC, from `downlink`, at `now`.
fn downlink_request(core: &Core, downlink: &Arc<NodeKey>, now: u64) -> Vec<u8> {
    downlink_request_on(core, downlink, now, "stub", &[])
}

/// A peering request from `downlink` on `scheme`, with the scheme's `data`.
fn downlink_request_on(
    core: &Core,
    downlink: &Arc<NodeKey>,
    now: u64,
    scheme: &str,
    data: &[u8],
) -> Vec<u8> {
    use dassie_oer::Decode;
    use dassie_peer::auth::{AuthConfig, Authenticator};
    use dassie_peer::wire::{
        Authentication, Content, EXTENSION_PERIPHERY, Entry, NodeInfo, PeeringRequest, SchemeId,
        SignedNodeInfo, encode_envelope, sign_node_info,
    };
    let stub = SchemeId::parse(scheme).unwrap();
    let info = NodeInfo {
        node_id: downlink.node_id(),
        sequence: now,
        url: "",
        alias: "",
        public_key: *downlink.public_key().as_bytes(),
        entries: vec![
            Entry::Scheme(stub),
            Entry::Extension {
                kind: EXTENSION_PERIPHERY,
                value: &[],
            },
        ],
    };
    let state = sign_node_info(downlink, &info);
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: stub,
        node_info: SignedNodeInfo::decode_exact(&state).unwrap(),
        data,
        versions: vec![1],
        credit: 0,
    });
    let bytes = content.to_vec();
    let mut auth = Authenticator::new(Arc::clone(downlink), AuthConfig::DEFAULT, now - 1);
    let own = NodeKey::from_bytes(&[7; 32]);
    let (hmac, _) = auth
        .authenticate(
            now,
            &mut SeededRng::new(3),
            core.peer.own(),
            &own.public_key(),
            &bytes,
        )
        .unwrap();
    encode_envelope(&downlink.node_id(), &Authentication::Hmac(hmac), &bytes)
}

// spec/areas/network.md#advertised-peers: a downlink that owes nothing is
// never short of room in our books, though we extend it nothing as its
// senior: withdrawing it would only stop the payments to it that raise its
// room.
#[test]
fn a_downlink_that_owes_nothing_is_never_short_of_room() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = Core::start(
        CoreParts {
            store,
            key: Arc::new(NodeKey::from_bytes(&[7; 32])),
            realm: Realm::Test,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(5)),
            peer_rng: Box::new(SeededRng::new(6)),
            own_info: dassie_peer::OwnInfo {
                url: Some("https://core.test".into()),
                alias: String::new(),
                quic: true,
                feed_port: None,
                hub: None,
            },
            bootstrap: Vec::new(),
            uplinks: Vec::new(),
        },
        NOW,
    )
    .unwrap();
    core.add_scheme("stub", String::new()).unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let downlink = Arc::new(NodeKey::from_bytes(&[11; 32]));
    let at = NOW + 1_000;
    core.begin_step(at);
    let body = downlink_request(&core, &downlink, at);
    core.peer_inbound(dassie_peer::InboundId(1), &body, None);
    core.finish_step(&mut outputs);
    let id = downlink.node_id();
    assert!(core.peer.is_downlink(&id) && core.peer.peering_of(&id).is_some());
    assert_eq!(core.peer.peering_of(&id).unwrap().credit_out, 0);
    let grace = Policies::DEFAULT.peer.advertise_short_grace_ms;
    for second in 2..=grace / 1_000 * 2 {
        core.begin_step(NOW + second * 1_000);
        core.finish_step(&mut outputs);
    }
    assert_eq!(core.peer.peering_of(&id).unwrap().short_for_ms, 0);
}

/// Closes a ledger of `ledger`, and hands the XRP Ledger account what its
/// streams say.
fn close_xrpl(core: &mut Core, ledger: &mut dassie_xrpl::fake::FakeLedger) {
    use super::LedgerEvent;

    let epoch = core.assets[&dassie_xrpl::account::FIRST].epoch;
    for (_, message) in ledger.close() {
        core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Message(message));
    }
    follow_xrpl(core, ledger);
}

/// A store on the database the node wrote before external assets, migrated:
/// the XRP Ledger scheme's state split between the account and the scheme.
fn store_from_before_external_assets(dir: &std::path::Path) -> Arc<Store> {
    let database = dir.join(crate::store::DATABASE_FILE);
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute_batch(include_str!("../../store/testdata/xrpl-scheme-v25.sql"))
        .unwrap();
    let store = Arc::new(Store::open(dir).unwrap());
    let checkpoint = store.checkpoints().unwrap().remove(0).1;
    assert_eq!(checkpoint.balance, 999_000_000_000);
    let scheme: serde_json::Value = serde_json::from_slice(&checkpoint.state).unwrap();
    let keys: Vec<&String> = scheme.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["cursor", "peers", "recovering"]);
    let (id, state) = store.asset_checkpoints().unwrap().remove(0);
    assert_eq!(id, XRPL_ACCOUNT);
    let account: serde_json::Value = serde_json::from_slice(&state).unwrap();
    assert_eq!(account["sequence"], 5_000_003);
    assert_eq!(
        account["newest"],
        serde_json::json!([5_000_001, 800_000_004])
    );
    assert_eq!(account["never_sent"], serde_json::json!([]));
    let in_flight = account["in_flight"].as_array().unwrap();
    assert_eq!(in_flight.len(), 2);
    assert!(in_flight.iter().all(|t| t["scheme"] == "xrpl-testnet"));
    assert_eq!(in_flight[1]["executed"], false);
    store
}

/// What the core shows `address`, its only external asset, to hold: what
/// `ledger` says.
fn assert_holds_what_the_ledger_says(
    core: &Core,
    ledger: &dassie_xrpl::fake::FakeLedger,
    address: dassie_xrpl::AccountId,
) {
    use dassie_settlement::LedgerStatus;

    let views = core.asset_views();
    let [view] = views.as_slice() else {
        panic!("{views:?}");
    };
    assert_eq!(
        (view.id.as_str(), view.status, view.schemes.as_slice()),
        (
            XRPL_ACCOUNT,
            LedgerStatus::Synced,
            &["xrpl-testnet".to_string()][..]
        )
    );
    assert_eq!(view.address, Some(address.to_address()));
    let [holding] = view.holdings.as_slice() else {
        panic!("{views:?}");
    };
    assert_eq!(
        (holding.code.as_str(), holding.issuer.as_deref()),
        ("XRP", None)
    );
    assert_eq!(holding.scale, 6);
    assert_eq!(holding.amount, u128::from(ledger.balance(address).unwrap()));
    assert_eq!(holding.reserve, u128::from(dassie_xrpl::fake::RESERVE_BASE));
}

// spec/areas/xrpl.md#state-and-restarts: a database the node wrote before
// external assets, with the XRP Ledger scheme's state in one blob, is split
// between the account and the scheme when it migrates, and the node restarts
// on it booking nothing anew. What was in flight is submitted again and
// posted once, with its fee; what was looked up by its hash is still looked
// up; the balance, the owner's funds and suspense stay as they were. What
// the account holds, the node shows as the ledger says.
#[test]
fn a_database_from_before_external_assets_restarts_and_books_nothing_anew() {
    use dassie_ledger::{Reference, TransferState};
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::Wallet;
    use dassie_xrpl::fake::FakeLedger;

    const XRP: u64 = 1_000_000_000;
    let temp = tempfile::tempdir().unwrap();
    let store = store_from_before_external_assets(temp.path());
    let rows = |store: &Store| store.transfers_after(None, 100, None).unwrap();
    assert_eq!(rows(&store).len(), 5);

    // The ledger as the node left it: the account funded, nothing sent.
    let entropy = NodeKey::from_bytes(&[7; 32])
        .settlement_entropy("xrpl-testnet", "seed-testnet")
        .unwrap();
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = FakeLedger::new();
    ledger.fund(address);
    ledger.close();
    let mut core = core_on(Arc::clone(&store), 10);
    follow_xrpl(&mut core, &mut ledger);
    assert_eq!(core.ledger_views()[0].status, LedgerStatus::Synced);
    close_xrpl(&mut core, &mut ledger);
    core.flush();
    assert_holds_what_the_ledger_says(&core, &ledger, address);

    let after = rows(&store);
    let state = |id: u64| {
        after
            .iter()
            .rev()
            .find(|row| row.transfer.id.0 == id)
            .map(|row| row.state)
    };
    assert_eq!(state(2), Some(TransferState::Posted), "in flight");
    assert_eq!(state(3), Some(TransferState::Posted), "not yet executed");
    assert_eq!(state(4), Some(TransferState::Pending), "looked up");
    let new: Vec<&Reference> = after
        .iter()
        .filter(|row| row.transfer.id.0 >= 5)
        .map(|row| &row.transfer.reference)
        .collect();
    assert_eq!(new.len(), 2, "{new:?}");
    assert!(
        new.iter()
            .all(|reference| matches!(reference, Reference::Fee { .. })),
        "{new:?}"
    );
    assert_eq!(
        store.checkpoints().unwrap()[0].1.balance,
        999 * XRP - 50 * XRP - 20_000
    );
    let account = store.asset_checkpoints().unwrap().remove(0).1;
    let account: serde_json::Value = serde_json::from_slice(&account).unwrap();
    assert_eq!(account["in_flight"], serde_json::json!([]));
    assert_eq!(account["sequence"], 5_000_003);
}

/// A second scheme on the node's first XRPL testnet account, for the tests,
/// as RLUSD is one: it serves the stub's ledger, follows the account from
/// nowhere, and books and signs nothing.
pub(super) const WITNESS: &str = "witness";

/// The scheme [`WITNESS`] names.
pub(super) struct Witness;

impl dassie_settlement::SettlementScheme for Witness {
    fn id(&self) -> dassie_ledger::Name {
        dassie_ledger::name!(WITNESS)
    }

    fn ledger(&self) -> dassie_ledger::LedgerId {
        dassie_ledger::LedgerId::StubUsd
    }

    fn asset(&self) -> Option<dassie_settlement::AssetId> {
        Some(dassie_xrpl::account::FIRST)
    }

    fn supported_versions(&self) -> &'static [u16] {
        &[1]
    }

    fn balance(&self) -> u64 {
        0
    }

    fn on_asset(
        &mut self,
        _now_ms: u64,
        asset: &mut dyn dassie_settlement::ExternalAsset,
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) {
        let any: &mut dyn std::any::Any = asset;
        let account = any.downcast_mut::<dassie_xrpl::XrplAccount>().unwrap();
        account.follow(self.id(), None, false);
    }

    fn peering_info(&self, out: &mut Vec<u8>) {
        out.clear();
    }

    fn create_peering_request(
        &mut self,
        _peer: dassie_ledger::Name,
        _info: &[u8],
        _out: &mut Vec<u8>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn accept_peering_request(
        &mut self,
        _peer: dassie_ledger::Name,
        _data: &[u8],
        _response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn finalize_peering_request(
        &mut self,
        _peer: dassie_ledger::Name,
        _info: &[u8],
        _data: &[u8],
    ) -> Result<Vec<u8>, dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn prepare_settlement(
        &mut self,
        _rng: &mut dyn rand_core::CryptoRng,
        _asset: Option<&mut dyn dassie_settlement::ExternalAsset>,
        _peer: dassie_ledger::Name,
        _amount: u64,
        _state: &[u8],
    ) -> Result<dassie_settlement::PreparedSettlement, dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unavailable)
    }

    fn execute_settlement(
        &mut self,
        _asset: Option<&mut dyn dassie_settlement::ExternalAsset>,
        _settlement: dassie_ledger::Name,
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) {
    }

    fn reconcile(
        &mut self,
        _asset: Option<&mut dyn dassie_settlement::ExternalAsset>,
        _pending: &[(dassie_ledger::Name, u64)],
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) {
    }

    fn handle_settlement(
        &mut self,
        _peer: dassie_ledger::Name,
        _amount: u64,
        _data: &[u8],
        _state: &[u8],
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::InvalidData)
    }

    fn handle_message(
        &mut self,
        _peer: dassie_ledger::Name,
        _message: &[u8],
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) {
    }

    fn deposit(
        &mut self,
        _amount: u64,
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn check_invariants(&self) {}
}

/// The config both schemes on the XRPL account name: its server and its
/// faucet.
const SHARED_CONFIG: &str = r#"{"server":"ws://ledger.test","faucet":null}"#;

/// The ledger with the node's first XRPL testnet account funded, and the
/// account's address.
fn funded_ledger() -> (dassie_xrpl::fake::FakeLedger, dassie_xrpl::AccountId) {
    let entropy = NodeKey::from_bytes(&[7; 32])
        .settlement_entropy("xrpl-testnet", "seed-testnet")
        .unwrap();
    let account = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = dassie_xrpl::fake::FakeLedger::new();
    ledger.fund(account);
    ledger.close();
    (ledger, account)
}

/// The connection numbers of the connections `outputs` open.
fn connects(outputs: &[Output]) -> Vec<u64> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::LedgerConnect { epoch, .. } => Some(*epoch),
            _ => None,
        })
        .collect()
}

/// Whether `outputs` close connection `epoch`.
fn closes(outputs: &[Output], epoch: u64) -> bool {
    outputs
        .iter()
        .any(|output| matches!(output, Output::LedgerClose { epoch: e, .. } if *e == epoch))
}

// spec/areas/settlement.md#the-scheme-interface: what an input asks of the
// ledger goes out only if the books took what it booked. Here the testnet
// account is absent, as of a ledger it has seen nothing past: its scheme
// writes off what it held, which the books refuse, so the scheme stops, and
// the faucet the account would ask to make it again is not asked, nor is
// its connection made anew.
#[test]
fn a_refused_write_off_asks_the_ledger_nothing() {
    use super::LedgerEvent;
    use dassie_ledger::{AccountName, LedgerId, MAX_TOTAL, Reference, TransferSpec};
    use dassie_settlement::LedgerStatus;

    let config = r#"{"server":"ws://ledger.test","faucet":"https://faucet.test/accounts"}"#;
    let (mut ledger, account) = funded_ledger();
    let mut core = core(9);
    core.add_scheme("xrpl-testnet", config.into()).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    assert_eq!(core.ledger_views()[0].status, LedgerStatus::Synced);
    // Through suspense and back, leaving it room for less to be lost
    // than the scheme holds.
    let ledger_id = LedgerId::XrplTestnetXrp;
    let suspense = core
        .account(ledger_id, AccountName::EquitySuspense)
        .unwrap();
    let losses = core
        .account(ledger_id, AccountName::ExpensesLosses)
        .unwrap();
    let filler = MAX_TOTAL - 1;
    for (debit, credit) in [(suspense, losses), (losses, suspense)] {
        let spec = TransferSpec {
            debit,
            credit,
            amount: filler,
            pending: false,
            reference: Reference::Reconciliation,
        };
        core.ledger.create(NOW, &spec).unwrap();
    }
    ledger.remove_account(account);
    ledger.close();
    let epoch = core.assets[&dassie_xrpl::account::FIRST].epoch;
    core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Disconnected);
    core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Connected);
    let outputs = follow_xrpl_outputs(&mut core, &mut ledger);
    assert!(matches!(
        core.ledger_views()[0].status,
        LedgerStatus::Failed(_)
    ));
    assert!(closes(&outputs, epoch), "{outputs:?}");
    assert!(
        !outputs
            .iter()
            .any(|output| matches!(output, Output::Fund { .. } | Output::LedgerConnect { .. })),
        "{outputs:?}"
    );
}

// spec/areas/settlement.md#external-assets: a scheme activated on an account
// that has read its ledger already is told it has synced, and is peered on
// from then: the account connects anew, and reads again from the earliest
// place of its schemes.
#[test]
fn a_scheme_activated_on_a_synced_account_is_told_it_has_synced() {
    use dassie_ledger::Name;
    use dassie_peer::wire::SchemeId;
    use dassie_settlement::LedgerStatus;

    let (mut ledger, _) = funded_ledger();
    let mut core = core(9);
    core.add_scheme(WITNESS, SHARED_CONFIG.into()).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    assert_eq!(core.asset_views()[0].status, LedgerStatus::Synced);
    let before = core.assets[&dassie_xrpl::account::FIRST].epoch;
    core.add_scheme("xrpl-testnet", SHARED_CONFIG.into())
        .unwrap();
    let outputs = follow_xrpl_outputs(&mut core, &mut ledger);
    assert!(closes(&outputs, before), "{outputs:?}");
    assert!(!connects(&outputs).is_empty(), "{outputs:?}");
    let xrpl = Name::new("xrpl-testnet").unwrap();
    assert!(core.schemes[&xrpl].scheme.has_synced());
    let view = core
        .ledger_views()
        .into_iter()
        .find(|view| view.scheme == "xrpl-testnet")
        .unwrap();
    assert_eq!(view.status, LedgerStatus::Synced);
    let id = SchemeId::parse("xrpl-testnet").unwrap();
    assert!(core.peer.schemes().contains(&id));
}

// spec/areas/settlement.md#external-assets: an account whose connection was
// closed, every scheme on it stopped, is connected again when a scheme
// activates on it.
#[test]
fn a_scheme_activated_on_a_closed_account_connects_it_again() {
    use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
    use dassie_settlement::{Effect, LedgerStatus};

    let (mut ledger, _) = funded_ledger();
    let mut core = core(9);
    core.add_scheme(WITNESS, SHARED_CONFIG.into()).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let epoch = core.assets[&dassie_xrpl::account::FIRST].epoch;
    let deposit = Effect::Deposit { amount: MAX_TOTAL };
    let effects = [deposit.clone(), deposit];
    let witness = Name::new(WITNESS).unwrap();
    let stopped = core.apply_effects(witness, LedgerId::StubUsd, &effects);
    assert!(stopped.is_err(), "the deposits overflow the account");
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(closes(&outputs, epoch), "{outputs:?}");
    core.add_scheme("xrpl-testnet", SHARED_CONFIG.into())
        .unwrap();
    let outputs = follow_xrpl_outputs(&mut core, &mut ledger);
    assert!(!connects(&outputs).is_empty(), "{outputs:?}");
    let xrpl = Name::new("xrpl-testnet").unwrap();
    assert!(core.schemes[&xrpl].scheme.has_synced());
    assert_eq!(core.asset_views()[0].status, LedgerStatus::Synced);
}

// spec/decisions/external-assets.md: the RLUSD scheme added
// beside the XRP scheme settles through the same account. The account is
// connected anew and reads the line, which is set with a TrustSet whose fee
// is a ledger fee on `xrpl-testnet+xrp` and whose reserve is reconciled
// there, and the wallet sees both holdings as the ledger says, a freeze
// included.
// spec/areas/settlement.md#when-peers-cannot-pay-us: while the issuer froze
// the line deep, or froze its token for every holder (read at the
// settlement check), the link state leaves the scheme out; it stays synced.
#[test]
#[allow(clippy::too_many_lines)]
fn rlusd_beside_xrp_on_one_account() {
    use dassie_ledger::{LedgerId, Reference};
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::Wallet;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::{FAUCET_DROPS, FEE_BASE, FakeLedger, RESERVE_BASE, RESERVE_INC, Token};

    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32])
        .settlement_entropy("xrpl-testnet", "seed-testnet")
        .unwrap();
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = FakeLedger::new();
    ledger.fund(address);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 12);
    core.add_scheme("xrpl-testnet", config.into()).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    core.add_scheme("xrpl-testnet-rlusd", config.into())
        .unwrap();
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    let view = |core: &Core, ledger: LedgerId| {
        core.ledger_views()
            .into_iter()
            .find(|view| view.id == ledger)
            .unwrap()
    };
    assert_eq!(
        view(&core, LedgerId::XrplTestnetRlusdUsd).status,
        LedgerStatus::Synced
    );
    let xrp = view(&core, LedgerId::XrplTestnetXrp);
    assert_eq!(xrp.status, LedgerStatus::Synced);
    let units = |drops: u64| drops * 1_000;
    assert_eq!(
        xrp.on_ledger_balance,
        units(FAUCET_DROPS - FEE_BASE - RESERVE_BASE - RESERVE_INC)
    );
    core.flush();
    let rows = store.transfers_after(None, 100, None).unwrap();
    let fees = rows
        .iter()
        .filter(|row| matches!(row.transfer.reference, Reference::Fee { .. }))
        .count();
    assert_eq!(fees, 1, "the TrustSet's fee, once");
    let views = core.asset_views();
    assert_eq!(views[0].schemes.len(), 2);
    let codes: Vec<&str> = views[0]
        .holdings
        .iter()
        .map(|holding| holding.code.as_str())
        .collect();
    assert_eq!(codes, ["XRP", "RLUSD"]);
    let advertised = |core: &Core| {
        core.peer
            .schemes()
            .iter()
            .any(|scheme| scheme.as_str() == "xrpl-testnet-rlusd")
    };
    assert!(advertised(&core));

    ledger
        .issue(
            address,
            Token::RLUSD.amount(TokenValue::parse("10").unwrap()),
        )
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    let usd = view(&core, LedgerId::XrplTestnetRlusdUsd);
    assert_eq!(usd.owner_balance, 10_000_000_000);

    let freeze = |core: &Core| {
        let views = core.asset_views();
        views[0]
            .holdings
            .iter()
            .find(|holding| holding.code == "RLUSD")
            .unwrap()
            .freeze
    };
    assert_eq!(freeze(&core), dassie_settlement::Freeze::default());
    ledger.deep_freeze(address, Token::RLUSD, true).unwrap();
    close_xrpl(&mut core, &mut ledger);
    assert!(!advertised(&core), "advertised while it cannot receive");
    let frozen = freeze(&core);
    assert!(frozen.line && frozen.deep && !frozen.global, "{frozen:?}");
    assert_eq!(
        view(&core, LedgerId::XrplTestnetRlusdUsd).status,
        LedgerStatus::Synced
    );
    ledger.freeze(address, Token::RLUSD, false).unwrap();
    close_xrpl(&mut core, &mut ledger);
    assert!(advertised(&core));

    // A global freeze changes only the issuer's account: read at the
    // settlement check.
    ledger
        .global_freeze(dassie_xrpl::rlusd::ISSUER, true)
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    assert!(advertised(&core));
    core.begin_step(NOW + 60_000);
    follow_xrpl(&mut core, &mut ledger);
    assert!(!advertised(&core), "advertised during a global freeze");
    let frozen = freeze(&core);
    assert!(!frozen.line && frozen.global, "{frozen:?}");
}

// spec/areas/xrpl.md#dsusd: the XRP, RLUSD and DSUSD schemes all settle
// through the node's one XRPL account: each token scheme sets its own
// line, whose TrustSet's fee is a ledger fee on `xrpl-testnet+xrp` and whose
// reserve is held back there; each books only its own token, on a dollar
// ledger of its own; the wallet sees the three holdings, and the link state
// offers the three schemes.
#[test]
#[allow(clippy::too_many_lines)]
fn all_three_xrpl_schemes_on_one_account() {
    use dassie_ledger::{LedgerId, Reference};
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::Wallet;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::{FAUCET_DROPS, FEE_BASE, FakeLedger, RESERVE_BASE, RESERVE_INC, Token};

    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32])
        .settlement_entropy("xrpl-testnet", "seed-testnet")
        .unwrap();
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = FakeLedger::new();
    ledger.fund(address);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 12);
    for scheme in ["xrpl-testnet", "xrpl-testnet-rlusd", "xrpl-testnet-dsusd"] {
        core.add_scheme(scheme, config.into()).unwrap();
        follow_xrpl(&mut core, &mut ledger);
    }
    close_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    let view = |core: &Core, ledger: LedgerId| {
        core.ledger_views()
            .into_iter()
            .find(|view| view.id == ledger)
            .unwrap()
    };
    for id in [
        LedgerId::XrplTestnetXrp,
        LedgerId::XrplTestnetRlusdUsd,
        LedgerId::XrplTestnetDsusdUsd,
    ] {
        assert_eq!(view(&core, id).status, LedgerStatus::Synced, "{id}");
    }
    let units = |drops: u64| drops * 1_000;
    assert_eq!(
        view(&core, LedgerId::XrplTestnetXrp).on_ledger_balance,
        units(FAUCET_DROPS - 2 * FEE_BASE - RESERVE_BASE - 2 * RESERVE_INC)
    );
    core.flush();
    let rows = store.transfers_after(None, 100, None).unwrap();
    let fees = rows
        .iter()
        .filter(|row| matches!(row.transfer.reference, Reference::Fee { .. }))
        .count();
    assert_eq!(fees, 2, "each TrustSet's fee, once");
    let views = core.asset_views();
    assert_eq!(views.len(), 1, "one account");
    assert_eq!(views[0].schemes.len(), 3);
    let mut codes: Vec<&str> = views[0]
        .holdings
        .iter()
        .map(|holding| holding.code.as_str())
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, ["DSUSD", "RLUSD", "XRP"]);
    let advertised: Vec<&str> = core.peer.schemes().iter().map(|s| s.as_str()).collect();
    for scheme in ["xrpl-testnet", "xrpl-testnet-rlusd", "xrpl-testnet-dsusd"] {
        assert!(advertised.contains(&scheme), "{scheme}: {advertised:?}");
    }

    ledger
        .issue(
            address,
            Token::DSUSD.amount(TokenValue::parse("10").unwrap()),
        )
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    assert_eq!(
        view(&core, LedgerId::XrplTestnetDsusdUsd).owner_balance,
        10_000_000_000
    );
    assert_eq!(view(&core, LedgerId::XrplTestnetRlusdUsd).owner_balance, 0);
}

/// A core node that takes downlinks, on RLUSD alone, synced and holding $10
/// on `ledger`.
fn rlusd_core(ledger: &mut dassie_xrpl::fake::FakeLedger) -> Core {
    use dassie_ledger::Name;
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::Wallet;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::Token;

    let key = Arc::new(NodeKey::from_bytes(&[7; 32]));
    let address = Wallet::from_entropy(
        key.settlement_entropy("xrpl-testnet", "seed-testnet")
            .unwrap()
            .as_bytes(),
    )
    .account();
    ledger.fund(address);
    ledger.close();
    let mut core = Core::start(
        CoreParts {
            store: Arc::new(Store::open_in_memory().unwrap()),
            key,
            realm: Realm::Test,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(5)),
            peer_rng: Box::new(SeededRng::new(6)),
            own_info: dassie_peer::OwnInfo {
                url: Some("https://core.test".into()),
                alias: String::new(),
                quic: true,
                feed_port: None,
                hub: None,
            },
            bootstrap: Vec::new(),
            uplinks: Vec::new(),
        },
        NOW,
    )
    .unwrap();
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    core.add_scheme("xrpl-testnet-rlusd", config.into())
        .unwrap();
    follow_xrpl(&mut core, ledger);
    close_xrpl(&mut core, ledger);
    ledger
        .issue(
            address,
            Token::RLUSD.amount(TokenValue::parse("10").unwrap()),
        )
        .unwrap();
    close_xrpl(&mut core, ledger);
    let rlusd = Name::new("xrpl-testnet-rlusd").unwrap();
    assert_eq!(core.schemes[&rlusd].scheme.status(), LedgerStatus::Synced);
    core
}

/// A downlink that peers with `core` on RLUSD at `now`, and uses up the
/// dollar of credit we extend it: it owes us, and our room for it is gone.
fn rlusd_downlink_owing(
    core: &mut Core,
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    now: u64,
) -> dassie_identity::NodeId {
    use dassie_ledger::{AccountName, LedgerId, Name, Reference, TransferSpec};
    use dassie_xrpl::Wallet;

    let downlink = Arc::new(NodeKey::from_bytes(&[11; 32]));
    let id = downlink.node_id();
    let theirs = Wallet::from_entropy(&[11; 32]).account().to_address();
    let mut data = vec![u8::try_from(theirs.len()).unwrap()];
    data.extend_from_slice(theirs.as_bytes());
    core.begin_step(now);
    let body = downlink_request_on(core, &downlink, now, "xrpl-testnet-rlusd", &data);
    core.peer_inbound(dassie_peer::InboundId(1), &body, None);
    follow_xrpl(core, ledger);
    assert!(core.peer.peering_of(&id).is_some(), "peered");
    let name = Name::new(id.as_str()).unwrap();
    let on = LedgerId::XrplTestnetRlusdUsd;
    let account = |core: &Core, name| core.account(on, name).unwrap();
    let interledger = account(core, AccountName::AssetsInterledger(name));
    let dollar = 1_000_000_000;
    for (debit, credit, reference) in [
        (
            account(core, AccountName::ContraTrust(name)),
            interledger,
            Reference::Trust,
        ),
        (
            interledger,
            account(core, AccountName::EquitySuspense),
            Reference::Reconciliation,
        ),
    ] {
        let spec = TransferSpec {
            debit,
            credit,
            amount: dollar,
            pending: false,
            reference,
        };
        core.ledger.create(now, &spec).unwrap();
    }
    id
}

// spec/areas/settlement.md#when-peers-cannot-pay-us: while RLUSD's issuer
// charges a transfer fee, no RLUSD node can pay us, as none pays the fee.
// The node asks no peer on the scheme to settle, and reports a peer short of
// room as one that cannot pay us (`Room::ShortUnpayable`), so its time short
// counts toward withdrawing it but never toward ending it for room. The
// scheme leaves the link state meanwhile, and stays synced. Once the fee is
// gone, the peer is asked again, and its time short counts in full.
#[test]
fn a_transfer_fee_asks_no_peer_and_ends_none_for_room() {
    use dassie_ledger::Name;
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::fake::FakeLedger;

    let mut ledger = FakeLedger::new();
    let mut core = rlusd_core(&mut ledger);
    let rlusd = Name::new("xrpl-testnet-rlusd").unwrap();
    let mut now = NOW + 1_000;
    let id = rlusd_downlink_owing(&mut core, &mut ledger, now);

    // The issuer charges a transfer fee, read at the settlement check.
    ledger
        .set_transfer_rate(dassie_xrpl::rlusd::ISSUER, 1_002_000_000)
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    let interval = core.settle.interval_ms;
    let check = |core: &mut Core, ledger: &mut FakeLedger, now: &mut u64| {
        *now += interval;
        core.begin_step(*now);
        follow_xrpl(core, ledger);
    };
    check(&mut core, &mut ledger, &mut now);
    check(&mut core, &mut ledger, &mut now);
    assert!(!core.schemes[&rlusd].scheme.can_receive());
    assert_eq!(core.schemes[&rlusd].scheme.status(), LedgerStatus::Synced);
    assert!(
        !core
            .peer
            .schemes()
            .iter()
            .any(|scheme| scheme.as_str() == "xrpl-testnet-rlusd"),
        "in the link state during a transfer fee"
    );
    core.settle.asked.remove(&id);
    let short = core.peer.peering_of(&id).unwrap().short_for_ms;
    let reached = core.peer.peering_of(&id).unwrap().short_reached_ms;
    check(&mut core, &mut ledger, &mut now);
    check(&mut core, &mut ledger, &mut now);
    assert!(
        !core.settle.asked.contains_key(&id),
        "asked during a transfer fee"
    );
    let peering = core.peer.peering_of(&id).unwrap();
    assert!(
        peering.short_for_ms > short,
        "short of room, toward withdrawing it"
    );
    assert_eq!(
        peering.short_reached_ms, reached,
        "never toward ending it for room"
    );

    // The fee is gone: the peer is asked again, and its time short counts.
    ledger
        .set_transfer_rate(dassie_xrpl::rlusd::ISSUER, 0)
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    check(&mut core, &mut ledger, &mut now);
    check(&mut core, &mut ledger, &mut now);
    assert!(core.schemes[&rlusd].scheme.can_receive());
    assert!(
        core.settle.asked.contains_key(&id),
        "asked once the fee is gone"
    );
    assert!(core.peer.peering_of(&id).unwrap().short_reached_ms > reached);
}

/// A new incoming payment on the stub ledger, of the owner's or a
/// client's, beneath `uplink` if any.
fn new_incoming(
    client: Option<&str>,
    uplink: Option<dassie_identity::NodeId>,
) -> super::NewIncoming {
    super::NewIncoming {
        ledger: dassie_ledger::LedgerId::StubUsd,
        wallet_address: "https://node.example/.well-known/pay".into(),
        client: client.map(Into::into),
        incoming_amount: None,
        expires_at_ms: None,
        metadata: None,
        uplink,
    }
}

// spec/areas/payments.md#receiving: a core node is paid at its own
// address, so an uplink named for it is refused, and nothing is created.
#[test]
fn a_core_node_refuses_an_uplink() {
    let mut core = core(12);
    core.add_scheme("stub", String::new()).unwrap();
    assert!(!core.peer.is_periphery());
    let uplink = NodeKey::from_bytes(&[11; 32]).node_id();
    let refused = core.create_incoming_payment(new_incoming(None, Some(uplink)));
    assert!(
        matches!(refused, Err(super::EngineError::Precondition(_))),
        "{refused:?}"
    );
    assert!(core.incoming.is_empty());
    let own = core.connector.own_address().to_string();
    let payment = core
        .create_incoming_payment(new_incoming(None, None))
        .unwrap();
    assert!(payment.ilp_address.starts_with(&format!("{own}.")));
}

// spec/areas/payments.md#receiving: a periphery node is paid beneath an
// uplink its owner names only if it is one confirmed now; a refused
// request closes no other payment to make room for itself.
#[test]
fn a_periphery_node_refuses_an_uplink_it_has_not() {
    let mut core = Core::start(
        CoreParts {
            store: Arc::new(Store::open_in_memory().unwrap()),
            key: Arc::new(NodeKey::from_bytes(&[7; 32])),
            realm: Realm::Test,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(13)),
            peer_rng: Box::new(SeededRng::new(14)),
            own_info: dassie_peer::OwnInfo {
                url: None,
                alias: String::new(),
                quic: true,
                feed_port: None,
                hub: None,
            },
            bootstrap: Vec::new(),
            uplinks: Vec::new(),
        },
        NOW,
    )
    .unwrap();
    core.add_scheme("stub", String::new()).unwrap();
    assert!(core.peer.is_periphery());
    // The clients' share is full, with a payment that received nothing.
    core.incoming_policy.max_open_clients = 1;
    let unpaid = core
        .create_incoming_payment(new_incoming(Some("client"), None))
        .unwrap();
    let unknown = NodeKey::from_bytes(&[11; 32]).node_id();
    let refused = core.create_incoming_payment(new_incoming(Some("client"), Some(unknown)));
    let Err(super::EngineError::Precondition(message)) = refused else {
        panic!("{refused:?}");
    };
    assert!(message.contains("not a confirmed uplink"), "{message}");
    assert_eq!(core.incoming.keys().collect::<Vec<_>>(), [&unpaid.id]);
    let refused = core.create_incoming_payment(new_incoming(None, Some(unknown)));
    assert!(matches!(refused, Err(super::EngineError::Precondition(_))));
    assert_eq!(core.incoming.len(), 1);
}

// What a peering over a hub cost beyond its hold is booked as a loss on the
// peer (spec/areas/hubs.md#the-members-side): a claim's fee from
// `assets/settlement`, a packet paid late from the peer's account, each
// journaled as a loss whatever the owner's funds, and reported.
#[test]
fn a_peer_loss_is_booked_from_where_the_money_left() {
    use dassie_ledger::{AccountName, LedgerId, Name, Reference};
    use dassie_settlement::{Effect, LossCause};

    use super::{Event, SettlementEvent};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 9);
    core.add_scheme("stub", String::new()).unwrap();
    let stub = Name::new("stub").unwrap();
    let peer = Name::new("dpeer").unwrap();
    let (interledger, _) = core
        .ledger
        .open_peer(NOW, LedgerId::StubUsd, peer, 0)
        .unwrap();
    let settlement = core
        .account(LedgerId::StubUsd, AccountName::AssetsSettlement)
        .unwrap();
    let losses = core
        .account(LedgerId::StubUsd, AccountName::ExpensesLosses)
        .unwrap();
    let effects = [
        Effect::PeerLoss {
            peer,
            amount: 7,
            cause: LossCause::ClaimFee,
            id: Name::new("op1").unwrap(),
        },
        Effect::PeerLoss {
            peer,
            amount: 30,
            cause: LossCause::PaidLate,
            id: Name::new("op2").unwrap(),
        },
    ];
    core.apply_effects(stub, LedgerId::StubUsd, &effects)
        .unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let reported = outputs
        .iter()
        .filter(|output| {
            matches!(
                output,
                Output::Event(Event::Settlement(SettlementEvent::PeerLoss { .. }))
            )
        })
        .count();
    assert_eq!(reported, 2);
    let balance = |id| *core.ledger.account(id).unwrap().balance();
    assert_eq!(balance(losses).debits_posted, 37);
    assert_eq!(balance(settlement).credits_posted, 7);
    assert_eq!(balance(interledger).credits_posted, 30);
    core.flush();
    let rows = store.transfers_after(None, 100, None).unwrap();
    let ids: Vec<(Name, Name)> = rows
        .iter()
        .filter_map(|row| match row.transfer.reference {
            Reference::Loss { scheme, id } => Some((scheme, id)),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        [
            (stub, Name::new("op1").unwrap()),
            (stub, Name::new("op2").unwrap())
        ]
    );
}

/// A hub's rules, as the scheme's config takes them: four validators,
/// commitments expiring `margin` early, and legs between members of
/// `allowance`.
fn hub_config(margin: u64, allowance: u64) -> String {
    use dassie_hub::escrow::{ValidatorKeys, escrow_master};
    use dassie_hub::rules::{Fees, Limits, Rules, Terms, Timers};
    use dassie_identity::NodeKey;
    use dassie_oer::Encode;

    let nonce = [7; 32];
    let sponsor = NodeKey::from_bytes(&[100; 32]);
    let master = escrow_master(&sponsor, &nonce).unwrap();
    let terms = Terms {
        nonce,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow: master.account().0,
        fees: Fees {
            operation: 10,
            ledger: 100,
            reserve_share_ppm: 100_000,
            account_reserve: 2_000_000,
            hold_reserve: 200_000,
            starting_balance: 1_000,
            least_payout: 1_000_000,
        },
        limits: Limits {
            accounts: 100,
            holds: 100,
            holds_per_account: 8,
            withdrawals: 16,
            pending_deposits: 64,
            recent_deposits: 256,
            block_operations: 64,
            deadline_operations: 16,
            claimed_per_hold: 1_100,
            commitments_per_claim: 16,
            vote_window: 32,
        },
        timers: Timers {
            hold_notice: 60_000,
            shutdown_notice: 60_000,
            reconfigure_notice: 120_000,
            absence: 86_400_000,
            stall: 3_600_000,
            precision: 100,
            margin,
            allowance,
            view_timeout: 200,
        },
    };
    let validators = (1..=4u8)
        .map(|i| {
            let node = NodeKey::from_bytes(&[i; 32]);
            ValidatorKeys::derive(&node, &nonce)
                .unwrap()
                .accept(&node, &terms, format!("v{i}.example:443"))
                .unwrap()
        })
        .collect();
    let rules = Rules { terms, validators };
    let hex: String = rules
        .to_vec()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    serde_json::json!({ "rules": hex }).to_string()
}

// A node joins a hub only when its legs between members fit in the
// connector's message window (spec/designs/settlement-hubs.md#4-peering-over-a-hub):
// a hub too far apart is refused, and nothing of it is kept. One that fits
// is joined, and its account asks the hub's validators for itself.
#[test]
fn a_hub_is_joined_only_if_it_fits_the_message_window() {
    let mut core = core(9);
    let refused = core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(500, 600));
    assert!(refused.is_err(), "500 ms + 600 ms is past the 1 s window");
    assert!(core.ledger_views().is_empty());
    core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(500, 300))
        .unwrap();
    assert_eq!(core.ledger_views().len(), 1);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let mut sent = 0;
    for output in &outputs {
        if let Output::HubSend {
            endpoint, address, ..
        } = output
        {
            assert_eq!(*address, format!("v{}.example:443", endpoint + 1));
            sent += 1;
        }
    }
    assert!(sent > 0, "the account asks the validators: {outputs:?}");
}

// The wallet is offered a settlement hub's scheme only once the node knows
// a hub to join without being told one: until then, adding it with nothing
// filled in could only fail.
#[test]
fn a_hub_scheme_is_offered_only_with_a_default_hub() {
    assert!(super::offered("stub"));
    assert!(super::offered(dassie_xrpl::scheme::SCHEME_ID));
    assert_eq!(
        super::offered(dassie_hub::member::SCHEME_ID),
        dassie_hub::member::DEFAULT_TESTNET_HUB.is_some()
    );
}

/// A core node at `https://core.test` that takes QUIC, over the stub with
/// commitments, with a downlink peered on it: the downlink's ID.
fn hub_core_with_downlink(seed: u64) -> (Core, dassie_identity::NodeId) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = Core::start(
        CoreParts {
            store,
            key: Arc::new(NodeKey::from_bytes(&[7; 32])),
            realm: Realm::Test,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(seed)),
            peer_rng: Box::new(SeededRng::new(seed + 1)),
            own_info: dassie_peer::OwnInfo {
                url: Some("https://core.test".into()),
                alias: String::new(),
                quic: true,
                feed_port: None,
                hub: None,
            },
            bootstrap: Vec::new(),
            uplinks: Vec::new(),
        },
        NOW,
    )
    .unwrap();
    core.add_scheme("stub", r#"{"commitments":1}"#.into())
        .unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let downlink = Arc::new(NodeKey::from_bytes(&[11; 32]));
    let at = NOW + 1_000;
    core.begin_step(at);
    let body = downlink_request(&core, &downlink, at);
    core.peer_inbound(dassie_peer::InboundId(1), &body, None);
    core.finish_step(&mut outputs);
    let id = downlink.node_id();
    assert!(core.peer.peering_of(&id).is_some());
    (core, id)
}

/// The credit `core` extends to `peer` over the stub, its hold toward the
/// peer, and its settlements to the peer still pending.
fn hub_books(core: &mut Core, peer: dassie_identity::NodeId) -> (u64, u64, u64) {
    let stub = dassie_ledger::name!("stub");
    let name = dassie_ledger::Name::new(peer.as_str()).unwrap();
    let toward = core
        .with_commitments(stub, |c| c.holds(name).toward)
        .unwrap();
    let (credit, _, settling) = core.hub_books(stub, peer).unwrap();
    (credit, toward, settling)
}

// spec/areas/hubs.md#in-the-node: asked to settle over a hub, a payer funds
// its hold toward the peer with what it would otherwise have paid, as a
// junior that extends the default credit would, though it extends none:
// the credit is raised by it first, so that the books take the settlement,
// and once the settlement executed the credit is the hold it funded.
#[test]
fn asked_over_a_hub_a_payer_funds_its_hold() {
    let (mut core, peer) = hub_core_with_downlink(31);
    assert_eq!(hub_books(&mut core, peer), (0, 0, 0));
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    core.begin_step(NOW + 2_000);
    let settling = core.settlement_requested(peer, scheme);
    let expected = dassie_settlement::policy::requested_amount(
        u128::from(Policies::DEFAULT.peer_credit),
        core.settle.trigger,
        core.settle.minimum,
    );
    assert!(expected > 0);
    assert_eq!(settling, expected);
    assert_eq!(hub_books(&mut core, peer), (expected, 0, expected));
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (expected, expected, 0));
}

// spec/areas/hubs.md#in-the-node: a payer whose hub account can spend
// nothing funds no hold when asked: it answers that nothing is on its way,
// and its credit and books stay as they were, so the payee's room for it
// stays what it was.
#[test]
fn a_payer_without_hub_funds_funds_nothing() {
    let (mut core, peer) = hub_core_with_downlink(32);
    // Everything the stub holds goes elsewhere, behind the books.
    let stub = dassie_ledger::name!("stub");
    let elsewhere = dassie_ledger::Name::new("delsewhere").unwrap();
    let scheme = &mut core.schemes.get_mut(&stub).unwrap().scheme;
    let balance = scheme.balance();
    let prepared = scheme
        .prepare_settlement(&mut SeededRng::new(9), None, elsewhere, balance, &[])
        .unwrap();
    scheme.execute_settlement(None, prepared.id, &mut Vec::new());
    assert_eq!(scheme.balance(), 0);
    core.begin_step(NOW + 2_000);
    let scheme_id = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    assert_eq!(core.settlement_requested(peer, scheme_id), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (0, 0, 0));
    assert!(core.ledger.pending_transfers().next().is_none());
}

// spec/areas/hubs.md#in-the-node: the credit over a hub follows the hold
// down as the payee takes from it, but never past what the peer may still
// owe us: the books cannot take more from its account than the ledger's
// limit leaves. Here the payee took a packet from our hold that our books
// never paid it, so it owes us all the room there is: the credit stays,
// and follows the hold once our books owe the peer the packet.
#[test]
fn the_credit_over_a_hub_is_lowered_only_as_far_as_the_books_allow() {
    use dassie_ledger::{AccountName, LedgerId, Reference, TransferSpec};
    use dassie_settlement::Promised;

    let (mut core, peer) = hub_core_with_downlink(33);
    let scheme_id = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    core.begin_step(NOW + 2_000);
    let funded = core.settlement_requested(peer, scheme_id);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (funded, funded, 0));
    // The payee takes a packet of 100 from our hold.
    let stub = dassie_ledger::name!("stub");
    let name = dassie_ledger::Name::new(peer.as_str()).unwrap();
    let promised = Promised {
        amount: 100,
        condition: [1; 32],
        expires_at_ms: NOW + 30_000,
    };
    core.with_commitments(stub, |c| {
        let mut out = Vec::new();
        c.commit(NOW, name, 7, promised, &mut out).unwrap();
        c.claim(NOW, name, 7, true, &mut out);
    });
    core.begin_step(NOW + 3_000);
    outputs.clear();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (funded, funded - 100, 0));
    // Our books owe the peer the packet: the credit follows the hold.
    let interledger = core
        .account(LedgerId::StubUsd, AccountName::AssetsInterledger(name))
        .unwrap();
    let pool = core
        .account(LedgerId::StubUsd, AccountName::EquityOwner)
        .unwrap();
    core.begin_step(NOW + 4_000);
    core.ledger
        .create(
            NOW + 4_000,
            &TransferSpec {
                debit: pool,
                credit: interledger,
                amount: 100,
                pending: false,
                reference: Reference::Deposit,
            },
        )
        .unwrap();
    outputs.clear();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (funded - 100, funded - 100, 0));
    let followed = outputs.iter().any(|output| {
        matches!(output, Output::Event(event @ super::Event::Settlement(
            super::SettlementEvent::HubCredit { credit, holds, .. }
        )) if *credit == funded - 100 && *holds == funded - 100
            && event.severity() == super::Severity::Debug)
    });
    assert!(followed, "{outputs:?}");
}

// Keeping the credit over a hub in step writes to the books however full
// the journal is: what came before goes to the batch first, so a step that
// moves many hub peerings' holds never finds the journal full.
#[test]
fn the_credit_over_a_hub_is_kept_in_step_on_a_full_journal() {
    use dassie_identity::NodeKey;
    use dassie_ledger::{AccountName, LedgerId, Name, Reference, TransferError, TransferSpec};

    let mut core = core(9);
    core.add_scheme("stub", r#"{"commitments": 4}"#.into())
        .unwrap();
    let stub = Name::new("stub").unwrap();
    let node = NodeKey::from_bytes(&[7; 32]).node_id();
    let peer = Name::new(node.as_str()).unwrap();
    // We extend it 1000, and it owes us 500, with no hold between us: the
    // credit goes down to what it owes.
    let (interledger, _) = core
        .ledger
        .open_peer(NOW, LedgerId::StubUsd, peer, 1_000)
        .unwrap();
    let owner = core
        .account(LedgerId::StubUsd, AccountName::EquityOwner)
        .unwrap();
    let suspense = core
        .account(LedgerId::StubUsd, AccountName::EquitySuspense)
        .unwrap();
    let book = |core: &mut Core, debit, credit, amount| {
        let spec = TransferSpec {
            debit,
            credit,
            amount,
            pending: false,
            reference: Reference::Reconciliation,
        };
        core.ledger.create(NOW, &spec)
    };
    book(&mut core, interledger, owner, 500).unwrap();
    let mut written = 0;
    loop {
        match book(&mut core, suspense, owner, 1) {
            Ok(_) => written += 1,
            Err(TransferError::JournalFull) => break,
            Err(error) => panic!("{error}"),
        }
        assert!(written < 100_000, "the journal fills");
    }
    core.sync_credit_with(stub, node);
    // Down to what it owes: no room left beyond it.
    let (credit, room, _) = core.hub_books(stub, node).unwrap();
    assert_eq!((credit, room), (500, 0));
}

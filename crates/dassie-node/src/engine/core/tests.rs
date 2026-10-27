#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use dassie_identity::NodeKey;
use dassie_ilp::{Timestamp, ildcp};
use dassie_kit::sim::SeededRng;
use dassie_node_core::config::Realm;
use dassie_oer::Encode;
use dassie_xrpl::network::{MAINNET, TESTNET};

use super::{Core, CoreParts, Outbound, Output};
use crate::node::Policies;
use crate::store::Store;

mod hub_moves;
mod slots;

const NOW: u64 = 1_800_000_000_000;

/// The stub with unlimited trust, for the tests whose subject is not
/// settlement: no credit runs out, and no request to settle or settlement
/// shapes what they check (spec/areas/settlement.md#the-stub-scheme).
const TRUSTING: &str = r#"{"trust":"unlimited"}"#;

/// The stub with commitments, the stand-in for a hub's member, for the tests
/// of settlement, so that what they check holds on the path over a hub
/// (spec/areas/settlement.md#the-stub-scheme).
const COMMITTING: &str = r#"{"commitments":1}"#;

/// The scheme ID `id`, as the peer protocol names a settlement method.
fn scheme_id(id: &str) -> dassie_peer::wire::SchemeId {
    dassie_peer::wire::SchemeId::parse(id).unwrap()
}

/// Adds the stub of `config` to `core`.
fn add_stub(core: &mut Core, config: &str) -> super::LedgerView {
    core.add_scheme("stub", config.into()).unwrap()
}

/// What `core`'s stub writes as its peering info, which a peer of the stub
/// with unlimited trust writes too.
fn stub_info(core: &mut Core) -> Vec<u8> {
    use dassie_peer::PeeringHooks;

    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    let mut info = Vec::new();
    core.with_peer(|_, hooks, _| hooks.peering_info(scheme, &mut info))
        .unwrap();
    info
}

/// The node's first XRPL testnet account, which the XRP Ledger scheme
/// settles through: its connection is the account's.
const XRPL_ACCOUNT: &str = "xrpl-testnet-account/0";

fn core(seed: u64) -> Core {
    core_on(Arc::new(Store::open_in_memory().unwrap()), seed)
}

fn core_on(store: Arc<Store>, seed: u64) -> Core {
    let refused = dassie_node_core::config::LiveRealm::Refused;
    core_in(store, seed, Realm::Test, refused).unwrap()
}

/// A core of `realm`, which may be the live realm as `live` allows.
fn core_in(
    store: Arc<Store>,
    seed: u64,
    realm: Realm,
    live: dassie_node_core::config::LiveRealm,
) -> Result<Core, String> {
    core_with(store, seed, (realm, live), Policies::DEFAULT)
}

/// [`core_in`] with `policies`.
fn core_with(
    store: Arc<Store>,
    seed: u64,
    (realm, live): (Realm, dassie_node_core::config::LiveRealm),
    policies: Policies,
) -> Result<Core, String> {
    let parts = CoreParts {
        store,
        key: Arc::new(NodeKey::from_bytes(&[7; 32])),
        realm,
        live,
        policies,
        rng: Box::new(SeededRng::new(seed)),
        peer_rng: Box::new(SeededRng::new(seed + 1)),
        own_info: dassie_peer::OwnInfo {
            url: Some("https://core.test".into()),
            alias: String::new(),
            feed_port: None,
            hub: None,
        },
        bootstrap: Vec::new(),
        uplinks: Vec::new(),
    };
    Core::start(parts, NOW)
}

/// A short session: a scheme, a token, a client that asks for its address.
fn session(seed: u64) -> Vec<Output> {
    let mut core = core(seed);
    let mut outputs = Vec::new();
    add_stub(&mut core, TRUSTING);
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

// spec/areas/network.md#pinned-uplinks: the production and dev bounds hold
// a periphery node pinned to the most uplinks: an address beneath each,
// an endpoint, and room in the packet table and the ledger for each one's
// share; and a configuration sized for fewer is refused at start.
#[test]
fn the_bounds_hold_the_most_uplinks() {
    for policies in [Policies::DEFAULT, Policies::DEV] {
        assert!(policies.peer.max_core_peers() >= dassie_peer::MAX_UPLINKS);
        super::check_bounds(&policies);
    }
    let short = Policies {
        connector: dassie_connector::Config {
            max_pending: Policies::DEFAULT.connector.max_pending - 512,
            ..Policies::DEFAULT.connector
        },
        ..Policies::DEFAULT
    };
    let refused = std::panic::catch_unwind(|| super::check_bounds(&short));
    assert!(refused.is_err());
}

// Time never goes back within a core, whatever its driver says.
#[test]
fn time_only_moves_forward() {
    let mut core = core(1);
    core.begin_step(NOW + 100);
    core.begin_step(NOW);
    assert_eq!(core.now_ms(), NOW + 100);
}

// A settlement whose pending transfer reached the disk, but which
// a crash stopped before it executed, is handed to its scheme at the next
// start. The stub never executed it, so its money never moved: it is
// voided, and what we owe the peer is as before.
#[test]
fn a_settlement_cut_short_by_a_crash_is_reconciled() {
    use dassie_ledger::{AccountName, AccountPath, LedgerId, Name, Reference, TransferSpec};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let peer = Name::new("peeraaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let path = AccountPath::new(LedgerId::StubUsd, AccountName::AssetsInterledger(peer));
    {
        let mut core = core_on(Arc::clone(&store), 5);
        add_stub(&mut core, COMMITTING);
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
    add_stub(&mut core, COMMITTING);
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
    add_stub(&mut core, COMMITTING);
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

/// The fake XRP Ledger, testnet or `mainnet`, its clock at the tests'
/// [`NOW`], so that the validations a node reads of it are fresh.
fn fake_ledger(mainnet: bool) -> dassie_xrpl::fake::FakeLedger {
    let mut ledger = if mainnet {
        dassie_xrpl::fake::FakeLedger::mainnet()
    } else {
        dassie_xrpl::fake::FakeLedger::new()
    };
    ledger.set_close_time(dassie_xrpl::unl::ledger_seconds(NOW));
    ledger
}

/// `config`, of the account the XRPL scheme `scheme` settles through,
/// trusting the validators of the fake ledger of its network.
fn trusting_fake(scheme: &str, config: &str) -> String {
    dassie_xrpl::fake::with_publishers(config, xrpl_network(scheme))
}

/// The network of the XRPL scheme `scheme`.
fn xrpl_network(scheme: &str) -> &'static dassie_xrpl::Network {
    if matches!(scheme, "xrpl-xrp" | "xrpl-rlusd") {
        &dassie_xrpl::network::MAINNET
    } else {
        &dassie_xrpl::network::TESTNET
    }
}

/// Adds the XRPL scheme `scheme` to `core` with `config` as an owner might
/// have written it before an account's config was its own: what it says of
/// the account (its server and faucet), trusting the fake ledger's
/// validators, is the account's config, and whether it peers the scheme's
/// (spec/areas/settlement.md#external-assets).
fn add_xrpl(
    core: &mut Core,
    scheme: &str,
    config: &str,
) -> Result<super::LedgerView, super::EngineError> {
    let mut account: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(config).unwrap();
    let peers = account.remove("peers");
    let account = serde_json::Value::Object(account).to_string();
    let asset = xrpl_network(scheme).first().to_string();
    core.set_asset_config(&asset, &trusting_fake(scheme, &account))?;
    let config = match peers {
        Some(peers) => serde_json::json!({ "peers": peers }).to_string(),
        None => String::new(),
    };
    core.add_scheme(scheme, config)
}

/// What a fetch of `url` from `ledger`'s list site gives.
fn fetched(ledger: &dassie_xrpl::fake::FakeLedger, url: &str) -> super::LedgerEvent {
    super::LedgerEvent::Fetched {
        url: url.to_string(),
        body: ledger.fetch(url),
    }
}

/// Carries the XRP Ledger account's requests to `ledger` and the answers
/// back, until it has nothing more to say: the account on the fake's
/// network.
fn follow_xrpl(core: &mut Core, ledger: &mut dassie_xrpl::fake::FakeLedger) {
    follow_xrpl_outputs(core, ledger);
}

/// [`follow_xrpl`], returning every output of the steps.
fn follow_xrpl_outputs(core: &mut Core, ledger: &mut dassie_xrpl::fake::FakeLedger) -> Vec<Output> {
    use super::LedgerEvent;

    let asset = ledger.network().first().to_string();
    let mut all = Vec::new();
    for _ in 0..50 {
        let mut outputs = Vec::new();
        core.finish_step(&mut outputs);
        let events: Vec<(u64, LedgerEvent)> = outputs
            .iter()
            .filter_map(|output| match output {
                Output::LedgerConnect {
                    epoch, asset: a, ..
                } if *a == asset => Some((*epoch, LedgerEvent::Connected)),
                Output::LedgerSend {
                    epoch,
                    message,
                    asset: a,
                } if *a == asset => {
                    Some((*epoch, LedgerEvent::Message(ledger.request(1, message))))
                }
                Output::Fetch {
                    epoch,
                    url,
                    asset: a,
                    ..
                } if *a == asset => Some((*epoch, fetched(ledger, url))),
                _ => None,
            })
            .collect();
        all.extend(outputs);
        if events.is_empty() {
            return all;
        }
        for (epoch, event) in &events {
            core.ledger_event(&asset, *epoch, event);
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
    use dassie_xrpl::{UNITS_PER_DROP, Wallet};

    const SCHEME: &str = "xrpl-testnet-xrp";
    const XRP: u64 = 1_000_000;
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(TESTNET.id, 0);
    let account = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(false);
    ledger.fund(account);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let settlement = {
        let mut core = core_on(Arc::clone(&store), 9);
        add_xrpl(&mut core, SCHEME, config).unwrap();
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
    use dassie_xrpl::fake::{RESERVE_BASE, RESERVE_INC};

    use super::{Event, LedgerEvent, SettlementEvent, Severity};

    const SCHEME: &str = "xrpl-testnet-xrp";
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(TESTNET.id, 0);
    let account = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(false);
    ledger.fund(account);
    ledger.close();
    let mut core = core(9);
    add_xrpl(&mut core, SCHEME, config).unwrap();
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
    let view = add_stub(&mut core, TRUSTING);
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
        add_stub(&mut core, TRUSTING);
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
    add_stub(&mut core, TRUSTING);
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
        .add_peer(peer, "test.das.other", Some(account), LedgerId::StubUsd, 0)
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
    add_stub(&mut core, TRUSTING);
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

// spec/areas/payments.md#the-nodes-names: the names the node answers to
// follow its view. A name it gains is handed over at the end of the step,
// and so is a name it no longer holds, which it then answers 421 to.
#[test]
fn a_held_name_gained_or_lost_is_handed_over() {
    use dassie_peer::{Event, NameEvent};

    let mut core = core(14);
    // At start, the view restored.
    assert!(core.take_held_names_changed());
    assert!(!core.take_held_names_changed());
    for event in [
        NameEvent::HoldersChanged { held: 1, unheld: 0 },
        NameEvent::HoldersChanged { held: 0, unheld: 1 },
    ] {
        core.peer_event(Event::Name(event));
        assert!(core.take_held_names_changed(), "{event:?}");
        assert!(!core.take_held_names_changed(), "{event:?}");
    }
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
    add_stub(&mut core, TRUSTING);
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
    add_stub(&mut core, TRUSTING);
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
        core.add_scheme("stub", TRUSTING.into()).ok();
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
        // The tests, in `tests.rs` and its folder, may read what they like.
        if path
            .file_name()
            .is_some_and(|name| name == "tests.rs" || name == "tests")
        {
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
    add_stub(&mut core, TRUSTING);
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
    add_stub(&mut core, TRUSTING);
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
        network: Some(dassie_kit::ClientKey::network_of(network)),
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

    const SCHEME: &str = "xrpl-testnet-xrp";
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let connected = |outputs: &[Output]| {
        outputs.iter().find_map(|output| match output {
            Output::LedgerConnect { epoch, .. } => Some(*epoch),
            _ => None,
        })
    };
    let mut core = core(9);
    add_xrpl(&mut core, SCHEME, config).unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let epoch = connected(&outputs).unwrap();
    let mut ledger = fake_ledger(false);
    ledger.set_network_id(Some(0));
    core.ledger_event(XRPL_ACCOUNT, epoch, &LedgerEvent::Connected);
    outputs.clear();
    core.finish_step(&mut outputs);
    // Its validator list is fetched meanwhile.
    let sent: Vec<&Output> = (outputs.iter())
        .filter(|output| !matches!(output, Output::Fetch { .. }))
        .collect();
    let [Output::LedgerSend { message, .. }] = sent.as_slice() else {
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
    add_xrpl(&mut core, SCHEME, config).unwrap();
    outputs.clear();
    core.finish_step(&mut outputs);
    assert!(connected(&outputs).is_some_and(|again| again != epoch));
    assert_eq!(core.ledger_views()[0].status, LedgerStatus::Connecting);
}

// spec/areas/settlement.md#the-stub-scheme: on the stub with unlimited trust,
// the node that accepts and the node that asked alike extend the unlimited
// credit, and a trust account from an earlier peering that holds less is
// raised to it.
#[test]
fn on_unlimited_trust_both_sides_extend_the_unlimited_credit() {
    use dassie_ledger::{AccountName, AccountPath, LedgerId, Name};
    use dassie_peer::PeeringHooks;
    use dassie_settlement::policy::{UNLIMITED_CREDIT, trust};

    let mut core = core(7);
    core.add_scheme("stub", r#"{"trust":"unlimited"}"#.into())
        .unwrap();
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    let mut info = Vec::new();
    core.with_peer(|_, hooks, _| hooks.peering_info(scheme, &mut info))
        .unwrap();
    let trust_of = |core: &Core, peer: dassie_identity::NodeId| {
        let name = Name::new(peer.as_str()).unwrap();
        let path = AccountPath::new(LedgerId::StubUsd, AccountName::ContraTrust(name));
        trust(
            core.ledger
                .account(core.ledger.find(&path).unwrap())
                .unwrap()
                .balance(),
        )
    };
    // Accepting, to a new peer, and to one we once extended less.
    let fresh = seeded_node(1);
    let known = seeded_node(2);
    let name = Name::new(known.as_str()).unwrap();
    core.ledger
        .open_peer(NOW, LedgerId::StubUsd, name, 0)
        .unwrap();
    for peer in [fresh, known] {
        let agreed = core
            .with_peer(|_, hooks, _| hooks.accept(NOW, peer, false, scheme, &info))
            .unwrap();
        assert_eq!(agreed.credit_out, UNLIMITED_CREDIT);
        assert_eq!(trust_of(&core, peer), UNLIMITED_CREDIT);
    }
    // Asking, to a peer we once extended less.
    let ended = seeded_node(3);
    let name = Name::new(ended.as_str()).unwrap();
    core.ledger
        .open_peer(NOW, LedgerId::StubUsd, name, 0)
        .unwrap();
    let agreed = core
        .with_peer(|_, hooks, _| hooks.finalize(NOW, ended, scheme, &info))
        .unwrap();
    assert_eq!(agreed.credit_out, UNLIMITED_CREDIT);
    assert_eq!(trust_of(&core, ended), UNLIMITED_CREDIT);
    // A peer that is not on the stub with unlimited trust is refused.
    let refused =
        core.with_peer(|_, hooks, _| hooks.accept(NOW, seeded_node(4), false, scheme, &[]));
    assert_eq!(
        refused.err(),
        Some(dassie_peer::wire::RefusalReason::SchemeRefused)
    );
    // A downlink's accounts open with it too, once value moves with it.
    assert_eq!(
        super::downlinks::start_credit(&core.schemes, Name::new("stub").unwrap()),
        UNLIMITED_CREDIT
    );
}

// spec/areas/settlement.md#the-stub-scheme: on the stub with unlimited trust,
// the node never asks a peer to settle on its own account, not even a
// downlink nothing moved with yet, which stands where one extended no
// credit does and which the stub with commitments asks at once
// (a_downlink_that_moves_no_value_leaves_nothing_behind). Asked to settle,
// as at a peering's end, it prepares nothing and answers that nothing is on
// its way.
#[test]
fn on_unlimited_trust_none_is_asked_and_nothing_is_settled() {
    use super::Event;
    use dassie_peer::PeeringHooks;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, id) = stub_core_with_downlink_on(store, r#"{"trust":"unlimited"}"#);
    let stub = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    assert!(core.peer.active_peers().any(|(peer, _)| *peer == id));
    assert_eq!(core.owed(id, stub), Some(0));
    let asked = core.peer.stats().settlements_requested;
    let mut now = NOW + 1_000;
    let mut outputs = Vec::new();
    for _ in 0..3 {
        now += core.settle.interval_ms;
        core.begin_step(now);
        core.finish_step(&mut outputs);
    }
    assert_eq!(
        core.peer.stats().settlements_requested,
        asked,
        "asked to settle on unlimited trust"
    );
    // Once value moves with it, its accounts open with the unlimited
    // credit, which a peer with limited trust would be asked to settle
    // half of. Asked, it settles nothing, and nothing fails.
    core.begin_step(now + 1_000);
    assert!(core.open_downlink_accounts(id, dassie_ledger::name!("stub")));
    assert_eq!(
        core.credit_out(id, stub),
        Some(dassie_settlement::policy::UNLIMITED_CREDIT)
    );
    core.finish_step(&mut outputs);
    outputs.clear();
    core.begin_step(now + 2_000);
    assert_eq!(core.settlement_requested(id, stub), 0);
    core.finish_step(&mut outputs);
    let settlement = outputs
        .iter()
        .find(|output| matches!(output, Output::Event(Event::Settlement(_))));
    assert!(settlement.is_none(), "a settlement event: {settlement:?}");
    assert!(core.ledger.pending_transfers().next().is_none());
    assert!(!core.with_peer(|_, hooks, _| hooks.settling_to(id)));
    core.check_invariants();
}

// spec/decisions/free-receivers-half.md: a settlement of at least one largest
// packet booked from a node marks its channel, for good: it paid us, and is
// no free receiver. A smaller one leaves it free. Neither downlink had
// accounts: its settlement opens them, and is booked as its own
// (spec/decisions/lazy-downlink-accounts.md).
#[test]
fn a_settlement_of_a_largest_packet_marks_its_payer() {
    use dassie_ledger::{LedgerId, Name};
    use dassie_peer::{Channel, PeerBatch, StoredChannel};
    use dassie_settlement::Effect;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let paying = NodeKey::from_bytes(&[11; 32]);
    let short = NodeKey::from_bytes(&[12; 32]);
    let channels = [&paying, &short].map(|key| StoredChannel {
        node: key.node_id(),
        channel: Channel::new(key.public_key(), None),
    });
    let batch = crate::store::LedgerBatch {
        peer: PeerBatch {
            channels: channels.to_vec(),
            ..PeerBatch::default()
        },
        ..crate::store::LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
    let mut core = core_on(Arc::clone(&store), 9);
    add_stub(&mut core, COMMITTING);
    let minimum = core.settle.minimum;
    let mut effects = Vec::new();
    for (key, amount) in [(&paying, minimum), (&short, minimum - 1)] {
        let peer = Name::new(key.node_id().as_str()).unwrap();
        effects.push(Effect::IncomingSettlement {
            peer,
            amount,
            id: None,
        });
    }
    core.apply_effects(Name::new("stub").unwrap(), LedgerId::StubUsd, &effects)
        .unwrap();
    assert_eq!(core.downlinks.opened.len(), 2, "opened as downlinks'");
    let paid = |core: &Core, key: &NodeKey| core.peer.channel(&key.node_id()).unwrap().paid_us;
    assert!(paid(&core, &paying));
    assert!(!paid(&core, &short));
    core.flush();
    drop(core);
    let core = core_on(Arc::clone(&store), 10);
    assert!(paid(&core, &paying), "written with the batch");
    assert!(!paid(&core, &short));
}

// spec/decisions/closing-channels.md, spec/decisions/account-budgets.md:
// peers' accounts stay when their channels close, so new ones stop where the
// room the node keeps for its own accounts begins, each kind of peer within
// its share of what is left.
#[test]
fn peers_accounts_leave_room_for_the_nodes_own() {
    use dassie_ledger::{Config, Ledger, LedgerId, Name};

    let most = super::ledgers::OWN_ACCOUNTS + 8;
    let max_accounts = u32::try_from(most).unwrap();
    assert_eq!(super::peers::peer_budget(max_accounts, false), 4);
    assert_eq!(super::peers::peer_budget(max_accounts, true), 4);
    let mut ledger = Ledger::new(Config {
        max_accounts,
        ..Config::DEFAULT
    });
    let mut downlinks = super::downlinks::Downlinks::default();
    for (seed, downlink) in [(1, false), (2, true), (3, false), (4, true)] {
        let node = NodeKey::from_bytes(&[seed; 32]).node_id();
        let name = Name::new(node.as_str()).unwrap();
        if downlink {
            assert!(downlinks.fit(max_accounts, &node));
            downlinks.opened.insert((LedgerId::StubUsd, name));
        } else {
            assert!(super::peers::room_for_core_peer(&ledger, &downlinks.opened));
        }
        ledger.open_peer(NOW, LedgerId::StubUsd, name, 1).unwrap();
    }
    let next = NodeKey::from_bytes(&[5; 32]).node_id();
    assert!(
        !super::peers::room_for_core_peer(&ledger, &downlinks.opened)
            && !downlinks.fit(max_accounts, &next),
        "the rest is the node's"
    );
    // Whatever the peers took, the node's own accounts still open, the
    // owner's pool first: each batch takes the journal, as the node's step
    // does.
    let path = |name| dassie_ledger::AccountPath::new(LedgerId::StubUsd, name);
    ledger
        .open(path(dassie_ledger::AccountName::EquityOwner))
        .unwrap();
    for n in 1..super::ledgers::OWN_ACCOUNTS {
        while ledger.poll_journal().is_some() {}
        let sub = dassie_ledger::OwnerSub::Btp(u64::try_from(n).unwrap());
        ledger
            .open(path(dassie_ledger::AccountName::EquityOwnerSub(sub)))
            .unwrap();
    }
}

/// The node of key `[seed, 1, ..]`.
fn seeded_node(seed: u32) -> dassie_identity::NodeId {
    let mut bytes = [1; 32];
    bytes[..4].copy_from_slice(&seed.to_be_bytes());
    NodeKey::from_bytes(&bytes).node_id()
}

/// Accepts a peering request from [`seeded_node`] `seed` on the stub, with
/// the stub's own peering info, as a downlink's if `downlink`: a core peer's
/// accounts open, a downlink's only once value moves with it.
fn accept_on_stub(
    core: &mut Core,
    seed: u32,
    downlink: bool,
) -> Result<(), dassie_peer::wire::RefusalReason> {
    use dassie_peer::PeeringHooks;

    let peer = seeded_node(seed);
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    let info = stub_info(core);
    let accepted = core.with_peer(|_, hooks, _| hooks.accept(NOW, peer, downlink, scheme, &info));
    core.take_journal();
    accepted.map(|_| ())
}

/// Opens the accounts of [`seeded_node`] `seed` on the stub as a
/// downlink's, as value that moved with it would: whether they fit.
fn open_downlink_on_stub(core: &mut Core, seed: u32) -> bool {
    let mut opening = super::downlinks::Opening {
        downlinks: &mut core.downlinks,
        written: &mut core.batch.downlink_accounts,
        journal: &mut core.batch.journal,
    };
    let node = seeded_node(seed);
    opening
        .open(
            NOW,
            &mut core.ledger,
            node,
            dassie_ledger::LedgerId::StubUsd,
            0,
        )
        .is_some()
}

// spec/decisions/account-budgets.md: downlinks, which anyone may make by
// minting keys, open accounts only within their own share of the peers'
// account room. Once they filled it, no downlink with a new key is taken,
// and core nodes still peer, inbound and outbound; and the other way round.
#[test]
fn downlinks_never_take_the_room_core_peers_need() {
    use dassie_peer::PeeringHooks;
    use dassie_peer::wire::RefusalReason;

    let mut core = core(7);
    add_stub(&mut core, TRUSTING);
    let max_accounts = Policies::DEFAULT.ledger.max_accounts;
    let pairs = u32::try_from(super::peers::peer_budget(max_accounts, true) / 2).unwrap();
    for seed in 0..pairs {
        assert!(open_downlink_on_stub(&mut core, seed));
    }
    assert!(
        !open_downlink_on_stub(&mut core, pairs),
        "the share is full"
    );
    assert_eq!(
        accept_on_stub(&mut core, pairs, true),
        Err(RefusalReason::Capacity),
        "no room is left for a new downlink's accounts"
    );
    // A downlink whose accounts are open peers again regardless.
    accept_on_stub(&mut core, 0, true).unwrap();
    assert_eq!(core.downlinks.opened.len(), usize::try_from(pairs).unwrap());
    // Core peers have all their room, inbound and outbound.
    let core_pairs = u32::try_from(super::peers::peer_budget(max_accounts, false) / 2).unwrap();
    for seed in pairs + 1..pairs + core_pairs {
        accept_on_stub(&mut core, seed, false).unwrap();
    }
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    let last = NodeKey::from_bytes(&[2; 32]).node_id();
    let info = stub_info(&mut core);
    core.with_peer(|_, hooks, _| hooks.finalize(NOW, last, scheme, &info))
        .unwrap();
    core.take_journal();
    assert_eq!(
        accept_on_stub(&mut core, pairs + core_pairs, false),
        Err(RefusalReason::Capacity),
        "the core peers' share is full"
    );
    // A core node that turned periphery keeps its accounts, opened as a core
    // peer's, and peers again; the downlinks' count stays as it was.
    accept_on_stub(&mut core, pairs + 1, true).unwrap();
    assert_eq!(core.downlinks.opened.len(), usize::try_from(pairs).unwrap());
    core.check_invariants();
}

// spec/decisions/account-budgets.md, spec/decisions/lazy-downlink-accounts.md:
// which share a downlink's accounts take is kept with them, and a downlink
// still waiting for its accounts keeps its room, across a restart.
#[test]
fn the_downlinks_accounts_are_known_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(dir.path()).unwrap());
    let (mut core, opened) = stub_core_with_downlink_on(Arc::clone(&store), TRUSTING);
    let waiting = Arc::new(NodeKey::from_bytes(&[12; 32]));
    let at = NOW + 2_000;
    core.begin_step(at);
    let body = stub_downlink_request(&mut core, &waiting, at);
    core.peer_inbound(dassie_peer::InboundId(2), waiting.public_key(), &body, None);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert_eq!(
        core.downlinks
            .waiting
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .len(),
        2
    );
    assert!(core.open_downlink_accounts(opened, dassie_ledger::name!("stub")));
    accept_on_stub(&mut core, 1, false).unwrap();
    core.check_invariants();
    core.flush();
    drop(core);
    let refused = dassie_node_core::config::LiveRealm::Refused;
    let core = core_taking_downlinks(store, Realm::Test, refused, at + 1_000);
    let name = dassie_ledger::Name::new(opened.as_str()).unwrap();
    assert_eq!(
        core.downlinks.opened.iter().copied().collect::<Vec<_>>(),
        [(dassie_ledger::LedgerId::StubUsd, name)]
    );
    assert_eq!(
        core.downlinks.waiting.iter().copied().collect::<Vec<_>>(),
        [waiting.node_id()]
    );
}

/// Whether `node` has accounts on any ledger of `core`.
fn has_any_accounts(core: &Core, node: dassie_identity::NodeId) -> bool {
    let name = dassie_ledger::Name::new(node.as_str()).unwrap();
    dassie_ledger::LedgerId::ALL.into_iter().any(|on| {
        core.ledger
            .find(&dassie_ledger::AccountPath::new(
                on,
                dassie_ledger::AccountName::AssetsInterledger(name),
            ))
            .is_some()
    })
}

// spec/decisions/lazy-downlink-accounts.md: the attack the shares bound. A
// downlink that peers, moves no value and goes away leaves nothing behind:
// while it is peered it has no accounts, stands as one with nothing on
// them, and is asked to settle as such a downlink is; once its peering
// ended and its channel closed, the room kept for its accounts is free
// again, and neither share holds anything of it.
#[test]
fn a_downlink_that_moves_no_value_leaves_nothing_behind() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, id) = stub_core_with_downlink_on(store, COMMITTING);
    let stub = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    assert!(!has_any_accounts(&core, id));
    assert!(core.downlinks.waiting.contains(&id));
    let endpoint = dassie_connector::EndpointId::Peer(core.peer_endpoints[&id]);
    assert_eq!(core.connector.account_of(endpoint), None);
    assert_eq!(
        (core.owed(id, stub), core.credit_out(id, stub)),
        (Some(0), Some(0))
    );
    let asked = core.peer.stats().settlements_requested;
    let mut now = NOW + 1_000 + core.settle.interval_ms;
    let mut outputs = Vec::new();
    core.begin_step(now);
    core.finish_step(&mut outputs);
    assert!(
        core.peer.stats().settlements_requested > asked,
        "asked to settle, as a downlink we extend no credit is"
    );
    // It goes silent: its peering ends, and its channel closes.
    let most = now + 3 * Policies::DEFAULT.peer.peer_dead_ms;
    while core.peer.channel(&id).is_some() {
        assert!(now < most, "the channel never closed");
        now += 600_000;
        core.begin_step(now);
        outputs.clear();
        core.finish_step(&mut outputs);
    }
    assert!(core.peer.peering_of(&id).is_none());
    assert!(!has_any_accounts(&core, id));
    assert!(core.downlinks.waiting.is_empty() && core.downlinks.opened.is_empty());
    assert_eq!(super::peers::peer_accounts(&core.ledger), 0);
    core.check_invariants();
}

// spec/decisions/closing-channels.md, spec/decisions/lazy-downlink-accounts.md:
// a downlink nothing moved with yet may have settled on any scheme we run,
// so its channel closes only while every one of them sees its ledger, as a
// peer's closes only while the schemes of its accounts do. While one is
// stopped, it stays open, and its room kept.
#[test]
fn a_downlinks_channel_without_accounts_waits_for_every_scheme() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, id) = stub_core_with_downlink_on(store, COMMITTING);
    let stub = dassie_ledger::name!("stub");
    core.stop_scheme(stub, super::EngineError::Invalid("blind".into()));
    let mut now = NOW + 1_000;
    let most = now + 3 * Policies::DEFAULT.peer.peer_dead_ms;
    let mut outputs = Vec::new();
    while now < most {
        now += 600_000;
        core.begin_step(now);
        outputs.clear();
        core.finish_step(&mut outputs);
    }
    assert!(core.peer.peering_of(&id).is_none(), "the peering ended");
    assert!(core.peer.channel(&id).is_some(), "the channel stays open");
    assert!(core.downlinks.waiting.contains(&id), "and its room kept");
    core.check_invariants();
}

// spec/areas/settlement.md#incoming-settlements: the books first, for a
// downlink nothing moved with yet too. A settlement its accounts could not
// take once opened is refused before the scheme sees it, as any peer's
// is: the scheme keeps running, and nothing opens.
#[test]
fn a_downlinks_settlement_the_books_cannot_take_is_refused_first() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, id) = stub_core_with_downlink_on(store, COMMITTING);
    let stub = dassie_ledger::name!("stub");
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    core.begin_step(NOW + 2_000);
    for amount in [0, dassie_ledger::MAX_TOTAL + 1, u64::MAX] {
        assert!(
            core.incoming_settlement(id, scheme, amount, &[]).is_err(),
            "{amount} is refused"
        );
    }
    assert!(core.schemes[&stub].failed.is_none(), "the stub runs on");
    assert!(core.downlinks.waiting.contains(&id) && core.downlinks.opened.is_empty());
    // What the books take opens its accounts.
    core.incoming_settlement(id, scheme, 5_000, &[]).unwrap();
    assert_eq!(core.owed(id, scheme), Some(5_000));
    assert!(core.downlinks.waiting.is_empty());
    core.check_invariants();
}

// spec/decisions/lazy-downlink-accounts.md: over a hub, our settlement to a
// downlink opens its accounts only when something is settled: a request
// that settles nothing opens nothing.
#[test]
fn over_a_hub_a_request_that_settles_nothing_opens_nothing() {
    let (mut core, peer) = hub_core_with_downlink(35);
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    let trigger = core.settle.trigger;
    core.settle.trigger = u64::MAX;
    core.begin_step(NOW + 2_000);
    assert_eq!(core.settlement_requested(peer, scheme), 0);
    assert!(!has_any_accounts(&core, peer));
    core.settle.trigger = trigger;
    assert!(core.settlement_requested(peer, scheme) > 0);
    assert!(has_any_accounts(&core, peer));
}

// spec/decisions/lazy-downlink-accounts.md: the first packet that moves
// money to a downlink opens its accounts, in the room kept for them, with
// the packet's transfer; they are written with the step, as a downlink's.
#[test]
fn a_payment_to_a_downlink_opens_its_accounts() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, id) = stub_core_with_downlink_on(Arc::clone(&store), TRUSTING);
    let token = core
        .create_token("wallet".into(), "stub+usd", None)
        .unwrap();
    let link = core.btp_connect(&token.secret).unwrap().link;
    let to = format!("{}.{id}.wallet", core.own_address());
    let packet = |amount| {
        dassie_ilp::Prepare {
            amount,
            expires_at: Timestamp::from_unix_ms(NOW + 30_000),
            execution_condition: [7; 32],
            destination: dassie_ilp::Address::parse(&to).unwrap(),
            data: &[],
        }
        .to_vec()
    };
    core.begin_step(NOW + 2_000);
    core.btp_packet(link, 1, &packet(0), false);
    assert!(!has_any_accounts(&core, id), "nothing moved");
    core.btp_packet(link, 2, &packet(1_000), false);
    assert_eq!(core.connector.pending_count(), 2);
    let name = dassie_ledger::Name::new(id.as_str()).unwrap();
    let account = core
        .account(
            dassie_ledger::LedgerId::StubUsd,
            dassie_ledger::AccountName::AssetsInterledger(name),
        )
        .unwrap();
    let endpoint = dassie_connector::EndpointId::Peer(core.peer_endpoints[&id]);
    assert_eq!(core.connector.account_of(endpoint), Some(account));
    assert!(core.downlinks.waiting.is_empty());
    core.check_invariants();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert_eq!(
        store.downlink_accounts().unwrap(),
        [(dassie_ledger::LedgerId::StubUsd, name)]
    );
}

// spec/decisions/lazy-downlink-accounts.md: a downlink's first settlement
// that the books take opens its accounts, and is booked as its settlement,
// not the owner's deposit.
#[test]
fn a_downlinks_first_settlement_opens_its_accounts() {
    use dassie_settlement::Effect;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, id) = stub_core_with_downlink_on(store, COMMITTING);
    let stub = dassie_ledger::name!("stub");
    let name = dassie_ledger::Name::new(id.as_str()).unwrap();
    core.begin_step(NOW + 2_000);
    let settled = Effect::IncomingSettlement {
        peer: name,
        amount: 5_000,
        id: None,
    };
    core.apply_effects(stub, dassie_ledger::LedgerId::StubUsd, &[settled])
        .unwrap();
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    assert_eq!(core.owed(id, scheme), Some(5_000));
    assert!(core.downlinks.waiting.is_empty() && core.downlinks.opened.len() == 1);
    core.check_invariants();
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
    add_stub(&mut core, COMMITTING);
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
        add_stub(&mut core, TRUSTING);
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
        add_stub(&mut core, TRUSTING);
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
        add_stub(&mut core, TRUSTING);
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

    add_stub(core, TRUSTING);
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

/// A periphery node's peering request on the stub with commitments, which
/// writes no peering info, from `downlink`, at `now`.
fn downlink_request(downlink: &Arc<NodeKey>, now: u64) -> Vec<u8> {
    downlink_request_on(downlink, now, "stub", &[])
}

/// A periphery node's peering request to `core`, from `downlink`, at `now`,
/// on `core`'s stub, with the peering info the stub writes.
fn stub_downlink_request(core: &mut Core, downlink: &Arc<NodeKey>, now: u64) -> Vec<u8> {
    let info = stub_info(core);
    downlink_request_on(downlink, now, "stub", &info)
}

/// A peering request from `downlink` on `scheme`, with the scheme's `data`.
fn downlink_request_on(downlink: &Arc<NodeKey>, now: u64, scheme: &str, data: &[u8]) -> Vec<u8> {
    use dassie_oer::Decode;
    use dassie_peer::wire::{
        Content, EXTENSION_PERIPHERY, Entry, NodeInfo, PeeringRequest, SchemeId, SignedNodeInfo,
        sign_node_info,
    };
    let stub = SchemeId::parse(scheme).unwrap();
    let info = NodeInfo {
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
    });
    content.to_vec()
}

// spec/areas/network.md#advertised-peers: a downlink that owes nothing is
// never short of room in our books, though we extend it nothing over the
// hub: withdrawing it would only stop the payments to it that raise its
// room.
#[test]
fn a_downlink_that_owes_nothing_is_never_short_of_room() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let refused = dassie_node_core::config::LiveRealm::Refused;
    let mut core = core_taking_downlinks(store, Realm::Test, refused, NOW);
    add_stub(&mut core, COMMITTING);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let downlink = Arc::new(NodeKey::from_bytes(&[11; 32]));
    let at = NOW + 1_000;
    core.begin_step(at);
    let body = stub_downlink_request(&mut core, &downlink, at);
    core.peer_inbound(
        dassie_peer::InboundId(1),
        downlink.public_key(),
        &body,
        None,
    );
    core.finish_step(&mut outputs);
    let id = downlink.node_id();
    assert!(core.peer.is_downlink(&id) && core.peer.peering_of(&id).is_some());
    assert_eq!(core.peer.peering_of(&id).unwrap().first().credit_out, 0);
    let grace = Policies::DEFAULT.peer.advertise_short_grace_ms;
    for second in 2..=grace / 1_000 * 2 {
        core.begin_step(NOW + second * 1_000);
        core.finish_step(&mut outputs);
    }
    assert_eq!(core.peer.peering_of(&id).unwrap().first().short_for_ms, 0);
}

/// A core node that takes at most `max_downlinks` downlinks, on the store.
fn core_taking_few_downlinks(store: Arc<Store>, max_downlinks: usize, now: u64) -> Core {
    let policies = Policies {
        peer: dassie_peer::Config {
            max_downlinks,
            ..Policies::DEFAULT.peer
        },
        ..Policies::DEFAULT
    };
    Core::start(
        CoreParts {
            store,
            key: Arc::new(NodeKey::from_bytes(&[7; 32])),
            realm: Realm::Test,
            live: dassie_node_core::config::LiveRealm::Refused,
            policies,
            rng: Box::new(SeededRng::new(5)),
            peer_rng: Box::new(SeededRng::new(6)),
            own_info: dassie_peer::OwnInfo {
                url: Some("https://core.test".into()),
                alias: String::new(),
                feed_port: None,
                hub: None,
            },
            bootstrap: Vec::new(),
            uplinks: Vec::new(),
        },
        now,
    )
    .unwrap()
}

/// `core` takes `downlink`'s peering request on `scheme`, with the scheme's
/// `data`, at `at`: why it refused, if it did.
fn asks_as_downlink(
    core: &mut Core,
    downlink: &Arc<NodeKey>,
    at: u64,
    (scheme, data): (&str, &[u8]),
) -> Option<dassie_peer::wire::RefusalReason> {
    use dassie_oer::Decode;
    use dassie_peer::wire::PeeringResponse;

    core.begin_step(at);
    let body = downlink_request_on(downlink, at, scheme, data);
    core.peer_inbound(
        dassie_peer::InboundId(at),
        downlink.public_key(),
        &body,
        None,
    );
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let reply = outputs
        .iter()
        .find_map(|output| match output {
            Output::PeerReply { reply, .. } => Some(reply),
            _ => None,
        })
        .unwrap();
    match PeeringResponse::decode_exact(&reply.body).unwrap() {
        PeeringResponse::Accepted => None,
        PeeringResponse::Refused(reason) => Some(reason),
    }
}

/// An XRPL scheme's peering data naming `account`: its account ID.
fn address_data(account: dassie_xrpl::AccountId) -> Vec<u8> {
    account.0.to_vec()
}

/// What `node`'s account on `ledger` took in, posted: its settlements.
fn settled_in(
    core: &Core,
    ledger: dassie_ledger::LedgerId,
    node: &dassie_identity::NodeId,
) -> u128 {
    let peer = dassie_ledger::Name::new(node.as_str()).unwrap();
    let name = dassie_ledger::AccountName::AssetsInterledger(peer);
    let account = core.account(ledger, name).unwrap();
    core.ledger
        .account(account)
        .unwrap()
        .balance()
        .credits_posted
}

/// `wallet` pays `drops` to `to` on `ledger` before `node` asks to peer with
/// it, as a settlement of `node`'s would: with the XRP scheme's memo naming
/// `node` (spec/decisions/pay-to-enter.md).
fn pay_to_enter(
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    wallet: &dassie_xrpl::Wallet,
    to: dassie_xrpl::AccountId,
    node: &NodeKey,
    drops: u64,
) {
    use dassie_xrpl::codec::{Amount, Memo, Payment, Signing, Transact};

    let payment = Payment {
        account: wallet.account(),
        destination: to,
        destination_tag: None,
        amount: Amount::Xrp(drops),
        send_max: None,
        fee: dassie_xrpl::fake::FEE_BASE,
        sequence: ledger.sequence(wallet.account()).unwrap(),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: vec![Memo {
            memo_type: dassie_xrpl::scheme::MEMO_TYPE.to_vec(),
            memo_data: node.node_id().as_str().as_bytes().to_vec(),
        }],
        signing: Signing::by(wallet.public_key()),
    }
    .sign(wallet);
    let submit = serde_json::json!({
        "command": "submit",
        "tx_blob": dassie_xrpl::hex_upper(&payment.blob),
    });
    let submitted: serde_json::Value =
        serde_json::from_slice(&ledger.request(1, &serde_json::to_vec(&submit).unwrap())).unwrap();
    assert_eq!(submitted["result"]["engine_result"], "tesSUCCESS");
}

// spec/decisions/pay-to-enter.md: past a full free half, a newcomer that paid
// the uplink one largest packet on the XRP Ledger before it asked, from the
// address its request names and with the scheme's memo naming it, gets in.
// What it paid is held off the books until then, across a restart, and is
// booked once as its settlement as it peers, which marks it as a downlink
// that paid us. One that did not pay is told to.
// It had no accounts before: what it paid opens them, as a downlink's
// (spec/decisions/lazy-downlink-accounts.md).
// Goes with direct settlement: entry payments (spec/roadmap.md#hubs-only).
#[test]
fn a_newcomer_that_paid_to_enter_is_taken_and_booked_once() {
    use dassie_ledger::LedgerId;
    use dassie_peer::wire::RefusalReason;
    use dassie_xrpl::{UNITS_PER_DROP, Wallet};

    const SCHEME: &str = "xrpl-testnet-xrp";
    const ENTRY_DROPS: u64 = 100_000;
    let (mut ledger, uplink) = funded_ledger();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_taking_few_downlinks(Arc::clone(&store), 2, NOW);
    core.add_scheme("stub", String::new()).unwrap();
    add_xrpl(&mut core, SCHEME, SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let free = Arc::new(NodeKey::from_bytes(&[11; 32]));
    assert_eq!(
        asks_as_downlink(&mut core, &free, NOW + 1_000, ("stub", &[])),
        None
    );
    // The newcomer's own account, which its request names.
    let newcomer = Arc::new(NodeKey::from_bytes(&[12; 32]));
    let wallet = Wallet::from_seed([12; 16]);
    ledger.fund(wallet.account());
    close_xrpl(&mut core, &mut ledger);
    let data = address_data(wallet.account());
    let ask = |core: &mut Core, at: u64| asks_as_downlink(core, &newcomer, at, (SCHEME, &data));
    assert_eq!(ask(&mut core, NOW + 2_000), Some(RefusalReason::PayToEnter));
    pay_to_enter(&mut ledger, &wallet, uplink, &newcomer, ENTRY_DROPS);
    let owner_before = core.owner_available(LedgerId::XrplTestnetXrp);
    close_xrpl(&mut core, &mut ledger);
    assert_eq!(
        core.owner_available(LedgerId::XrplTestnetXrp),
        owner_before,
        "held, not the owner's"
    );
    core.flush();
    drop(core);
    let mut core = core_taking_few_downlinks(Arc::clone(&store), 2, NOW + 3_000);
    follow_xrpl(&mut core, &mut ledger);
    assert_eq!(ask(&mut core, NOW + 4_000), None);
    let id = newcomer.node_id();
    assert!(core.peer.is_downlink(&id));
    assert!(
        core.peer.channel(&id).unwrap().paid_us,
        "a downlink that paid us"
    );
    // It had no accounts: its entry payment opened them, in the room kept
    // for them, as a downlink's (spec/decisions/lazy-downlink-accounts.md).
    let name = dassie_ledger::Name::new(id.as_str()).unwrap();
    assert!(
        core.downlinks
            .opened
            .contains(&(LedgerId::XrplTestnetXrp, name))
    );
    assert!(!core.downlinks.waiting.contains(&id));
    core.check_invariants();
    let paid = |core: &Core| settled_in(core, LedgerId::XrplTestnetXrp, &id);
    assert_eq!(paid(&core), u128::from(ENTRY_DROPS * UNITS_PER_DROP));
    assert_eq!(core.owner_available(LedgerId::XrplTestnetXrp), owner_before);
    core.flush();
    drop(core);
    let mut core = core_taking_few_downlinks(Arc::clone(&store), 2, NOW + 5_000);
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    assert_eq!(
        paid(&core),
        u128::from(ENTRY_DROPS * UNITS_PER_DROP),
        "once"
    );
    assert!(core.peer.channel(&id).unwrap().paid_us);
}

/// Fills the downlinks' share of `core`'s peers' accounts, beside the room
/// kept for downlinks waiting for theirs, with former downlinks' accounts
/// on the stub's ledger.
fn fill_downlinks_share(core: &mut Core) {
    let max_accounts = core.ledger.config().max_accounts;
    let anyone = NodeKey::from_bytes(&[4; 32]).node_id();
    let mut seed = 1 << 20;
    while core.downlinks.fit(max_accounts, &anyone) {
        assert!(open_downlink_on_stub(core, seed));
        core.take_journal();
        seed += 1;
    }
}

// spec/decisions/pay-to-enter.md, spec/decisions/account-budgets.md: past a
// full free half, a newcomer whose accounts would not fit the downlinks'
// share is refused for capacity, not told to pay: a payment it made would
// get it no peering. One that paid anyway is refused all the same, and its
// payment stays held, not booked.
// Goes with direct settlement: entry payments (spec/roadmap.md#hubs-only).
#[test]
fn a_newcomer_without_room_for_its_accounts_is_not_told_to_pay() {
    use dassie_ledger::LedgerId;
    use dassie_peer::wire::RefusalReason;
    use dassie_xrpl::Wallet;

    const SCHEME: &str = "xrpl-testnet-xrp";
    let (mut ledger, uplink) = funded_ledger();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_taking_few_downlinks(Arc::clone(&store), 2, NOW);
    core.add_scheme("stub", String::new()).unwrap();
    add_xrpl(&mut core, SCHEME, SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let free = Arc::new(NodeKey::from_bytes(&[11; 32]));
    assert_eq!(
        asks_as_downlink(&mut core, &free, NOW + 1_000, ("stub", &[])),
        None
    );
    fill_downlinks_share(&mut core);
    let newcomer = Arc::new(NodeKey::from_bytes(&[12; 32]));
    let wallet = Wallet::from_seed([12; 16]);
    ledger.fund(wallet.account());
    close_xrpl(&mut core, &mut ledger);
    let data = address_data(wallet.account());
    let ask = |core: &mut Core, at: u64| asks_as_downlink(core, &newcomer, at, (SCHEME, &data));
    assert_eq!(ask(&mut core, NOW + 2_000), Some(RefusalReason::Capacity));
    pay_to_enter(&mut ledger, &wallet, uplink, &newcomer, 100_000);
    let owner_before = core.owner_available(LedgerId::XrplTestnetXrp);
    close_xrpl(&mut core, &mut ledger);
    // A new session, as a newcomer that paid would ask in.
    core.flush();
    drop(core);
    let mut core = core_taking_few_downlinks(Arc::clone(&store), 2, NOW + 3_000);
    follow_xrpl(&mut core, &mut ledger);
    assert_eq!(ask(&mut core, NOW + 4_000), Some(RefusalReason::Capacity));
    assert!(core.peer.peering_of(&newcomer.node_id()).is_none());
    assert_eq!(
        core.owner_available(LedgerId::XrplTestnetXrp),
        owner_before,
        "still held"
    );
    core.check_invariants();
}

/// Closes a ledger of `ledger`, and hands the XRP Ledger account what its
/// streams say.
fn close_xrpl(core: &mut Core, ledger: &mut dassie_xrpl::fake::FakeLedger) {
    use super::LedgerEvent;

    let asset = ledger.network().first();
    let epoch = core.assets[&asset].epoch;
    // On the core's clock, which a test may move on by minutes.
    let close_time = dassie_xrpl::unl::ledger_seconds(core.now_ms());
    for (_, message) in ledger.close_at(close_time) {
        core.ledger_event(&asset.to_string(), epoch, &LedgerEvent::Message(message));
    }
    follow_xrpl(core, ledger);
}

// spec/areas/settlement.md#external-assets: an XRPL account's config is its
// own: a scheme that names the account's server is refused, saying where it
// goes; the owner sets it apart, checked as the account reads it, and an
// account active with another takes it at the next start, which the owner
// sees meanwhile. Only the node's first account of its realm's network is
// configured.
#[test]
fn an_accounts_config_is_its_own() {
    use super::EngineError;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 4);
    let config = trusting_fake(
        "xrpl-testnet-xrp",
        r#"{"server":"ws://ledger.test","faucet":null}"#,
    );
    let refused = core.add_scheme("xrpl-testnet-xrp", config.clone());
    assert!(
        matches!(&refused, Err(EngineError::Invalid(why)) if why.contains("SetAssetConfig")),
        "{refused:?}"
    );
    assert!(matches!(
        core.set_asset_config("xrpl-testnet-account/1", &config),
        Err(EngineError::Invalid(_))
    ));
    assert!(matches!(
        core.set_asset_config("hub-testnet-account/0", ""),
        Err(EngineError::Invalid(_))
    ));
    assert!(matches!(
        core.set_asset_config("xrpl-account/0", ""),
        Err(EngineError::Precondition(_))
    ));
    assert!(matches!(
        core.set_asset_config(XRPL_ACCOUNT, r#"{"server":"https://a"}"#),
        Err(EngineError::Invalid(_))
    ));
    assert_eq!(core.set_asset_config(XRPL_ACCOUNT, &config), Ok(true));
    core.add_scheme("xrpl-testnet-xrp", String::new()).unwrap();
    let view = |core: &Core| core.asset_views().remove(0);
    let written = serde_json::from_str::<serde_json::Value>(&config)
        .unwrap()
        .to_string();
    assert_eq!(view(&core).config, written);
    assert!(!view(&core).config_waits);
    // Another server, while the scheme settles through the account.
    let other = trusting_fake(
        "xrpl-testnet-xrp",
        r#"{"server":"ws://other.test","faucet":null}"#,
    );
    assert_eq!(core.set_asset_config(XRPL_ACCOUNT, &other), Ok(false));
    assert!(view(&core).config_waits);
    assert_eq!(view(&core).config, written);
    core.flush();
    drop(core);
    let core = core_on(Arc::clone(&store), 4);
    let other = serde_json::from_str::<serde_json::Value>(&other)
        .unwrap()
        .to_string();
    assert_eq!(view(&core).config, other);
    assert!(!view(&core).config_waits);
    assert_eq!(
        store.asset_configs().unwrap(),
        [(XRPL_ACCOUNT.to_string(), other)]
    );
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
    ) -> Result<Vec<u8>, dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn finalize_peering_request(
        &mut self,
        _peer: dassie_ledger::Name,
        _info: &[u8],
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

    fn deposit(
        &mut self,
        _amount: u64,
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn check_invariants(&self) {}
}

/// A scheme on the DSUSD hub's ledger, for the tests ([`HubStandIn`]).
pub(super) const DSUSD_HUB: &str = "dsusd-hub";
/// A scheme on the XRP hub's ledger, for the tests ([`HubStandIn`]).
pub(super) const XRP_HUB: &str = "xrp-hub";

/// A scheme that books a hub's ledger, for the tests, with no account in a
/// hub: it settles nothing and converts as any scheme does, so whether its
/// ledger converts is up to its owner book's guard alone.
pub(super) struct HubStandIn {
    id: dassie_ledger::Name,
    ledger: dassie_ledger::LedgerId,
}

impl HubStandIn {
    /// On the DSUSD hub's ledger, whose owner book has a peg guard.
    pub(super) const DSUSD: Self = Self {
        id: dassie_ledger::name!(DSUSD_HUB),
        ledger: dassie_ledger::LedgerId::HubXrplTestnetDsusdUsd,
    };
    /// On the XRP hub's ledger, whose owner book has none.
    pub(super) const XRP: Self = Self {
        id: dassie_ledger::name!(XRP_HUB),
        ledger: dassie_ledger::LedgerId::HubXrplTestnetXrp,
    };
}

impl dassie_settlement::SettlementScheme for HubStandIn {
    fn id(&self) -> dassie_ledger::Name {
        self.id
    }

    fn ledger(&self) -> dassie_ledger::LedgerId {
        self.ledger
    }

    fn balance(&self) -> u64 {
        0
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
    ) -> Result<Vec<u8>, dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn finalize_peering_request(
        &mut self,
        _peer: dassie_ledger::Name,
        _info: &[u8],
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

    fn deposit(
        &mut self,
        _amount: u64,
        _effects: &mut Vec<dassie_settlement::Effect>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        Err(dassie_settlement::SchemeError::Unsupported)
    }

    fn check_invariants(&self) {}
}

/// A second stub with commitments, for the tests of a peering with two
/// settlement methods over hubs ([`StubTwo`]).
pub(super) const STUB_TWO: &str = "stub-two";

/// The pretend hub every [`StubTwo`] is a member of: another than the
/// stub's, so that two nodes running both share two hubs.
const STUB_TWO_HUB: [u8; 32] = *b"the second stub's pretend hub..!";

std::thread_local! {
    /// The packets any [`StubTwo`] of this thread committed to or took a
    /// commitment for. A test runs its cores on its own thread, so it
    /// counts the packets of that test's cores alone, whichever tests run
    /// beside it.
    static STUB_TWO_PACKETS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Whether every [`StubTwo`] of this thread cannot receive a peer's
    /// settlement, as a scheme whose line an issuer froze
    /// (spec/areas/settlement.md#when-peers-cannot-pay-us).
    static STUB_TWO_UNPAYABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes every [`StubTwo`] of this test unable to receive, or able again.
fn stub_two_unpayable(unpayable: bool) {
    STUB_TWO_UNPAYABLE.with(|cell| cell.set(unpayable));
}

/// The packets [`StubTwo`] carried in this test so far.
fn stub_two_packets() -> u64 {
    STUB_TWO_PACKETS.with(std::cell::Cell::get)
}

/// Counts a packet [`StubTwo`] carried.
fn count_stub_two_packet() {
    STUB_TWO_PACKETS.with(|count| count.set(count.get() + 1));
}

/// The stub with commitments under another name, on a ledger of its own,
/// the XRP hub's, whose owner book has no peg guard, and in a pretend hub
/// of its own: what it does is the stub's.
/// Its stub is in a cell: the commitments' reads take `&self`, and the
/// stub lends them only as `&mut`.
pub(super) struct StubTwo(std::cell::RefCell<dassie_settlement::Stub>);

impl StubTwo {
    /// The scheme, as the node makes it.
    pub(super) fn boxed(
        config: &str,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Box<dyn dassie_settlement::SettlementScheme>, dassie_settlement::SchemeError> {
        let stub = dassie_settlement::Stub::new(config, checkpoint, state)?;
        Ok(Box::new(Self(std::cell::RefCell::new(stub))))
    }

    /// Runs `f` on the stub.
    fn stub<R>(&self, f: impl FnOnce(&mut dassie_settlement::Stub) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }

    /// Runs `f` on the stub's commitments.
    fn with_inner<R>(&self, f: impl FnOnce(&mut dyn dassie_settlement::Commitments) -> R) -> R {
        use dassie_settlement::SettlementScheme;
        self.stub(|stub| f(stub.commitments().expect("a stub with commitments")))
    }
}

impl dassie_settlement::SettlementScheme for StubTwo {
    fn id(&self) -> dassie_ledger::Name {
        dassie_ledger::name!(STUB_TWO)
    }

    fn can_receive(&self) -> bool {
        !STUB_TWO_UNPAYABLE.with(std::cell::Cell::get)
    }

    fn ledger(&self) -> dassie_ledger::LedgerId {
        dassie_ledger::LedgerId::HubXrplTestnetXrp
    }

    fn balance(&self) -> u64 {
        self.stub(|stub| stub.balance())
    }

    fn state(&self) -> Vec<u8> {
        self.stub(|stub| stub.state())
    }

    fn state_version(&self) -> u64 {
        self.stub(|stub| stub.state_version())
    }

    fn peering_info(&self, out: &mut Vec<u8>) {
        self.stub(|stub| stub.peering_info(out));
    }

    fn create_peering_request(
        &mut self,
        peer: dassie_ledger::Name,
        info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        self.stub(|stub| stub.create_peering_request(peer, info, out))
    }

    fn accept_peering_request(
        &mut self,
        peer: dassie_ledger::Name,
        data: &[u8],
    ) -> Result<Vec<u8>, dassie_settlement::SchemeError> {
        self.stub(|stub| stub.accept_peering_request(peer, data))
    }

    fn finalize_peering_request(
        &mut self,
        peer: dassie_ledger::Name,
        info: &[u8],
    ) -> Result<Vec<u8>, dassie_settlement::SchemeError> {
        self.stub(|stub| stub.finalize_peering_request(peer, info))
    }

    fn unlimited_trust(&self) -> bool {
        self.stub(|stub| stub.unlimited_trust())
    }

    fn prepare_settlement(
        &mut self,
        rng: &mut dyn rand_core::CryptoRng,
        asset: Option<&mut dyn dassie_settlement::ExternalAsset>,
        peer: dassie_ledger::Name,
        amount: u64,
        state: &[u8],
    ) -> Result<dassie_settlement::PreparedSettlement, dassie_settlement::SchemeError> {
        self.stub(|stub| stub.prepare_settlement(rng, asset, peer, amount, state))
    }

    fn discard_settlement(
        &mut self,
        asset: Option<&mut dyn dassie_settlement::ExternalAsset>,
        settlement: dassie_ledger::Name,
    ) {
        self.stub(|stub| stub.discard_settlement(asset, settlement));
    }

    fn execute_settlement(
        &mut self,
        asset: Option<&mut dyn dassie_settlement::ExternalAsset>,
        settlement: dassie_ledger::Name,
        effects: &mut Vec<dassie_settlement::Effect>,
    ) {
        self.stub(|stub| stub.execute_settlement(asset, settlement, effects));
    }

    fn reconcile(
        &mut self,
        asset: Option<&mut dyn dassie_settlement::ExternalAsset>,
        pending: &[(dassie_ledger::Name, u64)],
        effects: &mut Vec<dassie_settlement::Effect>,
    ) {
        self.stub(|stub| stub.reconcile(asset, pending, effects));
    }

    fn handle_settlement(
        &mut self,
        peer: dassie_ledger::Name,
        amount: u64,
        data: &[u8],
        state: &[u8],
        effects: &mut Vec<dassie_settlement::Effect>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        self.stub(|stub| stub.handle_settlement(peer, amount, data, state, effects))
    }

    fn deposit(
        &mut self,
        amount: u64,
        effects: &mut Vec<dassie_settlement::Effect>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        self.stub(|stub| stub.deposit(amount, effects))
    }

    fn commitments(&mut self) -> Option<&mut dyn dassie_settlement::Commitments> {
        self.0.get_mut().commitments()?;
        Some(self)
    }

    fn check_invariants(&self) {
        self.stub(|stub| stub.check_invariants());
    }
}

impl dassie_settlement::Commitments for StubTwo {
    fn commit(
        &mut self,
        now_ms: u64,
        peer: dassie_ledger::Name,
        request_id: u32,
        promised: dassie_settlement::Promised,
        out: &mut Vec<u8>,
    ) -> Result<(), dassie_settlement::SchemeError> {
        count_stub_two_packet();
        self.with_inner(|inner| inner.commit(now_ms, peer, request_id, promised, out))
    }

    fn check_commitment(
        &mut self,
        now_ms: u64,
        peer: dassie_ledger::Name,
        request_id: u32,
        promised: dassie_settlement::Promised,
        commitment: &[u8],
    ) -> Result<(), dassie_settlement::SchemeError> {
        count_stub_two_packet();
        self.with_inner(|inner| {
            inner.check_commitment(now_ms, peer, request_id, promised, commitment)
        })
    }

    fn on_fulfilled(
        &mut self,
        now_ms: u64,
        peer: dassie_ledger::Name,
        request_id: u32,
        fulfillment: [u8; 32],
    ) {
        self.with_inner(|inner| inner.on_fulfilled(now_ms, peer, request_id, fulfillment));
    }

    fn on_released(&mut self, peer: dassie_ledger::Name, request_id: u32) {
        self.with_inner(|inner| inner.on_released(peer, request_id));
    }

    fn claim(
        &mut self,
        now_ms: u64,
        peer: dassie_ledger::Name,
        request_id: u32,
        taken: bool,
        out: &mut Vec<u8>,
    ) {
        self.with_inner(|inner| inner.claim(now_ms, peer, request_id, taken, out));
    }

    fn on_claim(&mut self, now_ms: u64, peer: dassie_ledger::Name, request_id: u32, claim: &[u8]) {
        self.with_inner(|inner| inner.on_claim(now_ms, peer, request_id, claim));
    }

    fn on_hub_wait(
        &mut self,
        now_ms: u64,
        peer: dassie_ledger::Name,
        request_id: u32,
        condition: [u8; 32],
        backstop_ms: u64,
    ) {
        self.with_inner(|inner| {
            inner.on_hub_wait(now_ms, peer, request_id, condition, backstop_ms);
        });
    }

    fn on_answered(&mut self, peer: dassie_ledger::Name, request_id: u32) {
        self.with_inner(|inner| inner.on_answered(peer, request_id));
    }

    fn take_decided(&mut self, out: &mut Vec<dassie_settlement::Decided>) {
        self.with_inner(|inner| inner.take_decided(out));
    }

    fn holds(&self, peer: dassie_ledger::Name) -> dassie_settlement::HoldsWith {
        self.with_inner(|inner| inner.holds(peer))
    }

    fn room_from(&self, peer: dassie_ledger::Name) -> Option<u64> {
        self.with_inner(|inner| inner.room_from(peer))
    }

    fn membership(&self) -> Option<dassie_settlement::Membership> {
        Some(dassie_settlement::Membership {
            hub: STUB_TWO_HUB,
            // An account, as the stub's: two members without one do not
            // peer over their hub.
            account: 1,
        })
    }

    fn held(&self, out: &mut Vec<(dassie_ledger::Name, dassie_settlement::Held)>) -> bool {
        self.with_inner(|inner| inner.held(out))
    }
}

/// The config both schemes on the XRPL account name: its server and its
/// faucet.
const SHARED_CONFIG: &str = r#"{"server":"ws://ledger.test","faucet":null}"#;

/// The ledger with the node's first XRPL testnet account funded, and the
/// account's address.
fn funded_ledger() -> (dassie_xrpl::fake::FakeLedger, dassie_xrpl::AccountId) {
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(TESTNET.id, 0);
    let account = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(false);
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
    add_xrpl(&mut core, "xrpl-testnet-xrp", config).unwrap();
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
// Its link-state checks go with direct settlement (spec/roadmap.md#hubs-only).
#[test]
fn a_scheme_activated_on_a_synced_account_is_told_it_has_synced() {
    use dassie_ledger::Name;
    use dassie_peer::wire::SchemeId;
    use dassie_settlement::LedgerStatus;

    let (mut ledger, _) = funded_ledger();
    let mut core = core(9);
    add_xrpl(&mut core, WITNESS, SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    assert_eq!(core.asset_views()[0].status, LedgerStatus::Synced);
    let before = core.assets[&dassie_xrpl::account::FIRST].epoch;
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    let outputs = follow_xrpl_outputs(&mut core, &mut ledger);
    assert!(closes(&outputs, before), "{outputs:?}");
    assert!(!connects(&outputs).is_empty(), "{outputs:?}");
    let xrpl = Name::new("xrpl-testnet-xrp").unwrap();
    assert!(core.schemes[&xrpl].scheme.has_synced());
    let view = core
        .ledger_views()
        .into_iter()
        .find(|view| view.scheme == "xrpl-testnet-xrp")
        .unwrap();
    assert_eq!(view.status, LedgerStatus::Synced);
    let id = SchemeId::parse("xrpl-testnet-xrp").unwrap();
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
    add_xrpl(&mut core, WITNESS, SHARED_CONFIG).unwrap();
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
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    let outputs = follow_xrpl_outputs(&mut core, &mut ledger);
    assert!(!connects(&outputs).is_empty(), "{outputs:?}");
    let xrpl = Name::new("xrpl-testnet-xrp").unwrap();
    assert!(core.schemes[&xrpl].scheme.has_synced());
    assert_eq!(core.asset_views()[0].status, LedgerStatus::Synced);
}

// spec/decisions/external-assets.md: the RLUSD scheme added beside the XRP
// scheme settles through the same account, mainnet's, in a node of the live
// realm (spec/decisions/mainnet-schemes.md). The account is connected anew
// and reads the line, which is set with a TrustSet whose fee is a ledger fee
// on `xrpl-xrp+xrp` and whose reserve is reconciled there, and the wallet sees
// both holdings as the ledger says, a freeze included.
// spec/areas/settlement.md#when-peers-cannot-pay-us: while the issuer froze
// the line deep, or froze its token for every holder (read at the
// settlement check), the link state leaves the scheme out; it stays synced.
// Its link-state checks go with direct settlement (spec/roadmap.md#hubs-only).
#[test]
#[allow(clippy::too_many_lines)]
fn rlusd_beside_xrp_on_one_account() {
    use dassie_ledger::{LedgerId, Reference};
    use dassie_node_core::config::LiveRealm;
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::Wallet;
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::{FAUCET_DROPS, FEE_BASE, RESERVE_BASE, RESERVE_INC};

    let config = r#"{"server":"ws://ledger.test"}"#;
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(MAINNET.id, 0);
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(true);
    // Mainnet has no faucet: the owner deposits.
    ledger.deposit(address, FAUCET_DROPS);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_in(Arc::clone(&store), 12, Realm::Live, LiveRealm::DevSwitch).unwrap();
    add_xrpl(&mut core, "xrpl-xrp", config).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    add_xrpl(&mut core, "xrpl-rlusd", config).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    let view = |core: &Core, ledger: LedgerId| {
        core.ledger_views()
            .into_iter()
            .find(|view| view.id == ledger)
            .unwrap()
    };
    assert_eq!(
        view(&core, LedgerId::XrplRlusdUsd).status,
        LedgerStatus::Synced
    );
    let xrp = view(&core, LedgerId::XrplXrp);
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
            .any(|scheme| scheme.as_str() == "xrpl-rlusd")
    };
    assert!(advertised(&core));

    ledger
        .issue(
            address,
            IssuedAsset::RLUSD.amount(TokenValue::parse("10").unwrap()),
        )
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    let usd = view(&core, LedgerId::XrplRlusdUsd);
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
    ledger
        .deep_freeze(address, IssuedAsset::RLUSD, true)
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    assert!(!advertised(&core), "advertised while it cannot receive");
    let frozen = freeze(&core);
    assert!(frozen.line && frozen.deep && !frozen.global, "{frozen:?}");
    assert_eq!(
        view(&core, LedgerId::XrplRlusdUsd).status,
        LedgerStatus::Synced
    );
    ledger.freeze(address, IssuedAsset::RLUSD, false).unwrap();
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

// spec/areas/xrpl.md#dsusd: the XRP and DSUSD schemes settle through the
// node's one XRPL testnet account: the token scheme sets its line, whose
// TrustSet's fee is a ledger fee on `xrpl-testnet-xrp+xrp` and whose reserve is
// held back there; it books only its own token, on a dollar ledger of its
// own; the wallet sees both holdings, and the link state offers both
// schemes. Test RLUSD, disabled, is refused beside them
// (spec/decisions/issued-assets.md), and has no ledger.
// Its link-state checks go with direct settlement (spec/roadmap.md#hubs-only).
#[test]
#[allow(clippy::too_many_lines)]
fn dsusd_beside_xrp_and_no_test_rlusd() {
    use dassie_ledger::{LedgerId, Reference};
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::Wallet;
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::{FAUCET_DROPS, FEE_BASE, RESERVE_BASE, RESERVE_INC};

    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(TESTNET.id, 0);
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(false);
    ledger.fund(address);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 12);
    for scheme in ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"] {
        add_xrpl(&mut core, scheme, config).unwrap();
        follow_xrpl(&mut core, &mut ledger);
    }
    assert!(matches!(
        add_xrpl(&mut core, dassie_xrpl::rlusd::TESTNET_SCHEME_ID, config),
        Err(super::EngineError::Precondition(_))
    ));
    close_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    let view = |core: &Core, ledger: LedgerId| {
        core.ledger_views()
            .into_iter()
            .find(|view| view.id == ledger)
            .unwrap()
    };
    for id in [LedgerId::XrplTestnetXrp, LedgerId::XrplTestnetDsusdUsd] {
        assert_eq!(view(&core, id).status, LedgerStatus::Synced, "{id}");
    }
    let units = |drops: u64| drops * 1_000;
    assert_eq!(
        view(&core, LedgerId::XrplTestnetXrp).on_ledger_balance,
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
    assert_eq!(views.len(), 1, "one account");
    assert_eq!(views[0].schemes.len(), 2);
    let mut codes: Vec<&str> = views[0]
        .holdings
        .iter()
        .map(|holding| holding.code.as_str())
        .collect();
    codes.sort_unstable();
    assert_eq!(codes, ["DSUSD", "XRP"]);
    let advertised: Vec<&str> = core.peer.schemes().iter().map(|s| s.as_str()).collect();
    for scheme in ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"] {
        assert!(advertised.contains(&scheme), "{scheme}: {advertised:?}");
    }
    assert_eq!(advertised.len(), 2);

    ledger
        .issue(
            address,
            IssuedAsset::DSUSD.amount(TokenValue::parse("10").unwrap()),
        )
        .unwrap();
    close_xrpl(&mut core, &mut ledger);
    assert_eq!(
        view(&core, LedgerId::XrplTestnetDsusdUsd).owner_balance,
        10_000_000_000
    );
    assert!(
        core.ledger_views()
            .iter()
            .all(|view| view.id != LedgerId::XrplTestnetRlusdUsd)
    );
}

// spec/decisions/issued-assets.md: RLUSD's peg guard, in the node. The node
// fetches XRP's rate for it beside its own ledgers', and hands it over at
// each settlement check. Once mainnet's pool has priced RLUSD off its peg
// for the trip hold, the price table leaves `xrpl-rlusd+usd` out, so nothing
// converts to or from it, the link state leaves the scheme out, and the
// owner sees the guard and is told; what arrives is still booked. Back at a
// dollar for the recovery hold, it comes back.
// Its link-state checks go with direct settlement (spec/roadmap.md#hubs-only).
#[test]
#[allow(clippy::too_many_lines)]
fn rlusd_off_its_peg_converts_nothing() {
    use dassie_ledger::LedgerId;
    use dassie_node_core::config::LiveRealm;
    use dassie_settlement::PegPause;
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger};
    use dassie_xrpl::peg::{RECOVER_HOLD_MS, TRIP_HOLD_MS};
    use dassie_xrpl::{DROPS_PER_XRP, Wallet};

    use super::events::{Event, SettlementEvent};

    let config = r#"{"server":"ws://ledger.test"}"#;
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(MAINNET.id, 0);
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(true);
    ledger.deposit(address, FAUCET_DROPS);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_in(store, 12, Realm::Live, LiveRealm::DevSwitch).unwrap();
    for scheme in ["xrpl-xrp", "xrpl-rlusd"] {
        add_xrpl(&mut core, scheme, config).unwrap();
        follow_xrpl(&mut core, &mut ledger);
    }
    assert_eq!(core.rate_currencies(), ["USD", "XRP"]);
    let token = |text: &str| TokenValue::parse(text).unwrap();
    ledger
        .issue(address, IssuedAsset::RLUSD.amount(token("10")))
        .unwrap();
    // A dollar buys two XRP: a pool of 200,000 XRP and 100,000 RLUSD
    // prices RLUSD at a dollar.
    let pool = |ledger: &mut FakeLedger, xrp: u64| {
        let drops = xrp * DROPS_PER_XRP;
        let value = token("100000");
        ledger
            .set_pool(IssuedAsset::RLUSD, drops, value, 205)
            .unwrap();
    };
    pool(&mut ledger, 200_000);
    let one = 10_u128.pow(dassie_connector::RATE_PRECISION);
    let mut now = NOW;
    // Minutes pass, a ledger closing in each, with the rates fresh: what
    // the steps put out.
    let mut minutes = |core: &mut Core, ledger: &mut FakeLedger, count: u64| {
        let mut outputs = Vec::new();
        for _ in 0..count {
            now += 60_000;
            core.begin_step(now);
            core.set_rates(&[("USD", one, now), ("XRP", 2 * one, now)]);
            let asset = ledger.network().first();
            let epoch = core.assets[&asset].epoch;
            let close_time = dassie_xrpl::unl::ledger_seconds(now);
            for (_, message) in ledger.close_at(close_time) {
                let event = super::LedgerEvent::Message(message);
                core.ledger_event(&asset.to_string(), epoch, &event);
            }
            outputs.extend(follow_xrpl_outputs(core, ledger));
        }
        outputs
    };
    let rlusd = LedgerId::XrplRlusdUsd;
    let priced = |core: &Core| core.price_table().ledger(rlusd).is_some();
    let advertised = |core: &Core| {
        core.peer
            .schemes()
            .iter()
            .any(|scheme| scheme.as_str() == "xrpl-rlusd")
    };
    let peg = |core: &Core| {
        let views = core.ledger_views();
        views
            .iter()
            .find(|view| view.id == rlusd)
            .unwrap()
            .peg
            .unwrap()
    };
    minutes(&mut core, &mut ledger, 2);
    assert_eq!(peg(&core).paused, None);
    assert_eq!(peg(&core).last.map(|(price, _)| price), Some(1_000_000));
    assert!(priced(&core) && advertised(&core));
    assert!(core.price_table().price(LedgerId::XrplXrp, rlusd).is_some());

    // 5% under a dollar: priced on until the hold is over.
    pool(&mut ledger, 190_000);
    minutes(&mut core, &mut ledger, TRIP_HOLD_MS / 60_000);
    assert!(priced(&core) && advertised(&core));
    let events = minutes(&mut core, &mut ledger, 1);
    assert_eq!(peg(&core).paused, Some(PegPause::OffPeg));
    assert_eq!(peg(&core).last.map(|(price, _)| price), Some(950_000));
    assert!(!priced(&core), "converts while off its peg");
    assert!(
        core.price_table().halted(rlusd),
        "converts at spot while off its peg"
    );
    assert!(core.price_table().ledger(LedgerId::XrplXrp).is_some());
    assert!(!advertised(&core), "in the link state while off its peg");
    let told = |events: &[Output], paused: bool| {
        events.iter().any(|output| match output {
            Output::Event(Event::Settlement(SettlementEvent::PegPaused { scheme, .. })) => {
                paused && scheme.as_str() == "xrpl-rlusd"
            }
            Output::Event(Event::Settlement(SettlementEvent::PegResumed { scheme })) => {
                !paused && scheme.as_str() == "xrpl-rlusd"
            }
            _ => false,
        })
    };
    assert!(told(&events, true));
    // What arrives is still booked.
    ledger
        .issue(address, IssuedAsset::RLUSD.amount(token("1")))
        .unwrap();
    minutes(&mut core, &mut ledger, 1);
    let view = core
        .ledger_views()
        .into_iter()
        .find(|v| v.id == rlusd)
        .unwrap();
    assert_eq!(view.owner_balance, 11_000_000_000);

    // Back at a dollar, for the recovery hold.
    pool(&mut ledger, 200_000);
    minutes(&mut core, &mut ledger, RECOVER_HOLD_MS / 60_000);
    assert!(!priced(&core));
    let events = minutes(&mut core, &mut ledger, 1);
    assert_eq!(peg(&core).paused, None);
    assert!(priced(&core) && advertised(&core));
    assert!(!core.price_table().halted(rlusd));
    assert!(told(&events, false));

    // Once the books stopped the scheme, its guard is fed nothing more and
    // can no longer tell a token off its peg: its ledger converts nothing.
    let at = core.now_ms + 1_000;
    core.begin_step(at);
    let scheme = dassie_ledger::Name::new("xrpl-rlusd").unwrap();
    core.stop_scheme(
        scheme,
        super::EngineError::Internal("stopped by the test".into()),
    );
    core.finish_step(&mut Vec::new());
    assert_eq!(peg(&core).paused, None);
    assert!(!priced(&core) && core.price_table().halted(rlusd));
    assert!(core.price_table().ledger(LedgerId::XrplXrp).is_some());
}

// On dev timers, as in dev and the end-to-end tests, the node runs RLUSD's
// peg guard on the same rule in seconds: a reading a second, and the trip
// hold ten of them (spec/areas/xrpl.md#the-peg-guard).
#[test]
fn on_dev_timers_the_peg_guard_trips_in_seconds() {
    use dassie_ledger::LedgerId;
    use dassie_node_core::config::LiveRealm;
    use dassie_settlement::PegPause;
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger};
    use dassie_xrpl::peg::Timings;
    use dassie_xrpl::{DROPS_PER_XRP, Wallet};

    let config = r#"{"server":"ws://ledger.test"}"#;
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(MAINNET.id, 0);
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(true);
    ledger.deposit(address, FAUCET_DROPS);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let live = (Realm::Live, LiveRealm::DevSwitch);
    let mut core = core_with(store, 12, live, Policies::DEV).unwrap();
    add_xrpl(&mut core, "xrpl-rlusd", config).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let value = TokenValue::parse("100000").unwrap();
    // 200,000 XRP for 100,000 RLUSD at two XRP to the dollar: a dollar.
    let pool = |ledger: &mut FakeLedger, xrp: u64| {
        let drops = xrp * DROPS_PER_XRP;
        ledger
            .set_pool(IssuedAsset::RLUSD, drops, value, 205)
            .unwrap();
    };
    pool(&mut ledger, 200_000);
    let one = 10_u128.pow(dassie_connector::RATE_PRECISION);
    let mut now = NOW;
    let mut seconds = |core: &mut Core, ledger: &mut FakeLedger, count: u64| {
        for _ in 0..count {
            now += Timings::DEV.read_every_ms;
            core.begin_step(now);
            core.set_rates(&[("USD", one, now), ("XRP", 2 * one, now)]);
            let asset = ledger.network().first();
            let epoch = core.assets[&asset].epoch;
            let close_time = dassie_xrpl::unl::ledger_seconds(now);
            for (_, message) in ledger.close_at(close_time) {
                let event = super::LedgerEvent::Message(message);
                core.ledger_event(&asset.to_string(), epoch, &event);
            }
            follow_xrpl_outputs(core, ledger);
        }
    };
    let peg = |core: &Core| {
        core.ledger_views()
            .into_iter()
            .find(|view| view.id == LedgerId::XrplRlusdUsd)
            .and_then(|view| view.peg)
            .unwrap()
    };
    seconds(&mut core, &mut ledger, 2);
    assert_eq!(peg(&core).last.map(|(price, _)| price), Some(1_000_000));
    pool(&mut ledger, 190_000);
    let hold = Timings::DEV.trip_hold_ms / Timings::DEV.read_every_ms;
    seconds(&mut core, &mut ledger, hold);
    assert_eq!(peg(&core).paused, None, "within the hold");
    seconds(&mut core, &mut ledger, 2);
    assert_eq!(peg(&core).paused, Some(PegPause::OffPeg));
    assert!(core.price_table().halted(LedgerId::XrplRlusdUsd));
}

// spec/decisions/issued-assets.md: peers settle only over hubs, so a dollar
// token converts on its hub's ledger, and the token's peg guard halts that
// ledger's conversions too. Without its owner book, the DSUSD hub's ledger
// converts nothing, since nothing can tell the token is on its peg; with
// the book running at a dollar, it converts; once the pool has priced DSUSD
// off its peg for the trip hold, the price table leaves the hub's ledger out
// and halts it, and the owner sees why on it; back at a dollar for the
// recovery hold, it comes back; once the books stopped the book, it
// converts nothing again. The XRP hub's ledger, whose book has no guard,
// converts throughout, its book never active.
#[test]
#[allow(clippy::too_many_lines)]
fn a_hub_ledger_converts_only_while_its_books_guard_runs() {
    use dassie_ledger::LedgerId;
    use dassie_settlement::{PegPause, PegView};
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::FakeLedger;
    use dassie_xrpl::peg::Timings;
    use dassie_xrpl::{DROPS_PER_XRP, Wallet};

    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let entropy = NodeKey::from_bytes(&[7; 32]).xrpl_account_entropy(TESTNET.id, 0);
    let address = Wallet::from_entropy(entropy.as_bytes()).account();
    let mut ledger = fake_ledger(false);
    ledger.fund(address);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let test = (Realm::Test, dassie_node_core::config::LiveRealm::Refused);
    let mut core = core_with(store, 12, test, Policies::DEV).unwrap();
    let one = 10_u128.pow(dassie_connector::RATE_PRECISION);
    let mut now = NOW;
    let mut seconds = |core: &mut Core, ledger: &mut FakeLedger, count: u64| {
        for _ in 0..count {
            now += Timings::DEV.read_every_ms;
            core.begin_step(now);
            core.set_rates(&[("USD", one, now), ("XRP", 2 * one, now)]);
            let asset = ledger.network().first();
            if let Some(account) = core.assets.get(&asset) {
                let epoch = account.epoch;
                let close_time = dassie_xrpl::unl::ledger_seconds(now);
                for (_, message) in ledger.close_at(close_time) {
                    let event = super::LedgerEvent::Message(message);
                    core.ledger_event(&asset.to_string(), epoch, &event);
                }
            }
            follow_xrpl_outputs(core, ledger);
        }
    };
    let dsusd_hub = LedgerId::HubXrplTestnetDsusdUsd;
    let xrp_hub = LedgerId::HubXrplTestnetXrp;
    let priced = |core: &Core, ledger: LedgerId| core.price_table().ledger(ledger).is_some();
    let halted = |core: &Core| !priced(core, dsusd_hub) && core.price_table().halted(dsusd_hub);
    let peg = |core: &Core, ledger: LedgerId| {
        core.ledger_views()
            .into_iter()
            .find(|view| view.id == ledger)
            .unwrap()
            .peg
    };
    let paused = |core: &Core| peg(core, dsusd_hub).and_then(|peg| peg.paused);

    // Without the DSUSD book, its hub's ledger converts nothing.
    for scheme in [DSUSD_HUB, XRP_HUB] {
        core.add_scheme(scheme, String::new()).unwrap();
    }
    seconds(&mut core, &mut ledger, 1);
    assert!(halted(&core), "converts without its book's guard");
    assert_eq!(
        peg(&core, dsusd_hub),
        Some(PegView {
            paused: Some(PegPause::Stale),
            last: None
        })
    );
    assert!(priced(&core, xrp_hub) && peg(&core, xrp_hub).is_none());

    // With the book at a dollar, it converts: 2,000 XRP for 1,000 DSUSD at
    // two XRP to the dollar.
    add_xrpl(&mut core, "xrpl-testnet-dsusd", config).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let value = TokenValue::parse("1000").unwrap();
    let pool = |ledger: &mut FakeLedger, xrp: u64| {
        let drops = xrp * DROPS_PER_XRP;
        ledger
            .set_pool(IssuedAsset::DSUSD, drops, value, 205)
            .unwrap();
    };
    pool(&mut ledger, 2_000);
    seconds(&mut core, &mut ledger, 2);
    let book = |core: &Core| peg(core, LedgerId::XrplTestnetDsusdUsd).unwrap();
    assert_eq!(book(&core).last.map(|(price, _)| price), Some(1_000_000));
    assert_eq!(peg(&core, dsusd_hub), Some(book(&core)));
    assert!(priced(&core, dsusd_hub) && !core.price_table().halted(dsusd_hub));

    // 5% under a dollar, past the trip hold: the book pauses, and its
    // hub's ledger converts nothing, while the XRP hub's still does.
    pool(&mut ledger, 1_900);
    let trip = Timings::DEV.trip_hold_ms / Timings::DEV.read_every_ms;
    seconds(&mut core, &mut ledger, trip);
    assert!(priced(&core, dsusd_hub), "paused within the hold");
    seconds(&mut core, &mut ledger, 2);
    assert_eq!(book(&core).paused, Some(PegPause::OffPeg));
    assert!(halted(&core), "converts while its token is off its peg");
    assert_eq!(paused(&core), Some(PegPause::OffPeg));
    assert_eq!(peg(&core, dsusd_hub), Some(book(&core)));
    assert!(priced(&core, xrp_hub));

    // Back at a dollar for the recovery hold, it converts again.
    pool(&mut ledger, 2_000);
    let recover = Timings::DEV.recover_hold_ms / Timings::DEV.read_every_ms;
    seconds(&mut core, &mut ledger, recover);
    assert!(halted(&core), "converts before the recovery hold is over");
    seconds(&mut core, &mut ledger, 2);
    assert_eq!(book(&core).paused, None);
    assert_eq!(paused(&core), None);
    assert!(priced(&core, dsusd_hub) && !core.price_table().halted(dsusd_hub));

    // Once the books stopped the book, its guard can no longer tell, and
    // the hub's ledger converts nothing, its last price still shown.
    let at = core.now_ms + 1_000;
    core.begin_step(at);
    let scheme = dassie_ledger::Name::new("xrpl-testnet-dsusd").unwrap();
    core.stop_scheme(
        scheme,
        super::EngineError::Internal("stopped by the test".into()),
    );
    core.finish_step(&mut Vec::new());
    assert!(halted(&core), "converts once its book stopped");
    assert_eq!(paused(&core), Some(PegPause::Stale));
    assert_eq!(
        peg(&core, dsusd_hub).and_then(|peg| peg.last),
        book(&core).last
    );
    assert!(priced(&core, xrp_hub));
}

/// A core node at `https://core.test`, which takes downlinks, on `store`,
/// of `realm` as `live` allows, started at `now`.
fn core_taking_downlinks(
    store: Arc<Store>,
    realm: Realm,
    live: dassie_node_core::config::LiveRealm,
    now: u64,
) -> Core {
    Core::start(
        CoreParts {
            store,
            key: Arc::new(NodeKey::from_bytes(&[7; 32])),
            realm,
            live,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(5)),
            peer_rng: Box::new(SeededRng::new(6)),
            own_info: dassie_peer::OwnInfo {
                url: Some("https://core.test".into()),
                alias: String::new(),
                feed_port: None,
                hub: None,
            },
            bootstrap: Vec::new(),
            uplinks: Vec::new(),
        },
        now,
    )
    .unwrap()
}

/// A core node of the live realm that takes downlinks, on RLUSD alone,
/// synced and holding $10 on `ledger`, mainnet.
fn rlusd_core(ledger: &mut dassie_xrpl::fake::FakeLedger) -> Core {
    rlusd_core_on(Arc::new(Store::open_in_memory().unwrap()), ledger, NOW)
}

/// [`rlusd_core`] on `store`, started at `now`.
fn rlusd_core_on(store: Arc<Store>, ledger: &mut dassie_xrpl::fake::FakeLedger, now: u64) -> Core {
    use dassie_ledger::Name;
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::Wallet;
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::fake::FAUCET_DROPS;

    let address = Wallet::from_entropy(
        NodeKey::from_bytes(&[7; 32])
            .xrpl_account_entropy(MAINNET.id, 0)
            .as_bytes(),
    )
    .account();
    ledger.deposit(address, FAUCET_DROPS);
    ledger.close();
    let live = dassie_node_core::config::LiveRealm::DevSwitch;
    let mut core = core_taking_downlinks(store, Realm::Live, live, now);
    let config = r#"{"server":"ws://ledger.test"}"#;
    add_xrpl(&mut core, "xrpl-rlusd", config).unwrap();
    follow_xrpl(&mut core, ledger);
    close_xrpl(&mut core, ledger);
    ledger
        .issue(
            address,
            IssuedAsset::RLUSD.amount(TokenValue::parse("10").unwrap()),
        )
        .unwrap();
    close_xrpl(&mut core, ledger);
    let rlusd = Name::new("xrpl-rlusd").unwrap();
    assert_eq!(core.schemes[&rlusd].scheme.status(), LedgerStatus::Synced);
    core
}

/// The downlink of key `[11; 32]` asks `core` to peer on `scheme`, a
/// scheme of the XRP Ledger on `ledger`, at `now`, and the peering is made.
fn xrpl_downlink_peers_on(
    core: &mut Core,
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    now: u64,
    scheme: &str,
) -> dassie_identity::NodeId {
    use dassie_xrpl::Wallet;

    let downlink = Arc::new(NodeKey::from_bytes(&[11; 32]));
    let id = downlink.node_id();
    let data = Wallet::from_entropy(&[11; 32]).account().0;
    core.begin_step(now);
    let body = downlink_request_on(&downlink, now, scheme, &data);
    core.peer_inbound(
        dassie_peer::InboundId(1),
        downlink.public_key(),
        &body,
        None,
    );
    follow_xrpl(core, ledger);
    let peering = core.peer.peering_of(&id).expect("peered");
    assert_eq!(peering.first().scheme.as_str(), scheme);
    id
}

/// A downlink that peers with `core` on RLUSD at `now`, and uses up the
/// dollar of credit we extend it: it owes us, and our room for it is gone.
fn rlusd_downlink_owing(
    core: &mut Core,
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    now: u64,
) -> dassie_identity::NodeId {
    use dassie_ledger::{AccountName, LedgerId, Name, Reference, TransferSpec};

    let id = xrpl_downlink_peers_on(core, ledger, now, "xrpl-rlusd");
    assert!(core.open_downlink_accounts(id, dassie_ledger::name!("xrpl-rlusd")));
    let name = Name::new(id.as_str()).unwrap();
    let on = LedgerId::XrplRlusdUsd;
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
// Goes with direct settlement: peering on the XRP Ledger (spec/roadmap.md#hubs-only).
#[test]
fn a_transfer_fee_asks_no_peer_and_ends_none_for_room() {
    use dassie_ledger::Name;
    use dassie_settlement::LedgerStatus;
    use dassie_xrpl::fake::FakeLedger;

    let mut ledger = fake_ledger(true);
    let mut core = rlusd_core(&mut ledger);
    let rlusd = Name::new("xrpl-rlusd").unwrap();
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
            .any(|scheme| scheme.as_str() == "xrpl-rlusd"),
        "in the link state during a transfer fee"
    );
    core.settle.asked.remove(&(id, scheme_id("xrpl-rlusd")));
    let short = core.peer.peering_of(&id).unwrap().first().short_for_ms;
    let reached = core.peer.peering_of(&id).unwrap().first().short_reached_ms;
    check(&mut core, &mut ledger, &mut now);
    check(&mut core, &mut ledger, &mut now);
    assert!(
        !core
            .settle
            .asked
            .contains_key(&(id, scheme_id("xrpl-rlusd"))),
        "asked during a transfer fee"
    );
    let peering = core.peer.peering_of(&id).unwrap();
    assert!(
        peering.first().short_for_ms > short,
        "short of room, toward withdrawing it"
    );
    assert_eq!(
        peering.first().short_reached_ms,
        reached,
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
        core.settle
            .asked
            .contains_key(&(id, scheme_id("xrpl-rlusd"))),
        "asked once the fee is gone"
    );
    assert!(core.peer.peering_of(&id).unwrap().first().short_reached_ms > reached);
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
    add_stub(&mut core, TRUSTING);
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
            live: dassie_node_core::config::LiveRealm::Refused,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(13)),
            peer_rng: Box::new(SeededRng::new(14)),
            own_info: dassie_peer::OwnInfo {
                url: None,
                alias: String::new(),
                feed_port: None,
                hub: None,
            },
            bootstrap: Vec::new(),
            uplinks: Vec::new(),
        },
        NOW,
    )
    .unwrap();
    add_stub(&mut core, TRUSTING);
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
    add_stub(&mut core, COMMITTING);
    let stub = Name::new("stub").unwrap();
    let peer = Name::new("peeraaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
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
pub(super) fn hub_config(margin: u64, allowance: u64) -> String {
    use dassie_oer::Encode;

    let hex: String = hub_rules(margin, allowance)
        .to_vec()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    serde_json::json!({ "rules": hex }).to_string()
}

/// The hub's rules: four validators, the nodes of keys 1 to 4, and an
/// escrow the sponsor of key 100 made.
fn hub_rules(margin: u64, allowance: u64) -> dassie_hub::Rules {
    use dassie_hub::escrow::{ValidatorKeys, escrow_master};
    use dassie_hub::ledger::TESTNET_NETWORK;
    use dassie_hub::rules::{Fees, Limits, Rules, Terms, Timers};
    use dassie_identity::NodeKey;

    let nonce = HUB_NONCE;
    let sponsor = NodeKey::from_bytes(&[100; 32]);
    let master = escrow_master(&sponsor, TESTNET_NETWORK, &nonce);
    let terms = Terms {
        version: dassie_hub::rules::HUB_VERSION,
        nonce,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow: master.account().0,
        asset: dassie_hub::rules::Asset::Xrp,
        fees: Fees {
            operation: 10,
            ledger: 100,
            gas: 0,
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
            margin,
            allowance,
        },
    };
    let validators = (1..=4u8)
        .map(|i| {
            let node = NodeKey::from_bytes(&[i; 32]);
            ValidatorKeys::derive(&node, TESTNET_NETWORK, &nonce)
                .unwrap()
                .accept(&node, &terms, format!("v{i}.example:443"))
                .unwrap()
        })
        .collect();
    Rules { terms, validators }
}

/// The nonce of the hub of [`hub_rules`].
const HUB_NONCE: [u8; 32] = [7; 32];

// A node joins a hub only when its legs between members fit in the
// connector's message window (spec/designs/settlement-hubs.md#4-peering-over-a-hub):
// a hub too far apart is refused, and nothing of it is kept. One that fits
// is joined, and its account asks the hub's validators for itself.
#[test]
fn a_hub_is_joined_only_if_it_fits_the_message_window() {
    let mut core = core(9);
    let refused = core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(1_000, 700));
    assert!(refused.is_err(), "1 s + 700 ms is past the 1.6 s window");
    // A margin below what finality takes with one of four validators
    // faulty voids the guarantee of a commitment: no member joins.
    let short = core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(500, 300));
    assert!(
        short.is_err(),
        "500 ms is below the 600 ms four validators need"
    );
    assert!(core.ledger_views().is_empty());
    core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(600, 300))
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
    let info = |id: &str| super::SCHEMES.iter().find(|info| info.id == id).unwrap();
    let test = dassie_ledger::Realm::Test;
    assert!(super::offered(info("stub"), test));
    assert!(super::offered(info(dassie_xrpl::scheme::SCHEME_ID), test));
    assert_eq!(
        super::offered(info(dassie_hub::member::SCHEME_ID), test),
        dassie_hub::member::DEFAULT_TESTNET_HUB.is_some()
    );
}

// spec/decisions/mainnet-schemes.md: a ledger runs only in its node's
// realm. A test-realm node activates no mainnet scheme, and a live-realm
// node, by the dev switch, no scheme of the test realm, the stub's
// included; it runs `xrpl-xrp`, at an address under `g.`.
#[test]
fn a_ledger_runs_only_in_its_nodes_realm() {
    use dassie_node_core::config::LiveRealm;

    let mainnet = dassie_xrpl::scheme::MAINNET_SCHEME_ID;
    let mut test = core(3);
    for id in [mainnet, dassie_xrpl::rlusd::SCHEME_ID] {
        assert!(
            matches!(
                test.add_scheme(id, String::new()),
                Err(super::EngineError::Precondition(_))
            ),
            "{id}"
        );
    }
    assert!(test.ledger_views().is_empty());
    assert!(test.assets.is_empty(), "the account was let go");
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut live = core_in(store, 3, Realm::Live, LiveRealm::DevSwitch).unwrap();
    assert!(live.own_address().starts_with("g.das."));
    for id in [
        "stub",
        dassie_xrpl::scheme::SCHEME_ID,
        dassie_xrpl::dsusd::SCHEME_ID,
        dassie_xrpl::rlusd::TESTNET_SCHEME_ID,
    ] {
        assert!(
            matches!(
                live.add_scheme(id, String::new()),
                Err(super::EngineError::Precondition(_))
            ),
            "{id}"
        );
    }
    let view = live.add_scheme(mainnet, String::new()).unwrap();
    assert_eq!(view.id, dassie_ledger::LedgerId::XrplXrp);
    assert!(!view.faucet, "mainnet has no faucet");
    assert_eq!(
        live.assets.keys().copied().collect::<Vec<_>>(),
        [dassie_xrpl::network::MAINNET.first()]
    );
}

// spec/decisions/mainnet-schemes.md: `live` stays refused where the
// realm is set, and at start: a node whose database names it starts only by
// the dev switch.
#[test]
fn a_stored_live_realm_is_refused_at_start() {
    use dassie_node_core::config::LiveRealm;

    let store = || Arc::new(Store::open_in_memory().unwrap());
    let refused = core_in(store(), 1, Realm::Live, LiveRealm::Refused);
    assert!(refused.err().unwrap().contains("live realm"));
    assert!(core_in(store(), 1, Realm::Live, LiveRealm::DevSwitch).is_ok());
    assert!(core_in(store(), 1, Realm::Test, LiveRealm::DevSwitch).is_ok());
}

// spec/decisions/mainnet-schemes.md: a node whose realm changed with a
// scheme of the other realm stored starts without it, and says so; the
// scheme is kept, and runs again once the realm is changed back.
#[test]
fn a_stored_scheme_of_another_realm_does_not_run() {
    use dassie_node_core::config::LiveRealm;

    use super::{Event, SettlementEvent};

    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut test = core_on(Arc::clone(&store), 4);
    add_stub(&mut test, TRUSTING);
    let mut outputs = Vec::new();
    test.finish_step(&mut outputs);
    drop(test);
    let mut live = core_in(Arc::clone(&store), 4, Realm::Live, LiveRealm::DevSwitch).unwrap();
    assert!(live.ledger_views().is_empty());
    let mut outputs = Vec::new();
    live.finish_step(&mut outputs);
    assert!(outputs.iter().any(|output| matches!(
        output,
        Output::Event(Event::Settlement(SettlementEvent::OtherRealm { scheme }))
            if scheme.as_str() == "stub"
    )));
    drop(live);
    let test = core_on(store, 4);
    assert_eq!(test.ledger_views().len(), 1);
}

// A node that stored a scheme whose config no longer makes one, as a hub
// that is no longer built in, starts with the rest and says so; the owner
// can remove the scheme, and its stored config goes.
#[test]
fn a_stored_scheme_whose_config_is_refused_does_not_run() {
    a_stored_hub_scheme_does_not_run("not a hub");
}

// The case this release makes: a node joined the default hub, whose config
// is empty, and no default hub is built in now, so the empty config names no
// hub. Once a default hub is built in again, the empty config names it.
#[test]
fn a_stored_hub_scheme_with_no_default_hub_does_not_run() {
    if dassie_hub::member::DEFAULT_TESTNET_HUB.is_some() {
        return;
    }
    a_stored_hub_scheme_does_not_run("");
}

/// Restores a node that stored the hub scheme with `config`, which this
/// version refuses, beside the stub, then removes the hub scheme.
fn a_stored_hub_scheme_does_not_run(config: &str) {
    use dassie_ledger::Name;

    use super::{Event, SettlementEvent};
    use crate::store::LedgerBatch;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let stub = Name::new("stub").unwrap();
    let hub = Name::new(dassie_hub::member::SCHEME_ID).unwrap();
    store
        .commit_ledger(&LedgerBatch {
            at_ms: NOW,
            schemes_added: vec![(stub, TRUSTING.into()), (hub, config.into())],
            ..LedgerBatch::default()
        })
        .unwrap();

    let mut core = core_on(Arc::clone(&store), 4);
    assert_eq!(core.ledger_views().len(), 1);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(outputs.iter().any(|output| matches!(
        output,
        Output::Event(Event::Settlement(SettlementEvent::Unusable { scheme, .. }))
            if *scheme == hub
    )));

    core.remove_scheme(hub.as_str()).unwrap();
    core.finish_step(&mut Vec::new());
    let stored: Vec<Name> = store
        .settlement_schemes()
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(stored, [stub]);
    assert!(core.remove_scheme(hub.as_str()).is_err());
}

// A scheme left out at start is added again with a config that works: its
// stored config is replaced, and it runs after a restart.
#[test]
fn a_stored_scheme_left_out_is_added_again() {
    use dassie_ledger::Name;

    use crate::store::LedgerBatch;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let stub = Name::new("stub").unwrap();
    store
        .commit_ledger(&LedgerBatch {
            at_ms: NOW,
            schemes_added: vec![(stub, "not a stub".into())],
            ..LedgerBatch::default()
        })
        .unwrap();

    let mut core = core_on(Arc::clone(&store), 4);
    assert!(core.ledger_views().is_empty());
    add_stub(&mut core, TRUSTING);
    core.finish_step(&mut Vec::new());
    assert_eq!(
        store.settlement_schemes().unwrap(),
        [(stub, TRUSTING.into())]
    );
    drop(core);
    assert_eq!(core_on(store, 4).ledger_views().len(), 1);
}

// spec/decisions/issued-assets.md: a node that ran test RLUSD starts
// without it, and says so. The scheme, what the books kept for it and its
// books on `xrpl-testnet-rlusd+usd` stay as they were, nothing is written
// off, and the testnet's XRP scheme on the same account runs.
#[test]
#[allow(clippy::too_many_lines)]
fn a_stored_test_rlusd_scheme_does_not_run() {
    use dassie_ledger::{AccountName, AccountPath, LedgerId, Reference, TransferSpec};

    use super::{Event, SettlementEvent};
    use crate::store::{Checkpoint, LedgerBatch};

    // What a node that ran test RLUSD left: the scheme, its checkpoint, and
    // the owner's deposit of 7 dollars on its ledger.
    let store = Arc::new(Store::open_in_memory().unwrap());
    let rlusd = dassie_ledger::Name::new(dassie_xrpl::rlusd::TESTNET_SCHEME_ID).unwrap();
    let on = LedgerId::XrplTestnetRlusdUsd;
    let dollars = 7_000_000_000;
    let mut books = dassie_ledger::Ledger::new(Policies::DEFAULT.ledger);
    let mut open = |name| books.open(AccountPath::new(on, name)).unwrap();
    let (settlement, owner) = (
        open(AccountName::AssetsSettlement),
        open(AccountName::EquityOwner),
    );
    let deposit = TransferSpec {
        debit: settlement,
        credit: owner,
        amount: dollars,
        pending: false,
        reference: Reference::Deposit,
    };
    books.create(NOW, &deposit).unwrap();
    let journal: Vec<_> = std::iter::from_fn(|| books.poll_journal()).collect();
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    let checkpoint = Checkpoint {
        balance: dollars,
        state: b"{}".to_vec(),
    };
    store
        .commit_ledger(&LedgerBatch {
            at_ms: NOW,
            journal,
            schemes_added: vec![(rlusd, config.into())],
            checkpoints: vec![(rlusd, checkpoint.clone())],
            ..LedgerBatch::default()
        })
        .unwrap();
    let held = |core: &Core| {
        let owner = core.account(on, AccountName::EquityOwner).unwrap();
        core.ledger.account(owner).unwrap().balance().net_credit()
    };

    let mut core = core_on(Arc::clone(&store), 4);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(outputs.iter().any(|output| matches!(
        output,
        Output::Event(Event::Settlement(SettlementEvent::Disabled { scheme }))
            if *scheme == rlusd
    )));
    assert!(core.ledger_views().is_empty());
    assert!(core.assets.is_empty(), "no account made for it");
    assert_eq!(held(&core), i128::from(dollars));
    let view = add_xrpl(&mut core, dassie_xrpl::scheme::SCHEME_ID, config).unwrap();
    assert_eq!(view.id, LedgerId::XrplTestnetXrp);
    core.flush();
    drop(core);
    let core = core_on(Arc::clone(&store), 4);
    assert_eq!(held(&core), i128::from(dollars));
    drop(core);
    assert_eq!(
        store.settlement_schemes().unwrap(),
        [
            (rlusd, config.into()),
            (
                dassie_ledger::Name::new("xrpl-testnet-xrp").unwrap(),
                String::new()
            ),
        ]
    );
    assert!(store.checkpoints().unwrap().contains(&(rlusd, checkpoint)));
    let transfers = store.transfers_after(None, 100, None).unwrap();
    assert_eq!(transfers.len(), 1, "the deposit alone: nothing written off");
}

/// A test node on `store` that takes downlinks, on the stub of `config`,
/// with the downlink of key `[11; 32]` peered on it at `NOW + 1_000`, asking
/// with the peering info the stub writes, and all of it written: the node,
/// and the downlink's ID.
fn stub_core_with_downlink_on(store: Arc<Store>, config: &str) -> (Core, dassie_identity::NodeId) {
    let refused = dassie_node_core::config::LiveRealm::Refused;
    let mut core = core_taking_downlinks(store, Realm::Test, refused, NOW);
    add_stub(&mut core, config);
    let downlink = Arc::new(NodeKey::from_bytes(&[11; 32]));
    let at = NOW + 1_000;
    core.begin_step(at);
    let body = stub_downlink_request(&mut core, &downlink, at);
    core.peer_inbound(
        dassie_peer::InboundId(1),
        downlink.public_key(),
        &body,
        None,
    );
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let id = downlink.node_id();
    assert_eq!(
        core.peer.peering_of(&id).unwrap().first().scheme.as_str(),
        "stub"
    );
    core.flush();
    (core, id)
}

/// Steps `core` a second at a time from `from`, until its peering with
/// `peer`, on a scheme it does not run, is over, and returns when. It is
/// ended at once, collecting nothing: no request to settle goes to the peer
/// (spec/areas/settlement.md#schemes-and-ledgers). The peer here never
/// answers, so the end is told until the termination times out.
fn until_unrun_ended(core: &mut Core, peer: &dassie_identity::NodeId, from: u64) -> u64 {
    let config = Policies::DEFAULT.peer;
    let asked = core.peer.stats().settlements_requested;
    let mut outputs = Vec::new();
    core.begin_step(from);
    core.finish_step(&mut outputs);
    assert!(
        core.peer.active_peers().all(|(id, _)| id != peer),
        "ended at the first review"
    );
    let most = from + config.termination_timeout_ms + config.heartbeat_min_ms;
    let mut now = from;
    while core.peer.peering_of(peer).is_some() {
        assert!(now < most, "the end outlived its termination");
        now += 1_000;
        core.begin_step(now);
        core.finish_step(&mut outputs);
    }
    assert_eq!(
        core.peer.stats().settlements_requested,
        asked,
        "nothing collected"
    );
    now
}

// spec/areas/settlement.md#schemes-and-ledgers: a node that peered on test
// RLUSD, whose scheme is disabled, starts without the scheme and ends the
// peering, collecting nothing: no settlement on it could be booked. The
// peer's accounts on the scheme's ledger stay as they were. The pair is not
// kept apart: the same peer then peers on DSUSD, which both run.
// Goes with direct settlement: peering on the XRP Ledger (spec/roadmap.md#hubs-only).
#[test]
#[allow(clippy::too_many_lines)]
fn a_peering_on_a_disabled_scheme_ends_and_the_pair_peers_on_another() {
    use dassie_ledger::{AccountName, LedgerId, Name};
    use dassie_xrpl::Wallet;
    use dassie_xrpl::fake::FakeLedger;

    use crate::store::LedgerBatch;

    // What a node that peered with a downlink on test RLUSD left: the
    // scheme, the peering, and the peer's accounts on the scheme's ledger,
    // with the credit extended. Made over the stub, then moved to test
    // RLUSD, which can no longer be added.
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, id) = stub_core_with_downlink_on(Arc::clone(&store), TRUSTING);
    let name = Name::new(id.as_str()).unwrap();
    let on = LedgerId::XrplTestnetRlusdUsd;
    let at = core.now_ms();
    core.ledger
        .open_peer(at, on, name, Policies::DEFAULT.peer_credit)
        .unwrap();
    core.refresh_waiting(id);
    core.flush();
    drop(core);
    let rlusd = dassie_xrpl::rlusd::TESTNET_SCHEME_ID;
    let mut node = store
        .load_peer()
        .unwrap()
        .nodes
        .into_iter()
        .find(|node| node.id == id)
        .unwrap();
    node.peering.as_mut().unwrap().methods[0].scheme =
        dassie_peer::wire::SchemeId::parse(rlusd).unwrap();
    let config = r#"{"server":"ws://ledger.test","faucet":null}"#;
    store
        .commit_ledger(&LedgerBatch {
            at_ms: at,
            schemes_removed: vec![Name::new("stub").unwrap()],
            schemes_added: vec![(Name::new(rlusd).unwrap(), config.into())],
            peer: dassie_peer::PeerBatch {
                nodes: vec![node],
                ..dassie_peer::PeerBatch::default()
            },
            ..LedgerBatch::default()
        })
        .unwrap();
    let books = |core: &Core| {
        [
            AccountName::AssetsInterledger(name),
            AccountName::ContraTrust(name),
        ]
        .map(|account| {
            let account = core.account(on, account).unwrap();
            core.ledger.account(account).unwrap().balance().net_credit()
        })
    };

    let refused = dassie_node_core::config::LiveRealm::Refused;
    let start = at + 1_000;
    let mut core = core_taking_downlinks(Arc::clone(&store), Realm::Test, refused, start);
    assert!(core.schemes.is_empty(), "test RLUSD does not run");
    assert_eq!(
        core.peer.peering_of(&id).unwrap().first().scheme.as_str(),
        rlusd,
        "its peering is loaded"
    );
    let before = books(&core);
    assert_ne!(before, [0, 0]);
    let ended = until_unrun_ended(&mut core, &id, start);
    assert_eq!(books(&core), before, "nothing written off or settled");

    let address = Wallet::from_entropy(
        NodeKey::from_bytes(&[7; 32])
            .xrpl_account_entropy(TESTNET.id, 0)
            .as_bytes(),
    )
    .account();
    let mut ledger = FakeLedger::new();
    ledger.fund(address);
    ledger.close();
    for scheme in ["xrpl-testnet-xrp", "xrpl-testnet-dsusd"] {
        add_xrpl(&mut core, scheme, config).unwrap();
        follow_xrpl(&mut core, &mut ledger);
    }
    close_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    xrpl_downlink_peers_on(&mut core, &mut ledger, ended + 1_000, "xrpl-testnet-dsusd");
}

// spec/areas/settlement.md#schemes-and-ledgers: so is a peering on a scheme
// of the other realm, once the node's realm changed: here a test node's
// peering on the stub, when the node starts in the live realm by the dev
// switch. The same peer then peers on RLUSD, mainnet's.
// Goes with direct settlement: peering on the XRP Ledger (spec/roadmap.md#hubs-only).
#[test]
fn a_peering_on_a_scheme_of_another_realm_ends_and_the_pair_peers_on_another() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (test, id) = stub_core_with_downlink_on(Arc::clone(&store), TRUSTING);
    let start = test.now_ms() + 1_000;
    drop(test);
    let mut ledger = dassie_xrpl::fake::FakeLedger::mainnet();
    let mut live = rlusd_core_on(Arc::clone(&store), &mut ledger, start);
    assert!(
        live.peer.peering_of(&id).is_some(),
        "its peering is loaded, and ending"
    );
    let ended = until_unrun_ended(&mut live, &id, start);
    xrpl_downlink_peers_on(&mut live, &mut ledger, ended + 1_000, "xrpl-rlusd");
}

/// What the books hold on `ledger`: its accounts with their balances, and
/// the transfers pending on it.
fn books_on(
    core: &Core,
    ledger: dassie_ledger::LedgerId,
) -> (
    Vec<(dassie_ledger::AccountPath, dassie_ledger::Balance)>,
    Vec<dassie_ledger::Transfer>,
) {
    let accounts = core
        .account_list()
        .into_iter()
        .filter(|(_, account)| account.path().ledger == ledger)
        .map(|(_, account)| (*account.path(), *account.balance()))
        .collect();
    let pending = core
        .ledger
        .pending_transfers()
        .filter(|transfer| {
            core.ledger
                .account(transfer.debit)
                .is_some_and(|account| account.path().ledger == ledger)
        })
        .copied()
        .collect();
    (accounts, pending)
}

// spec/decisions/mainnet-schemes.md: a test node that peered with a
// downlink on the stub, with a BTP token on the stub's ledger and a
// settlement to the downlink pending, starts in the live realm by the dev
// switch. The stub does not run: the peering ends as one on a scheme the
// node no longer runs, collecting nothing
// (spec/areas/settlement.md#schemes-and-ledgers); the settlement stays
// pending, the token stays but connects no client, and the books on the
// stub's ledger stay as they are, in memory and on disk. Back in the test
// realm, the stub runs again, the token connects, and the stub reconciles
// the settlement, which it never executed.
#[test]
#[allow(clippy::too_many_lines)]
fn a_stored_scheme_of_another_realm_keeps_what_is_pending_on_its_ledger() {
    use dassie_ledger::{AccountName, LedgerId, Name, Reference, TransferSpec};
    use dassie_node_core::config::LiveRealm;

    let on = LedgerId::StubUsd;
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut test, id) = stub_core_with_downlink_on(Arc::clone(&store), COMMITTING);
    assert!(test.open_downlink_accounts(id, dassie_ledger::name!("stub")));
    let token = test
        .create_token("wallet".into(), "stub+usd", None)
        .unwrap();
    let peer = Name::new(id.as_str()).unwrap();
    // The books take a settlement to it as one over a hub: the credit we
    // extend it is raised first.
    let now = test.now_ms();
    super::trust::raise_trust(&mut test.ledger, now, on, peer, 400).unwrap();
    let pending = TransferSpec {
        debit: test
            .account(on, AccountName::AssetsInterledger(peer))
            .unwrap(),
        credit: test.account(on, AccountName::AssetsSettlement).unwrap(),
        amount: 400,
        pending: true,
        reference: Reference::Settlement {
            scheme: Name::new("stub").unwrap(),
            id: Name::new("0123456789abcdef0123456789abcdef").unwrap(),
        },
    };
    test.ledger.create(now, &pending).unwrap();
    // On disk, and then the realm changes: never executed.
    test.flush();
    let before = books_on(&test, on);
    assert_eq!(before.1.len(), 1);
    let start = test.now_ms() + 1_000;
    drop(test);
    let written = store.transfers_after(None, 1_000, None).unwrap().len();

    let mut live =
        core_taking_downlinks(Arc::clone(&store), Realm::Live, LiveRealm::DevSwitch, start);
    assert!(live.schemes.is_empty(), "the stub does not run");
    assert!(live.ledger_views().is_empty());
    assert_eq!(books_on(&live, on), before);
    assert_eq!(
        live.token_views()
            .iter()
            .map(|view| (view.id, view.ledger))
            .collect::<Vec<_>>(),
        [(token.view.id, on)],
        "the token stays, for the owner to see and revoke"
    );
    // Its clients are refused: their packets to each other would move the
    // books of a ledger the node does not run.
    assert!(matches!(
        live.btp_connect(&token.secret),
        Err(super::EngineError::Precondition(_))
    ));
    let ended = until_unrun_ended(&mut live, &id, start);
    assert_eq!(
        books_on(&live, on),
        before,
        "nothing settled or written off"
    );
    live.flush();
    drop(live);
    assert_eq!(store.load_ledger().unwrap().pending.len(), 1);
    assert_eq!(
        store.transfers_after(None, 1_000, None).unwrap().len(),
        written,
        "nothing booked"
    );

    let refused = LiveRealm::Refused;
    let mut back = core_taking_downlinks(Arc::clone(&store), Realm::Test, refused, ended + 1_000);
    assert!(
        back.btp_connect(&token.secret).is_ok(),
        "the token connects again"
    );
    assert_eq!(back.active_schemes(), ["stub"]);
    let (accounts, pending) = books_on(&back, on);
    assert!(pending.is_empty(), "the stub voided what it never executed");
    let posted = |accounts: &[(dassie_ledger::AccountPath, dassie_ledger::Balance)]| {
        accounts
            .iter()
            .map(|(path, balance)| (*path, balance.debits_posted, balance.credits_posted))
            .collect::<Vec<_>>()
    };
    assert_eq!(posted(&accounts), posted(&before.0));
}

/// A core node at `https://core.test`, over the stub with commitments,
/// with a downlink peered on it: the downlink's ID.
fn hub_core_with_downlink(seed: u64) -> (Core, dassie_identity::NodeId) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = Core::start(
        CoreParts {
            store,
            key: Arc::new(NodeKey::from_bytes(&[7; 32])),
            realm: Realm::Test,
            live: dassie_node_core::config::LiveRealm::Refused,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(seed)),
            peer_rng: Box::new(SeededRng::new(seed + 1)),
            own_info: dassie_peer::OwnInfo {
                url: Some("https://core.test".into()),
                alias: String::new(),
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
    let body = downlink_request(&downlink, at);
    core.peer_inbound(
        dassie_peer::InboundId(1),
        downlink.public_key(),
        &body,
        None,
    );
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
    let (credit, _, settling) = core.hub_books(stub, peer).unwrap_or_default();
    (credit, toward, settling)
}

// spec/areas/hubs.md#in-the-node: asked to settle over a hub, a payer funds
// its hold toward the peer with what it would otherwise have paid, as if it
// extended the default credit, though it extends none:
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

// spec/designs/settlement-hubs.md#4-peering-over-a-hub: a peer over a hub
// can send us only what its hold toward us has room for, whatever we owe it
// the other way. Our books, which net both ways, show room enough while we
// owe it, yet its hold has no room: we ask it to settle all the same.
#[test]
fn over_a_hub_a_payee_asks_by_the_room_of_the_hold_toward_it() {
    use dassie_ledger::{AccountName, LedgerId, Reference, TransferSpec};

    let (mut core, peer) = hub_core_with_downlink(34);
    let stub = dassie_ledger::name!("stub");
    assert!(core.open_downlink_accounts(peer, stub));
    let name = dassie_ledger::Name::new(peer.as_str()).unwrap();
    // Our books owe the peer well above our trigger.
    let interledger = core
        .account(LedgerId::StubUsd, AccountName::AssetsInterledger(name))
        .unwrap();
    let pool = core
        .account(LedgerId::StubUsd, AccountName::EquityOwner)
        .unwrap();
    let owed = core.settle.trigger.saturating_mul(4);
    core.begin_step(NOW + 2_000);
    core.ledger
        .create(
            NOW + 2_000,
            &TransferSpec {
                debit: pool,
                credit: interledger,
                amount: owed,
                pending: false,
                reference: Reference::Deposit,
            },
        )
        .unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let scheme = dassie_peer::wire::SchemeId::parse("stub").unwrap();
    assert!(
        core.owed(peer, scheme).unwrap() > 0,
        "the books owe the peer"
    );
    // The peer's hold toward us has no room.
    assert_eq!(
        core.with_commitments(stub, |c| c.room_from(name)),
        Some(Some(0))
    );
    core.begin_step(NOW + 3_000);
    core.settle.at_ms = 0;
    core.settlement_timers();
    outputs.clear();
    core.finish_step(&mut outputs);
    assert!(
        core.settle.asked.contains_key(&(peer, scheme)),
        "the peer was not asked"
    );
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

/// The node's hub account, as `hub-testnet-account/0`.
const HUB_ACCOUNT: &str = "hub-testnet-account/0";

/// The hub of [`hub_rules`] as its validators speak for it: each signs
/// alone with its consensus key.
struct Validators {
    rules: dassie_hub::Rules,
    keys: Vec<dassie_identity::NodeKey>,
    /// The node's key in the hub.
    member: dassie_identity::PublicKey,
    height: u64,
}

impl Validators {
    fn new() -> Self {
        let rules = hub_rules(600, 300);
        let keys = (1..=4u8)
            .map(|i| {
                let node = NodeKey::from_bytes(&[i; 32]);
                dassie_hub::escrow::ValidatorKeys::derive(&node, rules.terms.network, &HUB_NONCE)
                    .unwrap()
                    .consensus
            })
            .collect();
        let member = dassie_hub::member::member_key(&NodeKey::from_bytes(&[7; 32]), &rules.id())
            .public_key();
        Self {
            rules,
            keys,
            member,
            height: 1,
        }
    }

    /// Every validator says `statements`, each from its own endpoint.
    fn say(&self, core: &mut Core, statements: &[dassie_hub::client::Statement]) {
        use dassie_hub::client::SignedStatement;
        use dassie_hub::validators::ValidatorIndex;

        let epoch = core.assets[&dassie_hub::member::FIRST].epoch;
        for (i, key) in self.keys.iter().enumerate() {
            let signed: Vec<SignedStatement> = statements
                .iter()
                .map(|statement| {
                    SignedStatement::sign(
                        &self.rules.id(),
                        ValidatorIndex::new(i).unwrap(),
                        statement.clone(),
                        key,
                    )
                })
                .collect();
            let message =
                dassie_oer::Encode::to_vec(&dassie_hub::service::wire::Message::Statements(signed));
            core.ledger_event(
                HUB_ACCOUNT,
                epoch,
                &super::LedgerEvent::From {
                    endpoint: i,
                    message,
                },
            );
        }
    }

    /// The hub has no account for the node.
    fn no_account(&mut self, core: &mut Core) {
        use dassie_hub::client::Statement;

        self.height += 1;
        let statements = [Statement::Account {
            height: self.height,
            key: self.member,
            account: None,
        }];
        self.say(core, &statements);
    }

    /// The node's account 5 holds `balance` drops, and deposits credited it
    /// `deposited`, as of a new final block.
    fn account(&mut self, core: &mut Core, balance: u64, deposited: u64) {
        self.account_with(core, balance, deposited, 0);
    }

    /// As [`account`](Self::account), with `returned` brought back by
    /// withdrawals the escrow could not pay.
    fn account_with(&mut self, core: &mut Core, balance: u64, deposited: u64, returned: u64) {
        use dassie_hub::client::{AccountView, Statement};
        use dassie_hub::operation::AccountNumber;

        self.height += 1;
        let view = AccountView {
            key: self.member,
            balance,
            reserve: self.rules.terms.fees.account_reserve,
            sequence: 1,
            disbursal: true,
            closing: false,
            deposited,
            withdrawn: 0,
            returned,
        };
        let statements = [
            Statement::Account {
                height: self.height,
                key: self.member,
                account: Some((AccountNumber(5), view)),
            },
            Statement::Final {
                height: self.height,
                timestamp: NOW + self.height,
            },
        ];
        self.say(core, &statements);
    }

    /// The node's account 5 opened a hold of `amount` toward account 9, in
    /// a new final block, after which it holds `balance`.
    fn opened_hold(&mut self, core: &mut Core, amount: u64, balance: u64, deposited: u64) {
        use dassie_hub::client::{AccountView, Statement};
        use dassie_hub::ledger::{Detail, Outcome};
        use dassie_hub::operation::{
            AccountNumber, MemberOperation, Operation, SignedMemberOperation,
        };
        use dassie_hub::promise::HoldNumber;

        self.height += 1;
        let key = dassie_hub::member::member_key(&NodeKey::from_bytes(&[7; 32]), &self.rules.id());
        let fees = self.rules.terms.fees;
        let operation = SignedMemberOperation::sign(
            &self.rules.id(),
            AccountNumber(5),
            1,
            fees.operation,
            MemberOperation::HoldOpen {
                payee: AccountNumber(9),
                amount,
            },
            &key,
        );
        let view = AccountView {
            key: self.member,
            balance,
            reserve: fees.account_reserve,
            sequence: 2,
            disbursal: true,
            closing: false,
            deposited,
            withdrawn: 0,
            returned: 0,
        };
        let statements = [
            Statement::Executed {
                height: self.height,
                timestamp: NOW + self.height,
                index: 0,
                operation: Operation::Member(operation),
                outcome: Outcome::Applied {
                    fee: fees.operation,
                    detail: Detail::Hold(HoldNumber(1)),
                },
            },
            Statement::Account {
                height: self.height,
                key: self.member,
                account: Some((AccountNumber(5), view)),
            },
            Statement::Final {
                height: self.height,
                timestamp: NOW + self.height,
            },
        ];
        self.say(core, &statements);
    }

    fn escrow(&self) -> dassie_xrpl::AccountId {
        dassie_xrpl::AccountId(self.rules.terms.escrow)
    }
}

/// A core with the XRP Ledger testnet scheme synced on `ledger`, and the
/// hub of [`hub_rules`].
fn funding_core(ledger: &mut dassie_xrpl::fake::FakeLedger) -> Core {
    let mut core = core(9);
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(600, 300))
        .unwrap();
    follow_xrpl(&mut core, ledger);
    core
}

/// Moves the core's time on by `ms`, then by a few ledgers' closes, each
/// a funding check: the XRP Ledger account, silent for long, connects
/// again and syncs first. What was submitted is validated.
fn wait_and_close(
    core: &mut Core,
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    ms: u64,
) -> Vec<Output> {
    let mut outputs = Vec::new();
    for step in [ms, 4_000, 4_000] {
        let now = core.now_ms() + step;
        core.begin_step(now);
        // Ledgers closed meanwhile, as they do every few seconds: the
        // account, connected anew, anchors on a fresh one.
        if step > 4_000 {
            ledger.close_at(dassie_xrpl::unl::ledger_seconds(now));
        }
        outputs.extend(follow_xrpl_outputs(core, ledger));
        close_xrpl(core, ledger);
    }
    outputs
}

/// Terms of an XRP hub whose escrow is `escrow`: all a validator reads a
/// deposit to it by.
fn xrp_hub_terms(escrow: dassie_xrpl::AccountId) -> dassie_hub::rules::Terms {
    let sponsor = dassie_identity::NodeKey::from_bytes(&[100; 32]).public_key();
    dassie_hub::founding::starting_terms([0; 32], sponsor, 1, escrow.0)
}

/// The deposits the node made to the escrow, as the validators read them.
fn deposits(
    ledger: &mut dassie_xrpl::fake::FakeLedger,
    escrow: dassie_xrpl::AccountId,
) -> Vec<dassie_hub::operation::Deposit> {
    use dassie_xrpl::api;

    let reply = ledger.request(77, &api::account_tx(5, escrow, 0, u32::MAX, None));
    let outcome = match api::parse(&reply) {
        api::Message::Response { outcome, .. } => Some(outcome),
        _ => None,
    };
    let records = api::account_tx_result(&outcome.unwrap().unwrap()).unwrap();
    records
        .transactions
        .iter()
        .filter_map(|record| dassie_hub::escrow::deposit_of(record, &xrp_hub_terms(escrow)))
        .collect()
}

/// The withdrawals the node submitted to the validators.
fn withdrawals(outputs: &[Output]) -> Vec<(u64, dassie_hub::operation::Destination)> {
    use dassie_hub::client::Request;
    use dassie_hub::operation::{MemberOperation, Operation};
    use dassie_hub::service::wire::Message;

    let mut seen = Vec::new();
    for output in outputs {
        let Output::HubSend { message, .. } = output else {
            continue;
        };
        if let Ok(Message::Request(Request::Submit(Operation::Member(signed)))) =
            Message::decode_exact(message)
            && let MemberOperation::Withdraw { amount, to } = signed.operation
            && !seen.contains(&(amount, to))
        {
            seen.push((amount, to));
        }
    }
    seen
}

// spec/areas/hubs.md#in-the-node: a node in a hub that has no account for
// it opens one by its first deposit (tag 0, its hub key as the memo, the
// account reserve, the floor and the fees of the first operations). Below
// the floor, it deposits up to
// the midpoint, by its account's tag, one move at a time: the next waits
// until the hub credited it. Above the ceiling, it withdraws down to the
// midpoint, to its own XRPL account.
#[test]
fn the_node_keeps_its_hub_between_floor_and_ceiling() {
    use dassie_settlement::policy;

    let (mut ledger, own) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    let fees = hub.rules.terms.fees;
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let credit = Policies::DEFAULT.peer_credit;
    let floor = policy::hub_floor(p, credit, 0) / dassie_xrpl::UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / dassie_xrpl::UNITS_PER_DROP;
    let middle = floor.midpoint(ceiling);
    assert_eq!((floor, ceiling), (475_000, 10_000_000));

    wait_and_close(&mut core, &mut ledger, 1_000);
    let made = deposits(&mut ledger, escrow);
    let opening = fees.account_reserve + floor + fees.starting_balance;
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(
        (made[0].tag, made[0].key, made[0].amount, made[0].sender),
        (0, Some(hub.member), opening, own.0)
    );

    // Credited, and spent below the floor: a deposit by tag, one at a time.
    hub.account(&mut core, floor + 1_000, opening);
    hub.account(&mut core, 100_000, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 2, "one on its way: {made:?}");
    assert_eq!((made[1].tag, made[1].key), (5, None));
    assert_eq!(made[1].amount, middle - 100_000);

    // Credited, then peers' settlements taken past the ceiling: withdrawn
    // to the midpoint, counting the account reserve.
    hub.account(&mut core, middle, opening + made[1].amount);
    let balance = 12_000_000;
    hub.account(&mut core, balance, opening + made[1].amount);
    let outputs = wait_and_close(&mut core, &mut ledger, 4_000);
    let exposure = balance + fees.account_reserve;
    let to = dassie_hub::operation::Destination {
        account: own.0,
        tag: None,
    };
    assert_eq!(withdrawals(&outputs), [(exposure - middle, to)]);
    assert_eq!(deposits(&mut ledger, escrow).len(), 2);
    let status = core.hub_status().unwrap();
    assert_eq!(status.account, Some(5));
    assert_eq!(
        (status.floor, status.ceiling),
        (
            floor * dassie_xrpl::UNITS_PER_DROP,
            ceiling * dassie_xrpl::UNITS_PER_DROP
        )
    );
    assert!(
        status
            .moving
            .is_some_and(|m| m.direction == super::Direction::Withdraw)
    );
}

// spec/areas/hubs.md#in-the-node: the owner's own moves go one at a time,
// open the account when the hub has none, and pause the automatic moves for
// a while; turned off, the node moves nothing by itself, and the database
// keeps that.
#[test]
fn the_owners_moves_pause_the_automatic_ones() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let mut core = core_on(Arc::clone(&store), 9);
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(600, 300))
        .unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    assert!(
        core.hub_deposit(3_000_000_000).is_err(),
        "not before the hub says"
    );
    hub.no_account(&mut core);
    let units = dassie_xrpl::UNITS_PER_DROP;
    assert!(core.hub_deposit(units).is_err(), "too little to open it");
    let opened = core.hub_deposit(3_000_000 * units).unwrap();
    assert!(opened.moving.is_some_and(|m| m.owner));
    assert!(
        core.hub_deposit(3_000_000 * units).is_err(),
        "one at a time"
    );
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    let made = deposits(&mut ledger, escrow);
    assert_eq!((made.len(), made[0].tag, made[0].amount), (1, 0, 3_000_000));

    // Credited, and below the floor: nothing moves while the owner's move
    // pauses the automatic ones.
    hub.account(&mut core, 100_000, 3_000_000);
    wait_and_close(&mut core, &mut ledger, super::hub_funding::FIRST_BACKOFF_MS);
    assert_eq!(deposits(&mut ledger, escrow).len(), 1);
    wait_and_close(&mut core, &mut ledger, super::hub_funding::OWNER_PAUSE_MS);
    assert_eq!(deposits(&mut ledger, escrow).len(), 2, "after the pause");

    // Off, and kept off across a restart.
    core.set_hub_automatic(None, false).unwrap();
    core.flush();
    drop(core);
    let core = core_on(Arc::clone(&store), 10);
    let hub = dassie_ledger::name!(dassie_hub::member::SCHEME_ID);
    assert!(!core.funding[&hub].automatic);
}

// spec/areas/hubs.md#in-the-node: an owner's move names its hub by its ID,
// or none while the node is in one hub; the scheme over the hub is not
// removed while a move of it is on its way, since the move is its
// account's in that hub.
#[test]
fn an_owners_move_names_its_hub_and_holds_its_scheme() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    let statuses = core.hub_statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].scheme.as_str(), dassie_hub::member::SCHEME_ID);
    let id = statuses[0].hub_id;
    let units = dassie_xrpl::UNITS_PER_DROP;
    let elsewhere = core.hub_deposit_to(Some([0; 32]), 3_000_000 * units);
    assert!(matches!(elsewhere, Err(super::EngineError::NotFound(_))));
    let opened = core.hub_deposit_to(Some(id), 3_000_000 * units).unwrap();
    assert!(opened.moving.is_some());
    let removed = core.remove_scheme(dassie_hub::member::SCHEME_ID);
    assert!(matches!(removed, Err(super::EngineError::Precondition(_))));
    let view = core
        .ledger_views()
        .into_iter()
        .find(|view| view.scheme == dassie_hub::member::SCHEME_ID)
        .unwrap();
    assert_eq!(view.removal_blockers, [super::RemovalBlocker::HubMove]);
}

// spec/areas/hubs.md#in-the-node: the move on its way is kept with the
// books, so a restart between the XRP Ledger applying a deposit and the hub
// crediting it never pays it again, however slow the hub: the move is late
// after a while, and done once the account counts it.
#[test]
fn a_restart_never_pays_a_deposit_twice() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let mut core = core_on(Arc::clone(&store), 9);
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(600, 300))
        .unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 1, "applied on the XRP Ledger");
    let opening = made[0].amount;
    core.flush();
    drop(core);

    let mut core = core_on(Arc::clone(&store), 10);
    follow_xrpl(&mut core, &mut ledger);
    let moving = core.hub_status().unwrap().moving.unwrap();
    assert_eq!(
        (moving.direction, moving.drops, moving.target),
        (super::Direction::Deposit, opening, opening)
    );
    hub.no_account(&mut core);
    for _ in 0..4 {
        wait_and_close(&mut core, &mut ledger, super::hub_funding::FIRST_BACKOFF_MS);
    }
    assert_eq!(deposits(&mut ledger, escrow).len(), 1, "not paid again");
    wait_and_close(&mut core, &mut ledger, super::hub_funding::CREDIT_WAIT_MS);
    let status = core.hub_status().unwrap();
    assert!(
        status.late && status.moving.is_some(),
        "late, and waited for"
    );
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        1,
        "still not paid again"
    );

    hub.account(&mut core, opening - 2_001_000, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let status = core.hub_status().unwrap();
    assert_eq!((status.moving, status.late), (None, false), "credited");
    assert_eq!(deposits(&mut ledger, escrow).len(), 1);
}

/// What the owner holds on the hub's ledger, as the books say.
fn hub_owner_balance(core: &Core) -> i128 {
    core.ledger_views()
        .into_iter()
        .find(|view| view.id == dassie_ledger::LedgerId::HubXrplTestnetXrp)
        .unwrap()
        .owner_balance
}

// spec/areas/hubs.md#in-the-node: the owner may give up a move only once it
// is late. Giving up forgets it and books nothing; the node then moves
// nothing by itself until the owner turns its moves on again, and a credit
// the hub makes later anyway is booked as a deposit, once.
#[test]
fn a_late_deposit_given_up_is_booked_once_if_credited() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, 100_000, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 2, "below the floor: {made:?}");
    let amount = made[1].amount;
    assert!(core.hub_give_up(None).is_err(), "not late yet");
    wait_and_close(&mut core, &mut ledger, super::hub_funding::CREDIT_WAIT_MS);
    assert!(core.hub_status().unwrap().late);
    let status = core.hub_give_up(None).unwrap();
    assert_eq!(status.moving, None);
    assert_eq!(status.given_up, Some((super::Direction::Deposit, amount)));
    for _ in 0..3 {
        wait_and_close(&mut core, &mut ledger, super::hub_funding::OWNER_PAUSE_MS);
    }
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        2,
        "waits for the owner"
    );

    let before = hub_owner_balance(&core);
    hub.account(&mut core, 100_000 + amount, opening + amount);
    hub.account(&mut core, 100_000 + amount, opening + amount);
    let units = i128::from(dassie_xrpl::UNITS_PER_DROP);
    assert_eq!(
        hub_owner_balance(&core) - before,
        i128::from(amount) * units,
        "the late credit is a deposit, once"
    );

    core.set_hub_automatic(None, true).unwrap();
    assert_eq!(core.hub_status().unwrap().given_up, None);
    hub.account(&mut core, 100_000, opening + amount);
    wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(
        deposits(&mut ledger, escrow).len(),
        3,
        "moving by itself again"
    );
}

// spec/areas/hubs.md#in-the-node: what the node keeps in the hub counts its
// own holds toward peers, the credit it extends them there: below the
// floor, a deposit fills only what the ceiling leaves once they and the
// hold's reserve count.
#[test]
fn our_holds_count_toward_the_hub_ceiling() {
    use dassie_settlement::policy;

    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let floor =
        policy::hub_floor(p, Policies::DEFAULT.peer_credit, 0) / dassie_xrpl::UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / dassie_xrpl::UNITS_PER_DROP;
    hub.account(&mut core, floor, opening);

    let fees = hub.rules.terms.fees;
    let hold = 7_000_000;
    let balance = 100_000;
    hub.opened_hold(&mut core, hold, balance, opening);
    let status = core.hub_status().unwrap();
    assert_eq!(status.paying.len(), 1, "the hold is ours");
    assert_eq!(status.paying[0].amount, hold * dassie_xrpl::UNITS_PER_DROP);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 2, "below the floor: {made:?}");
    let kept = balance + fees.account_reserve + fees.hold_reserve + hold;
    assert_eq!(made[1].amount, ceiling - kept, "up to the ceiling only");
    assert!(
        made[1].amount < floor.midpoint(ceiling) - balance,
        "less than the midpoint asks"
    );
}

// spec/areas/hubs.md#in-the-node: a deposit on its way is done only once
// the account's deposits counter shows it: a withdrawal that came back in
// the meantime counts apart, and does not end the move early.
#[test]
fn a_returned_withdrawal_does_not_end_a_deposit_early() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, 100_000, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 2, "below the floor: {made:?}");
    let amount = made[1].amount;
    hub.account_with(&mut core, 100_000, opening, amount);
    wait_and_close(&mut core, &mut ledger, 4_000);
    let status = core.hub_status().unwrap();
    assert!(
        status.moving.is_some(),
        "still waiting for the hub to credit it"
    );
    assert_eq!(status.returned, amount * dassie_xrpl::UNITS_PER_DROP);
    hub.account_with(&mut core, 100_000 + amount, opening + amount, amount);
    wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(core.hub_status().unwrap().moving, None, "credited");
}

/// A core with both schemes on `store`, whose hub says it has no account
/// for the node, and the hub.
fn holding_core(
    store: &Arc<Store>,
    ledger: &mut dassie_xrpl::fake::FakeLedger,
) -> (Core, Validators) {
    let mut core = core_on(Arc::clone(store), 9);
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(600, 300))
        .unwrap();
    follow_xrpl(&mut core, ledger);
    let mut hub = Validators::new();
    hub.no_account(&mut core);
    (core, hub)
}

/// Whether the books would let the owner's funds on the XRP Ledger pay
/// `amount` now, as a conversion or a payer's packet takes them: a pending
/// debit of the owner's group toward an account outside it.
fn owner_could_pay(core: &Core, amount: u64) -> bool {
    use dassie_ledger::{AccountName, LedgerId, Reference, TraceId, TransferSpec};

    let ledger = LedgerId::XrplTestnetXrp;
    let spec = TransferSpec {
        debit: core.account(ledger, AccountName::EquityOwner).unwrap(),
        credit: core.account(ledger, AccountName::AssetsSettlement).unwrap(),
        amount,
        pending: true,
        reference: Reference::Packet(TraceId {
            epoch: 0,
            sequence: 1,
        }),
    };
    core.ledger.validate(&spec).is_ok()
}

/// The owner's group on the XRP Ledger: posted, net, and still pending out.
fn owner_xrp(core: &Core) -> (i128, u128) {
    use dassie_ledger::{AccountName, LedgerId};

    let owner = core
        .account(LedgerId::XrplTestnetXrp, AccountName::EquityOwner)
        .unwrap();
    let balance = core.ledger.group_balance(owner).unwrap();
    let posted = i128::try_from(balance.credits_posted).unwrap()
        - i128::try_from(balance.debits_posted).unwrap();
    (posted, balance.debits_pending)
}

// spec/areas/settlement.md#the-scheme-interface: an owner's payment out of
// the XRP Ledger account, here a deposit into the hub that takes all the
// owner's XRP, is held against the owner's funds from the moment it is
// made: what a conversion or a payer's packet would spend meanwhile is
// refused, and once the payment applied the owner's funds are not below
// zero.
#[test]
fn a_deposit_holds_the_owners_funds_while_on_its_way() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let (mut core, _hub) = holding_core(&store, &mut ledger);
    let units = dassie_xrpl::UNITS_PER_DROP;
    let funds = core.owner_available(dassie_ledger::LedgerId::XrplTestnetXrp);
    let all = funds / units - dassie_xrpl::account::MAX_FEE_DROPS;
    assert!(owner_could_pay(&core, all * units));
    core.hub_deposit(all * units).unwrap();
    let (posted, pending) = owner_xrp(&core);
    let fee = dassie_xrpl::account::MAX_FEE_DROPS * units;
    assert_eq!(
        pending,
        u128::from(all * units + fee),
        "held while on its way, with the most its fee may cost"
    );
    assert!(!owner_could_pay(&core, all * units), "a packet is refused");
    assert!(!owner_could_pay(&core, 1), "nor even the fee's worth");
    assert!(!owner_could_pay(&core, funds - all * units + 1));
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    let (after, pending) = owner_xrp(&core);
    assert_eq!(pending, 0, "posted once it applied");
    assert!(after >= 0, "the owner's funds are not below zero: {after}");
    assert_eq!(
        after,
        posted - i128::from(all * units) - i128::from(10 * units),
        "the deposit and its fee"
    );
}

/// Carries the XRP Ledger account's requests to `ledger`, but loses every
/// submission, until it has nothing more to say.
fn follow_xrpl_losing_submits(core: &mut Core, ledger: &mut dassie_xrpl::fake::FakeLedger) {
    use super::LedgerEvent;

    for _ in 0..50 {
        let mut outputs = Vec::new();
        core.finish_step(&mut outputs);
        let events: Vec<(u64, LedgerEvent)> = outputs
            .iter()
            .filter_map(|output| match output {
                Output::LedgerConnect { epoch, .. } => Some((*epoch, LedgerEvent::Connected)),
                Output::LedgerSend { epoch, message, .. } => {
                    let request: serde_json::Value = serde_json::from_slice(message).unwrap();
                    (request["command"] != "submit")
                        .then(|| (*epoch, LedgerEvent::Message(ledger.request(1, message))))
                }
                Output::Fetch { epoch, url, .. } => Some((*epoch, fetched(ledger, url))),
                _ => None,
            })
            .collect();
        if events.is_empty() {
            return;
        }
        for (epoch, event) in &events {
            core.ledger_event(XRPL_ACCOUNT, *epoch, event);
        }
    }
    panic!("the scheme kept talking");
}

// spec/areas/settlement.md#the-scheme-interface: an owner's payment that
// never applies releases its hold, and costs no fee.
#[test]
fn a_failed_owner_payment_releases_its_hold() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let (mut core, _hub) = holding_core(&store, &mut ledger);
    let units = dassie_xrpl::UNITS_PER_DROP;
    let funds = core.owner_available(dassie_ledger::LedgerId::XrplTestnetXrp);
    core.hub_deposit(3_000_000 * units).unwrap();
    let fee = dassie_xrpl::account::MAX_FEE_DROPS * units;
    assert_eq!(owner_xrp(&core).1, u128::from(3_000_000 * units + fee));
    let epoch = core.assets[&dassie_xrpl::account::FIRST].epoch;
    for _ in 0..30 {
        follow_xrpl_losing_submits(&mut core, &mut ledger);
        for (_, message) in ledger.close() {
            core.ledger_event(XRPL_ACCOUNT, epoch, &super::LedgerEvent::Message(message));
        }
    }
    follow_xrpl_losing_submits(&mut core, &mut ledger);
    assert_eq!(owner_xrp(&core).1, 0, "released");
    assert_eq!(
        core.owner_available(dassie_ledger::LedgerId::XrplTestnetXrp),
        funds,
        "the owner's funds are whole again"
    );
}

// spec/areas/settlement.md#the-scheme-interface: the hold outlives a
// restart between the payment leaving and the XRP Ledger applying it, and is
// posted once after it.
#[test]
fn an_owner_payment_held_across_a_restart_is_posted_once() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let (mut core, _hub) = holding_core(&store, &mut ledger);
    let units = dassie_xrpl::UNITS_PER_DROP;
    let (posted, _) = owner_xrp(&core);
    core.hub_deposit(3_000_000 * units).unwrap();
    core.flush();
    drop(core);
    let mut core = core_on(Arc::clone(&store), 10);
    let fee = dassie_xrpl::account::MAX_FEE_DROPS * units;
    assert_eq!(
        owner_xrp(&core),
        (posted, u128::from(3_000_000 * units + fee)),
        "still held"
    );
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    assert_eq!(
        owner_xrp(&core),
        (
            posted - i128::from(3_000_000 * units) - i128::from(10 * units),
            0
        ),
        "posted once, with its fee"
    );
    core.flush();
    drop(core);
    let core = core_on(Arc::clone(&store), 11);
    assert_eq!(owner_xrp(&core).1, 0);
}

// spec/areas/settlement.md#the-scheme-interface: a withdrawal from the hub
// takes only the owner's funds on the hub's ledger, never what the account
// holds for what the node owes its hub peers.
#[test]
fn a_hub_withdrawal_takes_only_the_owners_funds() {
    use dassie_ledger::{AccountName, LedgerId, Reference, TransferSpec};

    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    // Only the floor, so nothing moves by itself.
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let floor = dassie_settlement::policy::hub_floor(p, Policies::DEFAULT.peer_credit, 0)
        / dassie_xrpl::UNITS_PER_DROP;
    let units = dassie_xrpl::UNITS_PER_DROP;
    let balance = floor + 3_000_000;
    hub.account(&mut core, balance, opening);
    wait_and_close(&mut core, &mut ledger, 4_000);
    // The node owes a hub peer most of it.
    let hub_ledger = LedgerId::HubXrplTestnetXrp;
    let peer = dassie_ledger::Name::new("peeraaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let (interledger, _) = core.ledger.open_peer(NOW, hub_ledger, peer, 0).unwrap();
    let owner = core.account(hub_ledger, AccountName::EquityOwner).unwrap();
    let owed = core.owner_available(hub_ledger) - 1_500_000 * units;
    let spec = TransferSpec {
        debit: owner,
        credit: interledger,
        amount: owed,
        pending: false,
        reference: Reference::Withdrawal,
    };
    core.ledger.create(NOW, &spec).unwrap();
    let refused = core.hub_withdraw(2_000_000 * units);
    assert!(
        matches!(refused, Err(super::EngineError::Refused(_))),
        "{refused:?}"
    );
    assert!(core.hub_status().unwrap().moving.is_none());
    core.hub_withdraw(1_000_000 * units).unwrap();
}

/// A core node that takes downlinks, with the XRP Ledger testnet scheme
/// as an owner book, configured not to peer, synced on `ledger`, and the hub
/// of [`hub_rules`].
fn owner_book_core(ledger: &mut dassie_xrpl::fake::FakeLedger) -> Core {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let refused = dassie_node_core::config::LiveRealm::Refused;
    let mut core = core_taking_downlinks(store, Realm::Test, refused, NOW);
    let book = r#"{"server":"ws://ledger.test","faucet":null,"peers":false}"#;
    add_xrpl(&mut core, "xrpl-testnet-xrp", book).unwrap();
    follow_xrpl(&mut core, ledger);
    core.add_scheme(dassie_hub::member::SCHEME_ID, hub_config(600, 300))
        .unwrap();
    follow_xrpl(&mut core, ledger);
    close_xrpl(&mut core, ledger);
    core
}

// spec/areas/settlement.md#owner-books, spec/areas/hubs.md#in-the-node: the
// XRP Ledger testnet scheme configured not to peer is the owner book of XRP
// on the testnet. The node never lists it in its link state, and refuses a
// peering on it as on a scheme it does not run; yet the hub is funded from
// it, and withdrawn to its account, as from one that peers.
#[test]
fn an_owner_book_funds_the_hub_and_never_peers() {
    use dassie_ledger::{LedgerId, Name};
    use dassie_peer::wire::RefusalReason;

    let (mut ledger, own) = funded_ledger();
    let mut core = owner_book_core(&mut ledger);
    let xrpl = Name::new("xrpl-testnet-xrp").unwrap();
    assert!(core.schemes[&xrpl].scheme.has_synced());
    assert!(!core.schemes[&xrpl].scheme.peers());
    let listed = |core: &Core| {
        core.peer
            .schemes()
            .iter()
            .any(|scheme| scheme.as_str() == "xrpl-testnet-xrp")
    };
    assert!(!listed(&core), "an owner book is never in the link state");

    // A downlink that asks to peer on it is refused, as on a scheme the
    // node does not run.
    let downlink = Arc::new(NodeKey::from_bytes(&[11; 32]));
    let theirs = dassie_xrpl::Wallet::from_entropy(&[11; 32]).account();
    let asked = asks_as_downlink(
        &mut core,
        &downlink,
        NOW + 1_000,
        ("xrpl-testnet-xrp", &address_data(theirs)),
    );
    assert_eq!(asked, Some(RefusalReason::UnsupportedScheme));
    assert!(!core.is_peer(&downlink.node_id()));

    // The hub is funded from it: the opening deposit leaves its account,
    // held against the owner's funds on its ledger, then booked there.
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    let before = core.owner_available(LedgerId::XrplTestnetXrp);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let made = deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!((made[0].tag, made[0].sender), (0, own.0));
    let opening = made[0].amount;
    let units = dassie_xrpl::UNITS_PER_DROP;
    let spent = before - core.owner_available(LedgerId::XrplTestnetXrp);
    assert!(
        spent >= opening * units,
        "the deposit left the owner book: {spent}"
    );
    assert_eq!(owner_xrp(&core).1, 0, "and is no longer held");

    // Credited, then taken past the ceiling: withdrawn to the owner book's
    // account.
    let fees = hub.rules.terms.fees;
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let credit = Policies::DEFAULT.peer_credit;
    let floor = dassie_settlement::policy::hub_floor(p, credit, 0) / units;
    let ceiling = dassie_settlement::policy::hub_ceiling(p) / units;
    hub.account(&mut core, floor + 1_000, opening);
    let balance = 12_000_000;
    hub.account(&mut core, balance, opening);
    let outputs = wait_and_close(&mut core, &mut ledger, 4_000);
    let to = dassie_hub::operation::Destination {
        account: own.0,
        tag: None,
    };
    let exposure = balance + fees.account_reserve;
    assert_eq!(
        withdrawals(&outputs),
        [(exposure - floor.midpoint(ceiling), to)]
    );
    let moving = core.hub_status().unwrap().moving.unwrap();
    assert_eq!(moving.direction, super::Direction::Withdraw);
    assert!(!listed(&core), "still not in the link state");
}

/// The core node `other`, at `url`, tells `core` (of key `[7; 32]`) its
/// link state, which lists `scheme` alone, at `at`: the outputs of the
/// step.
fn tells_link_state(
    core: &mut Core,
    other: &Arc<NodeKey>,
    (url, scheme): (&str, &str),
    at: u64,
) -> Vec<Output> {
    use dassie_oer::Decode;
    use dassie_peer::wire::{Content, Entry, NodeInfo, SchemeId, SignedNodeInfo, sign_node_info};

    let scheme = SchemeId::parse(scheme).unwrap();
    let info = NodeInfo {
        sequence: at,
        url,
        alias: "",
        public_key: *other.public_key().as_bytes(),
        entries: vec![Entry::Scheme(scheme)],
    };
    let state = sign_node_info(other, &info);
    let content = Content::LinkStateUpdate(SignedNodeInfo::decode_exact(&state).unwrap());
    let body = content.to_vec();
    core.begin_step(at);
    core.peer_inbound(dassie_peer::InboundId(1), other.public_key(), &body, None);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let held = core
        .peer
        .nodes()
        .into_iter()
        .find(|v| v.id == other.node_id());
    assert_eq!(held.unwrap().schemes, [scheme], "the link state is taken");
    outputs
}

/// Whether a core node with the XRP Ledger testnet scheme synced on a fake
/// ledger, peering on it if `peers` and an owner book otherwise, asks a
/// core node it bootstraps from, whose link state lists the scheme alone,
/// to peer, within a minute of learning that link state.
fn asks_a_core_node_to_peer_on_xrpl(peers: bool) -> bool {
    use dassie_peer::wire::Kind;

    let (mut ledger, _) = funded_ledger();
    let other = Arc::new(NodeKey::from_bytes(&[21; 32]));
    let url = "https://other.test";
    let mut core = Core::start(
        CoreParts {
            store: Arc::new(Store::open_in_memory().unwrap()),
            key: Arc::new(NodeKey::from_bytes(&[7; 32])),
            realm: Realm::Test,
            live: dassie_node_core::config::LiveRealm::Refused,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(5)),
            peer_rng: Box::new(SeededRng::new(6)),
            own_info: dassie_peer::OwnInfo {
                url: Some("https://core.test".into()),
                alias: String::new(),
                feed_port: None,
                hub: None,
            },
            bootstrap: vec![dassie_peer::Bootstrap {
                id: other.node_id(),
                endpoint: dassie_peer::wire::Endpoint::of_url(url).unwrap(),
                key: other.public_key(),
            }],
            uplinks: Vec::new(),
        },
        NOW,
    )
    .unwrap();
    let config = if peers {
        r#"{"server":"ws://ledger.test","faucet":null}"#
    } else {
        r#"{"server":"ws://ledger.test","faucet":null,"peers":false}"#
    };
    add_xrpl(&mut core, "xrpl-testnet-xrp", config).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    let xrpl = dassie_ledger::Name::new("xrpl-testnet-xrp").unwrap();
    assert!(core.schemes[&xrpl].scheme.has_synced());

    // The other node tells its link state, a core node's on the scheme.
    let at = core.now_ms() + 1_000;
    let mut outputs = tells_link_state(&mut core, &other, (url, "xrpl-testnet-xrp"), at);
    for second in 1..=60 {
        core.begin_step(at + second * 1_000);
        core.finish_step(&mut outputs);
    }
    outputs.iter().any(|output| {
        matches!(output, Output::PeerRequest(request)
            if request.to == other.node_id()
                && matches!(request.kind, Kind::PeeringInfoRequest | Kind::PeeringRequest))
    })
}

// spec/areas/settlement.md#owner-books: the node never asks to peer on an
// owner book, though a core node it knows lists it; on the same scheme
// configured to peer, it asks.
#[test]
fn an_owner_book_is_never_asked_for() {
    assert!(asks_a_core_node_to_peer_on_xrpl(true));
    assert!(!asks_a_core_node_to_peer_on_xrpl(false));
}

// spec/areas/settlement.md#the-scheme-interface: the XRP Ledger scheme
// removed while the owner's payment is on its way, and added again: the
// hold stays meanwhile, and is posted once, with the actual fee, once the
// scheme is back and reads what became of the payment.
#[test]
fn an_owner_payment_outlives_its_scheme_removed_and_added_again() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut ledger, _) = funded_ledger();
    let (mut core, _hub) = holding_core(&store, &mut ledger);
    let units = dassie_xrpl::UNITS_PER_DROP;
    let (posted, _) = owner_xrp(&core);
    core.hub_deposit(3_000_000 * units).unwrap();
    core.remove_scheme("xrpl-testnet-xrp").unwrap();
    follow_xrpl(&mut core, &mut ledger);
    let fee = dassie_xrpl::account::MAX_FEE_DROPS * units;
    assert_eq!(
        owner_xrp(&core),
        (posted, u128::from(3_000_000 * units + fee))
    );
    add_xrpl(&mut core, "xrpl-testnet-xrp", SHARED_CONFIG).unwrap();
    follow_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    close_xrpl(&mut core, &mut ledger);
    assert_eq!(
        owner_xrp(&core),
        (
            posted - i128::from(3_000_000 * units) - i128::from(10 * units),
            0
        ),
        "posted once, with its fee"
    );
}

// spec/areas/network.md#the-registrant: the owner's registration is on disk,
// salt and all, by the time its first commitment leaves, so that a node
// restarted from it commits to the same commitment; and only names an
// owner may register are registered, at the bootstrap nodes.
#[test]
fn a_registration_is_on_disk_before_its_commitment_leaves() {
    use crate::engine::EngineError;
    use dassie_peer::wire::Kind;

    let store = Arc::new(Store::open_in_memory().unwrap());
    let registrar = NodeKey::from_bytes(&[8; 32]);
    let parts = |bootstrap: Vec<dassie_peer::Bootstrap>| CoreParts {
        store: Arc::clone(&store),
        key: Arc::new(NodeKey::from_bytes(&[7; 32])),
        realm: Realm::Test,
        live: dassie_node_core::config::LiveRealm::Refused,
        policies: Policies::DEFAULT,
        rng: Box::new(SeededRng::new(14)),
        peer_rng: Box::new(SeededRng::new(15)),
        own_info: dassie_peer::OwnInfo {
            url: None,
            alias: String::new(),
            feed_port: None,
            hub: None,
        },
        bootstrap,
        uplinks: Vec::new(),
    };
    let mut alone = Core::start(parts(Vec::new()), NOW).unwrap();
    assert!(matches!(
        alone.register_name("alice"),
        Err(EngineError::Precondition(_))
    ));
    drop(alone);
    let bootstrap = vec![dassie_peer::Bootstrap {
        id: registrar.node_id(),
        endpoint: dassie_peer::wire::Endpoint::parse("registrar.example").unwrap(),
        key: registrar.public_key(),
    }];
    let mut core = Core::start(parts(bootstrap), NOW).unwrap();
    for refused in ["www", "Alice", "a"] {
        assert!(
            matches!(core.register_name(refused), Err(EngineError::Invalid(_))),
            "{refused}"
        );
    }
    core.register_name("alice").unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let commits = outputs
        .iter()
        .filter(|output| matches!(output, Output::PeerRequest(request) if request.kind == Kind::NameCommit))
        .count();
    assert_eq!(commits, 1);
    let stored = store.load_peer().unwrap().registrations;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name.as_str(), "alice");
    assert!(matches!(
        core.renew_name("alice"),
        Err(EngineError::NotFound(_))
    ));
    let names = core.names();
    assert_eq!(names.registrars, [registrar.node_id()]);
    assert_eq!(names.names.len(), 1);
}

// spec/areas/network.md#names: names run in the test realm only. A node of
// the live realm, though one of its own bootstrap nodes, is no registrar,
// reads no name lists, and refuses the owner's registrations and renewals,
// saying why; its names view says names are off.
#[test]
fn a_live_realm_node_takes_no_part_in_names() {
    use super::names::NAMES_OFF;
    use crate::engine::EngineError;

    let own = NodeKey::from_bytes(&[7; 32]);
    let other = NodeKey::from_bytes(&[8; 32]);
    let bootstrap = [&own, &other]
        .into_iter()
        .map(|key| dassie_peer::Bootstrap {
            id: key.node_id(),
            endpoint: dassie_peer::wire::Endpoint::parse("registrar.example").unwrap(),
            key: key.public_key(),
        })
        .collect();
    let mut core = Core::start(
        CoreParts {
            store: Arc::new(Store::open_in_memory().unwrap()),
            key: Arc::new(own),
            realm: Realm::Live,
            live: dassie_node_core::config::LiveRealm::DevSwitch,
            policies: Policies::DEFAULT,
            rng: Box::new(SeededRng::new(16)),
            peer_rng: Box::new(SeededRng::new(17)),
            own_info: dassie_peer::OwnInfo {
                url: None,
                alias: String::new(),
                feed_port: None,
                hub: None,
            },
            bootstrap,
            uplinks: Vec::new(),
        },
        NOW,
    )
    .unwrap();
    assert!(!core.peer.config().names_enabled);
    assert!(core.peer.registrar().is_none(), "no registrar");
    assert_eq!(core.peer.holders().registrars(), 0, "no lists read");
    for refused in [core.register_name("alice"), core.renew_name("alice")] {
        assert!(
            matches!(&refused, Err(EngineError::Precondition(reason)) if reason == NAMES_OFF),
            "{refused:?}"
        );
    }
    let names = core.names();
    assert!(names.off);
    assert!(names.names.is_empty());
    assert!(super::names_enabled(Realm::Test) && !super::names_enabled(Realm::Live));
}

/// Two nodes in the simulation, both running the stub with commitments and
/// [`StubTwo`], each its own pretend hub, so that they share two hubs: node
/// 0 held off from settling with node 1 on the stub if `held_off`, as a
/// node whose settlements on it keep failing is, so that it never funds its
/// hold there. Runs until each has a peering with the other on both, at
/// most ten minutes, as a network takes to be ready.
fn two_hub_pair(seed: u64, held_off: bool) -> crate::sim::Sim {
    use crate::sim::{START, Settling, Sim};

    let mut sim = Sim::with(
        seed,
        2,
        1,
        std::collections::BTreeMap::new(),
        Settling::StubHub(4),
    );
    sim.loss_per_mille = 0;
    for index in 0..2 {
        sim.step(index, |core| {
            core.add_scheme(STUB_TWO, r#"{"commitments":4}"#.into())
                .unwrap();
        });
    }
    if held_off {
        let peer = crate::sim::key(1).node_id();
        sim.step(0, |core| {
            core.settle
                .backoff
                .insert((peer, scheme_id("stub")), (u64::MAX, 1));
        });
    }
    let both = |sim: &Sim| {
        (0..2).all(|index| {
            let other = crate::sim::key(1 - index).node_id();
            sim.core(index)
                .peer
                .peering_of(&other)
                .is_some_and(|peering| peering.methods.len() == 2)
        })
    };
    assert!(
        sim.run_until_or(START + 600_000, both),
        "the pair did not peer on both hubs"
    );
    sim
}

/// The credit node `index` of `sim` extends its peer on the method on
/// `scheme`: its hold toward the peer there and its settlements to it on
/// their way, as the simulation checks after every step
/// (`check_hub_credit`).
fn method_credit(sim: &crate::sim::Sim, index: usize, scheme: &str) -> u64 {
    let other = crate::sim::key(1 - index).node_id();
    let name = dassie_ledger::Name::new(scheme).unwrap();
    sim.core(index)
        .hub_books(name, other)
        .map_or(0, |(credit, _, _)| credit)
}

/// Whether both nodes of a [`two_hub_pair`] funded their holds toward the
/// other on both methods.
fn funded(sim: &crate::sim::Sim) -> bool {
    (0..2).all(|index| {
        ["stub", STUB_TWO]
            .into_iter()
            .all(|scheme| method_credit(sim, index, scheme) > 0)
    })
}

/// Whether node 0 of a [`two_hub_pair`] has asked node 1 to settle on both
/// methods, and waits for the answers.
fn asked_on_both(sim: &crate::sim::Sim) -> bool {
    let other = crate::sim::key(1).node_id();
    let asked = &sim.core(0).settle.asked;
    asked.contains_key(&(other, scheme_id("stub")))
        && asked.contains_key(&(other, scheme_id(STUB_TWO)))
}

// spec/decisions/settlement-methods.md: two nodes that share two hubs peer
// on both, the first the one the peering was made on and the second added,
// in the same order on both sides. Each method has the peer's two accounts
// on its own ledger, is asked to settle and funds its hold on its own, and
// keeps the credit in step with its own hold; a restart keeps both methods
// with what each holds.
#[test]
fn a_pair_that_shares_two_hubs_keeps_each_method_apart() {
    use crate::sim::checks::{check_hub_credit, check_hub_packets};
    use dassie_ledger::LedgerId;

    let mut sim = two_hub_pair(41, false);
    for index in 0..2 {
        let other = crate::sim::key(1 - index).node_id();
        let core = sim.core(index);
        let peering = core.peer.peering_of(&other).unwrap();
        let schemes: Vec<String> = peering.schemes().map(|s| s.to_string()).collect();
        assert_eq!(schemes, ["stub", STUB_TWO], "node {index}");
        let name = dassie_ledger::Name::new(other.as_str()).unwrap();
        for ledger in [LedgerId::StubUsd, LedgerId::HubXrplTestnetXrp] {
            assert!(
                super::peers::has_accounts(&core.ledger, ledger, name),
                "node {index} on {ledger:?}"
            );
        }
        // Its endpoint has a slot per method, on each method's ledger.
        let endpoint = dassie_connector::EndpointId::Peer(core.peer_endpoints[&other]);
        let ledgers: Vec<LedgerId> = core.connector.slots(endpoint).map(|s| s.ledger).collect();
        assert_eq!(ledgers, [LedgerId::StubUsd, LedgerId::HubXrplTestnetXrp]);
    }
    // Each asks the other on each method, whose room starts empty, its
    // first check on both, each on its own, and each funds its hold on
    // each: the credit on a method is its hold.
    assert!(
        sim.run_until_or(sim.now + 600_000, |sim| asked_on_both(sim) || funded(sim)),
        "node 0 never asked on both methods"
    );
    assert!(asked_on_both(&sim), "node 0 asked on one method only");
    assert!(
        sim.run_until_or(sim.now + 600_000, funded),
        "the holds on both methods were not funded"
    );
    sim.run_until(sim.now + 60_000);
    check_hub_credit(&sim).unwrap();
    check_hub_packets(&sim).unwrap();
    let before: Vec<dassie_peer::Peering> = (0..2)
        .map(|index| {
            let other = crate::sim::key(1 - index).node_id();
            sim.core(index).peer.peering_of(&other).unwrap().clone()
        })
        .collect();
    let two = method_credit(&sim, 0, STUB_TWO);

    // Restarted, node 0 has both methods, as they were, and their books.
    sim.crash(0);
    sim.start(0);
    let other = crate::sim::key(1).node_id();
    let restored = sim.core(0).peer.peering_of(&other).unwrap().clone();
    let schemes = |peering: &dassie_peer::Peering| -> Vec<(String, Vec<u8>)> {
        peering
            .methods
            .iter()
            .map(|method| (method.scheme.to_string(), method.state.clone()))
            .collect()
    };
    assert_eq!(schemes(&restored), schemes(&before[0]));
    assert_eq!(method_credit(&sim, 0, STUB_TWO), two);
    sim.run_until(sim.now + 60_000);
    check_hub_credit(&sim).unwrap();
    assert_eq!(
        sim.core(0).peer.peering_of(&other).unwrap().methods.len(),
        2
    );
    // Packets go on the first method, beside the second.
    assert!(sim.pay(0, 1, 1_000_000, None, None));
    let paid = |sim: &crate::sim::Sim| crate::sim::received(sim, &sim.payments[0]) == 1_000_000;
    assert!(sim.run_until_or(sim.now + 120_000, paid), "not paid");
    check_hub_credit(&sim).unwrap();
    check_hub_packets(&sim).unwrap();
}

// spec/decisions/settlement-methods.md: settling is held off per method. A
// cancelled settlement's backoff on one method leaves the other free to
// settle, and a method dropped is forgotten by both the requests and the
// backoff.
#[test]
fn a_settlement_held_off_on_one_method_holds_up_no_other() {
    let mut sim = two_hub_pair(42, false);
    sim.run_until(sim.now + 60_000);
    let peer = crate::sim::key(1).node_id();
    let (stub, two) = (scheme_id("stub"), scheme_id(STUB_TWO));
    sim.step(0, |core| {
        assert!(core.can_settle(&peer, stub) && core.can_settle(&peer, two));
        let until = core.now_ms() + 60_000;
        core.settle.backoff.insert((peer, stub), (until, 1));
        assert!(!core.can_settle(&peer, stub), "held off on the stub");
        assert!(core.can_settle(&peer, two), "not on the second method");
        core.settle.asked.insert((peer, two), core.now_ms());
        core.peer_event(dassie_peer::Event::MethodDropped {
            peer,
            scheme: stub,
            by_us: true,
        });
        assert!(core.settle.backoff.is_empty());
        assert!(core.settle.asked.contains_key(&(peer, two)));
    });
}

// spec/decisions/settlement-methods.md: a method is chosen by structure,
// never by room. Node 0 never funds its hold on the stub, the method its
// packets to node 1 take, while its hold on the second method is funded:
// its payment gets nowhere, answered for want of room, and not one packet
// moves to the second method.
#[test]
fn a_method_short_of_room_answers_t04_and_never_moves_the_packet() {
    use crate::sim::checks::check_hub_packets;

    let mut sim = two_hub_pair(43, true);
    let held_on_two = |sim: &crate::sim::Sim| method_credit(sim, 0, STUB_TWO) > 0;
    assert!(
        sim.run_until_or(sim.now + 600_000, held_on_two),
        "node 0 did not fund its hold on the second method"
    );
    assert_eq!(
        method_credit(&sim, 0, "stub"),
        0,
        "nothing to fund it on the stub"
    );
    assert!(sim.pay(0, 1, 1_000_000, None, None));
    sim.run_until(sim.now + 120_000);
    let payment = &sim.payments[0];
    assert_eq!(
        crate::sim::received(&sim, payment),
        0,
        "paid on another method"
    );
    assert!(
        method_credit(&sim, 0, STUB_TWO) > 0,
        "room on the second method"
    );
    assert_eq!(stub_two_packets(), 0, "a packet moved to the second method");
    check_hub_packets(&sim).unwrap();
}

// spec/decisions/settlement-methods.md: node 1 sends a Prepare on the
// second method without a commitment. It is checked as any Prepare on a
// method over a hub, and refused for want of room on it (T04), with no
// commitment to look at.
#[test]
fn a_prepare_on_the_second_method_without_a_commitment_answers_t04() {
    use dassie_oer::Decode;
    use dassie_peer::wire::{Content, Kind};

    let mut sim = two_hub_pair(44, false);
    let one = crate::sim::key(1).node_id();
    let zero = crate::sim::key(0).node_id();
    let prepare = dassie_ilp::Prepare {
        amount: 10,
        expires_at: Timestamp::from_unix_ms(sim.now + 30_000),
        execution_condition: [7; 32],
        destination: dassie_ilp::Address::parse(&format!("test.das.{zero}")).unwrap(),
        data: &[],
    }
    .to_vec();
    let body = Content::MethodPacket {
        request_id: 77,
        scheme: scheme_id(STUB_TWO),
        packet: &prepare,
        commitment: &[],
    }
    .to_vec();
    let from = crate::sim::key(1).public_key();
    sim.step(0, |core| {
        core.peer_inbound(dassie_peer::InboundId(9_999), from, &body, None);
        let t04 = |packet: &[u8]| {
            matches!(dassie_ilp::Response::decode_exact(packet),
                Ok(dassie_ilp::Response::Reject(reject))
                    if reject.code == dassie_ilp::ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
        };
        let refused = core.outputs_peer.iter().any(|output| match output {
            Output::PeerRequest(request)
                if request.to == one && request.kind == Kind::InterledgerPacket =>
            {
                matches!(
                    Content::decode_exact(&request.body),
                    Ok(Content::InterledgerPacket { request_id: 77, packet }) if t04(packet)
                )
            }
            _ => false,
        });
        assert!(refused, "{:?}", core.outputs_peer);
    });
    assert_eq!(stub_two_packets(), 0, "a commitment was looked at");
}

// spec/decisions/settlement-methods.md: node 0 drops the method the pair's
// peering was made on. Node 1 drops it on being told, and the slot of each
// one's endpoint for it goes, while the endpoint stays with its other
// method's: it stays peered, with every check of the books and the
// endpoints holding after each step. The two share the stub's hub still, so
// they add its method again, after the other, with a slot of its own, and
// a payment's packets in dollars go on it.
#[test]
fn a_dropped_method_takes_its_slot_and_brings_it_back() {
    use crate::sim::checks::check_hub_packets;

    let mut sim = two_hub_pair(45, false);
    assert!(
        sim.run_until_or(sim.now + 600_000, funded),
        "the holds on both methods were not funded"
    );
    let one = crate::sim::key(1).node_id();
    sim.step(0, |core| {
        let now = core.now_ms();
        assert!(core.peer.drop_method(now, &one, scheme_id("stub")));
        core.after_peer();
    });
    let ledgers = |sim: &crate::sim::Sim, index: usize| {
        let other = crate::sim::key(1 - index).node_id();
        let core = sim.core(index);
        let endpoint = dassie_connector::EndpointId::Peer(core.peer_endpoints[&other]);
        core.connector
            .slots(endpoint)
            .filter(|slot| !slot.removed)
            .map(|slot| slot.ledger)
            .collect::<Vec<_>>()
    };
    let hub = dassie_ledger::LedgerId::HubXrplTestnetXrp;
    let moved = |sim: &crate::sim::Sim| (0..2).all(|index| ledgers(sim, index) == [hub]);
    assert!(
        sim.run_until_or(sim.now + 120_000, moved),
        "the dropped method kept its slot"
    );
    for index in 0..2 {
        let other = crate::sim::key(1 - index).node_id();
        let peering = sim.core(index).peer.peering_of(&other).unwrap();
        assert_eq!(peering.first().scheme, scheme_id(STUB_TWO), "node {index}");
    }
    // The stub's method comes back, after the other, with its slot.
    let usd = dassie_ledger::LedgerId::StubUsd;
    let back = |sim: &crate::sim::Sim| (0..2).all(|index| ledgers(sim, index) == [hub, usd]);
    assert!(
        sim.run_until_or(sim.now + 600_000, back),
        "the stub's method did not come back with its slot"
    );
    // A payment in dollars goes on it, not on the hub's method.
    let before = stub_two_packets();
    assert!(sim.pay(0, 1, 1_000_000, None, None));
    let paid = |sim: &crate::sim::Sim| crate::sim::received(sim, &sim.payments[0]) == 1_000_000;
    assert!(sim.run_until_or(sim.now + 300_000, paid), "not paid");
    assert_eq!(
        stub_two_packets(),
        before,
        "a dollar packet went on the hub"
    );
    sim.run_until(sim.now + 60_000);
    check_hub_packets(&sim).unwrap();
}

// The lifecycle of settlement methods in the node: winding down, dropping
// and the owner's removal of a scheme.
mod winding;

mod joining;

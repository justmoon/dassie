//! The moves between each hub and the owner book of its asset, on either
//! realm (spec/areas/hubs.md#in-the-node): which book funds which hub, the
//! frozen gate, a token hub's gas before a withdrawal, and the live
//! realm's hubs funded from its own books.

use std::sync::Arc;

use dassie_hub::Rules;
use dassie_hub::member::{DSUSD_SCHEME_ID, MAINNET_SCHEME_ID, RLUSD_SCHEME_ID, SCHEME_ID};
use dassie_hub::rules::{Asset, Millis};
use dassie_identity::{IdentityKey, PublicKey};
use dassie_ledger::{LedgerId, Name};
use dassie_node_core::config::{LiveRealm, Realm};
use dassie_settlement::{AssetId, AssetKind, LedgerStatus};
use dassie_xrpl::UNITS_PER_DROP;
use dassie_xrpl::fake::FakeLedger;
use dassie_xrpl::network::{MAINNET, TESTNET};

use super::super::hub_moves::{gas_book_ledger, owner_book_ledger};
use super::super::{Core, EngineError, Output};
use super::{NOW, Policies, Store};

/// The node's first XRPL account on `network`, where its accounts in that
/// network's hubs pay out.
fn own_account(network: &dassie_xrpl::Network) -> dassie_xrpl::AccountId {
    let entropy = IdentityKey::from_bytes(&[7; 32]).xrpl_account_entropy(network.id, 0);
    dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account()
}

/// A hub of the tests' rules ([`super::hub_rules`]) on `network`, holding
/// `asset`, as its validators speak for it: each signs alone with its
/// consensus key for that network.
struct Hub {
    rules: Rules,
    keys: Vec<dassie_identity::NodeKey>,
    /// The node's key in the hub.
    member: PublicKey,
    /// The node's account in the hub, as an external asset.
    account: AssetId,
    height: u64,
}

impl Hub {
    /// The hub `scheme` joins: its rules' network and asset name it.
    fn new(scheme: &str, network: u32, asset: Asset) -> Self {
        use dassie_hub::escrow::{ValidatorKeys, escrow_master};
        use dassie_hub::rules::LEAST_TOKEN_GAS_DROPS;

        let mut rules = super::hub_rules(600, 300);
        let nonce = super::HUB_NONCE;
        let sponsor = IdentityKey::from_bytes(&[100; 32]);
        rules.terms.network = network;
        rules.terms.asset = asset;
        rules.terms.escrow = escrow_master(&sponsor, network, &nonce).account().0;
        if asset != Asset::Xrp {
            rules.terms.fees.gas = LEAST_TOKEN_GAS_DROPS;
            rules.terms.fees.ledger = 0;
        }
        let terms = rules.terms.clone();
        let mut keys = Vec::new();
        for (index, acceptance) in rules.validators.iter_mut().enumerate() {
            let node = IdentityKey::from_bytes(&[u8::try_from(index + 1).unwrap(); 32]);
            let derive = || ValidatorKeys::derive(&node, network, &nonce).unwrap();
            keys.push(derive().consensus);
            let endpoint = acceptance.endpoint.clone();
            *acceptance = derive().accept(&node, &terms, endpoint).unwrap();
        }
        rules.check().unwrap();
        let named = dassie_hub::member::scheme_of(&rules.terms).map(|(id, _)| id);
        assert_eq!(named, Some(scheme), "the hub's network and asset name it");
        let member =
            dassie_hub::member::member_key(&IdentityKey::from_bytes(&[7; 32]), &rules.id())
                .public_key();
        Self {
            rules,
            keys,
            member,
            account: dassie_hub::member::account_id(scheme).unwrap(),
            height: 1,
        }
    }

    /// The config of the hub's scheme: its rules.
    fn config(&self) -> String {
        use dassie_oer::Encode;

        let hex: String = self
            .rules
            .to_vec()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        serde_json::json!({ "rules": hex }).to_string()
    }

    fn escrow(&self) -> dassie_xrpl::AccountId {
        dassie_xrpl::AccountId(self.rules.terms.escrow)
    }

    /// Every validator says `statements`, each from its own endpoint.
    fn say(&self, core: &mut Core, statements: &[dassie_hub::client::Statement]) {
        use dassie_hub::client::SignedStatement;
        use dassie_hub::validators::ValidatorIndex;

        let epoch = core.assets[&self.account].epoch;
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
                &self.account.to_string(),
                epoch,
                &super::super::LedgerEvent::From {
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

    /// The node's account 5 holds `balance` in the hub asset's smallest
    /// unit, and deposits credited it `deposited`, as of a new final block.
    fn account(&mut self, core: &mut Core, balance: u64, deposited: u64) {
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
            returned: 0,
        };
        let statements = [
            Statement::Account {
                height: self.height,
                key: self.member,
                account: Some((AccountNumber(5), view)),
            },
            Statement::Final {
                height: self.height,
                timestamp: self.stamp(),
            },
        ];
        self.say(core, &statements);
    }

    /// The issuer's hold on the escrow after a new block: frozen since the
    /// block stamped `frozen`, or not.
    fn issuer(&mut self, core: &mut Core, frozen: Option<Millis>) {
        use dassie_hub::client::Statement;

        self.height += 1;
        let statements = [Statement::Issuer {
            height: self.height,
            timestamp: self.stamp(),
            frozen,
            held: frozen.is_some(),
        }];
        self.say(core, &statements);
    }

    /// The stamp of the block at the current height.
    const fn stamp(&self) -> Millis {
        NOW + self.height
    }
}

/// RLUSD on mainnet, as a hub's rules name it.
const RLUSD: Asset = Asset::Token {
    currency: dassie_xrpl::rlusd::CURRENCY,
    issuer: dassie_xrpl::rlusd::ISSUER,
};

/// DSUSD on the testnet, as a hub's rules name it.
const DSUSD: Asset = Asset::Token {
    currency: dassie_xrpl::dsusd::CURRENCY,
    issuer: dassie_xrpl::dsusd::ISSUER,
};

// spec/areas/hubs.md#in-the-node, spec/decisions/hub-scheme-names.md: each
// hub's ledger, on either realm, is funded from the owner book of its
// asset on its own network, whose slug its own names after `hub-`, of the
// same realm and asset; a token hub's escrow's gas comes from the XRP book
// of that network. A ledger that is no hub's has neither.
#[test]
fn each_hub_is_funded_from_its_assets_book_on_its_network() {
    let mut realms = Vec::new();
    for ledger in LedgerId::ALL {
        let hub = ledger.as_str().starts_with("hub-");
        assert_eq!(owner_book_ledger(ledger).is_some(), hub, "{ledger}");
        assert_eq!(gas_book_ledger(ledger).is_some(), hub, "{ledger}");
        let (Some(book), Some(gas)) = (owner_book_ledger(ledger), gas_book_ledger(ledger)) else {
            continue;
        };
        assert_eq!(format!("hub-{book}"), ledger.as_str());
        assert_eq!(book.realm(), ledger.realm(), "{ledger}");
        assert_eq!(book.asset(), ledger.asset(), "{ledger}");
        assert_eq!(gas.realm(), ledger.realm(), "{ledger}");
        assert_eq!(gas.asset().code, "XRP", "{ledger}");
        realms.push(ledger.realm());
    }
    for realm in [dassie_ledger::Realm::Test, dassie_ledger::Realm::Live] {
        assert_eq!(
            realms.iter().filter(|r| **r == realm).count(),
            2,
            "{realm:?}"
        );
    }
    assert_eq!(
        gas_book_ledger(LedgerId::HubXrplRlusdUsd),
        Some(LedgerId::XrplXrp)
    );
    assert_eq!(
        gas_book_ledger(LedgerId::HubXrplTestnetDsusdUsd),
        Some(LedgerId::XrplTestnetXrp)
    );
}

/// A test core with the XRP Ledger testnet's XRP book synced on `ledger`,
/// in the DSUSD hub `hub`, whose account there holds `balance`, all of it
/// deposited: synced, with the node's own moves off.
fn dsusd_core(ledger: &mut FakeLedger, hub: &mut Hub, balance: u64) -> Core {
    let mut core = super::core(9);
    super::add_xrpl(&mut core, "xrpl-testnet-xrp", super::SHARED_CONFIG).unwrap();
    super::follow_xrpl(&mut core, ledger);
    core.add_scheme(DSUSD_SCHEME_ID, hub.config()).unwrap();
    super::follow_xrpl(&mut core, ledger);
    core.set_hub_automatic(DSUSD_SCHEME_ID, false).unwrap();
    hub.account(&mut core, balance, balance);
    assert_eq!(core.hub_status().unwrap().status, LedgerStatus::Synced);
    core
}

// spec/areas/hubs.md#a-frozen-hub, spec/decisions/token-hubs.md: while the
// validators say the issuer froze a token hub's escrow, its account is no
// longer in step and nothing moves into or out of it, the owner's moves
// refused as unavailable, though the account still shows what it holds;
// once they say it thawed, the owner's withdrawal goes on to the escrow's
// gas check.
#[test]
fn a_frozen_token_hub_moves_nothing_until_the_thaw() {
    let (mut ledger, _) = super::funded_ledger();
    let mut hub = Hub::new(DSUSD_SCHEME_ID, TESTNET.id, DSUSD);
    let balance = 5_000_000;
    let mut core = dsusd_core(&mut ledger, &mut hub, balance);
    let units = UNITS_PER_DROP;

    let since = hub.stamp();
    hub.issuer(&mut core, Some(since));
    let status = core.hub_status().unwrap();
    assert_ne!(status.status, LedgerStatus::Synced, "frozen");
    assert_eq!(status.balance, balance * units, "still shown");
    for refused in [
        core.hub_withdraw(1_000_000 * units),
        core.hub_deposit(1_000_000 * units),
    ] {
        assert!(
            matches!(&refused, Err(EngineError::Precondition(why)) if why.contains("unavailable")),
            "{refused:?}"
        );
    }

    hub.issuer(&mut core, None);
    assert_eq!(core.hub_status().unwrap().status, LedgerStatus::Synced);
    let gas = core.hub_withdraw(1_000_000 * units);
    assert!(
        matches!(&gas, Err(EngineError::Exhausted(why)) if why.contains("XRP")),
        "{gas:?}"
    );
}

/// Brings the escrow of `hub` on `ledger` into being, then low: its fees
/// burnt what it held above 1.6 XRP, short of the least gas by 0.4 XRP.
fn low_escrow(ledger: &mut FakeLedger, hub: &Hub) -> u64 {
    use dassie_hub::member::LEAST_ESCROW_GAS_DROPS;

    let short = 400_000;
    ledger.deposit(hub.escrow(), LEAST_ESCROW_GAS_DROPS);
    ledger.close();
    ledger.take_unrecorded(hub.escrow(), short);
    ledger.close();
    short
}

/// The owner of `core` withdraws from the token hub `hub` until the node
/// lets it, the escrow on `ledger` being `short` of its gas: the node reads
/// it and tops it up from `own`, the account of the XRP book of the hub's
/// network, and the withdrawal goes to that account once the top-up
/// applied.
fn withdraws_once_the_escrow_has_its_gas(
    (core, ledger): (&mut Core, &mut FakeLedger),
    hub: &Hub,
    own: dassie_xrpl::AccountId,
    short: u64,
) {
    use dassie_hub::member::LEAST_ESCROW_GAS_DROPS;

    let escrow = hub.escrow();
    let xrp_before = ledger.balance(own).unwrap();
    let amount = 1_000_000;
    let mut refused = 0;
    let withdrawn = loop {
        match core.hub_withdraw(amount * UNITS_PER_DROP) {
            Ok(status) => break status,
            // The wallet sends the withdrawal again on these words
            // (`isGasCheck` in apps/wallet/src/pages/hubs/hubs.ts).
            Err(EngineError::Exhausted(why)) => assert!(
                why.contains("escrow pays its fees") && why.contains("try again shortly"),
                "{why}"
            ),
            Err(other) => panic!("{other:?}"),
        }
        refused += 1;
        assert!(refused < 20, "the gas check ends");
        super::wait_and_close(core, ledger, 1_000);
    };
    assert!(refused >= 1, "refused until the top-up applied");
    assert_eq!(ledger.balance(escrow), Some(LEAST_ESCROW_GAS_DROPS));
    // The top-up leaves the escrow the reserve of its account and of the
    // two objects it owns, its signer list and its trust line, and more
    // for fees, so the payout set's change of signer list, which needs the
    // reserve met, goes through (spec/decisions/token-hubs.md).
    let reserve = dassie_xrpl::fake::RESERVE_BASE + 2 * dassie_xrpl::fake::RESERVE_INC;
    assert!(
        ledger.balance(escrow).unwrap() > reserve,
        "the least gas covers the escrow's reserve"
    );
    // The shortfall, from the XRP book's account, tagged with the node's
    // account in the hub, as the escrow reads an XRP payment.
    let paid: Vec<_> = super::deposits(ledger, escrow)
        .into_iter()
        .map(|paid| (paid.sender, paid.tag, paid.amount))
        .collect();
    assert_eq!(paid, [(own.0, 5, short)]);
    assert!(
        xrp_before - ledger.balance(own).unwrap() > short,
        "with its fee"
    );
    let moving = withdrawn.moving.unwrap();
    assert_eq!(moving.direction, super::super::Direction::Withdraw);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let to = dassie_hub::operation::Destination {
        account: own.0,
        tag: None,
    };
    assert_eq!(super::withdrawals(&outputs), [(amount, to)]);
}

// spec/decisions/token-hubs.md, spec/areas/hubs.md#in-the-node: before a
// token hub's withdrawal, the node reads the escrow's XRP through the XRP
// book of the hub's network; short of the least gas, it pays the escrow the
// shortfall from that book, tagged with its account's number, and asks for
// the withdrawal only once the top-up applied. The owner's withdrawal is
// refused meanwhile, to be asked again. Here on the testnet, in DSUSD.
#[test]
fn a_token_hubs_withdrawal_waits_for_the_escrows_gas() {
    let (mut ledger, own) = super::funded_ledger();
    let mut hub = Hub::new(DSUSD_SCHEME_ID, TESTNET.id, DSUSD);
    let short = low_escrow(&mut ledger, &hub);
    let mut core = dsusd_core(&mut ledger, &mut hub, 5_000_000);
    withdraws_once_the_escrow_has_its_gas((&mut core, &mut ledger), &hub, own, short);
}

// As above on mainnet, in RLUSD, for a node of the live realm by the dev
// switch: the escrow's gas comes from the live XRP book, `xrpl-xrp`, on
// the node's mainnet account, and the withdrawal goes to that account.
#[test]
fn a_mainnet_token_hubs_gas_comes_from_the_live_xrp_book() {
    let mut ledger = super::fake_ledger(true);
    let mut hub = Hub::new(RLUSD_SCHEME_ID, MAINNET.id, RLUSD);
    let short = low_escrow(&mut ledger, &hub);
    let mut core = super::rlusd_core(&mut ledger);
    super::add_xrpl(&mut core, "xrpl-xrp", r#"{"server":"ws://ledger.test"}"#).unwrap();
    super::follow_xrpl(&mut core, &mut ledger);
    core.add_scheme(RLUSD_SCHEME_ID, hub.config()).unwrap();
    super::follow_xrpl(&mut core, &mut ledger);
    core.set_hub_automatic(RLUSD_SCHEME_ID, false).unwrap();
    hub.account(&mut core, 5_000_000, 5_000_000);
    let own = own_account(&MAINNET);
    withdraws_once_the_escrow_has_its_gas((&mut core, &mut ledger), &hub, own, short);
}

// spec/areas/hubs.md#in-the-node, spec/decisions/mainnet-schemes.md: a node
// of the live realm, by the dev switch, in a mainnet hub of XRP opens its
// account from the live realm's XRP book, on its mainnet account, and
// withdraws past the ceiling to that account, never the testnet's.
#[test]
fn a_live_hub_is_funded_from_the_live_book() {
    use dassie_settlement::policy;

    let mut ledger = super::fake_ledger(true);
    let own = own_account(&MAINNET);
    ledger.deposit(own, dassie_xrpl::fake::FAUCET_DROPS);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = super::core_taking_downlinks(store, Realm::Live, LiveRealm::DevSwitch, NOW);
    super::add_xrpl(&mut core, "xrpl-xrp", r#"{"server":"ws://ledger.test"}"#).unwrap();
    super::follow_xrpl(&mut core, &mut ledger);
    let mut hub = Hub::new(MAINNET_SCHEME_ID, MAINNET.id, Asset::Xrp);
    let view = core.add_scheme(MAINNET_SCHEME_ID, hub.config()).unwrap();
    assert_eq!(view.id, LedgerId::HubXrplXrp);
    assert_eq!(hub.account, AssetId::sole(AssetKind::HubXrplXrp));
    assert_eq!(hub.account.to_string(), MAINNET_SCHEME_ID);
    super::follow_xrpl(&mut core, &mut ledger);
    let escrow = hub.escrow();

    hub.no_account(&mut core);
    let before = core.owner_available(LedgerId::XrplXrp);
    super::wait_and_close(&mut core, &mut ledger, 1_000);
    let made = super::deposits(&mut ledger, escrow);
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(
        (made[0].tag, made[0].key, made[0].sender),
        (0, Some(hub.member), own.0)
    );
    let opening = made[0].amount;
    let spent = before - core.owner_available(LedgerId::XrplXrp);
    assert!(
        spent >= opening * UNITS_PER_DROP,
        "from the live book: {spent}"
    );

    // Credited, then taken past the ceiling: what the node receives stays
    // in the hub by default (spec/areas/payments.md#where-received-money-is-kept),
    // where a deposit names the account by its number.
    let p = Policies::DEFAULT.connector.max_packet_amount;
    let credit = Policies::DEFAULT.peer_credit;
    let floor = policy::hub_floor(p, credit, 0) / UNITS_PER_DROP;
    let ceiling = policy::hub_ceiling(p) / UNITS_PER_DROP;
    hub.account(&mut core, floor + 1_000, opening);
    let balance = 12_000_000;
    hub.account(&mut core, balance, opening);
    let kept = super::wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(super::withdrawals(&kept), [], "kept in the hub");
    let hub_view = core
        .ledger_views()
        .into_iter()
        .find(|view| view.id == LedgerId::HubXrplXrp)
        .unwrap();
    assert_eq!(
        (hub_view.deposit_address, hub_view.deposit_tag),
        (Some(escrow.to_address()), Some(5))
    );
    assert!(!core.hub_status().unwrap().sweeps);

    // Kept on the owner's own account: withdrawn to the midpoint, to the
    // node's mainnet account.
    super::keep_xrp_on_own_account(&mut core);
    assert!(core.hub_status().unwrap().sweeps);
    let outputs = super::wait_and_close(&mut core, &mut ledger, 4_000);
    let exposure = balance + hub.rules.terms.fees.account_reserve;
    let to = dassie_hub::operation::Destination {
        account: own.0,
        tag: None,
    };
    assert_ne!(own, own_account(&TESTNET));
    assert_eq!(
        super::withdrawals(&outputs),
        [(exposure - floor.midpoint(ceiling), to)]
    );
}

// spec/decisions/hub-scheme-names.md, spec/areas/xrpl.md#the-peg-guard: a
// member scheme on a mainnet hub of RLUSD, made from its rules, books on
// `hub-xrpl-rlusd+usd` through an account of its own, `hub-xrpl-rlusd`;
// the owner's deposit into it leaves the live RLUSD book; and the
// RLUSD book's peg guard reaches its ledger: while the book runs pegged,
// the hub's ledger converts, and once the books stopped the book, it is
// halted for want of a reading.
#[test]
fn a_mainnet_rlusd_hub_follows_the_rlusd_book() {
    use dassie_settlement::PegPause;

    let mut ledger = super::fake_ledger(true);
    let mut core = super::rlusd_core(&mut ledger);
    let mut hub = Hub::new(RLUSD_SCHEME_ID, MAINNET.id, RLUSD);
    let view = core.add_scheme(RLUSD_SCHEME_ID, hub.config()).unwrap();
    let ledger_id = LedgerId::HubXrplRlusdUsd;
    assert_eq!(view.id, ledger_id);
    let scheme = Name::new(RLUSD_SCHEME_ID).unwrap();
    assert_eq!(core.schemes[&scheme].asset, Some(hub.account));
    assert_eq!(hub.account, AssetId::sole(AssetKind::HubXrplRlusd));
    assert_eq!(hub.account.to_string(), RLUSD_SCHEME_ID);
    let book = Name::new(dassie_xrpl::rlusd::SCHEME_ID).unwrap();
    let book_peg = core.schemes[&book].scheme.peg();
    assert!(book_peg.is_some_and(|peg| peg.paused.is_none()), "pegged");
    assert_eq!(core.hub_peg(ledger_id), book_peg);
    assert!(!core.hub_halted(ledger_id));

    // The owner's deposit opens the account from the RLUSD book, held
    // against the owner's funds there while on its way.
    core.set_hub_automatic(RLUSD_SCHEME_ID, false).unwrap();
    hub.no_account(&mut core);
    let before = core.owner_available(LedgerId::XrplRlusdUsd);
    let amount = 3_000_000 * UNITS_PER_DROP;
    let status = core.hub_deposit(amount).unwrap();
    assert!(status.moving.is_some_and(|moving| moving.owner));
    assert_eq!(
        before - core.owner_available(LedgerId::XrplRlusdUsd),
        amount
    );

    // Once the books stopped the book, the hub's ledger converts nothing.
    let at = core.now_ms() + 1_000;
    core.begin_step(at);
    core.stop_scheme(book, EngineError::Internal("stopped by the test".into()));
    let mut outputs: Vec<Output> = Vec::new();
    core.finish_step(&mut outputs);
    assert!(core.hub_halted(ledger_id));
    assert_eq!(
        core.hub_peg(ledger_id).and_then(|peg| peg.paused),
        Some(PegPause::Stale)
    );
    assert_eq!(core.hub_status().unwrap().scheme, scheme);
}

// spec/areas/xrpl.md#the-peg-guard: on mainnet, once the RLUSD/XRP pool
// has priced RLUSD off its peg for the trip hold, the RLUSD book's guard
// pauses it, and halts `hub-xrpl-rlusd+usd` with it, which carries the
// book's guard; back at a dollar for the recovery hold, both run again.
#[test]
fn rlusd_off_its_peg_halts_the_mainnet_rlusd_hubs_ledger() {
    use dassie_settlement::PegPause;
    use dassie_xrpl::DROPS_PER_XRP;
    use dassie_xrpl::amount::IssuedAsset;
    use dassie_xrpl::codec::TokenValue;
    use dassie_xrpl::peg::Timings;

    let mut ledger = super::fake_ledger(true);
    ledger.deposit(own_account(&MAINNET), dassie_xrpl::fake::FAUCET_DROPS);
    ledger.close();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let live = (Realm::Live, LiveRealm::DevSwitch);
    let mut core = super::core_with(store, 12, live, Policies::DEV).unwrap();
    super::add_xrpl(&mut core, "xrpl-rlusd", r#"{"server":"ws://ledger.test"}"#).unwrap();
    super::follow_xrpl(&mut core, &mut ledger);
    let hub = Hub::new(RLUSD_SCHEME_ID, MAINNET.id, RLUSD);
    core.add_scheme(RLUSD_SCHEME_ID, hub.config()).unwrap();
    let hub_ledger = LedgerId::HubXrplRlusdUsd;
    let book = LedgerId::XrplRlusdUsd;
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
                let event = super::super::LedgerEvent::Message(message);
                core.ledger_event(&asset.to_string(), epoch, &event);
            }
            super::follow_xrpl(core, ledger);
        }
    };
    let peg = |core: &Core, ledger: LedgerId| {
        core.ledger_views()
            .into_iter()
            .find(|view| view.id == ledger)
            .unwrap()
            .peg
    };
    // 200,000 XRP for 100,000 RLUSD at two XRP to the dollar.
    let value = TokenValue::parse("100000").unwrap();
    let pool = |ledger: &mut FakeLedger, xrp: u64| {
        let drops = xrp * DROPS_PER_XRP;
        ledger
            .set_pool(IssuedAsset::RLUSD, drops, value, 205)
            .unwrap();
    };
    pool(&mut ledger, 200_000);
    seconds(&mut core, &mut ledger, 2);
    let at_par = peg(&core, book).unwrap();
    assert_eq!(at_par.last.map(|(price, _)| price), Some(1_000_000));
    assert_eq!(peg(&core, hub_ledger), Some(at_par));
    assert!(!core.hub_halted(hub_ledger));

    // 5% under a dollar, past the trip hold.
    pool(&mut ledger, 190_000);
    let trip = Timings::DEV.trip_hold_ms / Timings::DEV.read_every_ms;
    seconds(&mut core, &mut ledger, trip + 2);
    let off = peg(&core, book).unwrap();
    assert_eq!(off.paused, Some(PegPause::OffPeg));
    assert_eq!(peg(&core, hub_ledger), Some(off));
    assert!(core.hub_halted(hub_ledger), "converts off its peg");

    // Back at a dollar for the recovery hold.
    pool(&mut ledger, 200_000);
    let recover = Timings::DEV.recover_hold_ms / Timings::DEV.read_every_ms;
    seconds(&mut core, &mut ledger, recover + 2);
    assert_eq!(peg(&core, book).unwrap().paused, None);
    assert!(!core.hub_halted(hub_ledger));
}

// spec/areas/hubs.md#in-the-node: the hubs the node is in are those of its
// schemes on a hub's ledger, each with its own funding. The stub with
// commitments beside them, which stands in for a member in tests and
// simulations, is in none and has none.
#[test]
fn the_stub_with_commitments_is_in_no_hub() {
    let (mut ledger, _) = super::funded_ledger();
    let dsusd = Hub::new(DSUSD_SCHEME_ID, TESTNET.id, DSUSD);
    let xrp = Hub::new(SCHEME_ID, TESTNET.id, Asset::Xrp);
    let mut core = super::core(9);
    super::add_stub(&mut core, super::COMMITTING);
    super::add_xrpl(&mut core, "xrpl-testnet-xrp", super::SHARED_CONFIG).unwrap();
    super::follow_xrpl(&mut core, &mut ledger);
    core.add_scheme(DSUSD_SCHEME_ID, dsusd.config()).unwrap();
    core.add_scheme(SCHEME_ID, xrp.config()).unwrap();
    super::follow_xrpl(&mut core, &mut ledger);
    // In the order of their names.
    let hubs = [
        Name::new(SCHEME_ID).unwrap(),
        Name::new(DSUSD_SCHEME_ID).unwrap(),
    ];
    assert_eq!(core.hub_schemes(), hubs);
    let funded: Vec<Name> = core.funding.keys().copied().collect();
    assert_eq!(funded, hubs);
    assert_eq!(core.hub_rules().len(), 2);
    // Each through an account of its own, which its scheme names
    // (spec/decisions/hub-scheme-names.md).
    let accounts: Vec<AssetId> = hubs
        .iter()
        .map(|scheme| core.schemes[scheme].asset.unwrap())
        .collect();
    for (scheme, account) in hubs.iter().zip(&accounts) {
        assert_eq!(account.to_string(), scheme.as_str());
        assert!(core.assets.contains_key(account), "{scheme}");
    }
    assert_ne!(accounts[0], accounts[1]);
}

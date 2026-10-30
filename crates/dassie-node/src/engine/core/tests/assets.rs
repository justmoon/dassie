//! The assets the owner chooses (spec/decisions/owner-chooses-assets.md):
//! an asset adds its owner book, then the hub that funds from it, and goes
//! hub first; and what the owner's one balance of it counts
//! (spec/decisions/one-balance-per-asset.md): its ledgers' books and the
//! reserves they hold back, our holds toward peers once, as part of the hub.

use dassie_hub::member::SCHEME_ID as HUB;
use dassie_ledger::LedgerId;
use dassie_xrpl::UNITS_PER_DROP;

use super::super::{BalanceView, Core, EngineError, LedgerView, RemovalBlocker};
use super::{
    SHARED_CONFIG, Validators, core, default_hubs, deposits, funded_ledger, funding_core,
    trusting_fake, wait_and_close, xrpl_network,
};

/// The XRP Ledger testnet's owner book.
const BOOK: &str = dassie_xrpl::scheme::SCHEME_ID;

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// A core whose testnet account trusts the fake ledger, with the tests'
/// hub as the default testnet XRP hub, and no default DSUSD hub.
fn choosing_core() -> Core {
    let mut core = core(4);
    core.settle.default_hubs = default_hubs(600, 300);
    let account = xrpl_network(BOOK).first().to_string();
    core.set_asset_config(&account, &trusting_fake(BOOK, SHARED_CONFIG))
        .unwrap();
    core
}

fn ids(views: &[LedgerView]) -> Vec<LedgerId> {
    views.iter().map(|view| view.id).collect()
}

fn ledger(core: &Core, id: LedgerId) -> LedgerView {
    core.ledger_views()
        .into_iter()
        .find(|view| view.id == id)
        .unwrap()
}

fn xrp(core: &Core) -> BalanceView {
    let views = core.ledger_views();
    core.balance_views(&views)
        .into_iter()
        .find(|balance| balance.name == "xrp")
        .unwrap()
}

// XRP adds the owner book of the testnet account, then the hub over it; the
// book stays while the hub funds from it, and once the hub is gone, adding
// XRP again adds the hub alone.
#[test]
fn an_asset_adds_its_book_then_its_hub_and_goes_hub_first() {
    let mut core = choosing_core();
    let added = core.add_assets(&names(&["xrp"])).unwrap();
    assert_eq!(
        ids(&added),
        [LedgerId::XrplTestnetXrp, LedgerId::HubXrplTestnetXrp]
    );
    assert!(core.add_assets(&names(&["xrp"])).unwrap().is_empty());

    let book = ledger(&core, LedgerId::XrplTestnetXrp);
    assert_eq!(book.removal_blockers, [RemovalBlocker::HubFunds(1)]);
    assert!(matches!(
        core.remove_scheme(BOOK),
        Err(EngineError::Precondition(why)) if why.contains("hubs fund from it")
    ));
    assert!(
        ledger(&core, LedgerId::HubXrplTestnetXrp)
            .removal_blockers
            .is_empty()
    );
    core.remove_scheme(HUB).unwrap();
    assert!(
        ledger(&core, LedgerId::XrplTestnetXrp)
            .removal_blockers
            .is_empty()
    );

    let added = core.add_assets(&names(&["xrp"])).unwrap();
    assert_eq!(ids(&added), [LedgerId::HubXrplTestnetXrp]);
    core.check_invariants();
}

// An asset the node does not offer, here DSUSD with no default DSUSD hub
// built in, adds nothing, not even the XRP it needs; an unknown name, one of
// the other realm and none at all are refused alike.
#[test]
fn an_asset_not_offered_adds_nothing() {
    let mut core = choosing_core();
    assert!(matches!(
        core.add_assets(&names(&["dsusd"])),
        Err(EngineError::Precondition(_))
    ));
    for refused in [&["eur"][..], &["rlusd"], &[]] {
        assert!(matches!(
            core.add_assets(&names(refused)),
            Err(EngineError::Invalid(_))
        ));
    }
    assert!(core.ledger_views().is_empty());
}

// The stub's dollars, which a build with debug assertions runs, are an asset
// of one scheme.
#[test]
fn the_stubs_dollars_are_an_asset() {
    let mut core = choosing_core();
    let added = core.add_assets(&names(&["usd"])).unwrap();
    assert_eq!(ids(&added), [LedgerId::StubUsd]);
}

// The owner's balance counts the reserves the books never count, the
// account's on the XRP Ledger and the hub account's, and is its parts
// together: what is in the account, what is in the hub and what is on its
// way between them.
#[test]
fn the_balance_counts_the_reserves_and_is_its_parts() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, 100_000, opening);

    let (book, in_hub) = (
        self::ledger(&core, LedgerId::XrplTestnetXrp),
        self::ledger(&core, LedgerId::HubXrplTestnetXrp),
    );
    assert!(book.reserve > 0 && in_hub.reserve > 0);
    let balance = xrp(&core);
    let reserves = i128::from(book.reserve) + i128::from(in_hub.reserve);
    assert_eq!(
        balance.owner_balance,
        book.owner_balance + in_hub.owner_balance + reserves
    );
    assert_eq!(balance.account_reserve, u128::from(book.reserve));
    assert_eq!(balance.hub_reserve, u128::from(in_hub.reserve));
    assert_eq!(
        balance.account,
        book.owner_balance + i128::from(book.reserve)
    );
    assert_eq!(
        balance.hub,
        in_hub.owner_balance + i128::from(in_hub.reserve)
    );
    assert_parts(&balance);
    assert_eq!(balance.held_for_peers, 0);

    // A deposit into the hub on its way counts, as a part of its own.
    wait_and_close(&mut core, &mut ledger, 4_000);
    assert_eq!(deposits(&mut ledger, escrow).len(), 2, "below the floor");
    let during = xrp(&core);
    assert_parts(&during);
}

// What our holds toward peers hold is part of what is in the hub, shown as
// the hub last agreed it, and never counted a second time.
#[test]
fn our_holds_are_part_of_the_hub() {
    let (mut ledger, _) = funded_ledger();
    let mut core = funding_core(&mut ledger);
    let mut hub = Validators::new();
    let escrow = hub.escrow();
    hub.no_account(&mut core);
    wait_and_close(&mut core, &mut ledger, 1_000);
    let opening = deposits(&mut ledger, escrow)[0].amount;
    hub.account(&mut core, 100_000, opening);
    let hold = 2_000;
    hub.opened_hold(&mut core, hold, 50_000, opening);

    let balance = xrp(&core);
    let paying: u64 = core
        .hub_status()
        .unwrap()
        .paying
        .iter()
        .map(|hold| hold.amount)
        .sum();
    assert_eq!(paying, hold * UNITS_PER_DROP);
    assert_eq!(balance.held_for_peers, u128::from(paying));
    let in_hub = self::ledger(&core, LedgerId::HubXrplTestnetXrp);
    assert_eq!(
        balance.hub,
        in_hub.owner_balance + i128::from(in_hub.reserve),
        "the holds are in the hub's books, not added to them"
    );
    assert_parts(&balance);
}

/// The balance is its parts together, each reserve within its part.
fn assert_parts(balance: &BalanceView) {
    let on_its_way = i128::try_from(balance.on_its_way).unwrap();
    assert_eq!(
        balance.owner_balance,
        balance.account + balance.hub + on_its_way
    );
    assert!(i128::try_from(balance.account_reserve).unwrap() <= balance.account);
    assert!(i128::try_from(balance.hub_reserve + balance.held_for_peers).unwrap() <= balance.hub);
}

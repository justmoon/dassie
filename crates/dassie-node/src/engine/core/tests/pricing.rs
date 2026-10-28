//! The engine's prices (spec/areas/connector.md#inventory-pricing): what
//! it builds its price table from.

use std::sync::Arc;

use dassie_connector::pricing::RATE_ONE_E15;
use dassie_connector::{Inventory, PriceTable, RATE_PRECISION};
use dassie_ledger::LedgerId;

use super::super::Core;
use super::{NOW, Policies, Store, XRP_HUB};

// spec/decisions/passive-rebalancing.md: the engine counts each asset
// whole, a hub's ledger with the owner book that funds it, so that with
// the owner's XRP all in its book and none in its XRP hub, the hub does
// not read short: what comes in on the hub converts into the book at spot
// less the spread, where priced apart it would get the capped bonus.
#[test]
fn the_prices_count_a_hub_with_the_book_that_funds_it() {
    let (mut ledger, _) = super::funded_ledger();
    let store = Arc::new(Store::open_in_memory().unwrap());
    let test = (
        dassie_node_core::config::Realm::Test,
        dassie_node_core::config::LiveRealm::Refused,
    );
    let mut core = super::core_with(store, 12, test, Policies::DEV).unwrap();
    super::add_xrpl(&mut core, "xrpl-testnet-xrp", super::SHARED_CONFIG).unwrap();
    super::follow_xrpl(&mut core, &mut ledger);
    core.add_scheme(XRP_HUB, String::new()).unwrap();
    let one = 10_u128.pow(RATE_PRECISION);
    let now = NOW + 1_000;
    core.begin_step(now);
    core.set_rates(&[("USD", one, now), ("XRP", 2 * one, now)]);
    core.finish_step(&mut Vec::new());

    let (book, hub) = (LedgerId::XrplTestnetXrp, LedgerId::HubXrplTestnetXrp);
    let table = *core.price_table();
    assert!(core.owner_available(book) > 0 && core.owner_available(hub) == 0);
    assert_eq!(table.ledger(hub).map(|entry| entry.asset), Some(book));
    assert_eq!(table.ledger(book).map(|entry| entry.asset), Some(book));
    assert_eq!(table.assets(), 1);
    let pricing = table.pricing();
    assert!(pricing.rebalance_ppm > 0);
    let spread = u64::from(1_000_000 - pricing.charges.spread_ppm);
    let whole = table.price(hub, book).map(|price| price.rate_e15);
    assert_eq!(whole, Some(RATE_ONE_E15 / 1_000_000 * spread));
    // Nothing goes out on the hub, which holds nothing.
    assert!(table.price(book, hub).is_none());

    // Priced apart, the empty hub would read short.
    let apart = apart(&core, &table);
    assert_eq!(apart.assets(), 2);
    let bonus = apart.price(hub, book).map(|price| price.rate_e15);
    assert!(bonus > whole, "{bonus:?} {whole:?}");
}

/// The table `core` builds from what `table` was built from, each ledger
/// an asset of its own.
fn apart(core: &Core, table: &PriceTable) -> PriceTable {
    let inventories: Vec<Inventory> = table
        .ledgers()
        .map(|entry| Inventory {
            ledger: entry.ledger,
            owner: entry.owner,
            holding: entry.holding,
        })
        .collect();
    PriceTable::build(
        core.rates(),
        &inventories,
        table.pricing(),
        table.built_at_ms(),
        core.connector.config().max_rate_age_ms,
    )
}

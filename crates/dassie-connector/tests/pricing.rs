//! The inventory curve's properties (spec/decisions/passive-rebalancing.md):
//! spot at target, monotonic in each ledger's holding, a bonus of at most
//! `k`, a packet priced at the worst state the packets in flight can leave
//! after its own move, so that no packet in flight improves another's price
//! and no round trip gains, whatever is in flight, and no overflow for any
//! rates and holdings. A settlement hub's ledger is one of the ledgers, as
//! any other with a currency.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_connector::pricing::{MAX_PRICED_LEDGERS, MAX_REBALANCE_PPM, RATE_ONE_E15};
use dassie_connector::{
    Charges, Holding, Holdings, Inventory, PriceTable, Pricing, RATE_PRECISION, Rates,
};
use dassie_ledger::{AccountId, LedgerId};
use num_bigint::BigUint;
use proptest::prelude::*;

const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;
const RLUSD: LedgerId = LedgerId::XrplTestnetRlusdUsd;
/// XRP in a settlement hub, a ledger of its own beside the XRP Ledger's.
const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
/// The ledgers the properties price: two currencies, each on two ledgers,
/// one of them a hub's.
const LEDGERS: [LedgerId; 4] = [USD, XRP, RLUSD, HUB];
const NOW: u64 = 1_700_000_000_000;
const AGE: u64 = 3_600_000;
const MILLION: u32 = 1_000_000;

fn rates(xrp_rate: u128) -> Rates {
    let mut rates = Rates::new(NOW);
    rates.set("USD", 10u128.pow(RATE_PRECISION));
    rates.set("XRP", xrp_rate);
    rates
}

fn pricing(rebalance_ppm: u32, spread_ppm: u32) -> Pricing {
    Pricing {
        charges: Charges {
            fee_ppm: 0,
            spread_ppm,
        },
        rebalance_ppm,
    }
}

/// `ledger` with `posted`, nothing in flight.
fn held(ledger: LedgerId, posted: u64) -> Inventory {
    with_pending(ledger, posted, 0, 0)
}

/// `ledger` with `posted`, and `debits` and `credits` in flight.
fn with_pending(ledger: LedgerId, posted: u64, debits: u64, credits: u64) -> Inventory {
    Inventory {
        ledger,
        owner: AccountId(0),
        holding: Holding {
            posted: u128::from(posted),
            debits_pending: u128::from(debits),
            credits_pending: u128::from(credits),
        },
    }
}

fn table(xrp_rate: u128, usd: u64, xrp: u64, pricing: Pricing) -> PriceTable {
    PriceTable::build(
        &rates(xrp_rate),
        &[held(USD, usd), held(XRP, xrp)],
        pricing,
        NOW,
        AGE,
    )
}

/// The rate from `from` to `to`, or 0 for a pair without a price.
fn rate(table: &PriceTable, from: LedgerId, to: LedgerId) -> u64 {
    table.price(from, to).map_or(0, |price| price.rate_e15)
}

/// The rate a packet bringing `amount_in` and converting `net` gets at
/// `holdings`, or 0 without one.
fn rate_for(
    table: &PriceTable,
    from: LedgerId,
    to: LedgerId,
    amount: u64,
    holdings: &Holdings,
) -> u64 {
    table.rate(from, to, amount, amount, holdings).unwrap_or(0)
}

/// What `net` converts to at `rate_e15`, or `None` without a rate.
fn convert(net: u64, rate_e15: u64) -> Option<u64> {
    (rate_e15 > 0)
        .then(|| u128::from(net) * u128::from(rate_e15) / u128::from(RATE_ONE_E15))
        .and_then(|out| u64::try_from(out).ok())
}

/// The most a price may be: spot from `from_rate` to `to_rate`, times
/// `1 + k`, less the spread, times 10^15, in arbitrary precision.
fn most(from_rate: u128, to_rate: u128, pricing: Pricing) -> BigUint {
    let million = BigUint::from(MILLION);
    BigUint::from(to_rate)
        * BigUint::from(RATE_ONE_E15)
        * BigUint::from(MILLION + pricing.rebalance_ppm)
        * BigUint::from(MILLION - pricing.charges.spread_ppm)
        / (BigUint::from(from_rate) * &million * &million)
}

/// The ledgers' holdings, posted and in flight, in the table's order.
fn holdings_of(entries: [(u64, u64, u64); 4]) -> Holdings {
    let mut holdings = [Holding::NONE; MAX_PRICED_LEDGERS];
    for (slot, (posted, debits, credits)) in holdings.iter_mut().zip(entries) {
        *slot = Holding {
            posted: u128::from(posted),
            debits_pending: u128::from(debits),
            credits_pending: u128::from(credits),
        };
    }
    holdings
}

/// The ledgers' table at `xrp_rate`, from what they hold.
fn four(xrp_rate: u128, holdings: &Holdings, pricing: Pricing) -> PriceTable {
    let inventories = LEDGERS.map(|ledger| held(ledger, 0));
    let mut inventories = inventories;
    for (inventory, holding) in inventories.iter_mut().zip(holdings) {
        inventory.holding = *holding;
    }
    PriceTable::build(&rates(xrp_rate), &inventories, pricing, NOW, AGE)
}

/// An amount: mostly of the size of a node's holdings and its packets, so
/// that a packet moves the curve, sometimes any.
fn sized() -> impl Strategy<Value = u64> {
    prop_oneof![
        4 => 0u64..=1_000_000_000_000,
        1 => any::<u64>(),
    ]
}

/// A holding: posted, and what is in flight either way, often nothing.
fn holding() -> impl Strategy<Value = (u64, u64, u64)> {
    let pending = || prop_oneof![Just(0), sized()];
    (sized(), pending(), pending())
}

proptest! {
    // With every ledger at its target share, the curve is 1, whatever k:
    // spot less the spread, exactly.
    #[test]
    fn at_target_the_price_is_spot(
        xrp_per_dollar in 1u128..=1_000,
        dollars in 1u64..=1_000_000_000_000_000,
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let xrp_rate = xrp_per_dollar * 10u128.pow(RATE_PRECISION);
        let xrp = dollars.checked_mul(u64::try_from(xrp_per_dollar).unwrap());
        prop_assume!(xrp.is_some());
        let inventories = [held(USD, dollars), held(XRP, xrp.unwrap()), held(RLUSD, dollars)];
        let table = PriceTable::build(
            &rates(xrp_rate),
            &inventories,
            pricing(rebalance_ppm, spread_ppm),
            NOW,
            AGE,
        );
        let spread = u64::from(MILLION - spread_ppm);
        let per_dollar = u64::try_from(xrp_per_dollar).unwrap();
        prop_assert_eq!(rate(&table, USD, XRP), per_dollar * spread * 1_000_000_000);
        prop_assert_eq!(rate(&table, USD, RLUSD), spread * 1_000_000_000);
        let flat = PriceTable::build(
            &rates(xrp_rate),
            &inventories,
            pricing(0, spread_ppm),
            NOW,
            AGE,
        );
        for price in table.prices() {
            prop_assert_eq!(price.rate_e15, rate(&flat, price.from, price.to));
        }
    }

    // More of a ledger never makes bringing it in better, nor taking it out
    // worse.
    #[test]
    fn the_price_is_monotonic_in_the_holding(
        xrp_rate in 1u128..=1_000 * 10u128.pow(RATE_PRECISION),
        usd in 0u64..=u64::MAX / 2,
        more in 0u64..=u64::MAX / 2,
        xrp in 0u64..=u64::MAX,
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let pricing = pricing(rebalance_ppm, spread_ppm);
        let less = table(xrp_rate, usd, xrp, pricing);
        let richer = table(xrp_rate, usd + more, xrp, pricing);
        prop_assume!(less.ledgers().count() == 2 && richer.ledgers().count() == 2);
        prop_assert!(rate(&richer, USD, XRP) <= rate(&less, USD, XRP));
        prop_assert!(rate(&richer, XRP, USD) >= rate(&less, XRP, USD));
    }

    // No price is above spot by more than k, less the spread, in the table
    // or for any packet at any holdings; a round trip at one state never
    // gains, in rates or in what a packet converts to; and no rates or
    // holdings overflow.
    #[test]
    fn the_bonus_is_bounded_and_round_trips_lose(
        xrp_rate in 1u128..,
        usd in holding(),
        xrp in holding(),
        rlusd in holding(),
        hub in holding(),
        amount in any::<u64>(),
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let pricing = pricing(rebalance_ppm, spread_ppm);
        let holdings = holdings_of([usd, xrp, rlusd, hub]);
        let rates = rates(xrp_rate);
        let table = four(xrp_rate, &holdings, pricing);
        let rate_of = |ledger: LedgerId| rates.get(ledger.asset().code).unwrap();
        for price in table.prices() {
            let bound = most(rate_of(price.from), rate_of(price.to), pricing);
            prop_assert!(BigUint::from(price.rate_e15) <= bound);
            let packet = rate_for(&table, price.from, price.to, amount, &holdings);
            prop_assert!(BigUint::from(packet) <= bound);
            let Some(back) = table.price(price.to, price.from) else {
                continue;
            };
            let product = u128::from(price.rate_e15) * u128::from(back.rate_e15);
            prop_assert!(product <= u128::from(RATE_ONE_E15) * u128::from(RATE_ONE_E15));
            let there_and_back = price.convert(amount).and_then(|out| back.convert(out));
            prop_assert!(there_and_back.is_none_or(|back| back <= amount));
        }
    }

    // The table's price is what a vanishingly small packet gets at the
    // holdings it was built from; any packet gets that or less.
    #[test]
    fn a_vanishing_packet_gets_the_tables_price(
        xrp_rate in 1u128..=1_000 * 10u128.pow(RATE_PRECISION),
        usd in holding(),
        xrp in holding(),
        rlusd in holding(),
        hub in holding(),
        amount in any::<u64>(),
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let pricing = pricing(rebalance_ppm, spread_ppm);
        let holdings = holdings_of([usd, xrp, rlusd, hub]);
        let table = four(xrp_rate, &holdings, pricing);
        prop_assert_eq!(table.holdings(), holdings);
        for from in LEDGERS {
            for to in LEDGERS {
                if from == to {
                    continue;
                }
                let at_table = rate(&table, from, to);
                prop_assert_eq!(rate_for(&table, from, to, 0, &holdings), at_table);
                prop_assert!(rate_for(&table, from, to, amount, &holdings) <= at_table);
            }
        }
    }

    // spec/decisions/passive-rebalancing.md: a packet held in flight, and
    // rejected later, never improves another packet's price. Whatever is
    // added in flight on any ledger, either way, no pair's price for any
    // packet gets better.
    #[test]
    fn nothing_in_flight_improves_a_price(
        xrp_rate in 1u128..=1_000 * 10u128.pow(RATE_PRECISION),
        usd in holding(),
        xrp in holding(),
        rlusd in holding(),
        hub in holding(),
        on in 0usize..4,
        debit in sized(),
        credit in sized(),
        amount in sized(),
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let pricing = pricing(rebalance_ppm, spread_ppm);
        let holdings = holdings_of([usd, xrp, rlusd, hub]);
        let table = four(xrp_rate, &holdings, pricing);
        let mut held = holdings;
        held[on].debits_pending += u128::from(debit);
        held[on].credits_pending += u128::from(credit);
        let mut posted_only = holdings;
        for holding in &mut posted_only {
            holding.debits_pending = 0;
            holding.credits_pending = 0;
        }
        for from in LEDGERS {
            for to in LEDGERS {
                if from == to {
                    continue;
                }
                let before = rate_for(&table, from, to, amount, &holdings);
                prop_assert!(rate_for(&table, from, to, amount, &held) <= before);
                prop_assert!(before <= rate_for(&table, from, to, amount, &posted_only));
            }
        }
    }

    // spec/decisions/passive-rebalancing.md: a round trip never gains,
    // whatever is in flight. `x` of `a` comes in and goes out on `b`; what
    // came out comes back on `b` and goes out on `a`, once the first leg is
    // posted or while it is still in flight, with anything else in flight
    // at either time, on any ledger.
    #[test]
    fn a_round_trip_never_gains(
        xrp_rate in 10u128.pow(RATE_PRECISION)..=1_000 * 10u128.pow(RATE_PRECISION),
        usd in holding(),
        xrp in holding(),
        rlusd in holding(),
        hub in holding(),
        later in [holding(), holding(), holding(), holding()],
        pair in 0usize..12,
        x in sized(),
        fee_ppm in 0..=Charges::MAX_PPM,
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
        posted in any::<bool>(),
    ) {
        let charges = Charges { fee_ppm, spread_ppm };
        let pricing = Pricing { charges, rebalance_ppm };
        let ledgers = LEDGERS;
        // Every ordered pair of two ledgers.
        let (a, b) = (pair / 3, (pair / 3 + 1 + pair % 3) % 4);
        let holdings = holdings_of([usd, xrp, rlusd, hub]);
        let table = four(xrp_rate, &holdings, pricing);
        let net = x - charges.fee(x);
        let out = convert(net, rate_for_net(&table, ledgers[a], ledgers[b], x, net, &holdings));
        // A first leg refused is no round trip.
        let Some(y) = out else {
            return Ok(());
        };
        // What the books show at the second leg: the first posted, or still
        // pending, and new packets in flight beside it.
        let mut after = holdings_of(later);
        for (index, slot) in after.iter_mut().enumerate() {
            slot.posted = holdings[index].posted;
        }
        if posted {
            // The group limit refused the first leg unless the owner had it.
            if holdings[b].posted < u128::from(y) {
                return Ok(());
            }
            after[a].posted += u128::from(x);
            after[b].posted -= u128::from(y);
        } else {
            after[a].credits_pending = after[a].credits_pending.saturating_add(u128::from(x));
            after[b].debits_pending = after[b].debits_pending.saturating_add(u128::from(y));
        }
        let back_net = y - charges.fee(y);
        let back = rate_for_net(&table, ledgers[b], ledgers[a], y, back_net, &after);
        let z = convert(back_net, back).unwrap_or(0);
        prop_assert!(z <= x, "{x} came back as {z}");
    }
}

/// The rate a packet bringing `amount_in` and converting `net` gets, or 0.
fn rate_for_net(
    table: &PriceTable,
    from: LedgerId,
    to: LedgerId,
    amount_in: u64,
    net: u64,
    holdings: &Holdings,
) -> u64 {
    table.rate(from, to, amount_in, net, holdings).unwrap_or(0)
}

// The curve's numbers at a few states: k = 5%, two ledgers held evenly by
// value or not.
#[test]
fn a_skew_costs_what_the_curve_says() {
    let pricing = pricing(50_000, 0);
    let two = 2 * 10u128.pow(RATE_PRECISION);
    let dollar = 1_000_000_000;
    // A quarter in dollars: about 6% worse out of dollars, 5% better in.
    let quarter = table(two, 25 * dollar, 150 * dollar, pricing);
    assert_eq!(rate(&quarter, XRP, USD), 468_253_968_253_968);
    assert_eq!(rate(&quarter, USD, XRP), 2_100_000_000_000_000);
    // A twentieth: about 33% worse.
    let twentieth = table(two, 5 * dollar, 190 * dollar, pricing);
    assert_eq!(rate(&twentieth, XRP, USD) / 1_000_000_000_000, 336);
}

// A round trip that the table alone would have paid: in at a node's
// target, and straight back once the first leg is posted. Priced before its
// own move, the second leg would get back more than came in.
#[test]
fn a_round_trip_pays_its_own_impact_both_ways() {
    let pricing = pricing(50_000, 0);
    let two = 2 * 10u128.pow(RATE_PRECISION);
    let dollar: u64 = 1_000_000_000;
    let table = table(two, 100 * dollar, 200 * dollar, pricing);
    let mut holdings = table.holdings();
    let x = 10 * dollar;
    let y = convert(x, table.rate(USD, XRP, x, x, &holdings).unwrap()).unwrap();
    holdings[0].posted += u128::from(x);
    holdings[1].posted -= u128::from(y);
    let z = convert(y, table.rate(XRP, USD, y, y, &holdings).unwrap()).unwrap();
    assert!(z < x, "{x} came back as {z}");
    // At the rates of a vanishing packet, before and after, it would gain.
    let rebuilt = PriceTable::build(
        &rates(two),
        &[
            held(USD, u64::try_from(holdings[0].posted).unwrap()),
            held(XRP, u64::try_from(holdings[1].posted).unwrap()),
        ],
        pricing,
        NOW,
        AGE,
    );
    let at_table = convert(
        convert(x, rate(&table, USD, XRP)).unwrap(),
        rate(&rebuilt, XRP, USD),
    )
    .unwrap();
    assert!(at_table > x, "{x} came back as {at_table}");
}

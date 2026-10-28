use super::*;
use crate::charges::Charges;
use crate::fx::{RATE_PRECISION, Rates};
use crate::pricing::{Holding, Inventory, MAX_REBALANCE_PPM, PriceError, Pricing, RATE_ONE_E15};
use alloc::vec::Vec;
use dassie_feed::{LEVELS, Ladder, MethodQuotes, MethodTag, Ratio, Snapshot, method_tag};
use dassie_ledger::AccountId;
use num_bigint::BigUint;
use proptest::test_runner::TestCaseError;

const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;
const RLUSD: LedgerId = LedgerId::XrplTestnetRlusdUsd;
const DSUSD: LedgerId = LedgerId::XrplTestnetDsusdUsd;
const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
const DSUSD_HUB: LedgerId = LedgerId::HubXrplTestnetDsusdUsd;
const DOLLAR: u64 = 1_000_000_000;
const NOW: u64 = 1_000_000;
const AGE: u64 = 3_600_000;
const GRID: &[u32; GRID_LEVELS] = &DEFAULT_GRID_PPM;

const _: () = assert!(GRID_LEVELS == LEVELS, "a snapshot carries the grid");

/// The asset a ledger counts toward, as a node names it.
const fn book_of(ledger: LedgerId) -> LedgerId {
    match ledger {
        LedgerId::HubXrplTestnetXrp => LedgerId::XrplTestnetXrp,
        LedgerId::HubXrplTestnetDsusdUsd => LedgerId::XrplTestnetDsusdUsd,
        other => other,
    }
}

/// A unit of USD buys one dollar, and `xrp` XRP times 10^15.
fn rates(xrp: u128) -> Rates {
    let mut rates = Rates::new();
    rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
    rates.set_at("XRP", xrp, NOW);
    rates
}

fn pricing(k: u32, fee_ppm: u32, spread_ppm: u32) -> Pricing {
    Pricing {
        charges: Charges {
            fee_ppm,
            spread_ppm,
        },
        rebalance_ppm: k,
    }
}

fn held(ledger: LedgerId, posted: u64) -> Inventory {
    Inventory {
        ledger,
        owner: AccountId(0),
        holding: Holding {
            posted: u128::from(posted),
            ..Holding::NONE
        },
    }
}

/// Two XRP to the dollar.
fn table(inventories: &[Inventory], pricing: Pricing) -> PriceTable {
    PriceTable::build_by_asset(
        &rates(2 * 10u128.pow(RATE_PRECISION)),
        inventories,
        book_of,
        pricing,
        NOW,
        AGE,
    )
}

fn quotes_of(table: &PriceTable, methods: &[LedgerId]) -> Quotes {
    table.quotes(methods, &table.holdings(), GRID, NOW, AGE)
}

/// `bid / ask` as out per in, times 10^15, rounded down.
fn promise_e15(bid: &Side, ask: &Side) -> u128 {
    mul_div(bid.quote, u128::from(RATE_ONE_E15), ask.quote).unwrap()
}

fn levels(side: &Side) -> &[u64] {
    &side.ladder[..GRID.len()]
}

/// The tag of a method on `ledger`, as a snapshot names it.
fn tag(ledger: LedgerId) -> MethodTag {
    method_tag(ledger.as_str())
}

/// The snapshot a node publishes of `quotes` on the default grid, as the
/// wire carries it: each bid rounded down, each ask up, each ladder down.
fn snapshot(quotes: &Quotes) -> Snapshot {
    let one = 10u128.pow(QUOTE_PRECISION);
    let side = |side: &Side, up: bool| {
        let quote = match (side.is_open(), up) {
            (false, _) => dassie_feed::Quote::ZERO,
            (true, false) => dassie_feed::Quote::floor(side.quote, one),
            (true, true) => dassie_feed::Quote::ceil(side.quote, one),
        };
        let ladder = if quote.is_zero() {
            Ladder::ZERO
        } else {
            Ladder::floor(core::array::from_fn(|level| side.ladder[level]))
        };
        (quote, ladder)
    };
    let mut methods: Vec<MethodQuotes> = quotes
        .iter()
        .map(|quote| {
            let ((bid, bid_ladder), (ask, ask_ladder)) =
                (side(&quote.bid, false), side(&quote.ask, true));
            MethodQuotes {
                tag: tag(quote.ledger),
                bid,
                ask,
                bid_ladder,
                ask_ladder,
            }
        })
        .collect();
    methods.sort_by_key(|method| method.tag);
    let snapshot = Snapshot {
        sequence_ms: NOW,
        valid_until_ms: NOW + AGE,
        grid_bp: DEFAULT_GRID_PPM.map(|drop| u16::try_from(drop / 100).unwrap()),
        methods,
    };
    snapshot.check().unwrap();
    snapshot
}

/// Where each level of `from`'s bid ladder and `to`'s ask ladder ends in
/// what comes in on `from`, as a reader's walk reads them: the bid's by the
/// amount, the ask's by its worth at the top-of-book price, the most in
/// whose `in · bid ≤ amount · ask`.
fn step_ends(snapshot: &Snapshot, from: MethodTag, to: MethodTag) -> Vec<u64> {
    let (from, to) = (snapshot.method(from).unwrap(), snapshot.method(to).unwrap());
    let (bid, ask) = (from.bid, to.ask);
    let low = bid.exponent().min(ask.exponent());
    let scaled = |value: BigUint, exponent: i32| value << usize::try_from(exponent - low).unwrap();
    let per = scaled(BigUint::from(bid.mantissa()), bid.exponent());
    let mut ends: Vec<u64> = from.bid_ladder.amounts().to_vec();
    for amount in to.ask_ladder.amounts() {
        let room = scaled(BigUint::from(amount) * ask.mantissa(), ask.exponent());
        ends.push(u64::try_from(room / &per).unwrap_or(u64::MAX));
    }
    ends
}

// A grid is what a snapshot carries: as many drops as its levels, each a
// whole number of basis points below 100%, ascending.
#[test]
fn grids() {
    assert!(grid_is_valid(GRID));
    let ascending = core::array::from_fn(|level| 100 * u32::try_from(level).unwrap());
    assert!(grid_is_valid(&ascending));
    let mut top = ascending;
    top[GRID_LEVELS - 1] = 999_900;
    assert!(grid_is_valid(&top));
    top[GRID_LEVELS - 1] = 1_000_000;
    assert!(!grid_is_valid(&top), "a drop below 100%");
    let mut repeated = ascending;
    repeated[1] = 0;
    assert!(!grid_is_valid(&repeated), "each larger than the one before");
    let mut fine = *GRID;
    fine[0] = 150;
    assert!(!grid_is_valid(&fine), "in whole basis points");
}

// Two assets at their targets, a hundred dollars each, k = 5%: each bid
// and ask is spot, and the ladders say what the curve does, a little less.
// One percent down, the curve takes 25 dollars in (a share of 62.5%) and
// lets 16.8 out (41.6%); the ladders allow for the bonus the other side
// can pay, and say about 24.2 and 16.3.
#[test]
fn at_target_quotes_are_spot_and_ladders_follow_the_curve() {
    let inventories = [held(USD, 100 * DOLLAR), held(XRP, 200 * DOLLAR)];
    let table = table(&inventories, pricing(50_000, 0, 0));
    let quotes = quotes_of(&table, &[USD, XRP]);
    let (usd, xrp) = (quotes.get(USD).unwrap(), quotes.get(XRP).unwrap());
    // A dollar is worth a billion units; a unit of XRP half that.
    let one = 10u128.pow(QUOTE_PRECISION);
    let near = |quote: u128, expected: u128| quote.abs_diff(expected) < expected / 1_000_000_000;
    assert!(near(usd.bid.quote, one) && usd.bid.quote < one);
    assert!(near(usd.ask.quote, one) && usd.ask.quote >= one);
    assert!(near(xrp.bid.quote, one / 2) && near(xrp.ask.quote, one / 2));
    assert!(promise_e15(&usd.bid, &xrp.ask) <= 2 * u128::from(RATE_ONE_E15));
    let at = |side: &Side, drop: u32| side.ladder[GRID.iter().position(|&d| d == drop).unwrap()];
    let dollars_in = at(&usd.bid, 10_000);
    assert!(
        (24 * DOLLAR..25 * DOLLAR).contains(&dollars_in),
        "{dollars_in}"
    );
    // In XRP's units: two to the dollar.
    let xrp_out = at(&xrp.ask, 10_000);
    assert!((32 * DOLLAR..34 * DOLLAR).contains(&xrp_out), "{xrp_out}");
    for side in [usd.bid, usd.ask, xrp.bid, xrp.ask] {
        assert!(levels(&side).windows(2).all(|pair| pair[0] <= pair[1]));
    }
    // Ten percent down, as much as anyone sends comes in: at a share of
    // one, the weight is 0.975.
    assert_eq!(at(&usd.bid, 100_000), u64::MAX);
    assert!(at(&usd.ask, 300_000) < 100 * DOLLAR);
}

// For two assets, the clamp is exact: at any skew, with anything in
// flight, `bid_a / ask_b` is the table's price of a vanishingly small
// packet, less the fee, to within rounding.
#[test]
fn for_two_assets_the_promise_is_the_curve() {
    for (usd, xrp, pending) in [
        (100, 200, 0),
        (25, 150, 0),
        (5, 190, 3),
        (90, 10, 7),
        (1, 1, 0),
    ] {
        for k in [0, 50_000, MAX_REBALANCE_PPM] {
            let mut inventories = [held(USD, usd * DOLLAR), held(HUB, xrp * DOLLAR)];
            inventories[0].holding.credits_pending = u128::from(pending * DOLLAR);
            inventories[1].holding.debits_pending = u128::from(pending * DOLLAR);
            let table = table(&inventories, pricing(k, 2_000, 3_000));
            let quotes = quotes_of(&table, &[USD, HUB]);
            for (from, to) in [(USD, HUB), (HUB, USD)] {
                let (bid, ask) = (quotes.get(from).unwrap().bid, quotes.get(to).unwrap().ask);
                let rate = u128::from(table.price(from, to).unwrap().rate_e15);
                let after_fee = rate * (1_000_000 - 2_000) / 1_000_000;
                let promise = promise_e15(&bid, &ask);
                assert!(promise <= after_fee, "{from} {to} {promise} {after_fee}");
                assert!(
                    after_fee - promise <= after_fee / 1_000_000_000 + 1,
                    "{promise} {after_fee}"
                );
            }
        }
    }
}

// With more than two assets, the clamp understates pairs whose other side
// is not the most plentiful, and never overstates any.
#[test]
fn with_more_assets_the_promise_understates() {
    let inventories = [
        held(USD, 10 * DOLLAR),
        held(XRP, 300 * DOLLAR),
        held(RLUSD, 60 * DOLLAR),
        held(DSUSD_HUB, 40 * DOLLAR),
    ];
    let table = table(&inventories, pricing(50_000, 0, 0));
    let methods = [USD, XRP, RLUSD, DSUSD_HUB];
    let quotes = quotes_of(&table, &methods);
    let mut under = 0;
    for from in methods {
        for to in methods.into_iter().filter(|&to| to != from) {
            let promise = promise_e15(&quotes.get(from).unwrap().bid, &quotes.get(to).unwrap().ask);
            let rate = u128::from(table.price(from, to).unwrap().rate_e15);
            assert!(promise <= rate, "{from} {to}");
            under += usize::from(promise < rate * 999 / 1_000);
        }
    }
    // Dollars, the scarcest, are clamped against XRP, the most plentiful.
    assert!(under > 0);
}

#[test]
fn sides_without_a_rate_or_funds_are_closed() {
    let k = pricing(50_000, 0, 0);
    // DSUSD has no ledger in the table: closed, both ways.
    let inventories = [held(USD, 100 * DOLLAR), held(XRP, 100 * DOLLAR)];
    let table = table(&inventories, k);
    let quotes = quotes_of(&table, &[USD, XRP, DSUSD_HUB]);
    let closed = quotes.get(DSUSD_HUB).unwrap();
    assert_eq!((closed.bid, closed.ask), (Side::CLOSED, Side::CLOSED));
    assert!(quotes.get(USD).unwrap().bid.is_open());
    assert_eq!(quotes.iter().count(), 3);
    // A snapshot of them prices no swap into or out of the closed method.
    let wire = snapshot(&quotes);
    assert_eq!(wire.swap_out(tag(USD), tag(DSUSD_HUB), 1), None);
    assert_eq!(wire.swap_out(tag(DSUSD_HUB), tag(USD), 1), None);
    assert!(wire.swap_out(tag(USD), tag(XRP), 1).is_some());
    // A rate gone stale since the table was built closes its method.
    let late = table.quotes(&[USD, XRP], &table.holdings(), GRID, NOW + AGE + 1, AGE);
    assert!(
        late.iter()
            .all(|quote| !quote.bid.is_open() && !quote.ask.is_open())
    );
    // Without a rate for XRP, only dollars are priced, and with nothing to
    // swap into, its bid is closed too; its ask stays, with no ladder.
    let mut dollars = Rates::new();
    dollars.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
    let unrated = PriceTable::build_by_asset(&dollars, &inventories, book_of, k, NOW, AGE);
    let quotes = quotes_of(&unrated, &[USD, XRP]);
    let (usd, xrp) = (quotes.get(USD).unwrap(), quotes.get(XRP).unwrap());
    assert!(!usd.bid.is_open() && usd.ask.is_open() && levels(&usd.ask).iter().all(|&l| l == 0));
    assert_eq!((xrp.bid, xrp.ask), (Side::CLOSED, Side::CLOSED));
    // Nothing of XRP to pay out: its ask is closed, its bid open.
    let none = [held(USD, 100 * DOLLAR), held(XRP, 0), held(HUB, 0)];
    let table = self::table(&none, k);
    let quotes = quotes_of(&table, &[USD, HUB]);
    let (usd, hub) = (quotes.get(USD).unwrap(), quotes.get(HUB).unwrap());
    assert!(hub.bid.is_open() && !hub.ask.is_open());
    assert!(!usd.bid.is_open() && usd.ask.is_open());
    // Bringing in XRP the node lacks gets the capped bonus, whatever comes
    // in on the first levels: the clamp is the cap.
    assert!(levels(&hub.bid).iter().all(|&most| most > 0));
}

// spec/decisions/peer-swaps.md: ladders count each asset whole. A hub
// that holds nothing of XRP the owner holds plenty of in its book quotes
// as if the hub held it all; a swap the hub cannot fund gets T04.
#[test]
fn ladders_count_each_asset_whole() {
    let k = pricing(50_000, 0, 0);
    let whole = [
        held(USD, 100 * DOLLAR),
        held(XRP, 0),
        held(HUB, 200 * DOLLAR),
    ];
    let split = [
        held(USD, 100 * DOLLAR),
        held(XRP, 200 * DOLLAR),
        held(HUB, 0),
    ];
    let (whole, split) = (table(&whole, k), table(&split, k));
    assert_eq!(
        quotes_of(&whole, &[USD, HUB]),
        quotes_of(&split, &[USD, HUB])
    );
    let holdings = split.holdings();
    assert_eq!(
        split.rate(USD, HUB, DOLLAR, DOLLAR, &holdings),
        Err(PriceError::NoFunds)
    );
}

#[test]
#[should_panic(expected = "one method per asset")]
fn one_method_per_asset() {
    let inventories = [held(USD, DOLLAR), held(XRP, DOLLAR), held(HUB, DOLLAR)];
    let table = table(&inventories, pricing(0, 0, 0));
    let _ = quotes_of(&table, &[USD, XRP, HUB]);
}

#[test]
#[should_panic(expected = "a method is named once")]
fn a_method_is_named_once() {
    let inventories = [held(USD, DOLLAR), held(XRP, DOLLAR)];
    let table = table(&inventories, pricing(0, 0, 0));
    let _ = quotes_of(&table, &[USD, XRP, USD]);
}

/// What one ledger holds, as a strategy draws it: `m · 10^e` posted, and
/// pending credits and debits of `c · 10^p` and `d · 10^p`, the debits at
/// most what is posted.
type Drawn = (u64, u32, u64, u64, u32);

fn holding((mantissa, exponent, credits, debits, pending): Drawn) -> Holding {
    let posted = u128::from(mantissa) * 10u128.pow(exponent);
    let scale = 10u128.pow(pending);
    Holding {
        posted,
        credits_pending: u128::from(credits) * scale,
        debits_pending: (u128::from(debits) * scale).min(posted),
    }
}

/// The ledgers of the test realm, by asset: each asset's book, and its
/// hub's ledger if it has one.
const ASSETS: [(LedgerId, Option<LedgerId>); 4] = [
    (USD, None),
    (XRP, Some(HUB)),
    (RLUSD, None),
    (DSUSD, Some(DSUSD_HUB)),
];

/// A table of the assets `chosen` picks, at least two, each ledger holding
/// what was drawn for it, and the method of each: its hub's ledger, or its
/// book where `on_book` says so or it has no hub.
fn drawn_table(
    chosen: u8,
    drawn: &[Drawn; 6],
    on_book: u8,
    xrp_rate: u128,
    pricing: Pricing,
) -> Option<(PriceTable, Vec<LedgerId>)> {
    let mut inventories = Vec::new();
    let mut methods = Vec::new();
    let mut draws = drawn.iter();
    for (index, (book, hub)) in ASSETS.into_iter().enumerate() {
        let picked = chosen & (1 << index) != 0;
        for ledger in core::iter::once(book).chain(hub) {
            let draw = *draws.next().unwrap();
            if picked {
                inventories.push(Inventory {
                    ledger,
                    owner: AccountId(0),
                    holding: holding(draw),
                });
            }
        }
        if picked {
            let book_first = on_book & (1 << index) != 0;
            methods.push(hub.filter(|_| !book_first).unwrap_or(book));
        }
    }
    let rates = rates(xrp_rate);
    let table = PriceTable::build_by_asset(&rates, &inventories, book_of, pricing, NOW, AGE);
    (methods.len() >= 2).then_some((table, methods))
}

/// A packet of a swap: what goes in, and the least it is promised out.
type Packet = (u64, u64);

/// How a swap is cut into packets.
#[derive(Clone, Copy, Debug)]
enum Cuts {
    /// One packet per step of the reader's walk.
    Steps,
    /// One packet for all of it.
    Whole,
    /// At these shares of the swap, in parts per million.
    At([u64; 3]),
}

/// The packets of a swap of `total` from `from` to `to`, cut by `cuts`,
/// each promised what the snapshot's reader sets as its minimum
/// (`Snapshot::packet_out`): the rate of the step its last unit falls in.
fn packets(
    snapshot: &Snapshot,
    (from, to): (MethodTag, MethodTag),
    total: u64,
    cuts: Cuts,
) -> Vec<Packet> {
    let mut ends: Vec<u64> = match cuts {
        Cuts::Steps => step_ends(snapshot, from, to),
        Cuts::Whole => Vec::new(),
        Cuts::At(shares) => shares
            .iter()
            .map(|&share| u64::try_from(u128::from(total) * u128::from(share) / 1_000_000).unwrap())
            .collect(),
    };
    ends.retain(|&end| end > 0 && end < total);
    ends.push(total);
    ends.sort_unstable();
    ends.dedup();
    let mut start = 0;
    let mut packets = Vec::new();
    for end in ends {
        let promised = snapshot.packet_out(from, to, start, end - start).unwrap();
        packets.push((end - start, promised));
        start = end;
    }
    packets
}

/// What each packet of a swap from `from` to `to` gets, sent one after
/// another or together, against what the snapshot promises it: never
/// less, unless the curve refuses the packet for want of funds on the out
/// method's own ledger, which ladders do not count.
fn check_swap(
    table: &PriceTable,
    (from, to): (LedgerId, LedgerId),
    packets: &[Packet],
    together: bool,
) -> Result<(), TestCaseError> {
    let index = |ledger| {
        table
            .ledgers()
            .position(|entry| entry.ledger == ledger)
            .unwrap()
    };
    let (a, b) = (index(from), index(to));
    let mut state = table.holdings();
    for (count, &(amount, promised)) in packets.iter().enumerate() {
        let promised = u128::from(promised);
        let net = amount - table.charges().fee(amount);
        let out = match table.rate(from, to, amount, net, &state) {
            Ok(rate_e15) => u128::from(net) * u128::from(rate_e15) / u128::from(RATE_ONE_E15),
            Err(error) => {
                let short = value_of(state[b].least(), table.ledger(to).unwrap())
                    <= value_of(u128::from(net), table.ledger(from).unwrap());
                proptest::prop_assert!(
                    promised == 0 || (error == PriceError::NoFunds && short),
                    "packet {count}: {error:?} with {promised} promised"
                );
                return Ok(());
            }
        };
        proptest::prop_assert!(out >= promised, "packet {count}: {out} < {promised}");
        if together {
            state[a].credits_pending += u128::from(amount);
            state[b].debits_pending += out;
        } else {
            state[a].posted += u128::from(amount);
            state[b].posted = state[b].posted.saturating_sub(out);
        }
    }
    Ok(())
}

fn cuts() -> impl proptest::strategy::Strategy<Value = Cuts> {
    use proptest::strategy::Strategy;
    proptest::prop_oneof![
        proptest::strategy::Just(Cuts::Steps),
        proptest::strategy::Just(Cuts::Whole),
        proptest::array::uniform3(0u64..=1_000_000).prop_map(Cuts::At),
    ]
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(4_096))]

    // spec/decisions/peer-swaps.md: the promise. For random holdings and
    // what is in flight on every ledger, two to four assets, any `k`, fee
    // and spread, any pair and any size the ladders cover, the quotes
    // published as a snapshot and read by its own reader, `dassie-feed`'s,
    // a swap cut into a packet per step of the reader's walk, one packet,
    // or packets anywhere, each with the minimum the reader sets on it,
    // and sent one after another or together: no packet gets less out
    // than its minimum.
    #[test]
    fn the_quotes_never_promise_more_than_the_curve_gives(
        chosen in 0u8..16,
        drawn in proptest::array::uniform6((0u64..1_000, 0u32..13, 0u64..1_000, 0u64..1_000, 0u32..12)),
        on_book in 0u8..16,
        xrp_rate in 10_000_000_000_000u128..100_000_000_000_000_000,
        k in 0u32..=MAX_REBALANCE_PPM,
        fee_ppm in 0u32..=Charges::MAX_PPM,
        spread_ppm in 0u32..=Charges::MAX_PPM,
        pair in (0usize..4, 0usize..3),
        fraction in proptest::prop_oneof![proptest::strategy::Just(1_000_000u64), 0u64..=1_000_000],
        cuts in cuts(),
        together: bool,
    ) {
        let pricing = pricing(k, fee_ppm, spread_ppm);
        let Some((table, methods)) = drawn_table(chosen, &drawn, on_book, xrp_rate, pricing) else {
            return Ok(());
        };
        let quotes = table.quotes(&methods, &table.holdings(), GRID, NOW, AGE);
        for quote in quotes.iter() {
            for side in [quote.bid, quote.ask] {
                proptest::prop_assert!(levels(&side).windows(2).all(|pair| pair[0] <= pair[1]));
                proptest::prop_assert!(side.is_open() || side == Side::CLOSED);
            }
        }
        let wire = snapshot(&quotes);
        let count = methods.len();
        let from = methods[pair.0 % count];
        let to = methods[(pair.0 % count + 1 + pair.1 % (count - 1)) % count];
        let tags = (tag(from), tag(to));
        let Some(most) = wire.swap_at_least(tags.0, tags.1, Ratio::new(0, 1).unwrap()) else {
            return Ok(());
        };
        let total = u64::try_from(u128::from(most.amount_in) * u128::from(fraction) / 1_000_000).unwrap();
        let packets = packets(&wire, tags, total, cuts);
        if matches!(cuts, Cuts::Steps) {
            // A packet per step gets what the walk adds up.
            let sum: u64 = packets.iter().map(|&(_, promised)| promised).sum();
            proptest::prop_assert_eq!(Some(sum), wire.swap_out(tags.0, tags.1, total));
        }
        check_swap(&table, (from, to), &packets, together)?;
    }

    // For two assets, more of the one coming in, or more of it on its way
    // in, never makes the promise better.
    #[test]
    fn the_promise_falls_with_the_skew(
        usd in 1u64..1_000,
        xrp in 1u64..1_000,
        more in 1u64..1_000,
        k in 0u32..=MAX_REBALANCE_PPM,
    ) {
        let pricing = pricing(k, 1_000, 1_000);
        let at = |usd: u64, coming: u64| {
            let mut inventories = [held(USD, usd * DOLLAR), held(HUB, 2 * xrp * DOLLAR)];
            inventories[0].holding.credits_pending = u128::from(coming * DOLLAR);
            let table = table(&inventories, pricing);
            let quotes = quotes_of(&table, &[USD, HUB]);
            let (bid, ask) = (quotes.get(USD).unwrap().bid, quotes.get(HUB).unwrap().ask);
            promise_e15(&bid, &ask)
        };
        let base = at(usd, 0);
        proptest::prop_assert!(at(usd + more, 0) <= base);
        proptest::prop_assert!(at(usd, more) <= base);
    }
}

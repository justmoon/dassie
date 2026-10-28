#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use dassie_connector::{Charges, Holding, Inventory, Pricing, RATE_PRECISION, Rates};
use dassie_feed::{FEED_HEARTBEAT_MS, FEED_VALIDITY_MS, SnapshotReader, decode_snapshot_datagram};
use dassie_ledger::AccountId;

use super::*;

const NOW: u64 = 1_800_000_000_000;
const AGE: u64 = 3_600_000;
const USD: LedgerId = LedgerId::StubUsd;
/// The testnet's XRP hub, a method's ledger.
const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
/// The owner book that funds it, which no peer pays the node on.
const BOOK: LedgerId = LedgerId::XrplTestnetXrp;
const STUB_ID: &str = "stub";
const HUB_ID: &str = "hub-xrpl-testnet-xrp";

fn key() -> Arc<IdentityKey> {
    Arc::new(IdentityKey::from_bytes(&[7; 32]))
}

fn scheme(id: &str) -> SchemeId {
    SchemeId::parse(id).unwrap()
}

/// The methods the node offers: the stub and the XRP hub.
fn methods() -> Vec<(SchemeId, LedgerId)> {
    vec![(scheme(STUB_ID), USD), (scheme(HUB_ID), HUB)]
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

/// Dollars and XRP at 2 XRP a dollar, learned at `as_of_ms`.
fn rates_at(as_of_ms: u64) -> Rates {
    let mut rates = Rates::new();
    rates.set_at("USD", 10u128.pow(RATE_PRECISION), as_of_ms);
    rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), as_of_ms);
    rates
}

/// The owner holding `usd` on the stub and `xrp` in the XRP hub, with a
/// fee of 1000 ppm, its rates learned at `as_of_ms`; each asset whole, the
/// hub with its owner book.
fn table_of(inventories: &[Inventory], as_of_ms: u64) -> PriceTable {
    let pricing = Pricing {
        charges: Charges {
            fee_ppm: 1_000,
            spread_ppm: 2_000,
        },
        rebalance_ppm: 50_000,
    };
    PriceTable::build_by_asset(
        &rates_at(as_of_ms),
        inventories,
        |ledger| if ledger == HUB { BOOK } else { ledger },
        pricing,
        NOW,
        AGE,
    )
}

fn table(usd: u64, xrp: u64) -> PriceTable {
    table_of(&[held(USD, usd), held(HUB, xrp)], NOW)
}

fn quoting<'a>(table: &'a PriceTable, methods: &'a [(SchemeId, LedgerId)]) -> Quoting<'a> {
    Quoting {
        table,
        methods,
        max_rate_age_ms: AGE,
    }
}

/// What `bytes` say, verified under the node's key.
fn read(bytes: &[u8], now_ms: u64) -> Snapshot {
    SnapshotReader::new(key().public_key())
        .receive(bytes, now_ms)
        .unwrap()
}

/// The tags of a snapshot's methods.
fn tags(snapshot: &Snapshot) -> Vec<MethodTag> {
    snapshot.methods.iter().map(|method| method.tag).collect()
}

/// The two methods' tags, in the snapshot's order.
fn both_tags() -> Vec<MethodTag> {
    let mut tags = vec![method_tag(STUB_ID), method_tag(HUB_ID)];
    tags.sort_unstable();
    tags
}

// spec/areas/rate-feed.md#what-it-publishes: the grid as a snapshot
// carries it, in basis points, each drop rounded up.
#[test]
fn the_grid_in_basis_points() {
    assert_eq!(
        grid_bp(&DEFAULT_GRID_PPM),
        [1, 3, 10, 30, 100, 300, 1_000, 3_000]
    );
}

// spec/areas/rate-feed.md#what-it-publishes: a snapshot quotes the
// methods the node offers whose ledger the table prices, by their tags,
// each side as the connector's quotes say it, rounded against the reader;
// it is signed with the node key and counts until the schedule's validity.
#[test]
fn a_snapshot_quotes_the_offered_methods_as_the_table_does() {
    let table = table(100_000_000_000, 150_000_000_000);
    let methods = methods();
    let mut own = OwnSnapshots::new(key());
    let bytes = own.current(NOW, &quoting(&table, &methods)).unwrap();
    let snapshot = read(&bytes, NOW);
    assert_eq!(snapshot.sequence_ms, NOW);
    assert_eq!(snapshot.valid_until_ms, NOW + FEED_VALIDITY_MS);
    assert_eq!(snapshot.grid_bp, grid_bp(&DEFAULT_GRID_PPM));
    assert_eq!(tags(&snapshot), both_tags());
    let quotes = table.quotes(&[USD, HUB], &table.holdings(), &DEFAULT_GRID_PPM, NOW, AGE);
    let one = 10u128.pow(QUOTE_PRECISION);
    for (id, ledger) in [(STUB_ID, USD), (HUB_ID, HUB)] {
        let said = snapshot.method(method_tag(id)).unwrap();
        let quote = quotes.get(ledger).unwrap();
        assert!(quote.bid.is_open() && quote.ask.is_open());
        assert_eq!(said.bid, Quote::floor(quote.bid.quote, one));
        assert_eq!(said.ask, Quote::ceil(quote.ask.quote, one));
        assert_eq!(said.bid_ladder, Ladder::floor(quote.bid.ladder));
        assert_eq!(said.ask_ladder, Ladder::floor(quote.ask.ladder));
    }
    // A swap between them is priced, at a little less than the table's
    // price for a small packet.
    let out = snapshot
        .swap_out(method_tag(STUB_ID), method_tag(HUB_ID), 1_000_000)
        .unwrap();
    let price = table.price(USD, HUB).unwrap().rate_e15;
    let table_out = u128::from(price) * 1_000_000 / 10u128.pow(15);
    assert!(out > 0 && u128::from(out) <= table_out, "{out} {table_out}");
}

// An owner book, which no peer can pay the node on, is not quoted, though
// the table prices it; nor is a method on a ledger the table does not
// price, as one without a fresh rate.
#[test]
fn only_offered_methods_the_table_prices_are_quoted() {
    let inventories = [
        held(HUB, 20_000_000_000),
        held(BOOK, 30_000_000_000),
        held(USD, 10_000_000_000),
    ];
    let table = table_of(&inventories, NOW);
    assert!(table.ledger(BOOK).is_some());
    let mut offered = methods();
    let dsusd = LedgerId::HubXrplTestnetDsusdUsd;
    assert!(table.ledger(dsusd).is_none());
    offered.push((scheme("hub-xrpl-testnet-dsusd"), dsusd));
    let content = Content::of(&quoting(&table, &offered), &DEFAULT_GRID_PPM, NOW);
    let tags: Vec<MethodTag> = content.methods.iter().map(|method| method.tag).collect();
    assert_eq!(tags, both_tags());
}

// spec/decisions/peer-swaps.md#accepted: two methods on one asset, which a
// bid over an ask cannot price, are neither quoted; the others are.
#[test]
fn two_methods_of_one_asset_are_neither_quoted() {
    let inventories = [
        held(HUB, 20_000_000_000),
        held(BOOK, 30_000_000_000),
        held(USD, 10_000_000_000),
    ];
    let table = table_of(&inventories, NOW);
    let mut offered = methods();
    offered.push((scheme("xrpl-testnet-xrp"), BOOK));
    let content = Content::of(&quoting(&table, &offered), &DEFAULT_GRID_PPM, NOW);
    let tags: Vec<MethodTag> = content.methods.iter().map(|method| method.tag).collect();
    assert_eq!(tags, [method_tag(STUB_ID)]);
}

// spec/areas/rate-feed.md#bounds: every scheme of a realm has a tag of its
// own, so a node never offers two methods whose tags collide.
#[test]
fn no_two_schemes_share_a_tag() {
    let mut tags: Vec<MethodTag> = super::super::SCHEMES
        .iter()
        .map(|info| method_tag(info.id))
        .collect();
    let schemes = tags.len();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(tags.len(), schemes);
}

// spec/areas/rate-feed.md#what-it-publishes: the snapshot is made again at
// most once per least interval however often the table changes, and
// signed again only when what it quotes changed or the heartbeat is due;
// in between, the same bytes.
#[test]
fn snapshots_are_made_on_the_feeds_schedule() {
    let methods = methods();
    let mut own = OwnSnapshots::new(key());
    let first_table = table(100_000_000_000, 150_000_000_000);
    let first = own.current(NOW, &quoting(&first_table, &methods)).unwrap();
    assert_eq!(own.next_ms(), Some(NOW + FEED_HEARTBEAT_MS));
    let again = own.current(NOW + 10, &quoting(&first_table, &methods));
    assert!(Arc::ptr_eq(&first, &again.unwrap()), "nothing changed");

    // A change within the interval waits for its end.
    let moved = table(300_000_000_000, 20_000_000_000);
    let held_back = own.current(NOW + 20, &quoting(&moved, &methods)).unwrap();
    assert!(Arc::ptr_eq(&first, &held_back));
    assert_eq!(own.next_ms(), Some(NOW + FEED_MIN_INTERVAL_MS));
    let at = NOW + FEED_MIN_INTERVAL_MS;
    let changed = own.current(at, &quoting(&moved, &methods)).unwrap();
    assert!(!Arc::ptr_eq(&first, &changed));
    let (before, after) = (read(&first, NOW), read(&changed, at));
    assert_eq!(after.sequence_ms, at);
    assert_ne!(before.methods, after.methods);
    assert_eq!(own.next_ms(), Some(at + FEED_HEARTBEAT_MS));

    // A table built again that quotes the same is no change: the same
    // bytes until the heartbeat, which signs them again, newer.
    let rebuilt = table(300_000_000_000, 20_000_000_000);
    let same = own.current(at + FEED_MIN_INTERVAL_MS, &quoting(&rebuilt, &methods));
    assert!(Arc::ptr_eq(&changed, &same.unwrap()));
    let beat_at = at + FEED_HEARTBEAT_MS;
    let beat = own.current(beat_at, &quoting(&rebuilt, &methods)).unwrap();
    let beat = read(&beat, beat_at);
    assert_eq!(beat.sequence_ms, beat_at);
    assert_eq!(beat.methods, after.methods);
}

// A snapshot never outlives the rates it quotes: it ends when they age
// out, and one made after quotes nothing.
#[test]
fn a_snapshot_ends_with_the_tables_rates() {
    let as_of_ms = NOW - AGE + 10_000;
    let table = table_of(
        &[held(USD, 1_000_000_000), held(HUB, 2_000_000_000)],
        as_of_ms,
    );
    let ends_ms = table.valid_until_ms();
    assert!(ends_ms < NOW + FEED_VALIDITY_MS);
    let methods = methods();
    let mut own = OwnSnapshots::new(key());
    let bytes = own.current(NOW, &quoting(&table, &methods)).unwrap();
    let snapshot = read(&bytes, NOW);
    assert_eq!(snapshot.valid_until_ms, ends_ms);
    assert_eq!(snapshot.methods.len(), 2);
    // Past it, the bytes are none; the heartbeat after says nothing.
    assert!(own.current(ends_ms, &quoting(&table, &methods)).is_none());
    let later = NOW + FEED_HEARTBEAT_MS;
    let bytes = own.current(later, &quoting(&table, &methods)).unwrap();
    let Ok(dassie_feed::SnapshotMessage::Snapshot(signed)) = decode_snapshot_datagram(&bytes)
    else {
        panic!("a snapshot");
    };
    assert!(signed.snapshot.methods.is_empty());
    assert_eq!(signed.snapshot.valid_until_ms, later + FEED_VALIDITY_MS);
}

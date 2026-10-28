//! The node's own snapshot (spec/areas/rate-feed.md#the-node-side,
//! spec/decisions/peer-swaps.md): its quotes and ladders per settlement
//! method it offers, from the connector's price table, signed by its node
//! key and cached.
//!
//! What a snapshot says is a pure function of the table, the methods and
//! the time ([`Content::of`]); [`OwnSnapshots`] decides when to make it
//! again and when to sign it, on the rate feed's schedule, and is the
//! engine's [`SnapshotSource`]. It quotes again at most once per
//! [`FEED_MIN_INTERVAL_MS`] however often the table changes, signs only
//! when the quotes changed or the heartbeat is due, and hands out the same
//! bytes in between.

use std::sync::Arc;

use dassie_connector::{
    DEFAULT_GRID_PPM, GRID_LEVELS, GRID_STEP_PPM, PriceTable, QUOTE_PRECISION, Side, grid_is_valid,
};
use dassie_feed::{
    FEED_MIN_INTERVAL_MS, LEVELS, Ladder, MAX_SNAPSHOT_METHODS, MethodQuotes, MethodTag, Poll,
    Quote, Schedule, Snapshot, method_tag, sign_snapshot,
};
use dassie_identity::IdentityKey;
use dassie_ledger::LedgerId;
use dassie_peer::wire::SchemeId;

use super::snapshots::{Quoting, SnapshotSource};

// A grid is exactly what a snapshot carries, and every method a node can
// offer fits in one: it lists at most one method per ledger of its realm,
// and a realm has at most `MAX_PRICED_LEDGERS` ledgers.
const _: () = assert!(GRID_LEVELS == LEVELS);
const _: () = assert!(grid_is_valid(&DEFAULT_GRID_PPM));
const _: () = assert!(dassie_connector::pricing::MAX_PRICED_LEDGERS <= MAX_SNAPSHOT_METHODS);
// A drop in basis points is below 100%, as a snapshot's drops are.
const _: () = assert!(GRID_STEP_PPM == 1_000_000 / dassie_feed::DROP_BP_LIMIT as u32);

/// `grid`, in parts per million, as a snapshot carries it: in basis
/// points, each drop rounded up, since a drop said smaller than it is
/// would promise more than the ladders give. A valid grid is in whole
/// basis points, so nothing rounds.
#[must_use]
pub fn grid_bp(grid: &[u32; GRID_LEVELS]) -> [u16; LEVELS] {
    assert!(grid_is_valid(grid), "a grid is checked before it is used");
    grid.map(|drop| {
        let bp = drop.div_ceil(GRID_STEP_PPM);
        // Below 100%, so below `DROP_BP_LIMIT`.
        u16::try_from(bp).unwrap_or(u16::MAX)
    })
}

/// What a snapshot says besides when it was made: its methods' quotes, and
/// until when the table's rates hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    /// In ascending order of their tags.
    methods: Vec<MethodQuotes>,
    /// Until when the table's rates hold: a snapshot made later quotes
    /// every method closed.
    valid_until_ms: u64,
}

/// A method the node offers, as a snapshot lists it.
#[derive(Clone, Copy, Debug)]
struct Listed {
    tag: MethodTag,
    ledger: LedgerId,
    /// The asset it counts toward, if the table prices its ledger.
    asset: Option<LedgerId>,
}

impl Content {
    /// What `quoting` says at `now_ms` on `grid`: each method the node
    /// offers, by its tag. One whose ledger the table prices has its bid
    /// and ask and their ladders, as the connector quotes them; one whose
    /// ledger it does not price, for want of a fresh rate or a dollar
    /// token's hub off its peg, is listed with both sides closed, so that a
    /// reader tells a method the node offers but does not convert on now
    /// from one it does not offer. A ledger no method is on, such as an
    /// owner book's, is not listed. Two methods whose tags collide, or two
    /// the table prices as one asset, which a bid over an ask cannot price
    /// (spec/decisions/peer-swaps.md#accepted), are neither listed.
    #[must_use]
    pub fn of(quoting: &Quoting<'_>, grid: &[u32; GRID_LEVELS], now_ms: u64) -> Self {
        let table = quoting.table;
        let mut offered: Vec<Listed> = quoting
            .methods
            .iter()
            .map(|(scheme, ledger)| Listed {
                tag: tag_of(*scheme),
                ledger: *ledger,
                asset: table.ledger(*ledger).map(|entry| entry.asset),
            })
            .collect();
        offered.sort_unstable_by_key(|method| method.tag);
        let clashes = |a: &Listed, b: &Listed| {
            a.tag == b.tag || a.ledger == b.ledger || (a.asset.is_some() && a.asset == b.asset)
        };
        let listed: Vec<Listed> = offered
            .iter()
            .enumerate()
            .filter(|(index, method)| {
                !offered
                    .iter()
                    .enumerate()
                    .any(|(other, them)| other != *index && clashes(method, them))
            })
            .map(|(_, method)| *method)
            .collect();
        let priced: Vec<LedgerId> = listed
            .iter()
            .filter(|method| method.asset.is_some())
            .map(|method| method.ledger)
            .collect();
        let quotes = table.quotes(
            &priced,
            &table.holdings(),
            grid,
            now_ms,
            quoting.max_rate_age_ms,
        );
        let methods: Vec<MethodQuotes> = listed
            .iter()
            .map(|method| {
                let Some(quote) = quotes.get(method.ledger) else {
                    return closed(method.tag);
                };
                let (bid, bid_ladder) = side(&quote.bid, false);
                let (ask, ask_ladder) = side(&quote.ask, true);
                MethodQuotes {
                    tag: method.tag,
                    bid,
                    ask,
                    bid_ladder,
                    ask_ladder,
                }
            })
            .collect();
        assert!(methods.len() == listed.len() && methods.len() <= MAX_SNAPSHOT_METHODS);
        assert!(
            methods
                .windows(2)
                .all(|pair| matches!(pair, [a, b] if a.tag < b.tag)),
            "in ascending order of their tags, each once"
        );
        Self {
            methods,
            valid_until_ms: table.valid_until_ms(),
        }
    }

    /// The snapshot of this content made at `sequence_ms`, valid until
    /// `valid_until_ms` or until the table's rates age out, whichever is
    /// first. Rates that aged out by `sequence_ms` are not said: every
    /// method is listed closed then.
    fn snapshot(&self, grid_bp: [u16; LEVELS], sequence_ms: u64, valid_until_ms: u64) -> Snapshot {
        let fresh = self.valid_until_ms > sequence_ms;
        Snapshot {
            sequence_ms,
            valid_until_ms: if fresh {
                valid_until_ms.min(self.valid_until_ms)
            } else {
                valid_until_ms
            },
            grid_bp,
            methods: if fresh {
                self.methods.clone()
            } else {
                self.methods
                    .iter()
                    .map(|method| closed(method.tag))
                    .collect()
            },
        }
    }
}

/// A method listed with both sides closed: the node offers it, but
/// converts nothing on it now.
const fn closed(tag: MethodTag) -> MethodQuotes {
    MethodQuotes {
        tag,
        bid: Quote::ZERO,
        ask: Quote::ZERO,
        bid_ladder: Ladder::ZERO,
        ask_ladder: Ladder::ZERO,
    }
}

/// A method's tag: of its scheme's ID.
fn tag_of(scheme: SchemeId) -> MethodTag {
    method_tag(scheme.as_str())
}

/// A side as a snapshot carries it: a bid rounded down, an ask up, each
/// ladder down, and a closed side's ladder all zero.
fn side(side: &Side, ask: bool) -> (Quote, Ladder) {
    let one = 10u128.pow(QUOTE_PRECISION);
    let quote = match (side.is_open(), ask) {
        (false, _) => Quote::ZERO,
        (true, false) => Quote::floor(side.quote, one),
        (true, true) => Quote::ceil(side.quote, one),
    };
    // A bid below the smallest quote rounds to zero: its side is closed.
    let ladder = if quote.is_zero() {
        Ladder::ZERO
    } else {
        Ladder::floor(side.ladder)
    };
    (quote, ladder)
}

/// What the quotes were last made from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inputs {
    table: PriceTable,
    methods: Vec<(SchemeId, LedgerId)>,
    max_rate_age_ms: u64,
}

/// The node's own snapshots: made from the price table on the rate feed's
/// schedule, signed with the node key, and cached.
pub struct OwnSnapshots {
    key: Arc<IdentityKey>,
    grid: [u32; GRID_LEVELS],
    schedule: Schedule,
    /// What the content was last made from.
    inputs: Option<Inputs>,
    /// Whether the inputs changed since the content was made.
    stale: bool,
    /// When the content was last made.
    quoted_at_ms: Option<u64>,
    content: Option<Content>,
    /// The last snapshot signed, and until when it counts.
    signed: Option<(Arc<[u8]>, u64)>,
    /// When to be asked again, at the latest.
    next_ms: Option<u64>,
}

impl OwnSnapshots {
    /// Snapshots signed with `key`, on the default grid.
    #[must_use]
    pub fn new(key: Arc<IdentityKey>) -> Self {
        Self {
            key,
            grid: DEFAULT_GRID_PPM,
            schedule: Schedule::new(),
            inputs: None,
            stale: false,
            quoted_at_ms: None,
            content: None,
            signed: None,
            next_ms: None,
        }
    }

    /// Whether the content may be made again at `now_ms`: once the least
    /// interval passed since it was last, or the clock went back.
    fn may_quote(&self, now_ms: u64) -> bool {
        self.quoted_at_ms
            .is_none_or(|at| now_ms < at || now_ms >= at.saturating_add(FEED_MIN_INTERVAL_MS))
    }
}

impl SnapshotSource for OwnSnapshots {
    fn current(&mut self, now_ms: u64, quoting: &Quoting<'_>) -> Option<Arc<[u8]>> {
        let same = self.inputs.as_ref().is_some_and(|inputs| {
            inputs.table == *quoting.table
                && inputs.methods == quoting.methods
                && inputs.max_rate_age_ms == quoting.max_rate_age_ms
        });
        if !same {
            self.inputs = Some(Inputs {
                table: *quoting.table,
                methods: quoting.methods.to_vec(),
                max_rate_age_ms: quoting.max_rate_age_ms,
            });
            self.stale = true;
        }
        let mut changed = false;
        if (self.stale || self.content.is_none()) && self.may_quote(now_ms) {
            let content = Content::of(quoting, &self.grid, now_ms);
            changed = self.content.as_ref() != Some(&content);
            self.content = Some(content);
            self.stale = false;
            self.quoted_at_ms = Some(now_ms);
        }
        let content = self.content.as_ref()?;
        let poll = self.schedule.poll(now_ms, changed);
        let next_poll_ms = match poll {
            Poll::Publish {
                sequence_ms,
                valid_until_ms,
                next_ms,
            } => {
                let snapshot = content.snapshot(grid_bp(&self.grid), sequence_ms, valid_until_ms);
                let signed = sign_snapshot(&self.key, &snapshot);
                assert!(signed.is_ok(), "the node's snapshot keeps the rules");
                self.signed = signed
                    .ok()
                    .map(|bytes| (Arc::from(bytes), snapshot.valid_until_ms));
                next_ms
            }
            Poll::Wait { next_ms } => next_ms,
        };
        let requote_ms = self
            .quoted_at_ms
            .filter(|_| self.stale)
            .map(|at| at.saturating_add(FEED_MIN_INTERVAL_MS));
        self.next_ms = [Some(next_poll_ms), requote_ms].into_iter().flatten().min();
        self.signed
            .as_ref()
            .filter(|(_, valid_until_ms)| *valid_until_ms > now_ms)
            .map(|(bytes, _)| Arc::clone(bytes))
    }

    fn next_ms(&self) -> Option<u64> {
        self.next_ms
    }
}

#[cfg(test)]
#[path = "tests/own_snapshot.rs"]
mod tests;

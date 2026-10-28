//! Quotes and ladders for peer swaps (spec/decisions/peer-swaps.md,
//! spec/areas/connector.md#quotes-and-ladders): for each settlement method a
//! node offers, what it pays per unit coming in on the method's ledger (its
//! bid) and charges per unit going out (its ask), in the accounting unit,
//! and for each drop of a grid how far a swap can go before its price falls
//! by more. They are the content of a snapshot, computed from the price
//! table without IO.
//!
//! **The promise.** A reader prices a swap from method `a` to method `b` as
//! `bid_a / ask_b`, walked along `a`'s bid ladder and `b`'s ask ladder as
//! steps, both indexed by what has come in on `a`: the bid ladder by the
//! amount, the ask ladder by what it is worth at the top-of-book price,
//! `amount · bid_a / ask_b`. Within the levels at drops `d_i` and `d_j`, a
//! unit in gets `bid · (1 − d_i) · (1 − d_j) / ask` out. A packet is priced
//! at its last unit, after the swap's packets before it, so each packet
//! gets at least the rate of the step its last unit falls in, on all of
//! it, at any size the ladders cover, however the swap is split into
//! packets, sent one after another or together, and whatever the packets
//! in flight when the quotes were made do. Each packet's own impact, the
//! worst state in flight, the cap `1 + k` on a pair's weight ratio and the
//! coupling of every asset through the owner's total value are all counted
//! against the reader. The promise is for one swap, between two methods,
//! from the state the quotes were made in: other swaps, the reader's
//! between other methods included, are traffic the quotes cannot foresee.
//! A swap the hub's own ledger cannot fund is refused (T04), as any packet
//! is: ladders count each asset whole.
//!
//! **How ours keeps it.** With `w` the curve's weights, each asset's bid
//! weight is its weight at its most (every pending credit of its own, every
//! pending debit of the others) and its ask weight at its least (the
//! opposite), so that a pair's state in flight is never worse for the
//! reader. The bid is clamped at `1 + k` times the least ask weight of the
//! other methods quoted, so that no pair's product passes the curve's cap;
//! for two assets that is exact. A bid ladder holds the owner's total
//! value at what the others hold at their least, less `k` times what came
//! in, which is the most the other side's bonus can take out. An ask
//! ladder holds it at what the others hold at their most, plus what came
//! in, and bounds what came in, at spot, by its worth at the quotes over
//! the least bid of the other methods. The fee and the spread are folded
//! into the bid. Every value rounds against the reader: a bid down, an ask
//! up, a ladder down, and the bid by a margin for the rounding of the
//! curve's rate.

use dassie_ledger::LedgerId;

use crate::pricing::{
    Holdings, LedgerPrice, MAX_PRICED_LEDGERS, MILLION, PriceTable, SHARE_ONE, VALUE_BASE, fresh,
    mul_div, value_of, value_unit, wide_mul,
};

/// Drops a grid has: the levels of every ladder, exactly as many as a
/// snapshot carries (`dassie_feed::LEVELS`, which the node checks against
/// this).
pub const GRID_LEVELS: usize = 8;

/// What a drop is a whole number of, in parts per million: a basis point,
/// as a snapshot carries drops.
pub const GRID_STEP_PPM: u32 = 100;

/// The grid a node quotes on unless it says otherwise, in parts per
/// million: 0.01%, 0.03%, 0.1%, 0.3%, 1%, 3%, 10% and 30%, each a whole
/// number of basis points, as a snapshot carries them. The grid travels in
/// a snapshot's header, so every function here takes it as a parameter.
pub const DEFAULT_GRID_PPM: [u32; GRID_LEVELS] =
    [100, 300, 1_000, 3_000, 10_000, 30_000, 100_000, 300_000];

/// A quote's fixed point: the accounting unit's value of one unit of a
/// ledger, times 10^`QUOTE_PRECISION`. The accounting unit is a billionth
/// of the common base the exchange rates are relative to, as the curve
/// values holdings; only ratios of quotes mean anything to a reader.
pub const QUOTE_PRECISION: u32 = 18;

/// How far a publisher may round a quote against the reader, in parts per
/// billion of it: a bid down, an ask up. The ask ladders hold for a reader
/// whose quotes are within this of [`PriceTable::quotes`]'s.
pub const QUOTE_TOLERANCE_PPB: u32 = 1_000;

/// A weight's fixed point, the quotes': `w` times 10^18.
const WEIGHT_ONE: u128 = 10u128.pow(QUOTE_PRECISION);

/// Parts per billion.
const BILLION: u128 = 1_000_000_000;

/// The weight of `1 − k`, times [`WEIGHT_ONE`] per part per million of
/// `1 − k`: 10^12.
const WEIGHT_PER_PPM: u128 = WEIGHT_ONE / MILLION;

const _: () = assert!(grid_is_valid(&DEFAULT_GRID_PPM));
const _: () = assert!(QUOTE_TOLERANCE_PPB < 1_000_000_000);

/// Whether `grid` is one a snapshot can carry: each drop a whole number of
/// basis points ([`GRID_STEP_PPM`]), below 100%, and larger than the one
/// before. Its length, [`GRID_LEVELS`], is a snapshot's by its type.
#[must_use]
pub const fn grid_is_valid(grid: &[u32; GRID_LEVELS]) -> bool {
    let mut before: Option<u32> = None;
    let mut rest: &[u32] = grid;
    while let [drop, tail @ ..] = rest {
        let ascending = match before {
            Some(before) => before < *drop,
            None => true,
        };
        if !ascending || *drop % GRID_STEP_PPM != 0 || *drop as u128 >= MILLION {
            return false;
        }
        before = Some(*drop);
        rest = tail;
    }
    true
}

/// One side of a method's quote: the bid, for what comes in on its ledger,
/// or the ask, for what goes out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Side {
    /// In the accounting unit per unit of the ledger, times
    /// 10^[`QUOTE_PRECISION`], fee and spread folded into the bid. Zero: the
    /// side does not convert.
    pub quote: u128,
    /// For each drop of the grid, in its order, in units of the ledger: on
    /// the bid, the most that can come in while its last unit still gets
    /// at least the bid less the drop; on the ask, the most that can go
    /// out, counted by what came in at the top-of-book price (the amount
    /// in times the in method's bid over this ask), while its last unit
    /// still costs at most the ask over one less the drop. Never smaller at
    /// a larger drop; `u64::MAX` for as much as anyone could send. Zero
    /// everywhere on a closed side.
    pub ladder: [u64; GRID_LEVELS],
}

impl Side {
    /// A side that does not convert.
    pub const CLOSED: Self = Self {
        quote: 0,
        ladder: [0; GRID_LEVELS],
    };

    /// Whether it converts.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.quote > 0
    }
}

/// A method's quote: its ledger, its bid and its ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quote {
    /// The method's ledger.
    pub ledger: LedgerId,
    /// What the node pays for what comes in on it.
    pub bid: Side,
    /// What it charges for what goes out on it.
    pub ask: Side,
}

/// The quotes of the methods a snapshot names, in their order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quotes {
    quotes: [Option<Quote>; MAX_PRICED_LEDGERS],
}

impl Quotes {
    /// Every quote, in the order of the methods asked for.
    pub fn iter(&self) -> impl Iterator<Item = &Quote> + '_ {
        self.quotes.iter().flatten()
    }

    /// The quote of the method on `ledger`.
    #[must_use]
    pub fn get(&self, ledger: LedgerId) -> Option<&Quote> {
        self.iter().find(|quote| quote.ledger == ledger)
    }
}

/// What the owner holds of one asset, or of all of them, valued at spot,
/// at the two ends the packets in flight can leave.
#[derive(Clone, Copy, Debug, Default)]
struct Extremes {
    /// Every pending credit fulfilled, every pending debit rejected.
    most: u128,
    /// Every pending debit fulfilled, every pending credit rejected.
    least: u128,
}

/// A method being quoted, with what its quotes are built from.
#[derive(Clone, Copy, Debug)]
struct Method {
    ledger: LedgerId,
    entry: LedgerPrice,
    /// What a unit of its ledger divides into, as `value_of` has it.
    unit: u128,
    /// What the owner holds of its asset.
    asset: Extremes,
    /// The owner's total value as its bid sees it: its asset at its most,
    /// every other at its least.
    total_in: u128,
    /// What the other assets hold at their most, as its ask sees them.
    others_most: u128,
    /// The least its asset's weight can be on the way in, times
    /// [`WEIGHT_ONE`]; `None` for an asset the owner holds none of, whose
    /// weight has no bound.
    weight_in: Option<u128>,
    /// The most its asset's weight can be on the way out; `None` when the
    /// owner holds nothing of it to pay out, which closes its ask.
    weight_out: Option<u128>,
    /// Its ask, or zero.
    ask: u128,
}

/// A method's bid: the weight it is clamped to, and the quote.
#[derive(Clone, Copy, Debug)]
struct Bid {
    weight: u128,
    quote: u128,
}

impl PriceTable {
    /// The quotes of `methods`, each with a bid and an ask ladder on
    /// `grid`, with the owner holding `holdings` on the table's ledgers
    /// ([`Self::live_holdings`]), at `now_ms`
    /// (spec/areas/connector.md#quotes-and-ladders). A method whose ledger
    /// the table does not price, or whose rate is older than
    /// `max_rate_age_ms`, has both sides closed; so does a bid with no
    /// other method's ask open, and an ask the owner holds nothing of its
    /// asset for. Every ledger of the table counts toward the totals, the
    /// methods' or not.
    ///
    /// # Panics
    ///
    /// If `grid` is not valid ([`grid_is_valid`]), or `methods` are more
    /// than a table prices, name a ledger twice, or two of the table's
    /// ledgers of one asset: one method per asset, since a bid over an ask
    /// cannot say that two methods of one holding convert at spot.
    #[must_use]
    pub fn quotes(
        &self,
        methods: &[LedgerId],
        holdings: &Holdings,
        grid: &[u32; GRID_LEVELS],
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Quotes {
        assert!(grid_is_valid(grid), "a grid is checked before it is used");
        assert!(methods.len() <= MAX_PRICED_LEDGERS);
        let mut priced = [None; MAX_PRICED_LEDGERS];
        for (slot, (index, &ledger)) in priced.iter_mut().zip(methods.iter().enumerate()) {
            assert!(
                !methods.get(..index).unwrap_or_default().contains(&ledger),
                "a method is named once"
            );
            *slot = self
                .ledger(ledger)
                .filter(|entry| fresh(entry.as_of_ms, now_ms, max_rate_age_ms))
                .and_then(|entry| self.method(entry, holdings));
        }
        for (index, method) in priced.iter().enumerate() {
            let asset = method.map(|method| method.entry.asset);
            assert!(
                asset.is_none()
                    || priced
                        .iter()
                        .skip(index + 1)
                        .all(|other| other.map(|other| other.entry.asset) != asset),
                "one method per asset"
            );
        }
        let drop_max = u128::from(grid[GRID_LEVELS - 1]);
        let mut bids = [None; MAX_PRICED_LEDGERS];
        for (slot, method) in bids.iter_mut().zip(&priced) {
            *slot = method.and_then(|method| self.bid(&method, &priced, drop_max));
        }
        let mut quotes = Quotes {
            quotes: [None; MAX_PRICED_LEDGERS],
        };
        for (index, (slot, &ledger)) in quotes.quotes.iter_mut().zip(methods).enumerate() {
            let (Some(Some(method)), Some(bid)) = (priced.get(index), bids.get(index)) else {
                *slot = Some(Quote {
                    ledger,
                    bid: Side::CLOSED,
                    ask: Side::CLOSED,
                });
                continue;
            };
            *slot = Some(Quote {
                ledger,
                bid: bid.map_or(Side::CLOSED, |bid| self.bid_side(method, bid, grid)),
                ask: self.ask_side(method, least_bid(index, &priced, &bids), grid),
            });
        }
        quotes
    }

    /// The method on `entry`, with its asset's holdings and weights.
    fn method(&self, entry: &LedgerPrice, holdings: &Holdings) -> Option<Method> {
        let unit = value_unit(entry.rate, entry.ledger.asset().scale)?;
        let (asset, all) = (
            self.extremes(holdings, Some(entry.asset)),
            self.extremes(holdings, None),
        );
        // Each asset's values are part of the totals, which fit.
        let total_in = asset.most + (all.least - asset.least);
        let others_most = all.most - asset.most;
        let (assets, k) = self.curve();
        // The share on the way in rounded up, its weight down; on the way
        // out the share down, its weight up: as the curve's shares, rounded
        // down, can never be worse for the reader.
        let weight_in = (asset.most > 0)
            .then(|| mul_div_ceil(asset.most, SHARE_ONE, total_in))
            .flatten()
            .map(|share| weight(share, assets, k, false));
        let share_out = mul_div(asset.least, SHARE_ONE, asset.least + others_most).unwrap_or(0);
        let weight_out = (share_out > 0).then(|| weight(share_out, assets, k, true));
        let ask = weight_out
            .and_then(|weight| mul_div_ceil(VALUE_BASE, weight, unit))
            .unwrap_or(0);
        Some(Method {
            ledger: entry.ledger,
            entry: *entry,
            unit,
            asset,
            total_in,
            others_most,
            weight_in,
            weight_out: weight_out.filter(|_| ask > 0),
            ask,
        })
    }

    /// The table's `n`, its assets, and `k`, in parts per million.
    fn curve(&self) -> (u128, u128) {
        let assets = u128::try_from(self.assets()).unwrap_or(u128::MAX);
        let k = u128::from(self.pricing().rebalance_ppm);
        assert!((1..=16).contains(&assets) && k < MILLION);
        (assets, k)
    }

    /// What the owner holds of `asset`, every asset if `None`, valued at
    /// spot ledger by ledger as the curve values them, the owner's own
    /// moves on their way counted ([`LedgerPrice::counting_moves`]).
    fn extremes(&self, holdings: &Holdings, asset: Option<LedgerId>) -> Extremes {
        self.ledgers()
            .zip(holdings)
            .filter(|(entry, _)| asset.is_none_or(|asset| entry.asset == asset))
            .fold(Extremes::default(), |sum, (entry, holding)| {
                let whole = entry.counting_moves(holding);
                Extremes {
                    most: sum.most.saturating_add(value_of(whole.most(), entry)),
                    least: sum.least.saturating_add(value_of(whole.least(), entry)),
                }
            })
    }

    /// `method`'s bid: its weight on the way in, clamped at `1 + k` times
    /// the least weight on the way out of another method with its ask
    /// open, so that no pair's `bid / ask` passes the curve's cap, times
    /// what a unit is worth at spot, less the fee and the spread, less the
    /// margin for the rounding of the curve's rate against any of those
    /// asks. `None` with no other ask open: there is nothing to swap into.
    fn bid(&self, method: &Method, priced: &[Option<Method>], drop_max: u128) -> Option<Bid> {
        let others = || {
            priced
                .iter()
                .flatten()
                .filter(|other| other.ledger != method.ledger && other.ask > 0)
        };
        let least_out = others().filter_map(|other| other.weight_out).min()?;
        let (_, k) = self.curve();
        let clamp = least_out.checked_mul(MILLION + k)? / MILLION;
        let weight = method.weight_in.map_or(clamp, |weight| weight.min(clamp));
        let charges = self.charges();
        let kept =
            (MILLION - u128::from(charges.fee_ppm)) * (MILLION - u128::from(charges.spread_ppm));
        let raw = mul_div(
            mul_div(VALUE_BASE, weight, method.unit)?,
            kept,
            MILLION * MILLION,
        )?;
        let mut margin = 0u128;
        for other in others() {
            margin = margin.max(rate_margin(method, other, drop_max)?);
        }
        let quote = raw.checked_sub(margin).filter(|&quote| quote > 0)?;
        Some(Bid { weight, quote })
    }

    /// `method`'s bid side: the quote, and for each drop the most that can
    /// come in while its last unit's weight stays at least the bid's less
    /// the drop.
    fn bid_side(&self, method: &Method, bid: Bid, grid: &[u32; GRID_LEVELS]) -> Side {
        let mut ladder = [0; GRID_LEVELS];
        for (slot, &drop) in ladder.iter_mut().zip(grid) {
            *slot = self.most_in(method, bid.weight, u128::from(drop));
        }
        Side {
            quote: bid.quote,
            ladder,
        }
    }

    /// The most that can come in on `method` with its weight still at
    /// least `weight` less `drop`. Coming in raises the asset's share, so
    /// lowers its weight; its share after `D` in, valued at spot, is at
    /// most `(most + D + 1) / (total_in − k·D − 3)`: the other side can
    /// take out at most `1 + k` times what came in, at the curve's cap, and
    /// three for the values' rounding.
    fn most_in(&self, method: &Method, weight: u128, drop: u128) -> u64 {
        let (assets, k) = self.curve();
        let target = (weight * (MILLION - drop)).div_ceil(MILLION);
        // At a share of one, every asset's weight is its least.
        if target <= self::weight(SHARE_ONE, assets, k, false) {
            return u64::MAX;
        }
        // `(1 − k) + k / (n · s) ≥ target` while `s` is at most this.
        let excess = target - (MILLION - k) * WEIGHT_PER_PPM;
        let share_max = k * MILLION * WEIGHT_ONE / (assets * excess);
        if share_max >= SHARE_ONE {
            return u64::MAX;
        }
        let reach = method
            .total_in
            .checked_sub(3)
            .and_then(|room| mul_div(share_max, room, SHARE_ONE));
        let free = reach.and_then(|reach| reach.checked_sub(method.asset.most.saturating_add(1)));
        let value = free.and_then(|free| {
            mul_div(
                free,
                MILLION * SHARE_ONE,
                MILLION * SHARE_ONE + k * share_max,
            )
        });
        value
            .and_then(|value| mul_div(value, method.unit, VALUE_BASE))
            .map_or(0, |units| u64::try_from(units).unwrap_or(u64::MAX))
    }

    /// `method`'s ask side: the quote, and for each drop the most that can
    /// go out, counted by what came in at the top-of-book price, while its
    /// last unit's weight stays at most the ask's over one less the drop,
    /// from `least_bid` ([`least_bid`]). Closed when the owner holds nothing
    /// of its asset to pay out; with no other bid open, nothing can go out.
    fn ask_side(
        &self,
        method: &Method,
        least_bid: Option<u128>,
        grid: &[u32; GRID_LEVELS],
    ) -> Side {
        let Some(weight) = method.weight_out else {
            return Side::CLOSED;
        };
        let mut ladder = [0; GRID_LEVELS];
        if let Some(least_bid) = least_bid {
            for (slot, &drop) in ladder.iter_mut().zip(grid) {
                *slot = self.most_out(method, weight, least_bid, u128::from(drop));
            }
        }
        Side {
            quote: method.ask,
            ladder,
        }
    }

    /// The most that can go out on `method`, counted by what came in at
    /// the top-of-book price, with its weight still at most `weight` over
    /// one less `drop`. Going out lowers the asset's share. With `S` taken
    /// out, valued at spot by what came in, as the curve takes it, its
    /// value is at least `least − ρ·S − 2`, with `ρ` the most a unit taken
    /// can pay out (`1 + k` less the spread, or one), and the others' at
    /// most `others_most + S / (1 − fee) + 1`, what came in for it included.
    /// A reader counts what came in at the quotes, `in · bid / ask`: what it
    /// is worth at spot is at most that over the least bid of another
    /// method per what a unit of that is worth at spot (`least_bid`), less
    /// the publisher's rounding.
    fn most_out(&self, method: &Method, weight: u128, least_bid: u128, drop: u128) -> u64 {
        let (assets, k) = self.curve();
        let charges = self.charges();
        let limit = weight * MILLION / (MILLION - drop);
        // `(1 − k) + k / (n · s) ≤ limit` while `s` is at least this.
        let share_min = if k == 0 {
            0
        } else {
            match limit.checked_sub((MILLION - k) * WEIGHT_PER_PPM) {
                Some(excess) if excess > 0 => (k * MILLION * WEIGHT_ONE).div_ceil(assets * excess),
                _ => return 0,
            }
        };
        if share_min >= SHARE_ONE {
            return 0;
        }
        let held = method
            .asset
            .least
            .checked_sub(2)
            .and_then(|kept| mul_div(kept, SHARE_ONE - share_min, SHARE_ONE));
        let owed = mul_div_ceil(share_min, method.others_most.saturating_add(1), SHARE_ONE);
        let Some(free) = held
            .zip(owed)
            .and_then(|(held, owed)| held.checked_sub(owed))
        else {
            return 0;
        };
        let spread_kept = MILLION - u128::from(charges.spread_ppm);
        let fee_kept = MILLION - u128::from(charges.fee_ppm);
        let rho = MILLION.max(((MILLION + k) * spread_kept).div_ceil(MILLION));
        let denominator = rho * fee_kept * (SHARE_ONE - share_min) + MILLION * MILLION * share_min;
        let value = mul_div(free, MILLION * fee_kept * SHARE_ONE, denominator);
        let tolerance = u128::from(QUOTE_TOLERANCE_PPB);
        value
            .and_then(|value| mul_div(value, least_bid, method.ask))
            .and_then(|units| mul_div(units, BILLION - tolerance, BILLION + tolerance))
            .map_or(0, |units| u64::try_from(units).unwrap_or(u64::MAX))
    }
}

/// Of the methods other than the one at `index` with their bids open, the
/// least bid per what a unit is worth at spot, times [`WEIGHT_ONE`]: what
/// is left of a unit's weight on the way in after the clamp, the fee, the
/// spread and the margin. `None` if no other bid is open.
fn least_bid(index: usize, priced: &[Option<Method>], bids: &[Option<Bid>]) -> Option<u128> {
    priced
        .iter()
        .zip(bids)
        .enumerate()
        .filter(|&(other, _)| other != index)
        .filter_map(|(_, (method, bid))| {
            let (method, bid) = (method.as_ref()?, bid.as_ref()?);
            mul_div(bid.quote, method.unit, VALUE_BASE)
        })
        .min()
}

/// What `into`'s bid must be lowered by so that the rounding of the
/// curve's rate toward `out`'s ask never makes the promise more than the
/// packet gets: a rate is rounded down twice, to 10^-18 of the multiplier
/// on spot and to 10^-15 of a unit out per unit in, which loses at most
/// `spot · 10^-18 + 10^-15` units out per unit in; at the deepest drop of
/// the grid on both sides, the bid must cover that at the ask.
fn rate_margin(into: &Method, out: &Method, drop_max: u128) -> Option<u128> {
    // `ask · (spot + 1000) · 10^-18`, with `spot` out per in: `unit_out /
    // unit_in`; over `(1 − drop)²`, in parts per million squared.
    let per_unit = mul_div_ceil(
        out.ask,
        out.unit.checked_add(into.unit.checked_mul(1_000)?)?,
        into.unit,
    )?;
    let kept = (MILLION - drop_max) * (MILLION - drop_max);
    mul_div_ceil(per_unit, 1, MILLION * kept)?.checked_add(1)
}

/// The curve's `w = (1 − k) + k / (n · s)` for a share `s` of [`SHARE_ONE`]
/// and `n` assets, times [`WEIGHT_ONE`], rounded down, or up if `up`.
fn weight(share: u128, assets: u128, k: u128, up: bool) -> u128 {
    assert!(share > 0 && share <= SHARE_ONE && (1..=16).contains(&assets) && k < MILLION);
    // Below 2^64, so times 10^18 below 2^124.
    let numerator = ((MILLION - k) * assets * share + k * SHARE_ONE) * WEIGHT_ONE;
    let denominator = MILLION * assets * share;
    if up {
        numerator.div_ceil(denominator)
    } else {
        numerator / denominator
    }
}

/// `ceil(a × b / c)`, exact through a 256-bit product; `None` when `c` is
/// zero or it does not fit 128 bits.
fn mul_div_ceil(a: u128, b: u128, c: u128) -> Option<u128> {
    let quotient = mul_div(a, b, c)?;
    if wide_mul(quotient, c) == wide_mul(a, b) {
        Some(quotient)
    } else {
        quotient.checked_add(1)
    }
}

#[cfg(test)]
mod tests;

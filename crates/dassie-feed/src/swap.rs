//! Pricing a swap from a snapshot (spec/areas/rate-feed.md#reading-a-snapshot):
//! the least a node gives, in one method's ledger, for an amount in
//! another's.
//!
//! A swap from method `a` to method `b` is priced as `a`'s bid over `b`'s
//! ask, walked along both ladders as steps. Both ladders are indexed by
//! what has come in on `a`: the bid ladder by the amount, the ask ladder by
//! what it is worth at the top-of-book price, `amount · bid / ask`, in `b`'s
//! units. While what has come in is within the bid ladder's amount at drop
//! `d_i`, each unit of it is worth at least `bid · (1 − d_i)`; while it is
//! worth at most the ask ladder's amount at drop `d_j`, each unit out costs
//! at most `ask / (1 − d_j)`. So within both, a unit of `a` gets at least
//! `bid · (1 − d_i) · (1 − d_j) / ask` of `b`, and the walk adds that up a
//! step at a time, each step ending where a level of either ladder does,
//! and rounds what each step gets down. A swap beyond either ladder's last
//! amount is not covered, and is not priced.
//!
//! **A node prices a packet at its last unit,** after the swap's packets
//! before it, so a swap sent as one packet per step gets at least the
//! walk's sum. One packet across several steps gets at least the rate of
//! the step its last unit falls in on all of it, not the walk's sum:
//! [`Snapshot::packet_out`] is the least any packet is promised, the
//! minimum to set on it.
//!
//! Everything is exact integer arithmetic, so every reader prices a
//! snapshot alike.

use core::cmp::Ordering;

use crate::number::{LEVELS, Quote};
use crate::snapshot::{DROP_BP_LIMIT, MethodQuotes, MethodTag, Snapshot};

/// A rate a reader asks of a swap: `numerator / denominator` units of the
/// ledger it gets per unit of the one it pays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratio {
    numerator: u64,
    denominator: u64,
}

impl Ratio {
    /// The ratio `numerator / denominator`, if the denominator is not zero.
    #[must_use]
    pub const fn new(numerator: u64, denominator: u64) -> Option<Self> {
        if denominator == 0 {
            None
        } else {
            Some(Self {
                numerator,
                denominator,
            })
        }
    }
}

/// A swap: how much goes in, and the least that comes out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swap {
    /// In the ledger of the method paid on.
    pub amount_in: u64,
    /// At least this, in the ledger of the method paid back on.
    pub amount_out: u64,
}

impl Snapshot {
    /// The least the node gives on the method tagged `to` for `amount_in`
    /// on the method tagged `from`, or `None` when it does not say: it has
    /// not both methods, they are one, a side is closed, or the amount is
    /// beyond what the ladders cover.
    #[must_use]
    pub fn swap_out(&self, from: MethodTag, to: MethodTag, amount_in: u64) -> Option<u64> {
        let walk = Walk::new(self, from, to)?;
        let swap = walk.run(amount_in, None);
        (swap.amount_in == amount_in).then_some(swap.amount_out)
    }

    /// The least a packet of `amount` on the method tagged `from` gets out
    /// on the one tagged `to`, when `start` came in on the swap's packets
    /// before it: `amount` at the rate of the step the packet's last unit,
    /// `start + amount`, falls in, rounded down. A node prices a packet at
    /// its last unit, so this is the minimum to set on it. `None` when the
    /// snapshot does not price the pair or the last unit is beyond what the
    /// ladders cover, as for [`swap_out`](Self::swap_out).
    #[must_use]
    pub fn packet_out(
        &self,
        from: MethodTag,
        to: MethodTag,
        start: u64,
        amount: u64,
    ) -> Option<u64> {
        let walk = Walk::new(self, from, to)?;
        if amount == 0 {
            return Some(0);
        }
        let last = start.checked_add(amount)?;
        let level = |tops: &[u64; LEVELS]| tops.iter().position(|&top| last <= top);
        let (level_in, level_out) = (level(&walk.bid_amounts)?, level(&walk.ask_tops)?);
        let (Some(&keep_in), Some(&keep_out)) =
            (walk.keep_bp.get(level_in), walk.keep_bp.get(level_out))
        else {
            return None;
        };
        Some(StepRate::new(walk.bid, walk.ask, keep_in, keep_out).out_floor(amount))
    }

    /// Where the step of the walk from the method tagged `from` to the one
    /// tagged `to` that the next unit after `start` falls in ends, in what
    /// has come in on `from`: a packet from `start` up to it stays within
    /// one level of both ladders, and gets that step's rate on all of it
    /// ([`packet_out`](Self::packet_out)). `None` when the snapshot does
    /// not price the pair, or `start` is at or beyond what the ladders
    /// cover.
    #[must_use]
    pub fn step_end(&self, from: MethodTag, to: MethodTag, start: u64) -> Option<u64> {
        let walk = Walk::new(self, from, to)?;
        let top = |tops: &[u64; LEVELS]| tops.iter().copied().find(|&top| top > start);
        let end = top(&walk.bid_amounts)?.min(top(&walk.ask_tops)?);
        assert!(end > start, "a step ends after it starts");
        Some(end)
    }

    /// The largest swap from the method tagged `from` to the one tagged
    /// `to` whose every unit still gets at least `threshold`, at the lower
    /// bound the ladders give, and the least it gets out; or `None` when
    /// the snapshot does not price the pair, as for
    /// [`swap_out`](Self::swap_out).
    #[must_use]
    pub fn swap_at_least(&self, from: MethodTag, to: MethodTag, threshold: Ratio) -> Option<Swap> {
        let walk = Walk::new(self, from, to)?;
        Some(walk.run(u64::MAX, Some(threshold)))
    }
}

/// The two sides of a pair a snapshot prices.
struct Walk {
    bid: Quote,
    ask: Quote,
    bid_amounts: [u64; LEVELS],
    /// The most that can come in within each level of the ask ladder: the
    /// largest amount worth at most the level's amount at the top-of-book
    /// price, `amount · bid ≤ level · ask`.
    ask_tops: [u64; LEVELS],
    /// One less each drop, in basis points: at least 1.
    keep_bp: [u16; LEVELS],
}

impl Walk {
    fn new(snapshot: &Snapshot, from: MethodTag, to: MethodTag) -> Option<Self> {
        if from == to {
            return None;
        }
        let MethodQuotes {
            bid, bid_ladder, ..
        } = *snapshot.method(from)?;
        let MethodQuotes {
            ask, ask_ladder, ..
        } = *snapshot.method(to)?;
        if bid.is_zero() || ask.is_zero() {
            return None;
        }
        let keep_bp = snapshot
            .grid_bp
            .map(|drop| DROP_BP_LIMIT.saturating_sub(drop));
        assert!(keep_bp.iter().all(|&keep| keep > 0), "a checked grid");
        let ask_tops = ask_ladder.amounts().map(|amount| {
            // `amount · ask / bid`, rounded down: below 2^64 · 2^24.
            let scaled = u128::from(amount) * u128::from(ask.mantissa());
            let shift = ask.exponent() - bid.exponent();
            saturate(div_shifted(
                scaled,
                u128::from(bid.mantissa()),
                shift,
                false,
            ))
        });
        Some(Self {
            bid,
            ask,
            bid_amounts: bid_ladder.amounts(),
            ask_tops,
            keep_bp,
        })
    }

    /// Walks the two ladders until `limit_in` has gone in, either ladder
    /// ends, or the rate falls below `threshold`.
    fn run(&self, limit_in: u64, threshold: Option<Ratio>) -> Swap {
        let (mut level_in, mut level_out) = (0usize, 0usize);
        let mut swap = Swap::default();
        // Each pass moves a level on, or takes the rest of `limit_in` and
        // ends at the next.
        for _ in 0..2 * LEVELS + 2 {
            let (Some(&top_in), Some(&top_out), Some(&keep_in), Some(&keep_out)) = (
                self.bid_amounts.get(level_in),
                self.ask_tops.get(level_out),
                self.keep_bp.get(level_in),
                self.keep_bp.get(level_out),
            ) else {
                return swap;
            };
            if swap.amount_in >= limit_in {
                return swap;
            }
            let room_in = top_in.saturating_sub(swap.amount_in);
            if room_in == 0 {
                level_in += 1;
                continue;
            }
            let room_out = top_out.saturating_sub(swap.amount_in);
            if room_out == 0 {
                level_out += 1;
                continue;
            }
            let rate = StepRate::new(self.bid, self.ask, keep_in, keep_out);
            if threshold.is_some_and(|threshold| !rate.at_least(threshold)) {
                return swap;
            }
            let take_in = room_in.min(room_out).min(limit_in - swap.amount_in);
            swap.amount_in += take_in;
            swap.amount_out = swap.amount_out.saturating_add(rate.out_floor(take_in));
            level_in += usize::from(take_in == room_in);
            level_out += usize::from(take_in == room_out);
        }
        assert!(
            swap.amount_in >= limit_in || level_in >= LEVELS || level_out >= LEVELS,
            "the walk ends within its passes"
        );
        swap
    }
}

/// The rate of one step of a walk, units out per unit in:
/// `numerator · 2^shift / denominator`.
struct StepRate {
    /// The bid's mantissa times both keeps: below 2^24 · 2^14 · 2^14.
    numerator: u128,
    /// The ask's mantissa times 10^8: below 2^24 · 2^27.
    denominator: u128,
    /// The bid's exponent less the ask's.
    shift: i32,
}

impl StepRate {
    fn new(bid: Quote, ask: Quote, keep_in_bp: u16, keep_out_bp: u16) -> Self {
        let scale = u128::from(DROP_BP_LIMIT) * u128::from(DROP_BP_LIMIT);
        Self {
            numerator: u128::from(bid.mantissa())
                * u128::from(keep_in_bp)
                * u128::from(keep_out_bp),
            denominator: u128::from(ask.mantissa()) * scale,
            shift: bid.exponent() - ask.exponent(),
        }
    }

    /// What `amount` in gets out, rounded down, at most `u64::MAX`.
    fn out_floor(&self, amount: u64) -> u64 {
        // Below 2^64 · 2^52.
        let scaled = u128::from(amount) * self.numerator;
        saturate(div_shifted(scaled, self.denominator, self.shift, false))
    }

    /// Whether the rate is at least `threshold`:
    /// `numerator · 2^shift · t_d ≥ t_n · denominator`.
    fn at_least(&self, threshold: Ratio) -> bool {
        // Each below 2^116.
        let ours = self.numerator * u128::from(threshold.denominator);
        let theirs = u128::from(threshold.numerator) * self.denominator;
        let shift = self.shift.unsigned_abs();
        if self.shift >= 0 {
            compare_shifted(ours, shift, theirs) != Ordering::Less
        } else {
            compare_shifted(theirs, shift, ours) != Ordering::Greater
        }
    }
}

/// How `value · 2^shift` compares with `other`, for values below 2^127.
fn compare_shifted(value: u128, shift: u32, other: u128) -> Ordering {
    if value == 0 {
        return 0.cmp(&other);
    }
    let bits = u128::BITS - value.leading_zeros();
    if bits.saturating_add(shift) >= u128::BITS {
        // value · 2^shift ≥ 2^127 > other.
        return Ordering::Greater;
    }
    (value << shift).cmp(&other)
}

/// `numerator · 2^shift / denominator`, rounded down or up, or `None` when
/// it exceeds a `u128`. The denominator is not zero.
fn div_shifted(numerator: u128, denominator: u128, shift: i32, up: bool) -> Option<u128> {
    assert!(denominator > 0, "a rate's parts are not zero");
    let amount = shift.unsigned_abs();
    if shift >= 0 {
        if numerator == 0 {
            return Some(0);
        }
        let bits = u128::BITS - numerator.leading_zeros();
        if bits + amount >= u128::BITS {
            return None;
        }
        let shifted = numerator << amount;
        let quotient = shifted / denominator;
        let inexact = up && !shifted.is_multiple_of(denominator);
        return Some(quotient + u128::from(inexact));
    }
    // Dividing by the denominator, then by 2^amount, rounds as dividing by
    // their product does.
    let quotient = numerator / denominator;
    let inexact = up && !numerator.is_multiple_of(denominator);
    let quotient = quotient + u128::from(inexact);
    if amount >= u128::BITS {
        return Some(u128::from(up && quotient != 0));
    }
    let lost = quotient & ((1u128 << amount) - 1);
    Some((quotient >> amount) + u128::from(up && lost != 0))
}

/// A `u128` at most `u64::MAX`, `None` above.
fn saturate(value: Option<u128>) -> u64 {
    value.map_or(u64::MAX, |value| u64::try_from(value).unwrap_or(u64::MAX))
}

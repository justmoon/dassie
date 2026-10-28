//! Pricing a swap from a snapshot (spec/areas/rate-feed.md#reading-a-snapshot):
//! the least a node gives, in one method's ledger, for an amount in
//! another's.
//!
//! A swap from method `a` to method `b` is priced as `a`'s bid over `b`'s
//! ask, walked along both ladders as steps. While what has come in on `a`
//! is within its bid ladder's amount at drop `d_i`, each unit of it is
//! worth at least `bid · (1 − d_i)`; while what has gone out on `b` is
//! within its ask ladder's amount at drop `d_j`, each unit of it costs at
//! most `ask / (1 − d_j)`. So within both, a unit of `a` gets at least
//! `bid · (1 − d_i) · (1 − d_j) / ask` of `b`, and the walk adds that up a
//! step at a time, rounding every step against the reader: what it gets
//! down, what it pays up. A swap beyond either ladder's last amount is not
//! covered, and is not priced.
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
    ask_amounts: [u64; LEVELS],
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
        Some(Self {
            bid,
            ask,
            bid_amounts: bid_ladder.amounts(),
            ask_amounts: ask_ladder.amounts(),
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
                self.ask_amounts.get(level_out),
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
            let room_out = top_out.saturating_sub(swap.amount_out);
            if room_out == 0 {
                level_out += 1;
                continue;
            }
            let rate = StepRate::new(self.bid, self.ask, keep_in, keep_out);
            if threshold.is_some_and(|threshold| !rate.at_least(threshold)) {
                return swap;
            }
            let take_in = room_in.min(limit_in - swap.amount_in);
            let gets = rate.out_floor(take_in);
            if gets <= room_out {
                swap.amount_in += take_in;
                swap.amount_out += gets;
                if take_in == room_in {
                    level_in += 1;
                }
            } else {
                // The ask ladder's level ends first: pay what reaches it.
                swap.amount_in += rate.in_ceil(room_out).min(take_in);
                swap.amount_out += room_out;
                level_out += 1;
            }
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

    /// What must go in for `amount` out, rounded up, at most `u64::MAX`.
    fn in_ceil(&self, amount: u64) -> u64 {
        // Below 2^64 · 2^51.
        let scaled = u128::from(amount) * self.denominator;
        saturate(div_shifted(scaled, self.numerator, -self.shift, true))
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

//! Properties of snapshots over arbitrary input
//! (spec/areas/rate-feed.md#reading-a-snapshot): decoding never panics and reads
//! only canonical bytes, so decoding and encoding again gives the bytes;
//! and a priced swap never says more than the ladders promise. Fixed seed
//! in CI (.cargo/config.toml).

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

mod support;

use dassie_feed::{
    LEVELS, Ladder, MAX_DATAGRAM_BYTES, MAX_SNAPSHOT_METHODS, MethodQuotes, Quote, Ratio, Snapshot,
    SnapshotMessage, attach_signature, decode_snapshot_datagram, encode_request, method_tag,
    sign_snapshot,
};
use dassie_identity::Signature;
use num_bigint::BigUint;
use proptest::prelude::*;
use support::{key, snapshot_of};

/// Encodes a decoded datagram again.
fn encode(message: &SnapshotMessage<'_>) -> Vec<u8> {
    match message {
        SnapshotMessage::Request => encode_request().to_vec(),
        SnapshotMessage::Snapshot(signed) => attach_signature(
            signed.snapshot.unsigned_bytes().unwrap(),
            &Signature::from_bytes(signed.signature),
        ),
    }
}

fn quotes() -> impl Strategy<Value = Quote> {
    prop_oneof![
        1 => Just(Quote::ZERO),
        4 => (1u64..1 << 40, 1u64..1 << 40).prop_map(|(n, d)| Quote::floor(n.into(), d.into())),
        1 => any::<u32>().prop_filter_map("canonical", Quote::from_bits),
    ]
}

/// Non-decreasing amounts, from small to near a `u64`'s bound.
fn amounts() -> impl Strategy<Value = [u64; LEVELS]> {
    (0u32..64, proptest::array::uniform8(any::<u64>())).prop_map(|(bits, raw)| {
        let mut amounts = raw.map(|value| value >> (63 - bits.min(63)));
        amounts.sort_unstable();
        amounts
    })
}

fn grids() -> impl Strategy<Value = [u16; LEVELS]> {
    proptest::sample::subsequence((0u16..10_000).collect::<Vec<_>>(), LEVELS)
        .prop_map(|drops| drops.try_into().unwrap())
}

fn methods() -> impl Strategy<Value = MethodQuotes> {
    (any::<u32>(), quotes(), quotes(), amounts(), amounts()).prop_map(
        |(id, bid, ask, bid_amounts, ask_amounts)| {
            let ladder = |quote: Quote, amounts| {
                if quote.is_zero() {
                    Ladder::ZERO
                } else {
                    Ladder::floor(amounts)
                }
            };
            MethodQuotes {
                tag: method_tag(&format!("scheme-{id}")),
                bid,
                ask,
                bid_ladder: ladder(bid, bid_amounts),
                ask_ladder: ladder(ask, ask_amounts),
            }
        },
    )
}

fn snapshots() -> impl Strategy<Value = Snapshot> {
    (
        proptest::collection::vec(methods(), 0..=MAX_SNAPSHOT_METHODS),
        grids(),
        any::<u64>(),
        1..=u64::MAX,
    )
        .prop_map(|(mut methods, grid, sequence, validity)| {
            methods.sort_by_key(|method| method.tag);
            methods.dedup_by_key(|method| method.tag);
            let mut snapshot = snapshot_of(methods);
            snapshot.grid_bp = grid;
            snapshot.sequence_ms = sequence.min(u64::MAX - 1);
            snapshot.valid_until_ms = snapshot.sequence_ms.saturating_add(validity);
            snapshot
        })
}

proptest! {
    #[test]
    fn snapshots_round_trip(snapshot in snapshots()) {
        let bytes = sign_snapshot(&key(), &snapshot).unwrap();
        prop_assert!(bytes.len() <= MAX_DATAGRAM_BYTES);
        let message = decode_snapshot_datagram(&bytes).unwrap();
        let SnapshotMessage::Snapshot(signed) = &message else { panic!("not a snapshot") };
        prop_assert_eq!(&signed.snapshot, &snapshot);
        signed.verify(&key().public_key()).unwrap();
        prop_assert_eq!(encode(&message), bytes);
    }

    #[test]
    fn changed_bytes_decode_only_to_themselves(
        snapshot in snapshots(),
        offset in any::<proptest::sample::Index>(),
        value in any::<u8>(),
    ) {
        let mut bytes = sign_snapshot(&key(), &snapshot).unwrap();
        let offset = offset.index(bytes.len());
        bytes[offset] = value;
        if let Ok(message) = decode_snapshot_datagram(&bytes) {
            prop_assert_eq!(encode(&message), bytes);
        }
    }

    #[test]
    fn any_bytes_decode_only_to_themselves(
        body in proptest::collection::vec(any::<u8>(), 0..=MAX_DATAGRAM_BYTES + 1),
        prefix in proptest::sample::select(vec![vec![], vec![2], vec![2, 0x80], vec![2, 0x81]]),
    ) {
        let bytes = [prefix, body].concat();
        if let Ok(message) = decode_snapshot_datagram(&bytes) {
            prop_assert_eq!(encode(&message), bytes);
        }
    }
}

/// The two sides of a pair, for checking a swap exactly.
#[derive(Clone, Debug)]
struct Pair {
    bid: Quote,
    ask: Quote,
    bid_amounts: [u64; LEVELS],
    ask_amounts: [u64; LEVELS],
    keep_bp: [u64; LEVELS],
}

impl Pair {
    /// `value · 2^(exponent − low)`, for the lower of the two exponents.
    fn scaled(&self, value: BigUint, exponent: i32) -> BigUint {
        let low = self.bid.exponent().min(self.ask.exponent());
        value << usize::try_from(exponent - low).unwrap()
    }

    /// The last unit in within the ask ladder's level `j`: the most whose
    /// worth at the top-of-book price, `bid / ask`, is at most its amount,
    /// at most a `u64`.
    fn ask_top(&self, j: usize) -> u64 {
        let room = BigUint::from(self.ask_amounts[j]) * self.ask.mantissa();
        let room = self.scaled(room, self.ask.exponent());
        let per = self.scaled(BigUint::from(self.bid.mantissa()), self.bid.exponent());
        u64::try_from(room / per).unwrap_or(u64::MAX)
    }

    /// The levels the `unit`-th unit in lies in, if both ladders cover it.
    fn levels(&self, unit: u64) -> Option<(usize, usize)> {
        let i = self.bid_amounts.iter().position(|&top| unit <= top)?;
        let j = (0..LEVELS).find(|&j| unit <= self.ask_top(j))?;
        Some((i, j))
    }

    /// The rate of `levels`, scaled to the lower exponent: `bid · keep_i ·
    /// keep_j` over `ask · 10^8`.
    fn rate(&self, (i, j): (usize, usize)) -> (BigUint, BigUint) {
        let ours = BigUint::from(self.bid.mantissa()) * self.keep_bp[i] * self.keep_bp[j];
        let theirs = BigUint::from(self.ask.mantissa()) * 100_000_000u64;
        (
            self.scaled(ours, self.bid.exponent()),
            self.scaled(theirs, self.ask.exponent()),
        )
    }

    /// Whether, at the worst the ladders allow, what `amount_in` is worth
    /// covers `amount_out`: each unit in at the rate of the levels it lies
    /// in, both ladders read by what has come in.
    fn covers(&self, amount_in: u64, amount_out: u64) -> bool {
        // Where a level of either ladder ends within the swap, and its end.
        let mut ends: Vec<u64> = (0..LEVELS)
            .flat_map(|level| [self.bid_amounts[level], self.ask_top(level)])
            .filter(|&end| end > 0 && end < amount_in)
            .collect();
        ends.push(amount_in);
        ends.sort_unstable();
        ends.dedup();
        let (mut worth, mut below) = (BigUint::ZERO, 0u64);
        for end in ends.into_iter().filter(|&end| end > 0) {
            let Some(levels) = self.levels(end) else {
                return false;
            };
            worth += BigUint::from(end - below) * self.rate(levels).0;
            below = end;
        }
        BigUint::from(amount_out) * self.rate((0, 0)).1 <= worth
    }

    /// Whether the rate at `levels` is at least `n / d`.
    fn rate_at_least(&self, levels: (usize, usize), n: u64, d: u64) -> bool {
        let (ours, theirs) = self.rate(levels);
        ours * d >= theirs * n
    }

    /// What `amount` gets at the rate of `levels`, rounded down.
    fn out(&self, levels: (usize, usize), amount: u64) -> u64 {
        let (ours, theirs) = self.rate(levels);
        u64::try_from(BigUint::from(amount) * ours / theirs).unwrap_or(u64::MAX)
    }
}

fn pairs() -> impl Strategy<Value = (Snapshot, Pair)> {
    (quotes(), quotes(), amounts(), amounts(), grids()).prop_map(
        |(bid, ask, bid_amounts, ask_amounts, grid)| {
            let bid = if bid.is_zero() {
                Quote::floor(1, 1)
            } else {
                bid
            };
            let ask = if ask.is_zero() {
                Quote::floor(1, 1)
            } else {
                ask
            };
            let (bid_ladder, ask_ladder) = (Ladder::floor(bid_amounts), Ladder::floor(ask_amounts));
            let mut snapshot = snapshot_of(vec![
                MethodQuotes {
                    tag: method_tag("a"),
                    bid,
                    ask: Quote::ZERO,
                    bid_ladder,
                    ask_ladder: Ladder::ZERO,
                },
                MethodQuotes {
                    tag: method_tag("b"),
                    bid: Quote::ZERO,
                    ask,
                    bid_ladder: Ladder::ZERO,
                    ask_ladder,
                },
            ]);
            snapshot.grid_bp = grid;
            let pair = Pair {
                bid,
                ask,
                bid_amounts: bid_ladder.amounts(),
                ask_amounts: ask_ladder.amounts(),
                keep_bp: grid.map(|drop| u64::from(10_000 - drop)),
            };
            (snapshot, pair)
        },
    )
}

proptest! {
    #[test]
    fn a_swap_never_says_more_than_the_ladders(
        (snapshot, pair) in pairs(),
        share in 0u64..=1_000_000,
    ) {
        let (a, b) = (method_tag("a"), method_tag("b"));
        let top = pair.bid_amounts[LEVELS - 1];
        let amount = (u128::from(top) * u128::from(share) / 1_000_000) as u64;
        if let Some(out) = snapshot.swap_out(a, b, amount) {
            prop_assert!(pair.covers(amount, out), "{amount} → {out}");
            // Less in never gets more out.
            let less = snapshot.swap_out(a, b, amount / 2).unwrap();
            prop_assert!(less <= out);
        } else {
            // Only beyond what both ladders cover.
            prop_assert!(amount > 0 && pair.levels(amount).is_none());
        }
        prop_assert_eq!(snapshot.swap_out(b, a, 1), None);
    }

    // A packet is promised the rate of the levels its last unit lies in, on
    // all of it: one packet never more than the walk's sum, but for each
    // step's rounding.
    #[test]
    fn a_packet_gets_the_rate_of_its_last_unit(
        (snapshot, pair) in pairs(),
        start_share in 0u64..=1_000_000,
        size_share in 0u64..=1_000_000,
    ) {
        let (a, b) = (method_tag("a"), method_tag("b"));
        let top = pair.bid_amounts[LEVELS - 1];
        let start = (u128::from(top) * u128::from(start_share) / 1_000_000) as u64;
        let amount = (u128::from(top - start) * u128::from(size_share) / 1_000_000) as u64;
        let expected = if amount == 0 {
            Some(0)
        } else {
            pair.levels(start + amount).map(|levels| pair.out(levels, amount))
        };
        prop_assert_eq!(snapshot.packet_out(a, b, start, amount), expected);
        // A packet up to the end of its first unit's step is within the
        // levels of that unit, and gets their rate.
        if let Some(end) = snapshot.step_end(a, b, start) {
            prop_assert!(end > start);
            let first = pair.levels(start + 1);
            prop_assert!(first.is_some());
            prop_assert_eq!(pair.levels(end), first);
            prop_assert_eq!(
                snapshot.packet_out(a, b, start, end - start),
                first.map(|levels| pair.out(levels, end - start))
            );
        } else {
            let next = start.checked_add(1).and_then(|unit| pair.levels(unit));
            prop_assert!(next.is_none());
        }
        if let (Some(one), Some(walked)) = (
            snapshot.packet_out(a, b, 0, amount),
            snapshot.swap_out(a, b, amount),
        ) {
            // Up to the rounding of each of the walk's steps.
            prop_assert!(one <= walked + 2 * LEVELS as u64);
        }
    }

    #[test]
    fn a_threshold_swap_is_the_largest_at_or_above_it(
        (snapshot, pair) in pairs(),
        numerator in 0u64..1 << 40,
        denominator in 1u64..1 << 40,
    ) {
        let (a, b) = (method_tag("a"), method_tag("b"));
        let threshold = Ratio::new(numerator, denominator).unwrap();
        let swap = snapshot.swap_at_least(a, b, threshold).unwrap();
        prop_assert!(pair.covers(swap.amount_in, swap.amount_out));
        if swap.amount_in > 0 {
            // Its last unit still got the threshold.
            let levels = pair.levels(swap.amount_in).unwrap();
            prop_assert!(pair.rate_at_least(levels, numerator, denominator));
        }
        // Where both ladders go on, the next unit would not.
        if let Some(levels) = swap.amount_in.checked_add(1).and_then(|next| pair.levels(next)) {
            prop_assert!(!pair.rate_at_least(levels, numerator, denominator));
        }
    }
}

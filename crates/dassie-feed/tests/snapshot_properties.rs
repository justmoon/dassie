//! Properties of snapshots over arbitrary input
//! (spec/areas/rate-feed.md#snapshots): decoding never panics and reads
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
    /// Each level's share of `amount`, which the ladder covers.
    fn segments(amounts: &[u64; LEVELS], amount: u64) -> [u64; LEVELS] {
        let mut below = 0;
        amounts.map(|top| {
            let segment = amount.min(top) - amount.min(below);
            below = top;
            segment
        })
    }

    /// Whether, at the worst the ladders allow, what `amount_out` costs is
    /// covered by what `amount_in` is worth: `ask · Σ out_j / keep_j ≤ bid ·
    /// Σ in_i · keep_i`, the keeps in basis points.
    fn covers(&self, amount_in: u64, amount_out: u64) -> bool {
        let product: BigUint = self
            .keep_bp
            .iter()
            .map(|&keep| BigUint::from(keep))
            .product();
        let ins = Self::segments(&self.bid_amounts, amount_in);
        let outs = Self::segments(&self.ask_amounts, amount_out);
        let worth: BigUint = ins
            .iter()
            .zip(self.keep_bp)
            .map(|(&segment, keep)| BigUint::from(segment) * keep)
            .sum::<BigUint>()
            * &product
            * self.bid.mantissa();
        let cost: BigUint = outs
            .iter()
            .zip(self.keep_bp)
            .map(|(&segment, keep)| BigUint::from(segment) * (&product / keep))
            .sum::<BigUint>()
            * 100_000_000u64
            * self.ask.mantissa();
        let (bid_exponent, ask_exponent) = (self.bid.exponent(), self.ask.exponent());
        let low = bid_exponent.min(ask_exponent);
        let worth = worth << usize::try_from(bid_exponent - low).unwrap();
        let cost = cost << usize::try_from(ask_exponent - low).unwrap();
        cost <= worth
    }

    /// The level a unit lies in: the first whose amount reaches it.
    fn level(amounts: &[u64; LEVELS], unit: u64) -> Option<usize> {
        amounts.iter().position(|&top| unit <= top)
    }

    /// Whether the rate at levels `i` and `j` is at least `n / d`.
    fn rate_at_least(&self, i: usize, j: usize, n: u64, d: u64) -> bool {
        let ours = BigUint::from(self.bid.mantissa()) * self.keep_bp[i] * self.keep_bp[j] * d;
        let theirs = BigUint::from(self.ask.mantissa()) * 100_000_000u64 * n;
        let (bid_exponent, ask_exponent) = (self.bid.exponent(), self.ask.exponent());
        let low = bid_exponent.min(ask_exponent);
        (ours << usize::try_from(bid_exponent - low).unwrap())
            >= (theirs << usize::try_from(ask_exponent - low).unwrap())
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
        }
        prop_assert_eq!(snapshot.swap_out(b, a, 1), None);
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
        let last_in = Pair::level(&pair.bid_amounts, swap.amount_in.max(1));
        let last_out = Pair::level(&pair.ask_amounts, swap.amount_out.max(1));
        if swap.amount_in > 0 && swap.amount_out > 0 {
            // Its last units still got the threshold.
            let (i, j) = (last_in.unwrap(), last_out.unwrap());
            prop_assert!(pair.rate_at_least(i, j, numerator, denominator));
        }
        // Where both ladders go on, the next units would not.
        let next_in = Pair::level(&pair.bid_amounts, swap.amount_in + 1);
        let next_out = Pair::level(&pair.ask_amounts, swap.amount_out + 1);
        if let (Some(i), Some(j)) = (next_in, next_out) {
            prop_assert!(!pair.rate_at_least(i, j, numerator, denominator));
        }
    }
}

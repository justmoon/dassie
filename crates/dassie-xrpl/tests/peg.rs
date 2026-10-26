//! The peg guard's holds, whatever readings come and whenever
//! (spec/areas/xrpl.md#the-peg-guard, spec/decisions/issued-assets.md,
//! spec/areas/testing.md#property-tests): it pauses its scheme off its peg
//! only once every reading that counted was outside the band for the trip
//! hold, and lets it run again after a trip only once every reading was
//! inside the narrower band for the recovery hold, with no gap longer than
//! the staleness bound; without a reading that counted for that bound, it
//! pauses; it never lets its scheme run again but on a reading inside the
//! band; and a reading at an anchor too old never counts. What it keeps
//! comes back as it was, at every step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_settlement::{DollarRate, PegPause};
use dassie_xrpl::AccountId;
use dassie_xrpl::codec::TokenValue;
use dassie_xrpl::peg::{
    Guard, INNER_BAND_PPM, MAX_ANCHOR_AGE_MS, MAX_RATE_AGE_MS, MAX_READING_AGE_MS, OUTER_BAND_PPM,
    PAR_PPM, Peg, PoolReading, RECOVER_HOLD_MS, Reading, TRIP_HOLD_MS,
};
use proptest::prelude::*;

const PEG: Peg = Peg {
    min_pool_drops: 100_000_000,
};

/// When the guard starts.
const START: u64 = 1_000_000_000;

/// What the pool holds of the token.
const POOL_TOKENS: u64 = 100_000;

/// 2 XRP to the dollar, learnt at `as_of_ms`.
const fn rate(as_of_ms: u64) -> DollarRate {
    DollarRate {
        per_dollar: 2_000_000_000_000_000,
        precision: 15,
        as_of_ms,
    }
}

/// The pool priced at `ppm` millionths of a dollar at [`rate`]: as many
/// drops as `ppm · 200_000`, for [`POOL_TOKENS`], at an anchor that closed
/// at `closed_ms`.
fn pool(ppm: u64, closed_ms: u64) -> PoolReading {
    PoolReading {
        account: AccountId([7; 20]),
        drops: ppm * 200_000,
        value: TokenValue::from_units(POOL_TOKENS, 0).unwrap(),
        ledger_index: 10,
        closed_ms,
    }
}

/// What happens after some time passes.
#[derive(Clone, Copy, Debug)]
enum Op {
    /// Only the time.
    Tick,
    /// A reading at this price, in millionths of a dollar.
    Read(u64),
    /// A reading of no pool.
    NoPool,
    /// A reading at a dollar, with a stale rate.
    StaleRate,
    /// A reading at this price, at an anchor too old.
    OldAnchor(u64),
}

/// Prices around the bands' edges, where the holds decide, and far off.
fn price() -> impl Strategy<Value = u64> {
    let near = |edge: u64| (edge - 2)..=(edge + 2);
    prop_oneof![
        3 => (PAR_PPM - INNER_BAND_PPM)..=(PAR_PPM + INNER_BAND_PPM),
        1 => near(PAR_PPM - INNER_BAND_PPM),
        1 => near(PAR_PPM + INNER_BAND_PPM),
        1 => near(PAR_PPM - OUTER_BAND_PPM),
        1 => near(PAR_PPM + OUTER_BAND_PPM),
        2 => 500_000..=1_500_000_u64,
    ]
}

/// Steps: how long passes, and what happens then. Mostly a minute or so,
/// sometimes long enough for the guard to go stale; often the same for
/// long runs, so that holds end.
fn steps() -> impl Strategy<Value = Vec<(u64, Op)>> {
    let wait = prop_oneof![
        8 => 0..=120_000_u64,
        1 => 0..=3 * MAX_READING_AGE_MS,
    ];
    let op = prop_oneof![
        1 => Just(Op::Tick),
        8 => price().prop_map(Op::Read),
        1 => Just(Op::NoPool),
        1 => Just(Op::StaleRate),
        1 => price().prop_map(Op::OldAnchor),
    ];
    let run = (wait, op, 1..=80_usize).prop_map(|(wait, op, count)| vec![(wait, op); count]);
    proptest::collection::vec(run, 1..=12).prop_map(|runs| runs.concat())
}

/// Whether the readings that counted, from the last back to one at least
/// `hold` before `now`, were all `inside`, with no gap longer than the
/// staleness bound between two, or after the last.
fn held(readings: &[(u64, u64)], now: u64, hold: u64, inside: impl Fn(u64) -> bool) -> bool {
    let mut later = now;
    for &(at, price) in readings.iter().rev() {
        if !inside(price) || later - at > MAX_READING_AGE_MS {
            return false;
        }
        if now - at >= hold {
            return true;
        }
        later = at;
    }
    false
}

/// Checks a change of the guard's pause at `now`, from `before` to
/// `after`, on `reading` if one counted then; `tripped` is whether the
/// guard is off its peg by a trip seen from pegged.
fn check(
    tripped: &mut bool,
    readings: &[(u64, u64)],
    now: u64,
    (before, after): (Option<PegPause>, Option<PegPause>),
    reading: Option<u64>,
) -> Result<(), TestCaseError> {
    let off = |price: u64| price.abs_diff(PAR_PPM) > OUTER_BAND_PPM;
    let close = |price: u64| price.abs_diff(PAR_PPM) <= INNER_BAND_PPM;
    match (before, after) {
        // It trips only once every reading was off for the hold.
        (None, Some(PegPause::OffPeg)) => {
            prop_assert!(held(readings, now, TRIP_HOLD_MS, off));
            *tripped = true;
        }
        // It goes stale only without a reading for the bound.
        (None, Some(PegPause::Stale)) => {
            let last = readings.last().map_or(START, |(at, _)| *at);
            prop_assert!(now - last > MAX_READING_AGE_MS);
        }
        // It runs again only on a reading inside the band, and after a
        // trip only once every reading was close for the recovery hold.
        (Some(_), None) => {
            let Some(price) = reading else {
                return Err(TestCaseError::fail("resumed without a reading"));
            };
            prop_assert!(!off(price));
            if *tripped {
                prop_assert!(held(readings, now, RECOVER_HOLD_MS, close));
            }
            *tripped = false;
        }
        _ => {}
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn the_guard_keeps_its_holds(steps in steps()) {
        let mut guard = Guard::new(PEG);
        guard.tick(START);
        let mut now = START;
        // The readings that counted, at when.
        let mut readings: Vec<(u64, u64)> = Vec::new();
        let mut tripped = false;
        for (wait, op) in steps {
            now += wait;
            // The time first, as every reading takes it, then the op: each
            // a change of its own.
            let before = guard.paused();
            guard.tick(now);
            let ticked = guard.paused();
            check(&mut tripped, &readings, now, (before, ticked), None)?;
            let reading = match op {
                Op::Tick => None,
                Op::Read(ppm) => {
                    guard.rate(Some(rate(now)));
                    prop_assert_eq!(guard.reading(now, Some(&pool(ppm, now))), Reading::Counted(ppm));
                    readings.push((now, ppm));
                    Some(ppm)
                }
                Op::NoPool => {
                    prop_assert_eq!(guard.reading(now, None), Reading::NoPool);
                    None
                }
                Op::StaleRate => {
                    guard.rate(Some(rate(now - MAX_RATE_AGE_MS - 1)));
                    prop_assert_eq!(guard.reading(now, Some(&pool(PAR_PPM, now))), Reading::NoRate);
                    None
                }
                Op::OldAnchor(ppm) => {
                    guard.rate(Some(rate(now)));
                    let old = pool(ppm, now - MAX_ANCHOR_AGE_MS - 1);
                    prop_assert_eq!(guard.reading(now, Some(&old)), Reading::OldAnchor);
                    None
                }
            };
            guard.check_invariants();
            let after = guard.paused();
            check(&mut tripped, &readings, now, (ticked, after), reading)?;
            let view = guard.view();
            prop_assert_eq!(view.paused, after);
            prop_assert_eq!(view.last.map(|(price, _)| price), readings.last().map(|(_, price)| *price));
            // What it keeps comes back as it was.
            let restored = Guard::restore(PEG, &guard.state()).unwrap();
            prop_assert_eq!(restored.fingerprint(), guard.fingerprint());
            prop_assert_eq!(restored.paused(), after);
        }
    }
}

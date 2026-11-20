//! XRP's rate from mainnet's XRP/RLUSD pool (spec/areas/xrpl.md#xrps-rate)
//! against the fake mainnet and its lying servers: the rate is the median
//! of the pool's prices over the window, proven at ledgers the fake's UNL
//! validated; a pool pushed for less than half the window leaves it
//! unmoved, and one held off for longer moves it; a pool thinner than the
//! floor gives none; what a server lies about never counts; and nothing
//! the reader asks signs or submits.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::codec::TokenValue;
use dassie_xrpl::fake::{CLOSE_INTERVAL_S, FakeLedger, LIST_SITE, Lie};
use dassie_xrpl::pool_rate::{MIN_POOL_DROPS, Out, PoolRate, Seen, Timings};
use dassie_xrpl::unl::Secp256k1Signatures;
use dassie_xrpl::validator_list::UnlConfig;

/// The precision of the node's rates.
const PRECISION: u32 = 15;
/// The most turns one input takes to settle.
const MAX_TURNS: usize = 10_000;
/// Read once a ledger, as the fake closes them: a window of twenty.
const TIMINGS: Timings = Timings {
    read_every_ms: CLOSE_INTERVAL_S * 1_000,
    window_ms: 20 * CLOSE_INTERVAL_S * 1_000,
    min_readings: 11,
};
/// 0.4 XRP to the dollar.
const RATE: u128 = 400_000_000_000_000;

fn value(text: &str) -> TokenValue {
    TokenValue::parse(text).unwrap()
}

/// The reader on its own connection to the fake mainnet.
struct Run {
    ledger: FakeLedger,
    reader: PoolRate,
    connection: u64,
    now_ms: u64,
    /// Every request the reader sent.
    sent: Vec<serde_json::Value>,
    reconnects: u32,
    /// The lie each new connection tells, if any.
    lie: Option<Lie>,
}

impl Run {
    /// A reader that trusts the fake's publisher, connected, beside a pool
    /// of 1.86 million XRP and 4.65 million RLUSD: $2.50 an XRP.
    fn new() -> Self {
        let mut ledger = FakeLedger::mainnet();
        ledger
            .set_pool(IssuedAsset::RLUSD, 1_860_000_000_000, value("4650000"), 300)
            .unwrap();
        ledger.close();
        let config = UnlConfig::for_network(ledger.network().id)
            .unwrap()
            .with_override(&ledger.publishers(LIST_SITE))
            .unwrap();
        let reader = PoolRate::new(
            ledger.network().id,
            config,
            Box::new(Secp256k1Signatures),
            TIMINGS,
            PRECISION,
        );
        let now_ms = ledger.now_ms() + 2_000;
        let mut run = Self {
            ledger,
            reader,
            connection: 1,
            now_ms,
            sent: Vec::new(),
            reconnects: 0,
            lie: None,
        };
        run.reader.connected(now_ms);
        run.pump();
        run
    }

    /// Carries out what the reader asks, until it asks nothing more.
    fn pump(&mut self) {
        for _ in 0..MAX_TURNS {
            self.reader.check_invariants();
            let Some(out) = self.reader.poll() else {
                return;
            };
            match out {
                Out::Send(request) => {
                    self.sent.push(serde_json::from_slice(&request).unwrap());
                    let reply = self.ledger.request(self.connection, &request);
                    self.reader.message(self.now_ms, &reply);
                }
                Out::Fetch(url) => {
                    let body = self.ledger.fetch(&url);
                    self.reader.fetched(self.now_ms, &url, body.as_deref());
                }
                Out::Reconnect => {
                    self.reconnects += 1;
                    self.ledger.disconnect(self.connection);
                    self.connection += 1;
                    self.ledger.set_lie(self.connection, self.lie);
                    self.reader.disconnected();
                    self.reader.connected(self.now_ms);
                }
            }
        }
        panic!("the reader does not settle");
    }

    /// A ledger closes, and the reader does what is due.
    fn step(&mut self) {
        self.ledger.close();
        self.now_ms = self.now_ms.max(self.ledger.now_ms() + 2_000);
        self.reader.tick(self.now_ms);
        self.pump();
    }

    fn steps(&mut self, count: usize) {
        for _ in 0..count {
            self.step();
        }
    }

    /// The issuer sets the pool, in the next ledger.
    fn pool(&mut self, drops: u64, rlusd: &str) {
        self.ledger
            .set_pool(IssuedAsset::RLUSD, drops, value(rlusd), 300)
            .unwrap();
    }

    fn rate(&self) -> Option<u128> {
        self.reader.rate(self.now_ms)
    }
}

#[test]
fn the_rate_is_the_pool_s_once_the_window_holds_enough() {
    let mut run = Run::new();
    let mut first = None;
    for step in 1..=30 {
        run.step();
        if first.is_none() && run.rate().is_some() {
            first = Some(step);
        }
    }
    assert_eq!(run.rate(), Some(RATE));
    assert_eq!(run.reader.last(), Some(Seen::Counted));
    // A rate only once more than half the window counted: never from a
    // reading or two.
    let first = first.unwrap();
    assert!(
        first >= TIMINGS.min_readings,
        "a rate after {first} ledgers"
    );
    assert!(run.reader.readings(run.now_ms) >= TIMINGS.min_readings);
    // It asked for proofs and validations, and never signed, submitted or
    // subscribed to an account.
    for request in &run.sent {
        let command = request["command"].as_str().unwrap();
        assert!(
            !["submit", "sign", "subscribe", "account_info"].contains(&command),
            "the reader asked for {command}"
        );
    }
}

#[test]
fn a_pool_pushed_for_less_than_half_the_window_leaves_the_rate() {
    let mut run = Run::new();
    run.steps(30);
    assert_eq!(run.rate(), Some(RATE));
    // Pushed to $5 an XRP for nine ledgers of the window's twenty.
    run.pool(1_860_000_000_000, "9300000");
    run.steps(9);
    run.pool(1_860_000_000_000, "4650000");
    for _ in 0..30 {
        run.step();
        assert_eq!(run.rate(), Some(RATE));
    }
}

#[test]
fn a_pool_held_off_for_more_than_half_the_window_moves_the_rate() {
    let mut run = Run::new();
    run.steps(30);
    // $5 an XRP, held: 0.2 XRP to the dollar once most of the window says
    // so.
    run.pool(1_860_000_000_000, "9300000");
    let mut moved = None;
    for step in 1..=30 {
        run.step();
        if moved.is_none() && run.rate() == Some(RATE / 2) {
            moved = Some(step);
        }
    }
    let moved = moved.unwrap();
    assert!(moved > TIMINGS.min_readings / 2 + 1, "moved after {moved}");
    assert_eq!(run.rate(), Some(RATE / 2));
}

#[test]
fn a_thin_pool_gives_no_rate() {
    let mut run = Run::new();
    run.steps(30);
    assert_eq!(run.rate(), Some(RATE));
    // Drained below the floor, at the same price.
    run.pool(MIN_POOL_DROPS - 1_000_000, "249999.5");
    run.steps(30);
    assert_eq!(run.reader.last(), Some(Seen::TooThin));
    assert_eq!(run.rate(), None);
}

// A server a few ledgers behind, its validations still fresh
// (`Lie::OlderAnchor`), says nothing false: its ledgers count, a little late.
#[test]
fn what_a_lying_server_says_never_counts() {
    for lie in [
        Lie::AlteredObject,
        Lie::FalseAbsence,
        Lie::OtherBranch,
        Lie::OldAnchor,
        Lie::ForgedValidations,
        Lie::ReplayedValidations,
        Lie::WithheldValidations,
        Lie::WrongHeader,
        Lie::OtherChainSkipList,
        Lie::NoProofs,
    ] {
        // Honest for longer than validations stay fresh, so that an old
        // ledger the lie serves is one the reader could not anchor on.
        let mut run = Run::new();
        run.steps(70);
        assert_eq!(run.rate(), Some(RATE));
        run.lie = Some(lie);
        run.ledger.set_lie(run.connection, Some(lie));
        // An altered pool would move the rate: it never does, and once the
        // window holds only what the server said since, there is none.
        for _ in 0..60 {
            run.step();
            let rate = run.rate();
            assert!(
                rate.is_none() || rate == Some(RATE),
                "{lie:?} moved the rate to {rate:?}"
            );
        }
        assert_eq!(run.rate(), None, "a rate from {lie:?}");
    }
}

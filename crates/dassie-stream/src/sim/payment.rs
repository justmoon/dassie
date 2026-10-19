//! STREAM's random run (spec/areas/testing.md#the-simulations, D27): a client
//! pays and sends data to a server over a lossy path with an exchange rate
//! and a maximum packet amount, then closes. Both sides check their
//! invariants at every step, and the payment must arrive and the connection
//! close.

use alloc::format;
use alloc::string::{String, ToString};

use dassie_node_core::testing::{Outcome, Run};

use super::{Path, Simulation};
use crate::{ErrorCode, Policy};

/// How many steps a run may take to finish what it set out to do.
pub const STEPS: usize = 100_000;

/// For lossy paths: every lost packet costs its 30-second expiry, and a
/// few in a row would otherwise trip the 60-second idle timeout, which
/// these runs are not about.
pub const LOSSY: Policy = Policy {
    idle_timeout_ms: 600_000,
    ..Policy::DEFAULT
};

/// How much the client sends.
const AMOUNT: u64 = 300_000;

/// The path every run takes.
const PATH: Path = Path {
    loss_permille: 100,
    max_packet_amount: 7_000,
    rate: (3, 2),
    latency_ms: 10,
};

/// What the seed draws, before the run starts: only which packets are
/// lost, since every run takes the same path.
#[must_use]
pub fn describe(_seed: u64) -> String {
    format!("{PATH:?}, sending {AMOUNT}")
}

/// Runs the seed: the client sends, then closes.
#[must_use]
pub fn run(seed: u64) -> Run {
    let mut sim = Simulation::new(seed, PATH, LOSSY);
    sim.client.measure_exchange_rate();
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, AMOUNT).unwrap();
    sim.client.write(stream, &[42; 10_000]).unwrap();
    let sent = sim.try_run_until(STEPS, |sim| {
        sim.client
            .stream(stream)
            .is_some_and(|s| s.total_sent() == AMOUNT)
    });
    let closed = sent.and_then(|()| {
        let now = sim.now;
        sim.client.close(now, ErrorCode::NO_ERROR);
        sim.try_run_to_idle(STEPS)
    });
    let outcome = match closed {
        Ok(()) if sim.trace.contains("ConnectionClosed") => Outcome::Pass,
        Ok(()) => Outcome::Liveness("the connection never closed".into()),
        Err(problem) => Outcome::Liveness(problem),
    };
    let mut trace: alloc::vec::Vec<String> = sim.trace.lines().map(ToString::to_string).collect();
    trace.push(format!("{:?}", sim.client.stats()));
    Run { outcome, trace }
}

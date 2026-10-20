//! `dassie-sim record`, the process a campaign makes each run in: it prints
//! the run's record, as the campaign writes it, and exits 0 whatever the
//! run found.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::process::Command;

use serde_json::Value;

const DASSIE_SIM: &str = env!("CARGO_BIN_EXE_dassie-sim");

/// The record `dassie-sim record` prints for `sim` and `seed`.
fn record(sim: &str, seed: &str) -> Value {
    let output = Command::new(DASSIE_SIM)
        .args(["record", sim, seed])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "{text}");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn a_run_in_a_process_of_its_own_prints_its_record() {
    // Seed 7000 trips the tripwire; 7001 does not.
    let tripped = record("tripwire", "7000");
    assert_eq!(tripped["sim"], "tripwire");
    assert_eq!(tripped["seed"], 7_000);
    assert_eq!(tripped["signature"], "tripwire/safety/on-purpose");
    assert!(tripped["options"].is_string());
    let passed = record("tripwire", "7001");
    assert_eq!(passed["signature"], "tripwire/pass");
}

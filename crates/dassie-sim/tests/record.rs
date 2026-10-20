//! `dassie-sim record`, the process a campaign makes each run in: it prints
//! the run's record, as the campaign writes it, and exits 0 whatever the
//! run found. A campaign's run dies with its campaign.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::process::{Command, Stdio};

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

#[test]
fn a_run_whose_campaign_has_ended_does_not_start() {
    // The campaign it names is not its parent: its parent has ended, and
    // it was handed to another.
    let other = (std::process::id() + 1).to_string();
    let output = Command::new(DASSIE_SIM)
        .args(["record", "tripwire", "7001"])
        .env("DASSIE_SIM_CAMPAIGN", other)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert!(output.stdout.is_empty());
}

/// The processes whose parent is `parent`, from `/proc`.
#[cfg(target_os = "linux")]
fn children(parent: u32) -> Vec<u32> {
    let mut children = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if let Some((_, ppid)) = state(pid)
            && ppid == parent
        {
            children.push(pid);
        }
    }
    children
}

/// Process `pid`'s state and parent, while it exists.
#[cfg(target_os = "linux")]
fn state(pid: u32) -> Option<(char, u32)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The name, in parentheses, may hold anything; what follows does not.
    let mut fields = stat.get(stat.rfind(')')? + 1..)?.split_whitespace();
    let state = fields.next()?.chars().next()?;
    Some((state, fields.next()?.parse().ok()?))
}

#[cfg(target_os = "linux")]
#[test]
fn a_campaigns_runs_die_when_it_is_killed() {
    use std::time::{Duration, Instant};

    let mut campaign = Command::new(DASSIE_SIM)
        .args([
            "campaign",
            "--minutes",
            "1",
            "--jobs",
            "2",
            "--sims",
            "node",
        ])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let runs = loop {
        let runs = children(campaign.id());
        if !runs.is_empty() {
            break runs;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "no run started"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    // Killed, the campaign cannot stop its runs itself: the kernel does.
    campaign.kill().unwrap();
    campaign.wait().unwrap();
    let killed = Instant::now();
    for run in runs {
        // A run that ended is gone, or a zombie until its new parent
        // reaps it.
        while state(run).is_some_and(|(state, _)| state != 'Z') {
            assert!(
                killed.elapsed() < Duration::from_secs(10),
                "run {run} outlived its campaign"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

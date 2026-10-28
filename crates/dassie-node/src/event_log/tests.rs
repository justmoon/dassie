//! The event log's line format, its bounded queue and its rotation
//! (spec/areas/node.md#the-event-log).

use super::*;
use crate::clock::ManualClock;
use proto::hub_event::Event;

const TS: u64 = 1_761_667_200_000;
const NODE: &str = "32f6pclvm5p5xxdeqigvgxpjezayxacvmnnwgcy";

fn hub(event: Event) -> String {
    let event = proto::HubEvent {
        scheme: "hub-xrpl-testnet".into(),
        observed_at_unix_ms: 1_761_667_199_000,
        event: Some(event),
    };
    hub_line(TS, NODE, &event).unwrap()
}

/// The prefix of every hub line.
fn hub_prefix(event: &str) -> String {
    format!(
        r#"{{"ts":{TS},"node":"{NODE}","type":"hub","scheme":"hub-xrpl-testnet","observedAtUnixMs":"1761667199000","event":"{event}""#
    )
}

#[test]
fn a_start_and_a_lost_line() {
    assert_eq!(
        start_line(TS, NODE, "0.3.2"),
        format!(r#"{{"ts":{TS},"node":"{NODE}","type":"start","version":"0.3.2"}}"#)
    );
    assert_eq!(
        lost_line(TS, NODE, 7),
        format!(r#"{{"ts":{TS},"node":"{NODE}","type":"lost","count":7}}"#)
    );
}

#[test]
fn a_packet_line() {
    let packet = proto::Packet {
        trace: Some(proto::TraceId {
            epoch: 3,
            sequence: 41,
        }),
        source: "peer:2".into(),
        destination: "peer:5".into(),
        destination_address: "test.dassie.abc\"d".into(),
        amount_in: 18_446_744_073_709_551_615,
        amount_out: 990,
        received_at_unix_ms: 1_761_667_199_500,
        resolved_at_unix_ms: 0,
        state: proto::PacketState::Rejected.into(),
        reject_code: "T04".into(),
        transfers: vec![9, 10],
        condition: vec![0xab; 32],
        ledger_in: "hub-xrpl-testnet-xrp+xrp".into(),
        ledger_out: String::new(),
        source_node: "wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby".into(),
        destination_node: String::new(),
    };
    let condition = "ab".repeat(32);
    assert_eq!(
        packet_line(TS, NODE, &packet),
        format!(
            concat!(
                r#"{{"ts":{TS},"node":"{NODE}","type":"packet","trace":"3:41","state":"rejected","#,
                r#""rejectCode":"T04","source":"peer:2","destination":"peer:5","#,
                r#""sourceNode":"wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby","destinationNode":"","#,
                r#""destinationAddress":"test.dassie.abc\"d","amountIn":"18446744073709551615","#,
                r#""amountOut":"990","ledgerIn":"hub-xrpl-testnet-xrp+xrp","ledgerOut":"","#,
                r#""condition":"{condition}","receivedAtUnixMs":"1761667199500","#,
                r#""resolvedAtUnixMs":"0","transfers":["9","10"]}}"#
            ),
            TS = TS,
            NODE = NODE,
            condition = condition,
        )
    );
    // Every line is one JSON object.
    let parsed: serde_json::Value = serde_json::from_str(&packet_line(TS, NODE, &packet)).unwrap();
    assert_eq!(parsed["state"], "rejected");
    // A packet refused before it decoded has no condition.
    let bare = proto::Packet::default();
    assert!(packet_line(TS, NODE, &bare).contains(r#""trace":"0:0","state":"unspecified""#));
    assert!(packet_line(TS, NODE, &bare).contains(r#""condition":"","#));
}

/// The hub events read from the hub's blocks, with the fields their lines
/// end with.
fn chain_cases() -> Vec<(Event, &'static str, &'static str)> {
    vec![
        (
            Event::Block(proto::HubBlock {
                height: 12,
                timestamp_unix_ms: 1_761_667_198_000,
            }),
            "block",
            r#","height":"12","timestampUnixMs":"1761667198000""#,
        ),
        (
            Event::Operation(proto::HubOperation {
                height: 12,
                timestamp_unix_ms: 1_761_667_198_000,
                kind: "hold-open".into(),
                signer: 4,
                sequence: 7,
                hold: 3,
                peer_account: 5,
                amount: 1_000_000,
                outcome: "applied".into(),
                reason: String::new(),
                fee: 10,
                purpose: "settle:abc".into(),
            }),
            "operation",
            concat!(
                r#","height":"12","timestampUnixMs":"1761667198000","kind":"hold-open","signer":4,"#,
                r#""sequence":"7","hold":"3","peerAccount":5,"amount":"1000000","outcome":"applied","#,
                r#""reason":"","fee":"10","purpose":"settle:abc""#
            ),
        ),
        (
            Event::Account(proto::HubAccountState {
                height: 12,
                number: 4,
                balance: 20_000_000,
                reserve: 1_000_000,
                sequence: 8,
                deposited: 21_000_000,
                withdrawn: 0,
                returned: 0,
                closing: false,
            }),
            "account",
            concat!(
                r#","height":"12","number":4,"balance":"20000000","reserve":"1000000","#,
                r#""sequence":"8","deposited":"21000000","withdrawn":"0","returned":"0","closing":false"#
            ),
        ),
        (Event::Unfunded(proto::HubUnfunded {}), "unfunded", ""),
        (
            Event::Reduced(proto::HubHoldReduced {
                hold: 3,
                amount: 500,
            }),
            "reduced",
            r#","hold":"3","amount":"500""#,
        ),
        (
            Event::ClaimsPaid(proto::HubClaimsPaid {
                hold: 3,
                count: 2,
                first_sequence: 5,
                last_sequence: 6,
                unlisted: true,
            }),
            "claimsPaid",
            r#","hold":"3","count":2,"firstSequence":"5","lastSequence":"6","unlisted":true"#,
        ),
    ]
}

/// The others: the node's own view and moves.
fn own_cases() -> Vec<(Event, &'static str, &'static str)> {
    vec![
        (
            Event::Resynced(proto::HubResynced {
                height: 13,
                timestamp_unix_ms: 1_761_667_198_500,
                holds_before: 2,
                unread: "claim:3".into(),
            }),
            "resynced",
            r#","height":"13","timestampUnixMs":"1761667198500","holdsBefore":2,"unread":"claim:3""#,
        ),
        (
            Event::Dropped(proto::HubDropped {
                purpose: "withdraw".into(),
            }),
            "dropped",
            r#","purpose":"withdraw""#,
        ),
        (
            Event::Commitment(proto::HubCommitment {
                role: proto::HubRole::Payer.into(),
                peer: "wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby".into(),
                hold: 3,
                sequence: 6,
                amount: 250,
                condition: vec![0x01, 0xfe],
                expiry_unix_ms: 1_761_667_230_000,
            }),
            "commitment",
            concat!(
                r#","role":"payer","peer":"wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby","hold":"3","#,
                r#""sequence":"6","amount":"250","condition":"01fe","expiryUnixMs":"1761667230000""#
            ),
        ),
        (
            Event::Claim(proto::HubClaim {
                role: proto::HubRole::Payee.into(),
                peer: "wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby".into(),
                hold: 3,
                total: 750,
                through: 6,
            }),
            "claim",
            concat!(
                r#","role":"payee","peer":"wfubobuj5r7d7m4zkut4p2vvt5gd4szbif7czby","hold":"3","#,
                r#""total":"750","through":"6""#
            ),
        ),
        (
            Event::Move(proto::HubMoveEvent {
                stage: proto::hub_move_event::Stage::GivenUp.into(),
                direction: proto::hub_move::Direction::Withdraw.into(),
                amount: 5_000_000,
                payment: "ABCDEF".into(),
                by_owner: true,
            }),
            "move",
            r#","stage":"given_up","direction":"withdraw","amount":"5000000","payment":"ABCDEF","byOwner":true"#,
        ),
    ]
}

#[test]
fn a_line_for_each_hub_event() {
    let cases: Vec<_> = chain_cases().into_iter().chain(own_cases()).collect();
    // Every case of the contract's oneof but `lagged`.
    assert_eq!(cases.len(), 11);
    for (event, name, fields) in cases {
        let line = hub(event);
        assert_eq!(line, format!("{}{fields}}}", hub_prefix(name)));
        let _: serde_json::Value = serde_json::from_str(&line).unwrap();
    }
    // A `lagged` item is counted, not written.
    let lagged = proto::HubEvent {
        event: Some(Event::Lagged(3)),
        ..Default::default()
    };
    assert_eq!(hub_line(TS, NODE, &lagged), None);
}

/// A handle with a queue of `capacity` lines, and its other end, without a
/// writer thread.
fn queue(capacity: usize) -> (EventLog, Receiver<String>) {
    let (lines, queue) = std::sync::mpsc::sync_channel(capacity);
    let shared = Arc::new(Shared::default());
    let _ = shared.node.set(NODE.into());
    (EventLog { lines, shared }, queue)
}

#[test]
fn a_full_queue_drops_and_counts_and_the_writer_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let (log, receiver) = queue(2);
    for n in 0..5 {
        log.push(format!(r#"{{"n":{n}}}"#));
    }
    // Too long a line is dropped too.
    log.push("x".repeat(MAX_LINE_BYTES + 1));
    assert_eq!(log.shared.lost.load(Ordering::Relaxed), 4);
    let shared = Arc::clone(&log.shared);
    // With every handle gone, the writer writes what waits and ends.
    drop(log);
    let writer = Rotating::open(path.clone(), MIN_MAX_BYTES).unwrap();
    write_loop(writer, &receiver, &shared, &ManualClock::new(TS));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{{\"n\":0}}\n{{\"n\":1}}\n{}\n", lost_line(TS, NODE, 4))
    );
    assert_eq!(shared.lost.load(Ordering::Relaxed), 0);
}

#[test]
fn the_file_is_rotated_at_its_cap_and_old_files_go() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    // A file there already is appended to.
    std::fs::write(&path, "{\"old\":true}\n").unwrap();
    let mut writer = Rotating::open(path.clone(), MIN_MAX_BYTES).unwrap();
    let line = |n: u64| format!(r#"{{"n":{n},"pad":"{}"}}"#, "p".repeat(1000));
    let per_file = MIN_MAX_BYTES / (u64::try_from(line(0).len()).unwrap() + 1);
    // Enough for the cap to be passed more often than files are kept.
    let total = per_file * u64::from(KEPT_FILES + 3);
    for n in 0..total {
        writer.write_line(&line(n)).unwrap();
    }
    writer.flush().unwrap();
    let mut used = 0;
    for n in 0..=KEPT_FILES {
        let file = if n == 0 {
            path.clone()
        } else {
            numbered(&path, n)
        };
        let size = std::fs::metadata(&file).unwrap().len();
        assert!(size <= MIN_MAX_BYTES, "{} has {size} bytes", file.display());
        used += size;
    }
    assert!(used <= u64::from(KEPT_FILES + 1) * MIN_MAX_BYTES);
    assert!(!numbered(&path, KEPT_FILES + 1).exists());
    // The newest line is in the file, the line before the file's first in
    // `.1`, and the oldest are gone.
    let current = std::fs::read_to_string(&path).unwrap();
    let last = current.lines().last().unwrap();
    assert_eq!(last, line(total - 1));
    let first: serde_json::Value = serde_json::from_str(current.lines().next().unwrap()).unwrap();
    let first = first["n"].as_u64().unwrap();
    let previous = std::fs::read_to_string(numbered(&path, 1)).unwrap();
    assert_eq!(previous.lines().last().unwrap(), line(first - 1));
    let oldest = std::fs::read_to_string(numbered(&path, KEPT_FILES)).unwrap();
    assert!(!oldest.contains("\"old\""));
}

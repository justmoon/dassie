//! A deterministic simulation of the connector (spec 7 §7 item 4): children
//! send random packets, answer late, wrongly or never, and disconnect. The
//! invariants are checked after every step, every request gets exactly one
//! answer, the journal matches the packet trace, and the same seed gives the
//! same run. A failure names its seed; `DASSIE_SIM_SEEDS=from..to` runs a
//! longer campaign.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use dassie_connector::{Config, EndpointId, Outcome, PacketState};
use dassie_ilp::{ErrorCode, Timestamp, ildcp};
use dassie_ledger::{JournalEntry, LedgerId, Name, Reference, TransferState};
use dassie_node_core::testing::SeededRng;
use dassie_oer::Encode;
use support::*;

const STEPS: usize = 1_500;
const NAMES: [&str; 5] = ["alice", "bob", "carol", "dave", "eve"];

#[derive(Clone, Copy, Debug)]
enum Answer {
    Fulfill,
    WrongFulfillment,
    Reject,
    Garbage,
    Never,
    /// The packet surely did not reach its peer.
    Undelivered,
}

/// An answer to send: from, request ID, the right fulfillment, and how.
type Due = (EndpointId, u32, [u8; 32], Answer);

struct Simulation {
    rng: SeededRng,
    h: Harness,
    now: u64,
    /// Endpoint slot → its current ID, while connected.
    live: [Option<EndpointId>; 5],
    next_btp: u64,
    /// Answers due: (when, sequence) → (from, request ID, fulfillment, kind).
    due: BTreeMap<(u64, u64), Due>,
    sequence: u64,
    /// Requests forwarded and waiting for their one answer.
    awaiting: BTreeSet<(EndpointId, u32)>,
    gone: BTreeSet<EndpointId>,
}

impl Simulation {
    fn new(seed: u64) -> Self {
        let config = Config {
            epoch: 3,
            max_pending: 64,
            max_pending_per_endpoint: 24,
            trace_capacity: STEPS * 2,
            max_events: 8,
            ..Config::DEFAULT
        };
        let mut h = Harness::new(config, 100_000_000);
        h.add_btp(3, "carol");
        h.add_btp(4, "dave");
        // Eve's account has a limit: a peer's, trusted with a little.
        let (eve, _) = h
            .ledger
            .open_peer(
                0,
                LedgerId::StubUsd,
                Name::new("deve").unwrap(),
                150_000_000,
            )
            .unwrap();
        h.connector
            .add_endpoint(
                EndpointId::Btp(5),
                &format!("{OWN}.eve"),
                eve,
                LedgerId::StubUsd,
            )
            .unwrap();
        // Two peers, and a node beyond them they both lead to.
        h.add_peer(10, "dfar1", 300_000_000, 0);
        h.add_peer(11, "dfar2", 300_000_000, 1 << 31);
        h.connector
            .set_routes([(
                "test.das.dbeyond",
                &[EndpointId::Peer(10), EndpointId::Peer(11)][..],
            )])
            .unwrap();
        h.check();
        Self {
            rng: SeededRng::new(seed),
            h,
            now: NOW,
            live: [1, 2, 3, 4, 5].map(|id| Some(EndpointId::Btp(id))),
            next_btp: 6,
            due: BTreeMap::new(),
            sequence: 0,
            awaiting: BTreeSet::new(),
            gone: BTreeSet::new(),
        }
    }

    fn below(&mut self, n: u64) -> u64 {
        self.rng.next_u64() % n
    }

    fn step(&mut self) {
        match self.below(20) {
            0..=11 => self.send(),
            12..=17 => self.advance(),
            18 => self.disconnect(),
            _ => self.reconnect(),
        }
    }

    fn live_endpoints(&self) -> Vec<EndpointId> {
        let mut endpoints: Vec<_> = self.live.iter().flatten().copied().collect();
        endpoints.push(RECEIVER);
        endpoints.push(EndpointId::Peer(10));
        endpoints.push(EndpointId::Peer(11));
        endpoints
    }

    fn destination(&mut self) -> String {
        match self.below(10) {
            0 => OWN.to_string(),
            1 => format!("{OWN}.nobody"),
            2 => "test.elsewhere".to_string(),
            3 | 4 => format!("{OWN}.stream.{}", self.below(4)),
            5 => format!("test.das.dbeyond.{}", self.below(4)),
            _ => {
                let name = NAMES[self.below(5) as usize];
                format!("{OWN}.{name}.x")
            }
        }
    }

    fn packet(&mut self, fulfillment: [u8; 32]) -> Vec<u8> {
        match self.below(30) {
            0 => vec![0x0c; self.below(20) as usize],
            1 => ildcp::request(Timestamp::from_unix_ms(self.now)).to_vec(),
            _ => {
                let amount = match self.below(4) {
                    0 => 0,
                    1 => self.below(1_000),
                    2 => self.below(100_000_000),
                    _ => 90_000_000 + self.below(20_000_000),
                };
                let expires = self.now + 500 + self.below(40_000);
                let destination = self.destination();
                prepare(&destination, amount, expires, fulfillment)
            }
        }
    }

    fn send(&mut self) {
        let sources = self.live_endpoints();
        let source = sources[self.below(sources.len() as u64) as usize];
        let request_id = self.below(40) as u32;
        let mut fulfillment = [0u8; 32];
        fulfillment[..8].copy_from_slice(&self.rng.next_u64().to_be_bytes());
        let packet = self.packet(fulfillment);
        match self.h.prepare(self.now, source, request_id, &packet) {
            Outcome::Forward(sent) => {
                assert!(self.awaiting.insert((source, request_id)));
                let answer = match self.below(10) {
                    0..=4 => Answer::Fulfill,
                    5 => Answer::WrongFulfillment,
                    6 | 7 => Answer::Reject,
                    8 => Answer::Garbage,
                    _ if sent.to.is_peer() && self.below(2) == 0 => Answer::Undelivered,
                    _ => Answer::Never,
                };
                let at = self.now + self.below(25_000);
                self.sequence += 1;
                self.due.insert(
                    (at, self.sequence),
                    (sent.to, sent.request_id, fulfillment, answer),
                );
            }
            Outcome::Respond(_) => {}
            Outcome::Duplicate => assert!(self.awaiting.contains(&(source, request_id))),
            Outcome::UnknownSource => panic!("a live endpoint is unknown"),
        }
    }

    fn advance(&mut self) {
        self.now += self.below(3_000);
        while let Some((&key, _)) = self.due.first_key_value() {
            if key.0 > self.now {
                break;
            }
            let (from, request_id, fulfillment, answer) = self.due.remove(&key).unwrap();
            let bytes = match answer {
                Answer::Fulfill => fulfill(fulfillment),
                Answer::WrongFulfillment => fulfill([0xee; 32]),
                Answer::Reject => reject(ErrorCode::F99_APPLICATION_ERROR),
                Answer::Garbage => vec![0x0d, 0x01],
                Answer::Never => continue,
                Answer::Undelivered => {
                    let failed = self.h.connector.fail_forwarded(
                        key.0,
                        from,
                        request_id,
                        ErrorCode::T01_PEER_UNREACHABLE,
                        "not delivered",
                    );
                    self.h.check();
                    let _ = failed;
                    continue;
                }
            };
            if let Some(upstream) = self.h.respond(key.0, from, request_id, &bytes) {
                self.answered(upstream.to, upstream.request_id);
            }
        }
        while let Some(upstream) = self.h.timeout(self.now) {
            self.answered(upstream.to, upstream.request_id);
        }
    }

    fn answered(&mut self, to: EndpointId, request_id: u32) {
        assert!(!self.gone.contains(&to), "an answer to a removed endpoint");
        assert!(
            self.awaiting.remove(&(to, request_id)),
            "an answer nobody waited for, or a second one"
        );
    }

    fn disconnect(&mut self) {
        let slot = self.below(4) as usize; // Eve, slot 4, stays.
        if let Some(id) = self.live[slot].take() {
            assert!(self.h.connector.remove_endpoint(self.now, id));
            self.gone.insert(id);
            self.awaiting.retain(|(source, _)| *source != id);
            while let Some(upstream) = self.h.timeout(self.now) {
                self.answered(upstream.to, upstream.request_id);
            }
            self.h.check();
        }
    }

    fn reconnect(&mut self) {
        let slot = self.below(4) as usize;
        if self.live[slot].is_some() {
            return;
        }
        let id = self.next_btp;
        self.next_btp += 1;
        let account = self
            .h
            .ledger
            .open(dassie_ledger::AccountPath::new(
                LedgerId::StubUsd,
                dassie_ledger::AccountName::EquityOwnerSub(dassie_ledger::OwnerSub::Btp(id)),
            ))
            .unwrap();
        self.h
            .connector
            .add_endpoint(
                EndpointId::Btp(id),
                &format!("{OWN}.{}", NAMES[slot]),
                account,
                LedgerId::StubUsd,
            )
            .unwrap();
        self.live[slot] = Some(EndpointId::Btp(id));
        self.h.check();
    }

    /// Lets everything expire, then checks that every request got its
    /// answer and the journal matches the trace.
    fn finish(&mut self) -> Vec<String> {
        self.now += 60_000;
        self.advance();
        assert_eq!(self.h.connector.pending_count(), 0);
        assert_eq!(self.h.ledger.pending_transfers().count(), 0);
        assert!(self.awaiting.is_empty(), "unanswered: {:?}", self.awaiting);
        let mut posted = BTreeMap::new();
        for entry in &self.h.journal {
            if let JournalEntry::Transfer {
                transfer,
                state: TransferState::Posted,
                ..
            } = entry
                && let Reference::Packet(trace) = transfer.reference
            {
                posted.insert(transfer.id, (trace, transfer.amount));
            }
        }
        let mut seen = 0;
        let mut trace = Vec::new();
        for record in self.h.connector.packets_after(None) {
            trace.push(format!("{record:?}"));
            for id in record.transfers.into_iter().flatten() {
                match record.state {
                    PacketState::Fulfilled => {
                        let (traced, amount) = posted[&id];
                        assert_eq!(traced, record.trace);
                        assert_eq!(amount, record.amount_in);
                        seen += 1;
                    }
                    PacketState::Rejected(_) => assert!(!posted.contains_key(&id)),
                    PacketState::Pending => panic!("a packet still pending"),
                }
            }
        }
        assert_eq!(
            seen,
            posted.len(),
            "a posted packet transfer without its packet"
        );
        trace.push(format!("{:?}", self.h.connector.stats()));
        trace.push(format!("{:?}", self.h.ledger.stats()));
        trace
    }
}

fn run(seed: u64) -> Vec<String> {
    let mut simulation = Simulation::new(seed);
    for _ in 0..STEPS {
        simulation.step();
    }
    simulation.finish()
}

fn seeds() -> std::ops::Range<u64> {
    std::env::var("DASSIE_SIM_SEEDS")
        .ok()
        .and_then(|range| {
            let (from, to) = range.split_once("..")?;
            Some(from.parse().ok()?..to.parse().ok()?)
        })
        .unwrap_or(0..24)
}

#[test]
fn random_traffic_keeps_the_connector_sound() {
    for seed in seeds() {
        let result = catch_unwind(AssertUnwindSafe(|| run(seed)));
        assert!(result.is_ok(), "the simulation failed with seed {seed}");
    }
}

#[test]
fn the_runs_exercise_every_outcome() {
    let mut simulation = Simulation::new(1);
    for _ in 0..STEPS {
        simulation.step();
    }
    simulation.finish();
    let stats = simulation.h.connector.stats();
    for (name, count) in [
        ("forwarded", stats.forwarded),
        ("answered", stats.answered),
        ("fulfilled", stats.fulfilled),
        ("rejected", stats.rejected),
        ("expired", stats.expired),
        ("wrong fulfillments", stats.wrong_fulfillments),
        ("unexpected responses", stats.unexpected_responses),
        ("duplicates", stats.duplicates),
        ("refused by the ledger", simulation.h.ledger.stats().refused),
    ] {
        assert!(count > 0, "no {name}: {stats:?}");
    }
}

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(run(7), run(7));
    assert_ne!(run(7), run(8));
}

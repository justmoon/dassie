//! The connector's random run (spec/areas/testing.md#the-simulations,
//! spec/decisions/random-seed-simulations.md): children and peers send
//! random packets, answer late, wrongly or never, and disconnect; the peers'
//! settlement methods come and go, and are available or not. The invariants
//! are checked after every step, the money in flight on each method among
//! them; every request gets exactly one answer; a peer's packet beyond us
//! keeps the ledger it came in on whenever a hop has an available method on
//! it, and the owner's never converts (spec/decisions/settlement-methods.md);
//! and the journal matches the packet trace.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use dassie_ilp::{ErrorCode, Timestamp, ildcp};
use dassie_kit::sim::{Outcome, Run, SeededRng};
use dassie_ledger::{AccountId, JournalEntry, LedgerId, Name, Reference, TransferState};
use dassie_oer::Encode;

use super::*;
use crate::{Config, EndpointId, Outcome as Handled, PacketState, RATE_PRECISION};

const STEPS: usize = 1_500;
const NAMES: [&str; 5] = ["alice", "bob", "carol", "dave", "eve"];
/// The two peers, each with a method on each of [`LEDGERS`].
const PEERS: [(u64, &str); 2] = [(10, "dfar1"), (11, "dfar2")];
/// The peers' ledgers: the children's, and XRP.
const LEDGERS: [LedgerId; 2] = [LedgerId::StubUsd, LedgerId::XrplTestnetXrp];

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
    /// Each peer's account on each ledger, for a method added again.
    accounts: BTreeMap<(EndpointId, LedgerId), AccountId>,
    /// Peers' packets beyond us that kept their ledger, and that converted.
    kept: u64,
    converted: u64,
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
        // Two peers, each on USD and XRP, and a node beyond them they both
        // lead to; 1 USD buys 2 XRP, and the owner holds both.
        let [usd, xrp] = LEDGERS;
        h.open_books(usd);
        h.fund(xrp, 200_000_000);
        h.open_books(xrp);
        h.rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
        h.rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
        let mut accounts = BTreeMap::new();
        for ((id, node), first) in PEERS.into_iter().zip([0, 1 << 31]) {
            let peer = EndpointId::Peer(id);
            accounts.insert((peer, usd), h.add_peer(id, node, 300_000_000, first));
            accounts.insert((peer, xrp), h.add_slot(xrp, id, node, 600_000_000));
        }
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
            accounts,
            kept: 0,
            converted: 0,
        }
    }

    fn below(&mut self, n: u64) -> u64 {
        self.rng.next_u64() % n
    }

    /// An index below `n`.
    fn pick(&mut self, n: usize) -> usize {
        usize::try_from(self.below(u64::try_from(n).unwrap())).unwrap()
    }

    fn step(&mut self) {
        match self.below(22) {
            0..=11 => self.send(),
            12..=17 => self.advance(),
            18 => self.disconnect(),
            19 => self.reconnect(),
            _ => self.change_method(),
        }
    }

    /// A peer's method on a ledger is made available or not, removed, or
    /// added again.
    fn change_method(&mut self) {
        let peer = EndpointId::Peer(PEERS[self.pick(PEERS.len())].0);
        let ledger = LEDGERS[self.pick(LEDGERS.len())];
        let has = self
            .h
            .connector
            .slots(peer)
            .any(|slot| slot.ledger == ledger && !slot.removed);
        match self.below(3) {
            0 if has => assert!(self.h.connector.remove_slot(peer, ledger)),
            0 => {
                let account = self.accounts[&(peer, ledger)];
                self.h
                    .connector
                    .add_slot(peer, ledger, Some(account))
                    .unwrap();
            }
            available => {
                let set = self.h.connector.set_available(peer, ledger, available == 1);
                let slot = self.h.connector.slots(peer).any(|s| s.ledger == ledger);
                assert_eq!(set, slot, "a slot's availability is set");
            }
        }
        self.h.check();
    }

    /// Whether a hop beyond us other than `source` has an available method
    /// on `ledger`: then a packet from `source` on it keeps it.
    fn keeps(&self, source: EndpointId, ledger: LedgerId) -> bool {
        PEERS
            .iter()
            .map(|(id, _)| EndpointId::Peer(*id))
            .filter(|hop| *hop != source)
            .any(|hop| {
                self.h
                    .connector
                    .slots(hop)
                    .any(|slot| slot.ledger == ledger && slot.available && !slot.removed)
            })
    }

    fn live_endpoints(&self) -> Vec<EndpointId> {
        let mut endpoints: Vec<_> = self.live.iter().flatten().copied().collect();
        endpoints.push(RECEIVER);
        endpoints.extend(PEERS.map(|(id, _)| EndpointId::Peer(id)));
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
                let name = NAMES[self.pick(5)];
                format!("{OWN}.{name}.x")
            }
        }
    }

    fn packet(&mut self, fulfillment: [u8; 32]) -> Vec<u8> {
        match self.below(30) {
            0 => vec![0x0c; self.pick(20)],
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
        let source = sources[self.pick(sources.len())];
        let request_id = u32::try_from(self.below(40)).unwrap();
        let mut fulfillment = [0u8; 32];
        fulfillment[..8].copy_from_slice(&self.rng.next_u64().to_be_bytes());
        let packet = self.packet(fulfillment);
        self.h.draw = self.rng.next_u64();
        // A peer sends on one of its methods, or on one it has no more.
        let on = LEDGERS[self.pick(LEDGERS.len())];
        let keeps = self.keeps(source, on);
        let outcome = if source.is_peer() {
            self.h
                .prepare_on(self.now, (source, on), request_id, &packet)
        } else {
            self.h.prepare(self.now, source, request_id, &packet)
        };
        match outcome {
            Handled::Forward(sent) => {
                let prepare = decode_prepare(&packet);
                let beyond = prepare.destination.as_str().starts_with("test.das.dbeyond");
                if source.is_peer() && beyond {
                    // The ledger kept where a hop can keep it, and only then.
                    assert_eq!(sent.ledger == on, keeps, "kept the ledger: {keeps}");
                    if keeps {
                        self.kept += 1;
                    } else {
                        self.converted += 1;
                    }
                } else if !source.is_peer() && sent.to.is_peer() {
                    assert!(
                        prepare.amount == 0 || sent.ledger == LedgerId::StubUsd,
                        "the owner's packet converted"
                    );
                }
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
            Handled::Respond(_) => {}
            Handled::Duplicate => assert!(self.awaiting.contains(&(source, request_id))),
            Handled::UnknownSource => panic!("a live endpoint is unknown"),
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
        let slot = self.pick(4); // Eve, slot 4, stays.
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
        let slot = self.pick(4);
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
            // The first leg carries what came in, as transit is free here;
            // the second, of a packet that converted, what went out.
            let legs = [record.amount_in, record.amount_out, 0];
            for (id, leg) in record.transfers.into_iter().zip(legs) {
                let Some(id) = id else {
                    continue;
                };
                match record.state {
                    PacketState::Fulfilled => {
                        let (traced, amount) = posted[&id];
                        assert_eq!(traced, record.trace);
                        assert_eq!(amount, leg);
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
        trace.push(format!("kept {} converted {}", self.kept, self.converted));
        trace
    }
}

/// What the seed draws, before the run starts: only the traffic, since
/// every run has the same children, peers and number of steps.
#[must_use]
pub fn describe(_seed: u64) -> String {
    format!(
        "{STEPS} steps, {} children, {} peers on {} ledgers each",
        NAMES.len(),
        PEERS.len(),
        LEDGERS.len()
    )
}

/// Runs the seed. Every check is an assertion, so a run that returns
/// passed.
#[must_use]
pub fn run(seed: u64) -> Run {
    Run::new(Outcome::Pass, finished(seed).0)
}

/// Runs the seed, and returns its trace and the connector and ledger it
/// leaves, to check what the run reached.
#[must_use]
pub fn finished(seed: u64) -> (Vec<String>, Harness) {
    let mut simulation = Simulation::new(seed);
    for _ in 0..STEPS {
        simulation.step();
    }
    let trace = simulation.finish();
    (trace, simulation.h)
}

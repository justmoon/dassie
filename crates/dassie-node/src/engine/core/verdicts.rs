//! Verdicts on Fulfills between peers (D17).
//!
//! Only the upstream peer knows whether a Fulfill came in time, so it
//! says so in its reply: `accepted` once its transfers are posted and on
//! disk, `refused` otherwise. As the downstream, the node posts at once,
//! keeps each Fulfill it owes a peer in the database, and sends it until
//! the verdict comes back, across restarts. A refusal books one
//! compensation with the packet's trace: our own receiver's packet is
//! reversed, anyone else's is our loss. As the upstream, the node records
//! each Fulfill it took for 30 days, so a resend gets the same answer.

use std::collections::BTreeMap;

use dassie_connector::{EndpointId, Posted};
use dassie_identity::NodeId;
use dassie_ilp::{Address, ErrorCode, Reject, Response, condition_for};
use dassie_ledger::{AccountName, AccountPath, Reference, TraceId, TransferSpec};
use dassie_oer::{Decode, Encode};

use super::Core;
use crate::log::target;
use crate::store::{Compensation, OwedFulfill, PaymentId};

/// Most Fulfills owed one peer a verdict; a peer at it gets T03 on new
/// Prepares until it answers.
pub const MAX_OWED_PER_PEER: usize = 1024;

/// Most Fulfills owed a verdict in all.
pub const MAX_OWED: usize = 8192;

/// Most verdicts asked of one peer at once.
const MAX_ASKING_PER_PEER: usize = 16;

/// How long Fulfills taken are kept to answer resends.
pub const TAKEN_KEPT_MS: u64 = 30 * DAY_MS;

/// How long after a Fulfill's expiry its verdict is still asked for: a day
/// less than the upstream keeps it, for clocks that disagree.
const ASKED_FOR_MS: u64 = 29 * DAY_MS;

const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

/// The first wait before asking again, and the longest.
const RETRY_MIN_MS: u64 = 1_000;
const RETRY_MAX_MS: u64 = 60 * 60 * 1_000;

/// How often Fulfills taken long ago are forgotten.
const FORGET_EVERY_MS: u64 = 60 * 60 * 1_000;

/// A Fulfill owed a verdict.
struct Entry {
    row: OwedFulfill,
    tag: u64,
    attempts: u32,
    next_at_ms: u64,
    asking: bool,
}

/// The Fulfills owed a verdict.
pub(super) struct Owed {
    entries: BTreeMap<TraceId, Entry>,
    by_tag: BTreeMap<u64, TraceId>,
    per_peer: BTreeMap<NodeId, usize>,
    asking: BTreeMap<NodeId, usize>,
    next_tag: u64,
    forget_at_ms: u64,
}

impl Owed {
    pub fn new(now_ms: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            by_tag: BTreeMap::new(),
            per_peer: BTreeMap::new(),
            asking: BTreeMap::new(),
            next_tag: 0,
            forget_at_ms: now_ms,
        }
    }

    /// How many Fulfills `peer` owes us a verdict on.
    pub fn count(&self, peer: &NodeId) -> usize {
        self.per_peer.get(peer).copied().unwrap_or(0)
    }

    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// The next time a Fulfill is to be sent, among peers with room.
    pub fn deadline(&self) -> Option<u64> {
        let due = self
            .entries
            .values()
            .filter(|entry| !entry.asking && self.room(&entry.row.peer))
            .map(|entry| entry.next_at_ms)
            .min();
        [due, Some(self.forget_at_ms)].into_iter().flatten().min()
    }

    fn room(&self, peer: &NodeId) -> bool {
        self.asking.get(peer).copied().unwrap_or(0) < MAX_ASKING_PER_PEER
    }

    fn insert(&mut self, row: OwedFulfill, now_ms: u64) {
        let tag = self.next_tag;
        self.next_tag += 1;
        *self.per_peer.entry(row.peer).or_default() += 1;
        self.by_tag.insert(tag, row.trace);
        let previous = self.entries.insert(
            row.trace,
            Entry {
                row,
                tag,
                attempts: 0,
                next_at_ms: now_ms,
                asking: false,
            },
        );
        assert!(previous.is_none(), "a packet owed twice");
    }

    fn remove(&mut self, trace: TraceId) -> Option<OwedFulfill> {
        let entry = self.entries.remove(&trace)?;
        self.by_tag.remove(&entry.tag);
        decrement(&mut self.per_peer, &entry.row.peer);
        if entry.asking {
            decrement(&mut self.asking, &entry.row.peer);
        }
        Some(entry.row)
    }

    /// Checks the counts against the entries.
    pub fn check(&self) {
        assert!(self.entries.len() <= MAX_OWED, "too many Fulfills owed");
        assert_eq!(self.by_tag.len(), self.entries.len(), "tags and entries");
        let mut per_peer: BTreeMap<NodeId, usize> = BTreeMap::new();
        let mut asking: BTreeMap<NodeId, usize> = BTreeMap::new();
        for entry in self.entries.values() {
            *per_peer.entry(entry.row.peer).or_default() += 1;
            if entry.asking {
                *asking.entry(entry.row.peer).or_default() += 1;
            }
        }
        assert_eq!(per_peer, self.per_peer, "Fulfills owed per peer");
        assert_eq!(asking, self.asking, "verdicts asked per peer");
        assert!(
            asking.values().all(|count| *count <= MAX_ASKING_PER_PEER),
            "too many verdicts asked of a peer"
        );
    }
}

fn decrement(counts: &mut BTreeMap<NodeId, usize>, peer: &NodeId) {
    if let Some(count) = counts.get_mut(peer) {
        *count -= 1;
        if *count == 0 {
            counts.remove(peer);
        }
    }
}

/// How long to wait after `attempts` failed tries.
fn backoff(attempts: u32) -> u64 {
    RETRY_MIN_MS
        .checked_shl(attempts)
        .unwrap_or(RETRY_MAX_MS)
        .min(RETRY_MAX_MS)
}

impl Core {
    /// Reads the Fulfills owed a verdict back at a start: they are asked
    /// for again at once, and their incoming payments count them as
    /// waiting.
    pub(super) fn load_owed(&mut self) -> Result<(), String> {
        let rows = self.store.owed_fulfills().map_err(|e| e.to_string())?;
        let now = self.now_ms;
        for row in rows {
            if let Some((id, amount)) = row.incoming {
                self.incoming_waiting(id, amount);
            }
            self.owed.insert(row, now);
        }
        Ok(())
    }

    /// Whether a Prepare from `peer` is refused because it owes us too many
    /// verdicts: every packet in flight from a peer may become one more.
    pub(super) fn owes_too_much(&self, peer: &NodeId, endpoint: EndpointId) -> bool {
        let in_flight = self.connector.pending_from(endpoint);
        let all_in_flight: usize = self
            .peer_endpoints
            .values()
            .map(|id| self.connector.pending_from(EndpointId::Peer(*id)))
            .sum();
        self.owed.count(peer) + in_flight >= MAX_OWED_PER_PEER
            || self.owed.total() + all_in_flight >= MAX_OWED
    }

    /// Answers a Prepare at once with T03, writing the Reject for upstream.
    pub(super) fn refuse_busy(&mut self, now: u64, source: EndpointId, request_id: u32) {
        let reject = Reject {
            code: ErrorCode::T03_CONNECTOR_BUSY,
            triggered_by: Address::parse(self.connector.own_address()).ok(),
            message: "too many verdicts outstanding",
            data: &[],
        };
        let length = reject.encode(&mut self.up);
        self.upstream(
            now,
            dassie_connector::Upstream {
                to: source,
                request_id,
                length,
            },
        );
    }

    /// What a posted packet leaves to do: as the upstream of a peer that
    /// fulfilled it, record that we took it; as the downstream of a peer
    /// that sent it, owe that peer the Fulfill until its verdict. Returns
    /// whether the Fulfill is owed, so it goes out through the verdicts
    /// rather than at once. `incoming` is what our receiver paid in.
    pub(super) fn posted(
        &mut self,
        posted: &Posted,
        downstream_request: u32,
        fulfill: &[u8],
        incoming: Option<(PaymentId, u64)>,
    ) -> bool {
        if let EndpointId::Peer(endpoint) = posted.destination
            && let Some(&node) = self.endpoint_peers.get(&endpoint)
        {
            self.batch
                .verdicts
                .taken
                .push((node, downstream_request, posted.condition));
        }
        // A packet that moved no money needs no verdict: its Fulfill goes
        // as a Reject would, at most for a while.
        let moved = posted.transfers.iter().any(Option::is_some);
        let (EndpointId::Peer(endpoint), true) = (posted.source, moved) else {
            if let Some((id, amount)) = incoming {
                self.incoming_received(id, amount);
            }
            return false;
        };
        let Some(&peer) = self.endpoint_peers.get(&endpoint) else {
            // Peers leave only once drained: a bug.
            tracing::error!(target: target::PEER, endpoint, "a packet posted for a peer that is gone");
            return false;
        };
        let compensation = match posted.destination {
            EndpointId::Local(_) => Compensation::Reverse,
            _ => Compensation::Loss,
        };
        if let Some((id, amount)) = incoming {
            self.incoming_waiting(id, amount);
        }
        let row = OwedFulfill {
            trace: posted.trace,
            peer,
            request_id: posted.source_request_id,
            fulfill: fulfill.to_vec(),
            compensation,
            transfers: posted
                .transfers
                .map(|transfer| transfer.map(|t| (t.debit, t.credit, t.amount))),
            incoming,
            expires_at_ms: posted.expires_at_ms,
            created_at_ms: self.now_ms,
        };
        self.batch.verdicts.owed.push(row.clone());
        self.owed.insert(row, self.now_ms);
        true
    }

    /// Whether we took this Fulfill from `peer` for our Prepare
    /// `request_id` before: the answer to a resend.
    pub(super) fn taken_before(&self, peer: &NodeId, request_id: u32, packet: &[u8]) -> bool {
        let Ok(Response::Fulfill(fulfill)) = Response::decode_exact(packet) else {
            return false;
        };
        let condition = condition_for(&fulfill.fulfillment);
        // Taken in this step, not yet on disk.
        let pending = self
            .batch
            .verdicts
            .taken
            .contains(&(*peer, request_id, condition));
        pending
            || self
                .store
                .fulfill_taken(peer, request_id, &condition)
                .unwrap_or_else(|error| {
                    tracing::error!(target: target::PEER, %error, "looking up a Fulfill taken");
                    false
                })
    }

    /// Sends the Fulfills whose time has come, gives up on those past
    /// asking, and forgets Fulfills taken long ago.
    pub(super) fn verdict_timers(&mut self) {
        let now = self.now_ms;
        if self.owed.forget_at_ms <= now {
            self.owed.forget_at_ms = now.saturating_add(FORGET_EVERY_MS);
            self.batch.verdicts.forget_before_ms = Some(now.saturating_sub(TAKEN_KEPT_MS));
        }
        let due: Vec<TraceId> = self
            .owed
            .entries
            .iter()
            .filter(|(_, entry)| !entry.asking && entry.next_at_ms <= now)
            .map(|(trace, _)| *trace)
            .collect();
        for trace in due {
            let Some(entry) = self.owed.entries.get(&trace) else {
                continue;
            };
            if entry.row.expires_at_ms.saturating_add(ASKED_FOR_MS) <= now {
                tracing::error!(target: target::PEER, peer = %entry.row.peer, ?trace, "no verdict on a Fulfill for 29 days: booked as refused, and the books may disagree with the peer's");
                self.verdict(trace, false);
                continue;
            }
            if !self.owed.room(&entry.row.peer) {
                continue;
            }
            self.ask(trace);
        }
    }

    /// Sends one Fulfill for its verdict.
    fn ask(&mut self, trace: TraceId) {
        let now = self.now_ms;
        let Some(entry) = self.owed.entries.get_mut(&trace) else {
            return;
        };
        let peer = entry.row.peer;
        let sent = self.peer.send_fulfill(
            now,
            &peer,
            entry.row.request_id,
            &entry.row.fulfill,
            entry.tag,
        );
        match sent {
            Ok(()) => {
                entry.asking = true;
                *self.owed.asking.entry(peer).or_default() += 1;
            }
            Err(_) => {
                entry.next_at_ms = now.saturating_add(backoff(entry.attempts));
                entry.attempts = entry.attempts.saturating_add(1);
            }
        }
        self.after_peer();
    }

    /// No verdict came for `tag`: ask again later.
    pub(super) fn verdict_missing(&mut self, tag: u64) {
        let now = self.now_ms;
        let Some(trace) = self.owed.by_tag.get(&tag).copied() else {
            return;
        };
        let Some(entry) = self.owed.entries.get_mut(&trace) else {
            return;
        };
        if entry.asking {
            entry.asking = false;
            decrement(&mut self.owed.asking, &entry.row.peer);
        }
        entry.next_at_ms = now.saturating_add(backoff(entry.attempts));
        entry.attempts = entry.attempts.saturating_add(1);
    }

    /// The verdict on `tag` came: the Fulfill counts, or its compensation
    /// is booked. A verdict on one no longer owed changes nothing.
    pub(super) fn verdict_on(&mut self, tag: u64, accepted: bool) {
        if let Some(trace) = self.owed.by_tag.get(&tag).copied() {
            self.verdict(trace, accepted);
        }
    }

    fn verdict(&mut self, trace: TraceId, accepted: bool) {
        let Some(row) = self.owed.remove(trace) else {
            return;
        };
        self.batch.verdicts.settled.push(trace);
        if accepted {
            if let Some((id, amount)) = row.incoming {
                self.incoming_confirmed(id, amount);
            }
            return;
        }
        if let Some((id, amount)) = row.incoming {
            self.incoming_released(id, amount);
        }
        self.compensate(&row);
    }

    /// Books what a refused Fulfill leaves: the packet reversed, or the
    /// loss, so the peer's account ends where the peer's books have it.
    fn compensate(&mut self, row: &OwedFulfill) {
        let reference = Reference::Refusal(row.trace);
        let specs: Vec<TransferSpec> = match row.compensation {
            Compensation::Reverse => row
                .transfers
                .iter()
                .flatten()
                .map(|&(debit, credit, amount)| TransferSpec {
                    debit: credit,
                    credit: debit,
                    amount,
                    pending: false,
                    reference,
                })
                .collect(),
            Compensation::Loss => {
                let Some((peer_account, _, amount)) = row.transfers[0] else {
                    return;
                };
                let Some(losses) = self.losses_account(peer_account) else {
                    tracing::error!(target: target::PEER, peer = %row.peer, "no account for losses; the refusal is not booked");
                    return;
                };
                vec![TransferSpec {
                    debit: losses,
                    credit: peer_account,
                    amount,
                    pending: false,
                    reference,
                }]
            }
        };
        let amount: u64 = row
            .transfers
            .first()
            .copied()
            .flatten()
            .map_or(0, |(_, _, amount)| amount);
        tracing::warn!(target: target::PEER, peer = %row.peer, amount, trace = ?row.trace, compensation = ?row.compensation, "a peer refused our Fulfill");
        for spec in specs {
            self.take_journal();
            if let Err(error) = self.ledger.create(self.now_ms, &spec) {
                tracing::error!(target: target::PEER, peer = %row.peer, ?error, "booking a refusal failed");
            }
        }
        self.take_journal();
    }

    /// `expenses/losses` on the ledger of `account`.
    fn losses_account(
        &mut self,
        account: dassie_ledger::AccountId,
    ) -> Option<dassie_ledger::AccountId> {
        let ledger = self.ledger.account(account)?.path().ledger;
        self.take_journal();
        self.ledger
            .open(AccountPath::new(ledger, AccountName::ExpensesLosses))
            .ok()
    }
}

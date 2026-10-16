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

use std::collections::{BTreeMap, BTreeSet};

use dassie_connector::{EndpointId, Posted};
use dassie_identity::NodeId;
use dassie_ilp::{Address, ErrorCode, Reject, Response, condition_for};
use dassie_ledger::{AccountName, AccountPath, Reference, TraceId, TransferSpec};
use dassie_oer::{Decode, Encode};

use super::Core;
use super::events::{PeerEvent, StoreRead};
use crate::store::{Compensation, OwedFulfill, PaymentId};

/// Most Fulfills one peer may owe us a verdict on, counting the packets in
/// flight from it that may become more; a peer at it gets T03 on new
/// Prepares until it answers. Every peer holds a settlement channel, so
/// all that is owed is at most this times the channels.
pub const MAX_OWED_PER_PEER: usize = 1024;

/// Most verdicts asked of one peer at once.
const MAX_ASKING_PER_PEER: usize = 16;

/// How long Fulfills taken are kept to answer resends.
pub const TAKEN_KEPT_MS: u64 = 30 * DAY_MS;

/// How long after a Fulfill's expiry its verdict is still asked for: a day
/// less than the upstream keeps it, for clocks that disagree.
const ASKED_FOR_MS: u64 = 29 * DAY_MS;

/// How long a peering being ended waits for the verdicts it is owed; they
/// are still asked for after.
pub const VERDICT_DRAIN_MS: u64 = 10 * 60 * 1_000;

const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

/// The first wait before asking again, and the longest.
const RETRY_MIN_MS: u64 = 1_000;
const RETRY_MAX_MS: u64 = 60 * 60 * 1_000;

/// How often Fulfills taken long ago are forgotten, a batch at a time.
const FORGET_EVERY_MS: u64 = 60 * 1_000;

/// Where a Fulfill owed a verdict is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    /// To be sent at this time.
    Waiting(u64),
    /// Due, waiting for room among the verdicts asked of its peer.
    Blocked,
    /// Sent; its verdict is awaited.
    Asking,
}

/// A Fulfill owed a verdict. Its bytes stay in the database.
struct Entry {
    row: OwedFulfill,
    tag: u64,
    attempts: u32,
    state: State,
}

/// The Fulfills owed a verdict, in the order they are due.
pub(super) struct Owed {
    entries: BTreeMap<TraceId, Entry>,
    by_tag: BTreeMap<u64, TraceId>,
    per_peer: BTreeMap<NodeId, usize>,
    asking: BTreeMap<NodeId, usize>,
    due: BTreeSet<(u64, TraceId)>,
    blocked: BTreeMap<NodeId, BTreeSet<TraceId>>,
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
            due: BTreeSet::new(),
            blocked: BTreeMap::new(),
            next_tag: 0,
            forget_at_ms: now_ms,
        }
    }

    /// How many Fulfills `peer` owes us a verdict on.
    pub fn count(&self, peer: &NodeId) -> usize {
        self.per_peer.get(peer).copied().unwrap_or(0)
    }

    /// What each incoming payment waits for, over the Fulfills owed.
    pub fn waiting(&self) -> BTreeMap<PaymentId, u64> {
        let mut waiting = BTreeMap::<PaymentId, u64>::new();
        for entry in self.entries.values() {
            if let Some((id, amount)) = entry.row.incoming {
                *waiting.entry(id).or_default() += amount;
            }
        }
        waiting
    }

    /// The next time a Fulfill is to be sent, or old ones forgotten.
    pub fn deadline(&self) -> Option<u64> {
        let due = self.due.first().map(|(at, _)| *at);
        [due, Some(self.forget_at_ms)].into_iter().flatten().min()
    }

    fn room(&self, peer: &NodeId) -> bool {
        self.asking.get(peer).copied().unwrap_or(0) < MAX_ASKING_PER_PEER
    }

    fn insert(&mut self, mut row: OwedFulfill, now_ms: u64) {
        row.fulfill = Vec::new();
        let tag = self.next_tag;
        self.next_tag += 1;
        *self.per_peer.entry(row.peer).or_default() += 1;
        self.by_tag.insert(tag, row.trace);
        self.due.insert((now_ms, row.trace));
        let previous = self.entries.insert(
            row.trace,
            Entry {
                row,
                tag,
                attempts: 0,
                state: State::Waiting(now_ms),
            },
        );
        assert!(previous.is_none(), "a packet owed twice");
    }

    /// Moves an entry to `state`, keeping the queues and counts.
    fn set(&mut self, trace: TraceId, state: State) {
        let Some(entry) = self.entries.get_mut(&trace) else {
            return;
        };
        let peer = entry.row.peer;
        let old = std::mem::replace(&mut entry.state, state);
        match old {
            State::Waiting(at) => {
                self.due.remove(&(at, trace));
            }
            State::Blocked => {
                if let Some(blocked) = self.blocked.get_mut(&peer) {
                    blocked.remove(&trace);
                    if blocked.is_empty() {
                        self.blocked.remove(&peer);
                    }
                }
            }
            State::Asking => decrement(&mut self.asking, &peer),
        }
        match state {
            State::Waiting(at) => {
                self.due.insert((at, trace));
            }
            State::Blocked => {
                self.blocked.entry(peer).or_default().insert(trace);
            }
            State::Asking => *self.asking.entry(peer).or_default() += 1,
        }
        if old == State::Asking {
            self.unblock(peer);
        }
    }

    /// Lets one Fulfill blocked behind `peer`'s verdicts go, or all of
    /// them once none is asked any more.
    fn unblock(&mut self, peer: NodeId) {
        let all = !self.asking.contains_key(&peer);
        let next: Vec<TraceId> = self
            .blocked
            .get(&peer)
            .map(|blocked| {
                blocked
                    .iter()
                    .take(if all { usize::MAX } else { 1 })
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for trace in next {
            self.set(trace, State::Waiting(0));
        }
    }

    fn remove(&mut self, trace: TraceId) -> Option<OwedFulfill> {
        // Out of every queue first, which may let a blocked one go.
        self.set(trace, State::Waiting(0));
        self.due.remove(&(0, trace));
        let entry = self.entries.remove(&trace)?;
        self.by_tag.remove(&entry.tag);
        decrement(&mut self.per_peer, &entry.row.peer);
        Some(entry.row)
    }

    /// Checks the queues and counts against the entries.
    pub fn check(&self) {
        assert_eq!(self.by_tag.len(), self.entries.len(), "tags and entries");
        let (mut per_peer, mut asking) = (BTreeMap::new(), BTreeMap::new());
        let (mut due, mut blocked) = (0, 0);
        for (trace, entry) in &self.entries {
            *per_peer.entry(entry.row.peer).or_insert(0usize) += 1;
            match entry.state {
                State::Waiting(at) => {
                    assert!(self.due.contains(&(at, *trace)), "a waiting one is due");
                    due += 1;
                }
                State::Blocked => blocked += 1,
                State::Asking => *asking.entry(entry.row.peer).or_insert(0usize) += 1,
            }
        }
        assert_eq!(due, self.due.len(), "the due queue");
        assert_eq!(
            blocked,
            self.blocked.values().map(BTreeSet::len).sum::<usize>(),
            "the blocked queues"
        );
        assert_eq!(per_peer, self.per_peer, "Fulfills owed per peer");
        assert!(
            per_peer.values().all(|count| *count <= MAX_OWED_PER_PEER),
            "a peer owes more verdicts than it may"
        );
        assert_eq!(asking, self.asking, "verdicts asked per peer");
        assert!(
            asking.values().all(|count| *count <= MAX_ASKING_PER_PEER),
            "too many verdicts asked of a peer"
        );
        // Each answer lets one blocked Fulfill go, so none is left behind
        // a peer that has stopped being asked.
        assert!(
            self.blocked
                .keys()
                .all(|peer| self.asking.contains_key(peer)),
            "a Fulfill blocked behind a peer not asked"
        );
    }

    /// Whether `peer` still owes us verdicts that a peering being ended
    /// since `since_ms` waits for: for a while, as they normally come
    /// back within seconds.
    pub fn holds(&self, peer: &NodeId, since_ms: u64, now_ms: u64) -> bool {
        self.count(peer) > 0 && now_ms < since_ms.saturating_add(VERDICT_DRAIN_MS)
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
    (RETRY_MIN_MS << attempts.min(12)).min(RETRY_MAX_MS)
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
    /// verdicts: every packet in flight from it may become one more.
    pub(super) fn owes_too_much(&self, peer: &NodeId, endpoint: EndpointId) -> bool {
        self.owed.count(peer) + self.connector.pending_from(endpoint) >= MAX_OWED_PER_PEER
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
    /// that sent it, owe that peer the Fulfill until its verdict. A packet
    /// that moved no money needs neither: its Fulfill goes as a Reject
    /// would. Returns whether the Fulfill is owed, so it goes out through
    /// the verdicts rather than at once. `incoming` is what our receiver
    /// paid in; `fulfill` is empty when the source is gone.
    pub(super) fn posted(
        &mut self,
        posted: &Posted,
        downstream_request: u32,
        fulfill: &[u8],
        incoming: Option<(PaymentId, u64)>,
    ) -> bool {
        let moved = posted.transfers.iter().any(Option::is_some);
        if let (EndpointId::Peer(endpoint), true) = (posted.destination, moved)
            && let Some(&node) = self.endpoint_peers.get(&endpoint)
        {
            self.batch
                .verdicts
                .taken
                .push((node, downstream_request, posted.condition));
        }
        let peer = match posted.source {
            EndpointId::Peer(endpoint) if moved => self.endpoint_peers.get(&endpoint).copied(),
            _ => None,
        };
        let Some(peer) = peer.filter(|_| !fulfill.is_empty()) else {
            // Peers leave only once drained.
            assert!(
                peer.is_some() || !(posted.source.is_peer() && moved),
                "a packet posted for a peer that is gone: {}",
                posted.source
            );
            if let Some((id, amount)) = incoming {
                self.incoming_received(id, amount);
            }
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
    pub(super) fn taken_before(&mut self, peer: &NodeId, request_id: u32, packet: &[u8]) -> bool {
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
                    self.report(PeerEvent::StoreReadFailed {
                        what: StoreRead::FulfillTaken,
                        error: error.to_string(),
                    });
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
        while let Some(&(at, trace)) = self.owed.due.first()
            && at <= now
        {
            let Some(entry) = self.owed.entries.get(&trace) else {
                self.owed.due.remove(&(at, trace));
                continue;
            };
            let peer = entry.row.peer;
            if entry.row.expires_at_ms.saturating_add(ASKED_FOR_MS) <= now {
                self.report(PeerEvent::VerdictOverdue { peer, trace });
                self.verdict(trace, false);
            } else if self.peer.is_banned(&peer) {
                // A banned node is not asked; its Fulfills wait out their
                // time, or a lifted ban.
                self.owed
                    .set(trace, State::Waiting(now.saturating_add(RETRY_MAX_MS)));
            } else if self.owed.room(&peer) {
                self.ask(trace);
            } else {
                self.owed.set(trace, State::Blocked);
            }
        }
    }

    /// The bytes of a Fulfill owed: from this step's batch, or the disk.
    fn owed_bytes(&mut self, trace: TraceId) -> Option<Vec<u8>> {
        if let Some(row) = self
            .batch
            .verdicts
            .owed
            .iter()
            .find(|row| row.trace == trace)
        {
            return Some(row.fulfill.clone());
        }
        self.store
            .owed_fulfill_bytes(trace)
            .unwrap_or_else(|error| {
                self.report(PeerEvent::StoreReadFailed {
                    what: StoreRead::FulfillOwed,
                    error: error.to_string(),
                });
                None
            })
    }

    /// Sends one Fulfill for its verdict.
    fn ask(&mut self, trace: TraceId) {
        let now = self.now_ms;
        let bytes = self.owed_bytes(trace);
        let Some(entry) = self.owed.entries.get_mut(&trace) else {
            return;
        };
        let (peer, request_id, tag) = (entry.row.peer, entry.row.request_id, entry.tag);
        let sent = bytes.is_some_and(|bytes| {
            self.peer
                .send_fulfill(now, &peer, request_id, &bytes, tag)
                .is_ok()
        });
        if sent {
            self.owed.set(trace, State::Asking);
        } else {
            self.retry_later(trace);
        }
        self.after_peer();
    }

    /// Schedules the next try, a little later each time.
    fn retry_later(&mut self, trace: TraceId) {
        let now = self.now_ms;
        let Some(entry) = self.owed.entries.get_mut(&trace) else {
            return;
        };
        let at = now.saturating_add(backoff(entry.attempts));
        entry.attempts = entry.attempts.saturating_add(1);
        self.owed.set(trace, State::Waiting(at));
    }

    /// No verdict came for `tag`: ask again later.
    pub(super) fn verdict_missing(&mut self, tag: u64) {
        if let Some(trace) = self.owed.by_tag.get(&tag).copied() {
            self.retry_later(trace);
        }
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
    /// The accounts and amounts are the node's own, as it posted them:
    /// booking them back cannot fail but by a bug.
    // Panics only on a broken invariant: what is owed has its transfers.
    #[allow(clippy::panic)]
    fn compensate(&mut self, row: &OwedFulfill) {
        let reference = Reference::Refusal(row.trace);
        let Some((peer_account, _, _)) = row.transfers[0] else {
            panic!("a Fulfill owed without its transfers");
        };
        // What the peer's books never took: every leg that debited its
        // account, our fee with the packet.
        let amount: u64 = row
            .transfers
            .iter()
            .flatten()
            .filter(|(debit, _, _)| *debit == peer_account)
            .map(|(_, _, amount)| amount)
            .sum();
        let specs: Vec<TransferSpec> = match row.compensation {
            Compensation::Reverse => row
                .transfers
                .into_iter()
                .flatten()
                .map(|(debit, credit, amount)| TransferSpec {
                    debit: credit,
                    credit: debit,
                    amount,
                    pending: false,
                    reference,
                })
                .collect(),
            Compensation::Loss => vec![TransferSpec {
                debit: self.losses_account(peer_account),
                credit: peer_account,
                amount,
                pending: false,
                reference,
            }],
        };
        self.report(PeerEvent::FulfillRefused {
            peer: row.peer,
            amount,
            trace: row.trace,
            compensation: row.compensation,
        });
        if row.compensation == Compensation::Loss {
            self.grace.check_soon(row.peer);
        }
        for spec in specs {
            self.take_journal();
            let booked = self.ledger.create(self.now_ms, &spec);
            assert!(booked.is_ok(), "booking a refusal failed: {booked:?}");
        }
        self.take_journal();
    }

    /// `expenses/losses` on the ledger of `account`, which every active
    /// ledger has; opened if it is missing.
    // Panics only on a broken invariant: what is owed has its accounts.
    #[allow(clippy::panic)]
    fn losses_account(&mut self, account: dassie_ledger::AccountId) -> dassie_ledger::AccountId {
        let ledger = self
            .ledger
            .account(account)
            .map(|account| account.path().ledger);
        let Some(ledger) = ledger else {
            panic!("a Fulfill owed on an account that does not exist");
        };
        self.take_journal();
        let opened = self
            .ledger
            .open(AccountPath::new(ledger, AccountName::ExpensesLosses));
        assert!(opened.is_ok(), "no account for losses: {opened:?}");
        opened.unwrap_or(account)
    }
}

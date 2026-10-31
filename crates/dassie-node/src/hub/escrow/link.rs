//! What following a hub's escrow holds of its connection and its network
//! rather than of the escrow: the WebSocket's requests and their ids, the
//! verifier and its proof requests, where the server stands, and what the
//! host keeps of the verifier across a restart. One [`Link`] serves one
//! escrow ([`super::Watcher`]), or every escrow of the hubs the node
//! validates on its network ([`super::LinkWatcher`]); the escrow's own part
//! is [`EscrowWatcher`](super::watch::EscrowWatcher), which takes the link
//! along on each call.
//!
//! A link that serves several escrows tags what each asks and hands its
//! host with the hub whose call it is, so that answers and attestations
//! reach their own; gives each escrow a fair share of what is bounded, the
//! verifier's queries and its own requests, so that a flood of one's hints
//! leaves the others theirs; and connects anew for one escrow's reasons
//! only every so often ([`ESCROW_RECONNECT_MS`]), since a new connection
//! is every escrow's.
//!
//! The verifier answers some queries at once, in the call that asked: an
//! escrow's drain handles its own verdicts and anchors as they come, as
//! what queried relies on, and parks those that are another's
//! ([`Parked`]), for its host to hand on once the call returned.

use std::collections::{BTreeMap, VecDeque};

use dassie_hub::HubId;
use dassie_xrpl::api::{self, ApiError};
use dassie_xrpl::unl::{Anchor, ValidationSignatures};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::verifier::{self, ANCHOR_EVERY_MS, Ask, QueryId, Verdict, Verifier};
use serde_json::Value;

use super::watch::{HINT_RETRY_MS, MAX_HINT_TRIES};
use super::{Chain, KEEP_VERIFIER_EVERY_MS, Kept, Out, RETRY_MS, Unrestored, chain as judge_chain};
use crate::hub::config::MAX_VALIDATED_HUBS;
use crate::store::{EscrowCursor, MAX_HUB_VERIFIER_BYTES};

/// The most requests of its own waiting for an answer.
const MAX_PENDING: usize = 16;

/// The most of them that submit an escrow transaction: the rest are kept
/// for following the escrow.
const MAX_PENDING_SUBMITS: usize = MAX_PENDING / 2;

/// The most proof requests the verifier has waiting on a connection: the
/// verifier's own bound, with room for those of a round it gave up on.
const MAX_PROOF_ASKS: usize = 4 * verifier::MAX_QUERIES;

/// How long after an escrow's reason ended the connection another
/// escrow's, or the same one's, may end it again, on a link that serves
/// several: as long as one escrow takes to give up on a server that does
/// not prove what it hints at, so no escrow connects anew more often for
/// sharing its link than it would alone. The verifier's reasons, which are
/// every escrow's, end it at once.
pub(super) const ESCROW_RECONNECT_MS: u64 = MAX_HINT_TRIES as u64 * HINT_RETRY_MS;

/// Who a request or an output is for: the hub whose escrow it is, or none,
/// the link's own, as everything is on a link that serves one escrow.
pub(super) type Owner = Option<HubId>;

/// What a request of its own was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Asked {
    ServerState,
    Subscribe,
    History,
    Submit,
}

/// Where the connection is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Disconnected,
    /// Asked where the server stands.
    Checking,
    /// Its server is behind, on another network, or failed to answer:
    /// asked again later.
    Waiting {
        since_ms: u64,
    },
    /// Its server is on the hub's network, and on the chain followed, as
    /// far as it says: the escrow is subscribed to.
    Checked,
}

/// What the verifier decided that an escrow's drain met and did not take,
/// or not alone: for the link's host to hand on once the call returned.
/// Handed on within the same call, as the verifier hands its verdicts:
/// boxing would only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Parked {
    /// A verdict on a query that is not the draining escrow's.
    Verdict(QueryId, Verdict),
    /// A newest anchor, `true` if a reset, which the draining escrow, if
    /// one did, took at once: the others take it after.
    Anchored(Anchor, bool, Owner),
}

/// One connection to an XRPL server, and the verifier that proves what it
/// answers, for the hub's network.
#[derive(Debug)]
pub(super) struct Link {
    network: u32,
    /// Whether it serves several escrows, each tagged by its hub, rather
    /// than one.
    shared: bool,
    /// How many escrows it serves, at least one: each has a share of what
    /// is bounded.
    escrows: usize,
    /// The hub whose escrow's call this is, on a shared link: what it asks
    /// and hands the host is that hub's.
    serving: Owner,
    phase: Phase,
    next_id: u64,
    pending: BTreeMap<u64, (Asked, Owner)>,
    /// The verifier's asks on this connection, by their request's id.
    proof_asks: BTreeMap<u64, Ask>,
    verifier: Verifier,
    /// Whether it said the verifier stopped.
    failed_told: bool,
    /// When the verifier was last asked to anchor on the link's own.
    anchor_asked_ms: Option<u64>,
    /// Whether its verifier's anchor was taken back from before a restart,
    /// and none proven since: no hint is proven against it, as it may be of
    /// a chain gone while the validator was down.
    taken_back: bool,
    /// The verifier's count of changes when its state was last handed to
    /// the host, or taken back.
    kept_changes: u64,
    /// When it was last handed to the host.
    kept_ms: Option<u64>,
    /// What to write of the verifier with the cursor, until the host takes
    /// it.
    keeping: Option<Kept>,
    /// Whether its state was last too large to keep, and what the host
    /// kept before dropped.
    over_bound: bool,
    /// What escrows' drains parked, empty between calls.
    parked: VecDeque<Parked>,
    /// The anchor index of the reset its server was last asked again for,
    /// on this connection: every escrow's reset asks once.
    again_for: Option<u32>,
    /// When an escrow's reason last ended the connection.
    escrow_reconnect_ms: Option<u64>,
    /// The server's validated ledger, as it last said: what an escrow
    /// subscribed to later reads its history up to.
    validated: Option<u32>,
    /// The keys the verifier reads proven transactions' threads for, on a
    /// shared link: every escrow's.
    threads: Vec<[u8; 32]>,
    now_ms: u64,
    out: VecDeque<(Owner, Out)>,
}

/// Building and reading it.
impl Link {
    /// A link on `network`, trusting the validators the publishers of
    /// `unl` list, whose validations `signatures` checks.
    pub(super) fn new(
        network: u32,
        unl: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
    ) -> Self {
        Self {
            network,
            shared: false,
            escrows: 1,
            serving: None,
            phase: Phase::Disconnected,
            next_id: 1,
            pending: BTreeMap::new(),
            proof_asks: BTreeMap::new(),
            verifier: Verifier::new(network, unl, signatures),
            failed_told: false,
            anchor_asked_ms: None,
            taken_back: false,
            kept_changes: 0,
            kept_ms: None,
            keeping: None,
            over_bound: false,
            parked: VecDeque::new(),
            again_for: None,
            escrow_reconnect_ms: None,
            validated: None,
            threads: Vec::new(),
            now_ms: 0,
            out: VecDeque::new(),
        }
    }

    /// The same, to serve up to [`MAX_VALIDATED_HUBS`] escrows, each
    /// tagged by its hub.
    pub(super) fn shared(
        network: u32,
        unl: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
    ) -> Self {
        Self {
            shared: true,
            ..Self::new(network, unl, signatures)
        }
    }

    /// Takes back at `now_ms` what it handed its host of its verifier
    /// before a restart, verified again at `now_ms`, whose validations
    /// `signatures` checks ([`super::Watcher::restore`]).
    pub(super) fn restore(
        &mut self,
        kept: &[u8],
        signatures: Box<dyn ValidationSignatures + Send>,
        now_ms: u64,
    ) -> Result<(), Unrestored> {
        let (network, rest) = kept.split_first_chunk::<4>().ok_or(Unrestored::Unread)?;
        let network = u32::from_be_bytes(*network);
        if network != self.network {
            return Err(Unrestored::Network(network));
        }
        let (publishers, state) = rest.split_first_chunk::<32>().ok_or(Unrestored::Unread)?;
        let config = self.verifier.unl().config().clone();
        if *publishers != publishers_digest(&config) {
            return Err(Unrestored::Publishers);
        }
        let verifier = Verifier::restore(self.network, config, signatures, state, now_ms)
            .ok_or(Unrestored::Unread)?;
        self.verifier = verifier;
        // Its threads are no part of its state: they are told it again.
        self.threads.clear();
        self.now(now_ms);
        self.taken_back = self.verifier.anchor().is_some();
        self.kept_changes = self.verifier.changes();
        Ok(())
    }

    /// What to write of the verifier with the cursor just taken, if
    /// anything.
    pub(super) const fn take_kept(&mut self) -> Option<Kept> {
        self.keeping.take()
    }

    pub(super) const fn network(&self) -> u32 {
        self.network
    }

    /// Whether it serves several escrows rather than one.
    pub(super) const fn is_shared(&self) -> bool {
        self.shared
    }

    /// It serves `escrows` escrows now, of at most [`MAX_VALIDATED_HUBS`]:
    /// each has a share of what is bounded.
    pub(super) fn set_escrows(&mut self, escrows: usize) {
        assert!(self.shared, "only a shared link serves several");
        assert!(escrows <= MAX_VALIDATED_HUBS, "escrows are bounded");
        self.escrows = escrows.max(1);
    }

    /// What follows is the call of `hub`'s escrow, or the link's own.
    pub(super) fn serve(&mut self, hub: Owner) {
        assert!(self.shared || hub.is_none(), "one escrow is untagged");
        self.serving = hub;
    }

    /// How many of the verifier's queries an escrow may have open: an
    /// equal share, so the shares fit the verifier's bound together.
    pub(super) const fn query_share(&self) -> usize {
        verifier::MAX_QUERIES / self.escrows
    }

    /// How many of them may prove hints, leaving the rest of its share for
    /// the escrow's and the issuer's state and the walk.
    pub(super) const fn hint_share(&self) -> usize {
        self.query_share() / 2
    }

    /// How many of its own requests an escrow may have waiting, on a shared
    /// link: an equal share of all but one, which is the link's own.
    const fn pending_share(&self) -> usize {
        (MAX_PENDING - 1) / self.escrows
    }

    /// How many of them may submit an escrow transaction.
    const fn submit_share(&self) -> usize {
        MAX_PENDING_SUBMITS / self.escrows
    }

    /// The server's validated ledger, as it last said.
    pub(super) const fn validated(&self) -> Option<u32> {
        self.validated
    }

    /// The stream said ledger `index` closed, validated.
    pub(super) const fn closed(&mut self, index: u32) {
        self.validated = Some(index);
    }

    /// The verifier reads proven transactions for the threads of `keys`,
    /// of every escrow it serves, from now on.
    pub(super) fn thread(&mut self, keys: Vec<[u8; 32]>) {
        if keys != self.threads {
            self.verifier.thread(keys.clone());
            self.threads = keys;
        }
    }

    /// The keys it reads threads for.
    pub(super) fn threads(&self) -> &[[u8; 32]] {
        &self.threads
    }

    /// How many escrows it serves.
    pub(super) const fn escrows(&self) -> usize {
        self.escrows
    }

    pub(super) const fn now_ms(&self) -> u64 {
        self.now_ms
    }

    pub(super) const fn now(&mut self, now_ms: u64) {
        if now_ms > self.now_ms {
            self.now_ms = now_ms;
        }
    }

    /// Whether a connection is up.
    pub(super) fn is_connected(&self) -> bool {
        self.phase != Phase::Disconnected
    }

    /// Whether its server's network and chain were checked, and escrows
    /// subscribe on it.
    pub(super) fn checked(&self) -> bool {
        self.phase == Phase::Checked
    }

    pub(super) const fn verifier(&self) -> &Verifier {
        &self.verifier
    }

    pub(super) const fn verifier_mut(&mut self) -> &mut Verifier {
        &mut self.verifier
    }

    /// Whether its verifier's anchor was taken back, and none proven since.
    pub(super) const fn taken_back(&self) -> bool {
        self.taken_back
    }

    /// The verifier proved an anchor with fresh signatures, on the chain it
    /// is on: the chain lives, whichever the anchor taken back was on.
    pub(super) const fn anchor_proven(&mut self) {
        self.taken_back = false;
    }

    /// Hands `out` to the host, after what it was handed before, as the
    /// escrow's whose call this is. A new connection an escrow asks for, on
    /// a shared link, goes only [`ESCROW_RECONNECT_MS`] after the last
    /// one an escrow asked for.
    pub(super) fn push(&mut self, out: Out) {
        if self.shared && self.serving.is_some() && matches!(out, Out::Reconnect) {
            let now = self.now_ms;
            if (self.escrow_reconnect_ms)
                .is_some_and(|at| now.saturating_sub(at) < ESCROW_RECONNECT_MS)
            {
                return;
            }
            self.escrow_reconnect_ms = Some(now);
        }
        self.out.push_back((self.serving, out));
    }

    /// Hands `out` to the host as the link's own.
    fn push_own(&mut self, out: Out) {
        self.out.push_back((None, out));
    }

    /// The next thing to do.
    pub(super) fn poll(&mut self) -> Option<Out> {
        self.out.pop_front().map(|(_, out)| out)
    }

    /// The next thing to do, and whose it is.
    pub(super) fn poll_owned(&mut self) -> Option<(Owner, Out)> {
        self.out.pop_front()
    }

    /// How many things it has for the host to do.
    pub(super) fn queued(&self) -> usize {
        self.out.len()
    }

    /// The hub whose escrow's call this is, if any.
    pub(super) const fn serving(&self) -> Owner {
        self.serving
    }

    /// Whether what it handed the host holds a cursor.
    pub(super) fn cursor_queued(&self) -> bool {
        (self.out.iter()).any(|(_, out)| matches!(out, Out::Cursor(_)))
    }

    /// Whether it has the verifier's state to hand over with a cursor.
    pub(super) const fn keeping(&self) -> bool {
        self.keeping.is_some()
    }

    /// Parks what the draining escrow did not take, or not alone.
    pub(super) fn park(&mut self, parked: Parked) {
        self.parked.push_back(parked);
    }

    /// Parks a copy of a newest anchor, `true` if a reset, which the
    /// escrow whose call this is took at once.
    pub(super) fn park_anchor(&mut self, anchor: Anchor, reset: bool) {
        (self.parked).push_back(Parked::Anchored(anchor, reset, self.serving));
    }

    /// Forgets what `hub`'s escrow asked and was to hand the host: it is
    /// served no more.
    pub(super) fn forget_hub(&mut self, hub: HubId) {
        self.pending.retain(|_, (_, owner)| *owner != Some(hub));
        self.out.retain(|(owner, _)| *owner != Some(hub));
    }

    /// Whom its requests waiting are for.
    pub(super) fn pending_owners(&self) -> impl Iterator<Item = Owner> + '_ {
        self.pending.values().map(|(_, owner)| *owner)
    }

    /// The next thing parked, to hand on.
    pub(super) fn unpark(&mut self) -> Option<Parked> {
        self.parked.pop_front()
    }

    /// Asserts what holds of it.
    pub(super) fn check_invariants(&self) {
        self.verifier.check_invariants();
        assert!(self.pending.len() <= MAX_PENDING, "requests are bounded");
        assert!(
            self.proof_asks.len() <= MAX_PROOF_ASKS,
            "proof asks are bounded"
        );
        assert!(
            !matches!(&self.keeping, Some(Kept::State(kept)) if kept.len() > MAX_HUB_VERIFIER_BYTES),
            "the verifier's state handed over is bounded"
        );
        assert!(self.pending_submits() <= MAX_PENDING_SUBMITS);
        assert!(self.parked.is_empty(), "what is parked is handed on");
        assert!(self.serving.is_none(), "a call is served within it");
        assert!(self.escrows >= 1 && self.escrows <= MAX_VALIDATED_HUBS);
        assert!(self.threads.len() <= api::MAX_PROOF_KEYS);
        if !self.shared {
            assert_eq!(self.escrows, 1, "one escrow");
            assert!(
                (self.out.iter()).all(|(owner, _)| owner.is_none())
                    && self.pending_owners().all(|owner| owner.is_none()),
                "one escrow is untagged"
            );
        }
        if self.phase == Phase::Disconnected {
            assert!(self.pending.is_empty() && self.proof_asks.is_empty());
        }
    }
}

/// The connection.
impl Link {
    /// A connection is up: ask where the server stands, and have the
    /// verifier anchor on it.
    pub(super) fn connected(&mut self) {
        self.forget_connection();
        self.again_for = None;
        self.validated = None;
        self.phase = Phase::Checking;
        self.ask_own(Asked::ServerState, api::server_state);
        self.verifier.connected(self.now_ms);
    }

    /// The connection is gone. What it decided for the host stays to be
    /// carried out, but for what was to be sent on the connection and a
    /// request to end it.
    pub(super) fn disconnected(&mut self) {
        self.forget_connection();
        self.phase = Phase::Disconnected;
        self.verifier.disconnected();
        // The verifier asks everything anew on the next connection: its
        // asks on this one are forgotten with it.
        let _ = self.verifier.take_out();
        // What was to go on this connection, or to end it, goes with it:
        // the next one is asked anew, and is not ended for this one's
        // sake.
        self.out
            .retain(|(_, out)| !matches!(out, Out::Send(_) | Out::Reconnect));
    }

    /// What its server said goes with a reset of the chain, proven at
    /// `anchor`: it is asked where it stands again, if connected; once for
    /// each reset, however many escrows met it.
    pub(super) fn check_again(&mut self, anchor: u32) {
        if self.phase == Phase::Checking && self.again_for == Some(anchor) {
            return;
        }
        self.again_for = Some(anchor);
        self.forget_connection();
        self.phase = Phase::Checking;
        self.ask_own(Asked::ServerState, api::server_state);
    }

    /// What was asked on the connection is forgotten. The verifier asks its
    /// open queries again on the next.
    fn forget_connection(&mut self) {
        self.pending.clear();
        self.proof_asks.clear();
    }

    /// Waits, and asks the server where it stands again later.
    pub(super) const fn wait(&mut self) {
        self.phase = Phase::Waiting {
            since_ms: self.now_ms,
        };
    }

    /// Does what is due: ask a server that was behind again, and have the
    /// verifier anchor while the anchor, proven at `anchored_ms`, ages.
    pub(super) fn tick(&mut self, anchored_ms: Option<u64>) {
        self.verifier.tick(self.now_ms);
        if let Phase::Waiting { since_ms } = self.phase
            && self.now_ms.saturating_sub(since_ms) >= RETRY_MS
        {
            self.phase = Phase::Checking;
            self.ask_own(Asked::ServerState, api::server_state);
        }
        let aging = anchored_ms.is_none_or(|at| self.now_ms.saturating_sub(at) >= ANCHOR_EVERY_MS);
        let rested = self
            .anchor_asked_ms
            .is_none_or(|at| self.now_ms.saturating_sub(at) >= ANCHOR_EVERY_MS);
        if self.phase != Phase::Disconnected && aging && rested {
            self.anchor_now();
        }
    }

    /// Has the verifier anchor now, on the link's own.
    pub(super) fn anchor_now(&mut self) {
        self.anchor_asked_ms = Some(self.now_ms);
        self.verifier.anchor_now(self.now_ms);
    }

    /// An answer to the request `id`: the verifier's own it takes itself;
    /// another of its own it hands back, with what it was and whose.
    pub(super) fn response(
        &mut self,
        id: u64,
        outcome: Result<Value, ApiError>,
    ) -> Option<(Asked, Owner, Result<Value, ApiError>)> {
        if let Some(ask) = self.proof_asks.remove(&id) {
            self.verifier.answered(&ask, outcome, self.now_ms);
            return None;
        }
        let (asked, owner) = self.pending.remove(&id)?;
        Some((asked, owner, outcome))
    }

    /// Where the server stands, by `result`: on the hub's network, and on
    /// the chain followed, as far as it says, against `newest`, the newest
    /// ledger proven and when it closed. Its validated ledger, once both
    /// hold; none if it is to be waited for. A new chain is only a hint for
    /// the verifier to anchor, which proves it or not.
    pub(super) fn server_state(
        &mut self,
        result: &Value,
        newest: Option<(u32, u64)>,
    ) -> Option<u32> {
        let state = api::server_state_result(result)?;
        if state.network_id != Some(self.network) {
            self.push_own(Out::Problem(if self.shared {
                "the XRPL server is on another network than the hubs' escrows it serves"
            } else {
                "the XRPL server is on another network than the hub's escrow"
            }));
            return None;
        }
        self.validated = Some(state.validated_ledger);
        match judge_chain(
            newest,
            self.network,
            state.validated_ledger,
            state.close_time,
        ) {
            Chain::Current => {}
            Chain::Behind | Chain::Restarted => {
                self.verifier.anchor_now(self.now_ms);
                return None;
            }
            Chain::WentBack => {
                self.push_own(Out::Problem(
                    "the XRPL server's ledger went back on a network that is never reset",
                ));
                return None;
            }
        }
        self.phase = Phase::Checked;
        Some(state.validated_ledger)
    }
}

/// Requests.
impl Link {
    /// Asks the server for the escrow whose call this is, unless its
    /// requests are full, or that escrow's share of them on a shared link:
    /// whether it did.
    pub(super) fn ask(&mut self, asked: Asked, request: impl FnOnce(u64) -> Vec<u8>) -> bool {
        let owner = self.serving;
        if self.shared && owner.is_some() && self.pending_of(owner) >= self.pending_share() {
            return false;
        }
        self.ask_as(owner, asked, request)
    }

    /// Asks the server for the link itself.
    fn ask_own(&mut self, asked: Asked, request: impl FnOnce(u64) -> Vec<u8>) -> bool {
        self.ask_as(None, asked, request)
    }

    fn ask_as(&mut self, owner: Owner, asked: Asked, request: impl FnOnce(u64) -> Vec<u8>) -> bool {
        if self.pending.len() >= MAX_PENDING {
            return false;
        }
        let id = self.fresh_id();
        self.pending.insert(id, (asked, owner));
        self.out.push_back((owner, Out::Send(request(id))));
        true
    }

    /// How many of its requests waiting are `owner`'s.
    fn pending_of(&self, owner: Owner) -> usize {
        self.pending_owners().filter(|of| *of == owner).count()
    }

    /// Whether its requests have room for one more that submits an escrow
    /// transaction, of the escrow whose call this is: within its share on
    /// a shared link.
    pub(super) fn room_to_submit(&self) -> bool {
        if self.shared {
            let owner = self.serving;
            let submits = (self.pending.values())
                .filter(|(asked, of)| *asked == Asked::Submit && *of == owner)
                .count();
            return submits < self.submit_share();
        }
        self.pending_submits() < MAX_PENDING_SUBMITS
    }

    /// How many of its requests submit an escrow transaction.
    fn pending_submits(&self) -> usize {
        (self.pending.values())
            .filter(|(asked, _)| *asked == Asked::Submit)
            .count()
    }

    fn fresh_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Carries out what the verifier decided that is the link's: an ask,
    /// on the connection or of a list publisher's site, or a new
    /// connection. A verdict or an anchor is parked: no escrow's drain took
    /// it.
    pub(super) fn carry_out(&mut self, out: verifier::Out) {
        match out {
            verifier::Out::Ask(Ask::Fetch(url)) => self.push_own(Out::Fetch(url)),
            verifier::Out::Ask(ask) => self.ask_proof(ask),
            verifier::Out::Verdict(id, verdict) => self.park(Parked::Verdict(id, verdict)),
            verifier::Out::Anchored(anchor) => self.park(Parked::Anchored(anchor, false, None)),
            verifier::Out::Reset(anchor) => self.park(Parked::Anchored(anchor, true, None)),
            verifier::Out::Reconnect => {
                self.push_own(Out::Problem(
                    "the XRPL server's answers did not prove, or it refused or left them: connecting anew",
                ));
                self.push_own(Out::Reconnect);
            }
        }
    }

    /// Carries out what the verifier decided, with no escrow to take its
    /// verdicts and anchors: they are parked, for none.
    pub(super) fn drain_alone(&mut self) {
        for out in self.verifier.take_out() {
            self.carry_out(out);
        }
        self.tell_failed();
    }

    /// Says once that the verifier stopped, once it did.
    pub(super) fn tell_failed(&mut self) {
        if self.verifier.failed() && !self.failed_told {
            self.failed_told = true;
            self.push_own(Out::Problem(if self.shared {
                "the XRP Ledger's validators validated two chains of a network that never resets: no escrow on it is followed any more"
            } else {
                "the XRP Ledger's validators validated two chains of a network that never resets: the escrow is followed no more"
            }));
        }
    }

    fn ask_proof(&mut self, ask: Ask) {
        if self.phase == Phase::Disconnected {
            return;
        }
        if self.proof_asks.len() >= MAX_PROOF_ASKS {
            self.push_own(Out::Reconnect);
            return;
        }
        let id = self.fresh_id();
        if let Some(request) = ask.request(id) {
            self.proof_asks.insert(id, ask);
            self.push_own(Out::Send(request));
        }
    }
}

/// Keeping the verifier's state across a restart.
impl Link {
    /// Hands the verifier's state to the host with the cursor once it
    /// changed, and [`KEEP_VERIFIER_EVERY_MS`] passed since it last did, or
    /// at once if `now`: with a cursor going out already, or with
    /// `persisted`, the one persisted last. Before a cursor is, there is
    /// nothing to keep it with, and a restart starts as the first start
    /// did.
    pub(super) fn keep(&mut self, now: bool, persisted: Option<EscrowCursor>) {
        let changes = self.verifier.changes();
        let due = now
            || (self.kept_ms)
                .is_none_or(|at| self.now_ms.saturating_sub(at) >= KEEP_VERIFIER_EVERY_MS);
        let Some(cursor) = persisted else {
            return;
        };
        if changes == self.kept_changes || !due {
            return;
        }
        self.kept_changes = changes;
        self.kept_ms = Some(self.now_ms);
        let mut kept = self.network.to_be_bytes().to_vec();
        kept.extend(publishers_digest(self.verifier.unl().config()));
        kept.extend(self.verifier.state());
        if kept.len() > MAX_HUB_VERIFIER_BYTES {
            // What the host kept before is older, and may be of a chain
            // gone: it is dropped once, said once, and a restart starts
            // afresh until the state fits again.
            if self.over_bound {
                return;
            }
            self.over_bound = true;
            self.keeping = Some(Kept::Dropped);
            self.push_own(Out::Problem(
                "the XRPL verifier's lists are too large to keep: a restart fetches them and anchors anew",
            ));
        } else {
            self.over_bound = false;
            self.keeping = Some(Kept::State(kept));
        }
        if !self.cursor_queued() {
            self.push_own(Out::Cursor(cursor));
        }
    }
}

/// What tells the publishers a verifier's state was kept under from
/// others: a digest of their master keys, which alone make the UNL, in the
/// order of their bytes, so that a changed site or order is none
/// ([`super::Watcher::restore`]).
fn publishers_digest(config: &UnlConfig) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};

    let mut keys: Vec<&[u8; 33]> = (config.publishers.iter())
        .map(|publisher| publisher.master_key.as_bytes())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    let mut hash = Sha256::new();
    hash.update(b"dassie hub verifier publishers");
    for key in keys {
        hash.update(key);
    }
    hash.finalize().into()
}

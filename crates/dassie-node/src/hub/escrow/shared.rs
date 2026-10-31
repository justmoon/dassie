//! One link to an XRPL server, and the one verifier on it, serving the
//! escrows of every hub the node validates on its network: a [`LinkWatcher`]
//! is one [`Link`] and up to [`MAX_VALIDATED_HUBS`] escrows on it, each
//! followed as a [`Watcher`](super::Watcher) follows its one, by the hub it
//! is the escrow of.
//!
//! What is the link's is done once for all of them: its server's network
//! and chain are checked once, against the newest ledger any escrow proved;
//! the verifier anchors once each ledger closes, or once the newest anchor
//! any escrow took ages, and each escrow takes each anchor, judged against
//! its own cursor; a reset asks the server where it stands once, however
//! many escrows meet it, and is attested by each; and the verifier's state
//! goes to the host with every escrow's cursor, in one write ([`LinkOut::Keep`]).
//! What is an escrow's is its own: the server's answers to what it asked,
//! the verifier's verdicts on its queries, the streamed transactions that
//! name it, and its subscription, which it asks for again on its own when
//! the server fails to answer it, or when it is registered on a link that
//! follows the others already, without a new connection.
//!
//! What is bounded is shared fairly: each escrow has an equal share of the
//! verifier's queries and of the link's requests ([`Link::query_share`]),
//! so a flood of one escrow's hints leaves the others theirs, and the
//! verifier's bound holds for all of them together. A new connection is
//! every escrow's, so one escrow's reasons for one go only every so often
//! (`ESCROW_RECONNECT_MS`); the verifier's go at once.

use std::collections::{BTreeMap, BTreeSet};

use dassie_hub::HubId;
use dassie_hub::operation::{Issuer, ValidatorOperation};
use dassie_xrpl::AccountId;
use dassie_xrpl::api::{self, ApiError, MAX_PROOF_KEYS, Message, TxRecord};
use dassie_xrpl::unl::{Anchor, ValidationSignatures};
use dassie_xrpl::validator_list::UnlConfig;
use dassie_xrpl::verifier::{self, Verifier};
use serde_json::Value;

use super::link::{Asked, Link, Owner, Parked};
use super::watch::EscrowWatcher;
use super::{EscrowView, Kept, Out, Unrestored};
use crate::hub::config::MAX_VALIDATED_HUBS;
use crate::store::EscrowCursor;

/// The most the drains of one call park for [`LinkWatcher::hand_on`]: a
/// verdict on each of the verifier's open queries, and its anchors, the
/// verdicts that come at once while they are handed on being the querying
/// escrow's own. What is left is handed on in the next call.
const MAX_HANDED_ON: usize = 2 * verifier::MAX_QUERIES;

// Each escrow threads its root and a token hub's line: all of them fit one
// state proof's keys.
const _: () = assert!(2 * MAX_VALIDATED_HUBS <= MAX_PROOF_KEYS);

/// What a link watcher asks its host to do: on the link, or for one hub's
/// escrow. Taken apart at once: boxing would only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkOut {
    /// Send this on the WebSocket.
    Send(Vec<u8>),
    /// Fetch this validator list publisher's site, as [`Out::Fetch`] says,
    /// and hand the body to [`LinkWatcher::fetched`].
    Fetch(String),
    /// Drop the connection and connect anew.
    Reconnect,
    /// Something the owner should read in the log, of the link.
    Problem(&'static str),
    /// Have the validator of the hub attest this, and tell
    /// [`LinkWatcher::taken`] once it took it, as [`Out::Attest`] says.
    Attest(HubId, ValidatorOperation, Option<[u8; 32]>),
    /// Persist how far the hub's escrow was followed.
    Cursor(HubId, EscrowCursor),
    /// Persist what to keep of the verifier, its state or nothing, and with
    /// it, in the same write, how far each of these hubs' escrows was
    /// followed: every escrow's on the link that persisted a cursor.
    Keep(Kept, Vec<(HubId, EscrowCursor)>),
    /// Something the owner should read in the log, of the hub's escrow.
    HubProblem(HubId, &'static str),
}

/// Follows the escrows of the hubs the node validates on one network, over
/// one link and one verifier.
#[derive(Debug)]
pub struct LinkWatcher {
    link: Link,
    escrows: BTreeMap<HubId, EscrowWatcher>,
}

/// Building and reading it.
impl LinkWatcher {
    /// A watcher of no escrow yet on `network`, trusting the validators the
    /// publishers of `unl` list, whose validations `signatures` checks.
    #[must_use]
    pub fn new(
        network: u32,
        unl: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
    ) -> Self {
        Self {
            link: Link::shared(network, unl, signatures),
            escrows: BTreeMap::new(),
        }
    }

    /// Takes back at `now_ms` what it handed its host of its verifier
    /// before a restart ([`LinkOut::Keep`]), before it first connects, as
    /// [`Watcher::restore`](super::Watcher::restore) does.
    ///
    /// # Errors
    ///
    /// Why it is not taken back: the verifier then starts afresh.
    pub fn restore(
        &mut self,
        kept: &[u8],
        signatures: Box<dyn ValidationSignatures + Send>,
        now_ms: u64,
    ) -> Result<(), Unrestored> {
        assert!(
            !self.link.is_connected() && self.escrows.values().all(|e| !e.querying()),
            "taken back before it follows any escrow"
        );
        self.link.restore(kept, signatures, now_ms)?;
        self.rethread();
        Ok(())
    }

    /// The network it follows escrows on, which its host keeps the
    /// verifier's state by.
    #[must_use]
    pub const fn network(&self) -> u32 {
        self.link.network()
    }

    /// The hubs whose escrows it follows.
    pub fn hubs(&self) -> impl Iterator<Item = HubId> + '_ {
        self.escrows.keys().copied()
    }

    /// Whether it follows `hub`'s escrow's stream now.
    #[must_use]
    pub fn following(&self, hub: HubId) -> bool {
        self.escrows.get(&hub).is_some_and(EscrowWatcher::following)
    }

    /// How far it followed `hub`'s escrow.
    #[must_use]
    pub fn cursor(&self, hub: HubId) -> Option<EscrowCursor> {
        self.escrows.get(&hub)?.cursor()
    }

    /// The newest anchor `hub`'s escrow took, if any.
    #[must_use]
    pub fn anchor(&self, hub: HubId) -> Option<Anchor> {
        self.escrows.get(&hub)?.anchor()
    }

    /// Whether that anchor is fresh enough to judge the newest ledger by.
    #[must_use]
    pub fn fresh(&self, hub: HubId) -> bool {
        (self.escrows.get(&hub)).is_some_and(|escrow| escrow.fresh(self.link.now_ms()))
    }

    /// The newest state of `hub`'s issuer's hold it proved, if any.
    #[must_use]
    pub fn proven_issuer(&self, hub: HubId) -> Option<Issuer> {
        self.escrows.get(&hub)?.proven_issuer()
    }

    /// Its verifier, to read what it proved and the lies it caught.
    #[must_use]
    pub const fn verifier(&self) -> &Verifier {
        self.link.verifier()
    }

    /// The next thing to do. A cursor handed over while the verifier's
    /// state is to be kept is the last of them in the queue, which carries
    /// the state with every escrow's cursor ([`LinkOut::Keep`]): the ones
    /// before it, which it carries too, go no sooner.
    pub fn poll(&mut self) -> Option<LinkOut> {
        // Each turn takes one out of the queue: what it skips, a later one
        // carries.
        for _ in 0..=self.link.queued() {
            let (owner, out) = self.link.poll_owned()?;
            if let Some(out) = self.out_of(owner, out) {
                return Some(out);
            }
        }
        None
    }

    /// What `out`, `owner`'s, asks of the host, if it asks anything now.
    fn out_of(&mut self, owner: Owner, out: Out) -> Option<LinkOut> {
        let out = match (owner, out) {
            (_, Out::Cursor(_)) if self.link.keeping() && self.link.cursor_queued() => return None,
            (_, Out::Cursor(cursor)) if self.link.keeping() => {
                let Some(kept) = self.link.take_kept() else {
                    return owner.map(|hub| LinkOut::Cursor(hub, cursor));
                };
                let cursors = (self.escrows.iter())
                    .filter_map(|(hub, escrow)| Some((*hub, escrow.persisted()?)))
                    .collect();
                LinkOut::Keep(kept, cursors)
            }
            // The link hands over a cursor only to carry the verifier's
            // state, which it carried already; it attests nothing itself.
            (None, Out::Cursor(_) | Out::Attest(..)) => return None,
            (Some(hub), Out::Cursor(cursor)) => LinkOut::Cursor(hub, cursor),
            (Some(hub), Out::Attest(operation, transaction)) => {
                LinkOut::Attest(hub, operation, transaction)
            }
            (Some(hub), Out::Problem(problem)) => LinkOut::HubProblem(hub, problem),
            (None, Out::Problem(problem)) => LinkOut::Problem(problem),
            (_, Out::Send(bytes)) => LinkOut::Send(bytes),
            (_, Out::Fetch(url)) => LinkOut::Fetch(url),
            (_, Out::Reconnect) => LinkOut::Reconnect,
        };
        Some(out)
    }

    /// Asserts what holds of it: of its link, and of each escrow on it; at
    /// most [`MAX_VALIDATED_HUBS`] of them, whose shares fit the link's
    /// bounds together, and whose objects the verifier threads.
    pub fn check_invariants(&self) {
        assert!(
            self.escrows.len() <= MAX_VALIDATED_HUBS,
            "escrows are bounded"
        );
        assert!(self.link.is_shared());
        self.link.check_invariants();
        assert_eq!(self.link.escrows(), self.escrows.len().max(1));
        for escrow in self.escrows.values() {
            escrow.check_invariants(&self.link);
        }
        let open: usize = self.escrows.values().map(EscrowWatcher::open_queries).sum();
        assert!(open <= verifier::MAX_QUERIES, "queries fit the verifier's");
        assert!(
            (self.link.pending_owners())
                .all(|owner| owner.is_none_or(|hub| self.escrows.contains_key(&hub))),
            "requests are of escrows it serves"
        );
        assert_eq!(self.link.threads(), self.union().as_slice());
    }
}

/// Escrows.
impl LinkWatcher {
    /// Follows `hub`'s escrow on the link from now on, at `now_ms`, from
    /// where it left off, as `view` describes it, on the link's network.
    /// On a link whose server was checked, it subscribes and reads its
    /// history at once: the others follow on, on the same connection.
    pub fn register(
        &mut self,
        now_ms: u64,
        hub: HubId,
        cursor: Option<EscrowCursor>,
        view: &EscrowView,
    ) {
        assert!(!self.escrows.contains_key(&hub), "registered once");
        assert!(
            self.escrows.len() < MAX_VALIDATED_HUBS,
            "escrows are bounded"
        );
        assert_eq!(
            view.rules.terms.network,
            self.link.network(),
            "on its network"
        );
        self.link.now(now_ms);
        let escrow = EscrowWatcher::new(AccountId(view.rules.terms.escrow), cursor);
        self.escrows.insert(hub, escrow);
        self.link.set_escrows(self.escrows.len());
        self.with(hub, |escrow, link| {
            escrow.see(link, view);
            if let (true, Some(validated)) = (link.checked(), link.validated()) {
                escrow.catch_up(link, validated);
            }
        });
        self.rethread();
        self.drain();
        self.with(hub, EscrowWatcher::work);
        self.hand_on();
    }

    /// Follows `hub`'s escrow no more: what it asked is forgotten, and what
    /// it had to hand over goes no more.
    pub fn unregister(&mut self, hub: HubId) {
        let Some(mut escrow) = self.escrows.remove(&hub) else {
            return;
        };
        escrow.release(&mut self.link);
        self.link.forget_hub(hub);
        self.link.set_escrows(self.escrows.len());
        self.rethread();
    }

    /// Calls `call` on `hub`'s escrow, as that escrow's call to the link.
    fn with<T>(
        &mut self,
        hub: HubId,
        call: impl FnOnce(&mut EscrowWatcher, &mut Link) -> T,
    ) -> Option<T> {
        let escrow = self.escrows.get_mut(&hub)?;
        self.link.serve(Some(hub));
        let result = call(escrow, &mut self.link);
        self.link.serve(None);
        Some(result)
    }

    /// Calls `call` on each escrow, in turn, as its own call to the link.
    fn each(&mut self, mut call: impl FnMut(&mut EscrowWatcher, &mut Link)) {
        for (hub, escrow) in &mut self.escrows {
            self.link.serve(Some(*hub));
            call(escrow, &mut self.link);
        }
        self.link.serve(None);
    }

    /// The keys of every escrow's walked objects, in order, once each.
    fn union(&self) -> Vec<[u8; 32]> {
        let keys: BTreeSet<[u8; 32]> = (self.escrows.values())
            .flat_map(|escrow| escrow.walk_keys().iter().copied())
            .collect();
        keys.into_iter().collect()
    }

    /// The verifier threads every escrow's objects, rather than one
    /// escrow's in place of the others'.
    fn rethread(&mut self) {
        let keys = self.union();
        self.link.thread(keys);
    }

    /// What each escrow's validator's ledger says now, of those `views`
    /// names.
    fn see(&mut self, views: &BTreeMap<HubId, EscrowView>) {
        let mut rethread = false;
        self.each(|escrow, link| {
            if let Some(view) = link_view(link, views) {
                rethread |= escrow.see(link, view);
            }
        });
        if rethread {
            self.rethread();
        }
    }
}

/// The view of the escrow whose call this is, among `views`.
fn link_view<'a>(link: &Link, views: &'a BTreeMap<HubId, EscrowView>) -> Option<&'a EscrowView> {
    views.get(&link.serving()?)
}

/// Inputs.
impl LinkWatcher {
    /// `hub`'s validator took the attestation of `transaction`.
    pub fn taken(&mut self, hub: HubId, transaction: Option<[u8; 32]>) {
        self.with(hub, |escrow, link| escrow.taken(link, transaction));
    }

    /// The host dropped `operation` of `hub`'s before its validator took
    /// it, as [`Watcher::dropped`](super::Watcher::dropped) says.
    pub fn dropped(
        &mut self,
        hub: HubId,
        operation: &ValidatorOperation,
        transaction: Option<[u8; 32]>,
    ) {
        self.with(hub, |escrow, link| {
            escrow.dropped(link, operation, transaction)
        });
    }

    /// A connection is up at `now_ms`: ask where the server stands, and
    /// have the verifier anchor on it.
    pub fn connected(&mut self, now_ms: u64) {
        self.link.now(now_ms);
        self.each(EscrowWatcher::forget_connection);
        self.link.connected();
        self.drain();
        self.hand_on();
    }

    /// The connection is gone: as [`Watcher::disconnected`](super::Watcher::disconnected).
    pub fn disconnected(&mut self) {
        self.each(EscrowWatcher::forget_connection);
        self.link.disconnected();
    }

    /// Submits a signed transaction of `hub`'s escrow, as
    /// [`Watcher::submit`](super::Watcher::submit) does.
    pub fn submit(&mut self, hub: HubId, blob: Vec<u8>) {
        self.with(hub, |escrow, link| escrow.submit(link, blob));
    }

    /// Does what is due at `now_ms`, for the link and each escrow, with
    /// what `views` say of the escrows they name: as
    /// [`Watcher::tick`](super::Watcher::tick), and an escrow whose
    /// server failed it subscribes again once due. The verifier anchors
    /// once the newest anchor any escrow took ages.
    pub fn tick(&mut self, now_ms: u64, views: &BTreeMap<HubId, EscrowView>) {
        self.link.now(now_ms);
        self.see(views);
        let anchored = (self.escrows.values())
            .filter_map(EscrowWatcher::anchored_ms)
            .max();
        self.link.tick(anchored);
        self.each(EscrowWatcher::tick);
        self.resubscribe();
        self.drain();
        self.each(EscrowWatcher::work);
        let persisted = self.escrows.values().find_map(EscrowWatcher::persisted);
        self.link.keep(false, persisted);
        self.hand_on();
    }

    /// Takes a message from the server: an answer goes to whoever asked, a
    /// streamed transaction to each escrow it names, or to every escrow if
    /// it names none, and a closed ledger to each, anchoring once.
    pub fn message(&mut self, now_ms: u64, bytes: &[u8], views: &BTreeMap<HubId, EscrowView>) {
        self.link.now(now_ms);
        self.see(views);
        match api::parse(bytes) {
            Message::Response { id, outcome } => {
                if let Some((asked, owner, outcome)) = self.link.response(id, outcome) {
                    self.answer(asked, owner, outcome);
                }
            }
            Message::Transaction(record) => self.hinted(&record),
            Message::LedgerClosed(closed) => self.closed(closed.ledger_index),
            Message::Ignored => {}
        }
        self.drain();
        self.each(EscrowWatcher::work);
        self.hand_on();
    }

    /// A founding asks whether a quorum checked `hub`'s escrow, as
    /// [`Watcher::check_escrow`](super::Watcher::check_escrow) says.
    pub fn check_escrow(&mut self, hub: HubId, now_ms: u64, view: &EscrowView) {
        self.link.now(now_ms);
        let rethread = self.with(hub, |escrow, link| {
            let rethread = escrow.see(link, view);
            escrow.want_check(view);
            escrow.work(link);
            rethread
        });
        if rethread == Some(true) {
            self.rethread();
        }
        self.hand_on();
    }

    /// A fetch of a list publisher's `url` answered with `body`, or failed.
    pub fn fetched(&mut self, now_ms: u64, url: &str, body: Option<&[u8]>) {
        self.link.now(now_ms);
        let now_ms = self.link.now_ms();
        self.link.verifier_mut().fetched(url, body, now_ms);
        self.drain();
        self.each(EscrowWatcher::work);
        self.hand_on();
    }
}

/// Answers, the stream, and what the drains parked.
impl LinkWatcher {
    fn answer(&mut self, asked: Asked, owner: Owner, outcome: Result<Value, ApiError>) {
        match (asked, owner, outcome) {
            (Asked::ServerState, _, Ok(result)) => self.server_state(&result),
            (Asked::ServerState, _, Err(_)) => self.wait(),
            // Only where the server stands is asked by the link itself.
            (_, None, _) => {}
            (Asked::Subscribe, Some(hub), Ok(_)) => {
                self.with(hub, EscrowWatcher::read_history);
            }
            (Asked::History, Some(hub), Ok(result)) => {
                self.with(hub, |escrow, link| escrow.history(link, &result));
            }
            (Asked::Submit, Some(hub), Ok(result)) => {
                if api::submit_result(&result).is_none() {
                    self.with(hub, |_, link| {
                        link.push(Out::Problem(
                            "the XRPL server did not take an escrow transaction",
                        ));
                    });
                }
            }
            (Asked::Subscribe | Asked::History, Some(hub), Err(_)) => {
                self.with(hub, EscrowWatcher::fail);
            }
            (Asked::Submit, Some(_), Err(_)) => {}
        }
    }

    /// Where the server stands: on the network, and on the chain the
    /// escrows follow, as far as it says, against the newest ledger any of
    /// them proved; a server behind any of them is waited for. Then each
    /// escrow subscribes.
    fn server_state(&mut self, result: &Value) {
        let newest = self
            .escrows
            .values()
            .filter_map(EscrowWatcher::newest)
            .max();
        match self.link.server_state(result, newest) {
            Some(validated) => self.each(|escrow, link| {
                if !escrow.subscribed() {
                    escrow.catch_up(link, validated);
                }
            }),
            None => self.wait(),
        }
    }

    /// The server is waited for: no escrow follows it meanwhile.
    fn wait(&mut self) {
        self.link.wait();
        self.each(|escrow, _| escrow.unsubscribe());
    }

    /// Each escrow whose server failed it, on a link still checked,
    /// subscribes again once due.
    fn resubscribe(&mut self) {
        let (now, validated) = (self.link.now_ms(), self.link.validated());
        let (true, Some(validated)) = (self.link.checked(), validated) else {
            return;
        };
        self.each(|escrow, link| {
            if escrow.resubscribe_due(now) {
                escrow.catch_up(link, validated);
            }
        });
    }

    /// The server streamed `record`: a hint to each escrow it names that
    /// is subscribed, or to every one that is if it names none, as a
    /// rippling payment through an escrow's line names neither.
    fn hinted(&mut self, record: &TxRecord) {
        let named = (self.escrows.values()).any(|escrow| escrow.concerns(record));
        for escrow in self.escrows.values_mut() {
            if escrow.subscribed() && (!named || escrow.concerns(record)) {
                escrow.hinted(record);
            }
        }
    }

    /// The stream said ledger `index` closed: each escrow subscribed hears
    /// of it, and the verifier anchors on it once.
    fn closed(&mut self, index: u32) {
        let mut heard = false;
        for escrow in self
            .escrows
            .values_mut()
            .filter(|escrow| escrow.subscribed())
        {
            escrow.closed(index);
            heard = true;
        }
        if heard {
            self.link.closed(index);
            self.link.anchor_now();
        }
    }

    /// Carries out what the verifier decided: through an escrow's drain,
    /// which takes its own verdicts and anchors at once and parks the rest
    /// for [`Self::hand_on`], or the link's alone while it serves none.
    fn drain(&mut self) {
        match self.escrows.keys().next().copied() {
            Some(hub) => {
                self.with(hub, EscrowWatcher::drain);
            }
            None => self.link.drain_alone(),
        }
    }

    /// Hands on what the drains parked: a verdict to the escrow whose query
    /// it is, if any still is, and an anchor to each escrow but the one that
    /// took it at once. Then no escrow stays subscribed on a link whose
    /// server is not checked: one escrow's reset, or a server that failed
    /// to answer where it stands, has every escrow read its history again
    /// once it is.
    fn hand_on(&mut self) {
        for _ in 0..MAX_HANDED_ON {
            let Some(parked) = self.link.unpark() else {
                break;
            };
            match parked {
                Parked::Verdict(id, verdict) => {
                    let owner =
                        (self.escrows.iter()).find_map(|(hub, e)| e.owns(id).then_some(*hub));
                    if let Some(hub) = owner {
                        self.with(hub, |escrow, link| escrow.verdict(link, id, verdict));
                    }
                }
                Parked::Anchored(anchor, reset, took) => {
                    let others: Vec<HubId> = self.hubs().filter(|hub| Some(*hub) != took).collect();
                    for hub in others {
                        self.with(hub, |escrow, link| escrow.anchored(link, anchor, reset));
                    }
                }
            }
        }
        if !self.link.checked() {
            self.each(|escrow, _| escrow.unsubscribe());
        }
    }
}

#[cfg(test)]
mod tests;

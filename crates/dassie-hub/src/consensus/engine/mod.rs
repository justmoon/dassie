//! The Simplex engine: one validator's state machine. It takes messages and
//! the passing of time, and gives back messages to send, events, and what to
//! persist before sending them.

mod chain;
mod fetch;
mod receive;
mod round;
mod timers;
mod vote;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey};
use dassie_oer::Encode;

use super::config::Config;
use super::epochs::{Epochs, ReconfigureError};
use super::hooks::Application;
use super::io::{Event, Evidence, Output, PeerId, Recipient, Stats};
use super::persist::{Batch, Stored};
use super::types::{Block, Finalized, Vote};
use super::wire::Message;
use crate::digest::Digest;
use crate::validators::{MAX_VALIDATORS, ValidatorIndex, ValidatorSet};

use round::Round;

/// The most views held above the last finalized one. Beyond it, the lowest
/// is dropped and never voted in again: a hub that keeps notarizing for this
/// many views without finalizing anything is far outside the fault model.
pub const MAX_ROUNDS: usize = 256;

/// How far beyond our view votes and proposals are taken. Certificates are
/// taken from any view: they prove themselves.
pub const MAX_VIEWS_AHEAD: u64 = 16;

/// The most blocks held that are not final.
pub const MAX_BLOCKS: usize = 2 * MAX_ROUNDS;

/// The most finalized blocks kept, to serve catch-up and to record the
/// votes for them (spec/designs/settlement-hubs.md#7-fees, "Who gets fees":
/// K is at most this).
pub const RETAINED_BLOCKS: usize = 64;

/// The most votes for finalized blocks kept.
const MAX_VOTE_LOG: usize = RETAINED_BLOCKS * MAX_VALIDATORS * 2;

/// Most events kept for the host to poll.
const MAX_EVENTS: usize = 1024;

/// The most steps one call takes toward a fixed point: each enters a view,
/// signs a vote or finalizes a chain, and there are at most a few per view
/// held.
const MAX_PROGRESS_STEPS: usize = 4 * MAX_ROUNDS;

/// The timers of the view we are in.
#[derive(Clone, Copy, Debug, Default)]
struct Timers {
    /// When we entered the view.
    entered_ms: u64,
    /// When the leader's proposal is due.
    leader_ms: Option<u64>,
    /// When the view's notarization is due.
    advance_ms: Option<u64>,
    /// When to rebroadcast our nullify, and the interval after that.
    retry: Option<(u64, u64)>,
    /// When we lead: the earliest we may propose.
    propose_ms: Option<u64>,
    /// A step due once, after a start.
    kick_ms: Option<u64>,
}

/// One validator's consensus engine, generic over the application's
/// payloads ([`Application`]).
pub struct Engine {
    config: Config,
    hub: Digest,
    key: Arc<NodeKey>,
    public: PublicKey,
    epochs: Epochs,
    now_ms: u64,
    /// The view we are in.
    view: u64,
    /// No vote of ours below this view.
    floor: u64,
    timers: Timers,
    rounds: BTreeMap<u64, Round>,
    /// Blocks above the tip, by digest.
    blocks: BTreeMap<Digest, Block>,
    tip: Finalized,
    tip_digest: Digest,
    /// The last finalized blocks, the tip last.
    history: VecDeque<(Digest, Finalized)>,
    /// Votes for finalized blocks, oldest first.
    vote_log: VecDeque<Vote>,
    /// For each height above the tip, the block we voted to finalize.
    finalize_heights: BTreeMap<u64, Digest>,
    /// Finalized views a conflicting certificate was reported for, each
    /// once; within the history's views.
    settled_conflicts: BTreeSet<u64>,
    stalled: bool,
    /// The last view each validator of the running set voted in.
    last_voted: [u64; MAX_VALIDATORS],
    /// What we lack, and whom to ask: set when a proposal or
    /// finalization cannot be checked for want of blocks or certificates.
    want: Option<(u64, Recipient)>,
    /// When we last asked.
    fetched_ms: Option<u64>,
    /// Which validator to ask next when no one in particular holds it.
    next_ask: usize,
    batch: Batch,
    outputs: VecDeque<Output>,
    events: VecDeque<Event>,
    stats: Stats,
}

impl Engine {
    /// Starts from what was persisted: a new hub's [`Stored::genesis`], or
    /// the state the batches built.
    ///
    /// # Panics
    ///
    /// If the configuration or the stored state does not fit together:
    /// configuration and state the engine wrote, not input.
    #[must_use]
    pub fn new(
        config: Config,
        hub: Digest,
        key: Arc<NodeKey>,
        stored: Stored,
        now_ms: u64,
    ) -> Self {
        config.validate();
        let public = key.public_key();
        let tip_digest = stored.tip.block.digest(&hub);
        let mut engine = Self {
            config,
            hub,
            key,
            public,
            epochs: Epochs::new(stored.epochs),
            now_ms,
            view: stored.view.max(stored.tip.block.view.saturating_add(1)),
            floor: stored.floor,
            timers: Timers::default(),
            rounds: BTreeMap::new(),
            blocks: BTreeMap::new(),
            history: VecDeque::from([(tip_digest, stored.tip.clone())]),
            tip: stored.tip,
            tip_digest,
            vote_log: VecDeque::new(),
            finalize_heights: stored.finalize_heights.into_iter().collect(),
            settled_conflicts: BTreeSet::new(),
            stalled: stored.stalled,
            last_voted: [0; MAX_VALIDATORS],
            want: None,
            fetched_ms: None,
            next_ask: 0,
            batch: Batch::default(),
            outputs: VecDeque::new(),
            events: VecDeque::new(),
            stats: Stats::default(),
        };
        for block in stored.blocks {
            engine.hold_block(block);
        }
        for certificate in stored.certificates {
            if let Some(round) = engine.round_mut(certificate.view) {
                round.hold(certificate);
            }
        }
        engine.restore_votes(&stored.votes);
        let view = engine.view;
        engine.enter(view);
        // A first step at once, to propose or rebroadcast what is due.
        engine.timers.kick_ms = Some(now_ms);
        engine
    }

    /// Takes in the votes we signed before a restart, and sends those that
    /// may still count again.
    fn restore_votes(&mut self, votes: &[Vote]) {
        for vote in votes {
            if !self.holds_view(vote.view) {
                continue;
            }
            let Some(me) = self.member(vote.view) else {
                continue;
            };
            let vote = Vote {
                signer: me,
                ..*vote
            };
            if let Some(round) = self.round_mut(vote.view) {
                round.sign(vote);
                round.take(vote);
            }
            if vote.view >= self.view || vote.kind == super::types::VoteKind::Finalize {
                self.send(Recipient::All, &Message::Vote(vote));
            }
        }
    }

    /// The hub's ID.
    #[must_use]
    pub const fn hub(&self) -> &Digest {
        &self.hub
    }

    /// The view we are in.
    #[must_use]
    pub const fn view(&self) -> u64 {
        self.view
    }

    /// The last finalized block.
    #[must_use]
    pub const fn tip(&self) -> &Finalized {
        &self.tip
    }

    /// The last finalized block's digest.
    #[must_use]
    pub const fn tip_digest(&self) -> &Digest {
        &self.tip_digest
    }

    /// What has been counted.
    #[must_use]
    pub const fn stats(&self) -> &Stats {
        &self.stats
    }

    /// The finalized blocks kept, oldest first, the tip last.
    pub fn history(&self) -> impl Iterator<Item = &(Digest, Finalized)> {
        self.history.iter()
    }

    /// The validator set that runs `view`.
    #[must_use]
    pub fn set_for_view(&self, view: u64) -> &ValidatorSet {
        self.epochs.for_view(view)
    }

    /// The validator set that certifies a block at `height`.
    #[must_use]
    pub fn set_for_height(&self, height: u64) -> &ValidatorSet {
        self.epochs.for_height(height)
    }

    /// The leader of `view`: its number modulo the size of the set that
    /// runs it, a pure function of the view and the hub's sets.
    #[must_use]
    pub fn leader(&self, view: u64) -> Option<ValidatorIndex> {
        let set = self.epochs.for_view(view);
        let n = u64::try_from(set.len()).unwrap_or(u64::MAX);
        let at = usize::try_from(view.checked_rem(n)?).ok()?;
        ValidatorIndex::new(at)
    }

    /// Our index in the set that runs `view`, if we are in it.
    #[must_use]
    pub fn member(&self, view: u64) -> Option<ValidatorIndex> {
        self.epochs.for_view(view).index_of(&self.public)
    }

    /// Our index if we may vote in `view`: in its set, and not stalled.
    fn voter(&self, view: u64) -> Option<ValidatorIndex> {
        if self.stalled || view < self.floor || view <= self.tip.block.view {
            return None;
        }
        self.member(view)
    }

    /// Every vote for a block we hold, for a block of ours to record: those
    /// for finalized blocks kept, then those for views above the tip, by
    /// view, signer and kind.
    #[must_use]
    pub fn votes(&self) -> Vec<Vote> {
        let mut votes: Vec<Vote> = self.vote_log.iter().copied().collect();
        votes.extend(self.rounds.values().flat_map(Round::block_votes).copied());
        votes
    }

    /// How long since the last block was stamped, by our clock: the signal
    /// the ledger's stall rule reads (spec/designs/settlement-hubs.md#9-shutdown).
    #[must_use]
    pub const fn since_finalized_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.tip.block.timestamp_ms)
    }

    /// Whether we declared the hub stalled.
    #[must_use]
    pub const fn is_stalled(&self) -> bool {
        self.stalled
    }

    /// Declares the hub stalled: we never vote or propose in it again,
    /// even after a restart, but still learn of later final blocks
    /// (spec/designs/settlement-hubs.md#9-shutdown, "Stall").
    pub fn declare_stalled(&mut self, now_ms: u64) {
        self.advance(now_ms);
        if self.stalled {
            return;
        }
        self.stalled = true;
        self.batch.stalled = true;
        self.timers = Timers::default();
        self.event(Event::Stalled);
    }

    /// A new validator set takes over at block `from_height`
    /// (spec/designs/settlement-hubs.md#8-changing-the-validators): the
    /// running set certifies nothing from there, and the new set runs
    /// every view after the one that finalizes the block before it.
    ///
    /// # Errors
    ///
    /// Another change is waiting, or the height is too close to the tip.
    pub fn reconfigure(
        &mut self,
        now_ms: u64,
        from_height: u64,
        set: ValidatorSet,
    ) -> Result<(), ReconfigureError> {
        self.advance(now_ms);
        self.epochs
            .reconfigure(from_height, set, self.tip.block.height)?;
        self.batch.epochs = Some(self.epochs.to_vec());
        Ok(())
    }

    /// The next message for the host to send, once the batch is persisted.
    pub fn poll_output(&mut self) -> Option<Output> {
        self.outputs.pop_front()
    }

    /// The next event.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// What to persist before sending the outputs polled since the last
    /// batch.
    pub fn take_batch(&mut self) -> Batch {
        core::mem::take(&mut self.batch)
    }

    /// Moves time on; it never goes back.
    fn advance(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
    }

    fn event(&mut self, event: Event) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    fn evidence(&mut self, evidence: Evidence) {
        self.event(Event::Evidence(Box::new(evidence)));
    }

    fn send(&mut self, to: Recipient, message: &Message) {
        self.outputs.push_back(Output {
            to,
            message: message.to_vec(),
        });
    }

    /// Whether `view` is one we hold a round for, or may.
    fn holds_view(&self, view: u64) -> bool {
        view > self.tip.block.view && view >= self.floor
    }

    /// The round of `view`, made if there is room. Making room drops the
    /// lowest round, and raises the floor past it.
    fn round_mut(&mut self, view: u64) -> Option<&mut Round> {
        if !self.holds_view(view) {
            return None;
        }
        if !self.rounds.contains_key(&view) {
            let leader = self.leader(view)?;
            if self.rounds.len() >= MAX_ROUNDS {
                let lowest = self.rounds.first_key_value().map(|(lowest, _)| *lowest)?;
                if lowest > view {
                    return None;
                }
                self.rounds.remove(&lowest);
                self.raise_floor(lowest.saturating_add(1));
            }
            self.rounds.insert(view, Round::new(leader));
        }
        self.rounds.get_mut(&view)
    }

    fn raise_floor(&mut self, floor: u64) {
        if floor > self.floor {
            self.floor = floor;
            self.batch.floor = Some(floor);
            self.blocks.retain(|_, block| block.view >= floor);
        }
    }

    /// Notes that `signer` voted in `view`, for skipping leaders that are
    /// gone.
    fn note_voter(&mut self, signer: ValidatorIndex, view: u64) {
        if let Some(last) = self.last_voted.get_mut(signer.get()) {
            *last = (*last).max(view);
        }
    }

    /// Takes a message from `from`.
    pub fn handle_message(
        &mut self,
        now_ms: u64,
        app: &mut dyn Application,
        from: PeerId,
        bytes: &[u8],
    ) {
        self.advance(now_ms);
        match Message::decode_message(bytes) {
            Ok(message) => self.receive(app, from, message),
            Err(_) => self.stats.refused = self.stats.refused.saturating_add(1),
        }
        self.progress(app);
    }

    /// Does what is due.
    pub fn handle_timeout(&mut self, now_ms: u64, app: &mut dyn Application) {
        self.advance(now_ms);
        self.timers.kick_ms = None;
        self.retry();
        self.progress(app);
    }

    /// Steps toward a fixed point: enter the views certificates allow,
    /// finalize what is final, propose, and vote. Each step changes
    /// something, and there are at most a few per view held.
    fn progress(&mut self, app: &mut dyn Application) {
        let mut settled = false;
        for _ in 0..MAX_PROGRESS_STEPS {
            let changed = self.enter_certified()
                || self.finalize_chain(app)
                || self.expire()
                || self.vote_finalize()
                || self.propose(app)
                || self.vote_notarize(app);
            if !changed {
                settled = true;
                break;
            }
        }
        // Each step consumes a view's work; running out means a step that
        // changes nothing claims it did.
        assert!(settled, "consensus settles within its bound");
        self.ask();
    }

    /// Checks every invariant: tests and simulations call it after each
    /// step.
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        assert!(self.rounds.len() <= MAX_ROUNDS, "rounds within bound");
        assert!(self.blocks.len() <= MAX_BLOCKS, "blocks within bound");
        assert!(
            self.history.len() <= RETAINED_BLOCKS,
            "history within bound"
        );
        assert!(self.vote_log.len() <= MAX_VOTE_LOG, "vote log within bound");
        assert!(self.events.len() <= MAX_EVENTS, "events within bound");
        assert!(self.view > self.tip.block.view, "the view is above the tip");
        assert!(
            self.history
                .back()
                .is_some_and(|(digest, _)| *digest == self.tip_digest),
            "the tip is the last of the history"
        );
        assert!(
            self.history
                .iter()
                .zip(self.history.iter().skip(1))
                .all(|((digest, _), (_, next))| next.block.parent == *digest
                    && next.block.height == self.height_after(digest)),
            "the history is a chain"
        );
        for (view, round) in &self.rounds {
            assert!(self.holds_view(*view), "rounds above the tip and floor");
            assert!(
                !(round.voted_nullify() && round.voted_finalize().is_some()),
                "never nullify and finalize one view"
            );
            assert!(
                round
                    .voted_finalize()
                    .is_none_or(|block| round.voted_notarize().is_none_or(|n| n == block)),
                "finalize only the block notarized"
            );
        }
        for block in self.blocks.values() {
            assert!(
                block.height > self.tip.block.height,
                "blocks held are above the tip"
            );
        }
        assert!(
            self.finalize_heights
                .keys()
                .all(|height| *height > self.tip.block.height),
            "finalize heights above the tip"
        );
    }

    /// The height after the block `digest` of the history.
    fn height_after(&self, digest: &Digest) -> u64 {
        self.history
            .iter()
            .find(|(held, _)| held == digest)
            .map_or(0, |(_, finalized)| finalized.block.height.saturating_add(1))
    }
}

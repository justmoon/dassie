//! A validator of a hub, as sans-IO code: consensus and the ledger
//! together, what it tells members, what it attests from the XRP Ledger,
//! and its signatures of the escrow's transactions
//! (spec/designs/settlement-hubs.md).
//!
//! The host feeds it what arrives on the hub's stream ([`Validator::handle`]),
//! the time ([`Validator::tick`]) and what it sees of the escrow
//! ([`Validator::attest`]); it carries out what comes out
//! ([`Validator::poll`]), after it persisted the step's consensus batch.

mod chain;
mod mempool;
pub mod payload;
pub mod wire;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey};
use dassie_oer::Encode;
use dassie_xrpl::codec::{Signer, Transact};

pub use chain::{Chain, FinalBlock};
pub use mempool::{MAX_GAPPED, Mempool, SEQUENCE_WINDOW, VALIDATOR_WINDOW};

use crate::client::{Request, SignedStatement, Statement};
use crate::consensus::{
    Batch, Block, Config, Engine, Event, Evidence, Finalized, Output, PeerId, Recipient, Stored,
};
use crate::escrow::{
    PayoutBundle, PayoutSet, ValidatorKeys, payout_set, sign_payout_set, withdrawal_payment,
};
use crate::ledger::{Ledger, Phase};
use crate::operation::{
    AccountNumber, Deposit, Operation, SignedValidatorOperation, ValidatorOperation,
};
use crate::rules::{HubId, Millis, Rules, RulesError};
use crate::validators::ValidatorIndex;
use payload::Payload;
use wire::Message;

/// The most final blocks with operations a validator keeps to answer
/// `Since`; blocks without any take no room.
pub const KEPT_BLOCKS: usize = 4096;

/// The most bytes of operations the blocks kept to answer `Since` hold.
pub const KEPT_BYTES: usize = 64 * 1024 * 1024;

/// The most statements one answer to `Since` carries, over several
/// messages, but for its first block, which is always whole: a block can
/// touch an account more often than this.
pub const MAX_ANSWER: usize = 4 * crate::client::MAX_STATEMENTS;

/// The most bytes of statements one answer to `Since` carries, but for its
/// first block, which is always whole.
pub const MAX_ANSWER_BYTES: usize = 1 << 20;

/// What a statement that an operation was executed adds to the operation,
/// at most: its tag, height, timestamp and place, and the outcome.
pub const EXECUTED_OVERHEAD: usize = 1 + 8 + 8 + 4 + (1 + 8 + 1 + 8);

/// The most bytes of statements one block makes for one account: its
/// operations fit a payload, and each adds [`EXECUTED_OVERHEAD`].
pub const MAX_BLOCK_STATEMENT_BYTES: usize =
    crate::consensus::MAX_PAYLOAD_BYTES + payload::MAX_OPERATIONS * EXECUTED_OVERHEAD;

/// The encoded size of [`Statement::Final`]: its tag, height and
/// timestamp.
pub const FINAL_STATEMENT_BYTES: usize = 1 + 8 + 8;

/// The encoded size of [`Statement::Issuer`] at its longest: its tag,
/// height, timestamp, the freeze's stamp if any, and whether the issuer
/// holds the escrow.
pub const ISSUER_STATEMENT_BYTES: usize = 1 + 8 + 8 + (1 + 8) + 1;

/// How many statements end every answer to `Since` that is not
/// `Trimmed`: a token hub's issuer's hold and the final block it covers.
pub const SINCE_TAIL: usize = 2;

/// Room for what ends an answer to `Since`: the issuer's hold and the
/// final block.
const SINCE_TAIL_BYTES: usize = 64;

const _: () = assert!(SINCE_TAIL_BYTES >= FINAL_STATEMENT_BYTES + ISSUER_STATEMENT_BYTES);

/// The most bytes of statements an honest answer to `Since` carries, over
/// however many messages it spans: up to [`MAX_ANSWER_BYTES`] of them, or
/// its first block whole if that is more, the issuer's hold of a token
/// hub, and the final block it covers. What a member's tally must hold of
/// one validator's answer.
pub const MAX_SINCE_ANSWER_BYTES: usize =
    MAX_ANSWER_BYTES + MAX_BLOCK_STATEMENT_BYTES + SINCE_TAIL_BYTES;

const _: () = assert!(MAX_SINCE_ANSWER_BYTES >= MAX_ANSWER_BYTES + MAX_BLOCK_STATEMENT_BYTES);

/// The most of our own operations we follow until a block takes them: as
/// many as our share of the pool holds.
pub const MAX_OWN: usize = VALIDATOR_WINDOW;

/// How many times one of our operations that a later one of ours passed
/// before any block took it is attested again.
pub const MAX_AGAIN: u8 = 4;

/// The most of our attestations of deposits that failed for want of room
/// we keep to attest again once there is room and the issuer no longer
/// holds the escrow: a freeze credits no deposit, so our share of the
/// pending table fills and every later deposit fails `Full` until the
/// thaw. Past it, the newest is not kept: every honest validator drops
/// the same ones, and since the watcher has passed them, they are
/// credited to no one.
pub const MAX_WAITING_DEPOSITS: usize = 1024;

/// One of our own operations, until a block takes it.
#[derive(Clone, Debug)]
struct Own {
    /// As we signed it: a block that executed it holds it as it is, under
    /// the index we had then, whatever our place is once the block ran.
    signed: SignedValidatorOperation,
    /// How many times it was attested again.
    again: u8,
}

/// How often a validator that declared the hub stalled passes the
/// finalization of its last final block to the other validators: as often
/// as a stuck view's nullify goes again at the longest
/// (`Config::max_retry_ms`), so that one that missed it, and holds the
/// block, finalizes it too (spec/designs/settlement-hubs.md#9-shutdown,
/// "Passing views around").
pub const PASS_TIP_EVERY_MS: Millis = Config::DEFAULT.max_retry_ms;

/// What the host carries out. Taken apart at once: boxing would only
/// allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send this message to every other validator.
    Validators(Vec<u8>),
    /// Send this message to one validator, by consensus key.
    Validator(dassie_identity::PublicKey, Vec<u8>),
    /// Send this message to whoever sent `peer`'s message.
    Peer(PeerId, Vec<u8>),
    /// Submit this signed escrow transaction to the XRP Ledger.
    Submit(Vec<u8>),
    /// The hub ended or stalled: publish this validator's signatures of the
    /// payout set, to every validator and member, and to anyone who asks.
    Payout(PayoutSet, Vec<Signer>),
    /// We are further behind than peers keep blocks: fetch the state after
    /// this final block, already checked with a set we know, from a
    /// validator, and hand it to [`Validator::catch_up`]
    /// (spec/areas/hubs.md#the-validators-service).
    Behind(Box<Finalized>),
    /// Validators broke the rules no honest validator breaks: publish the
    /// evidence, which names them and proves itself to anyone with the set
    /// (spec/designs/settlement-hubs.md#10-consensus).
    Evidence(Box<Evidence>),
}

/// Why [`Validator::catch_up`] did not take a state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatchUpError {
    /// The block's payload does not name a state.
    Block,
    /// The state is not the one the block names, or not at its height.
    State,
    /// The engine did not take the block: not above our tip, or not proven
    /// by a set we know.
    Refused,
}

/// Why [`Validator::resume`] refused what the host persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeError {
    /// The ledger is not at the stored tip's height.
    Height,
    /// The ledger's state is not the one the stored tip certifies.
    State,
    /// The keys are not a validator's of any set the stored state knows.
    NotAValidator,
}

/// A hub's validator.
pub struct Validator {
    hub: HubId,
    /// Our consensus key's public half, by which we find our place in the
    /// set in force.
    consensus_public: PublicKey,
    consensus_key: Arc<NodeKey>,
    keys: ValidatorKeys,
    engine: Engine,
    chain: Chain,
    sequence: u64,
    history: VecDeque<FinalBlock>,
    /// What the blocks in `history` hold, in bytes.
    history_bytes: usize,
    /// The lowest height from which `history` holds every block with
    /// operations.
    complete_from: u64,
    /// Escrow signatures of withdrawals, by escrow sequence and by the
    /// signer's place in the rules.
    shares: BTreeMap<u32, BTreeMap<usize, Signer>>,
    signed: BTreeMap<u32, ()>,
    /// The withdrawals whose quorum of signatures we submitted.
    submitted: BTreeMap<u32, ()>,
    /// Our attestations of deposits that failed for want of room, oldest
    /// first, to attest again ([`MAX_WAITING_DEPOSITS`]).
    waiting_deposits: VecDeque<Deposit>,
    /// The height of the last block whose payout set we signed: never a
    /// lower one after it.
    payout_height: Option<u64>,
    /// When a stalled validator next passes its last final block on.
    pass_tip_at: Millis,
    /// The payout set we signed, for members that ask.
    payout: Option<PayoutBundle>,
    actions: VecDeque<Action>,
    batches: Vec<Batch>,
    /// Our own operations passed on and not yet taken by a final block,
    /// oldest first.
    own: VecDeque<Own>,
}

/// The genesis block of a hub: its rules' digest, at the ledger's start.
#[must_use]
pub fn genesis_block(rules: &Rules, timestamp: Millis) -> Block {
    Block {
        epoch: 0,
        view: 0,
        height: 0,
        parent: crate::digest::Digest::default(),
        parent_view: 0,
        timestamp_ms: timestamp,
        payload: rules.id().0.as_bytes().to_vec(),
    }
}

impl Validator {
    /// A validator of the hub of `rules` from genesis, whose keys `keys`
    /// are those of its acceptance.
    ///
    /// # Errors
    ///
    /// The rules do not check, or `keys` are not a validator's of them.
    pub fn genesis(
        rules: Rules,
        keys: ValidatorKeys,
        timestamp: Millis,
        now: Millis,
    ) -> Result<Self, RulesError> {
        let ledger = Ledger::genesis(rules.clone(), timestamp)?;
        let set = ledger.validators().clone();
        let consensus_public = keys.consensus.public_key();
        set.index_of(&consensus_public)
            .ok_or(RulesError::Validators)?;
        let block = genesis_block(&rules, timestamp);
        let hub = *ledger.hub();
        let stored = Stored::genesis(block, set);
        let consensus_key = Arc::new(NodeKey::from_bytes(keys.consensus.as_bytes()));
        let engine = Engine::new(Config::DEFAULT, hub.0, consensus_key.clone(), stored, now);
        let tip = *engine.tip_digest();
        Ok(Self {
            hub,
            consensus_public,
            consensus_key,
            keys,
            engine,
            chain: Chain::new(ledger, tip),
            sequence: now,
            history: VecDeque::new(),
            history_bytes: 0,
            complete_from: 1,
            shares: BTreeMap::new(),
            signed: BTreeMap::new(),
            submitted: BTreeMap::new(),
            waiting_deposits: VecDeque::new(),
            payout_height: None,
            pass_tip_at: 0,
            payout: None,
            actions: VecDeque::new(),
            batches: Vec::new(),
            own: VecDeque::new(),
        })
    }

    /// The validator again after a restart, from what the host persisted:
    /// the consensus state the steps' batches built, and the state after
    /// its last final block, which the host wrote in the same transaction
    /// (and restored against the root that block names). It knows no
    /// history from before, so it answers a `Since` from further back with
    /// `Trimmed`, and members read their state anew
    /// (spec/areas/hubs.md#when-the-history-is-gone).
    ///
    /// # Errors
    ///
    /// The ledger is not the state after the stored tip, or `keys` are not
    /// a validator's of it: what a host wrote together never is.
    pub fn resume(
        keys: ValidatorKeys,
        stored: Stored,
        ledger: Ledger,
        now: Millis,
    ) -> Result<Self, ResumeError> {
        let tip = &stored.tip.block;
        if ledger.height() != tip.height {
            return Err(ResumeError::Height);
        }
        let same = if tip.height == 0 {
            tip.payload == ledger.hub().0.as_bytes()
        } else {
            Payload::decode_exact(&tip.payload)
                .is_ok_and(|payload| payload.state == ledger.state_root())
        };
        if !same {
            return Err(ResumeError::State);
        }
        ledger.check_invariants();
        let consensus_public = keys.consensus.public_key();
        let in_set = stored
            .epochs
            .iter()
            .any(|epoch| epoch.set.index_of(&consensus_public).is_some());
        if !in_set {
            return Err(ResumeError::NotAValidator);
        }
        let hub = *ledger.hub();
        let consensus_key = Arc::new(NodeKey::from_bytes(keys.consensus.as_bytes()));
        let engine = Engine::new(Config::DEFAULT, hub.0, consensus_key.clone(), stored, now);
        let tip = *engine.tip_digest();
        // Our operations' sequence only grows, across restarts too.
        let sequence = ledger
            .validators()
            .index_of(&consensus_public)
            .and_then(|index| ledger.validator(index))
            .map_or(0, |state| state.sequence);
        let complete_from = ledger.height().saturating_add(1);
        Ok(Self {
            hub,
            consensus_public,
            consensus_key,
            keys,
            engine,
            chain: Chain::new(ledger, tip),
            sequence: sequence.max(now),
            history: VecDeque::new(),
            history_bytes: 0,
            complete_from,
            shares: BTreeMap::new(),
            signed: BTreeMap::new(),
            submitted: BTreeMap::new(),
            waiting_deposits: VecDeque::new(),
            payout_height: None,
            pass_tip_at: 0,
            payout: None,
            actions: VecDeque::new(),
            batches: Vec::new(),
            own: VecDeque::new(),
        })
    }

    /// The hub's ID.
    #[must_use]
    pub const fn hub(&self) -> &HubId {
        &self.hub
    }

    /// Our index in the set in force, if we are in it.
    #[must_use]
    pub fn index_in_force(&self) -> Option<ValidatorIndex> {
        self.index()
    }

    /// Takes the state after `finalized`, a block [`Action::Behind`] named,
    /// as fetched from a peer: only if it is the state the block's root
    /// names, at its height. The engine jumps to the block and takes in
    /// the change of validators that state has scheduled; the chain goes on
    /// from it. We vouch for no history below it: a member asking from
    /// further back is told to read its state anew.
    ///
    /// # Errors
    ///
    /// The block or the state is not one to take; nothing changes.
    pub fn catch_up(
        &mut self,
        now: Millis,
        finalized: Finalized,
        snapshot: &[u8],
    ) -> Result<(), CatchUpError> {
        let payload =
            Payload::decode_exact(&finalized.block.payload).map_err(|_| CatchUpError::Block)?;
        let ledger = Ledger::restore(snapshot, &payload.state).map_err(|_| CatchUpError::State)?;
        let height = finalized.block.height;
        if ledger.height() != height {
            return Err(CatchUpError::State);
        }
        let change = ledger.scheduled_change();
        self.engine
            .jump(now, finalized, change)
            .map_err(|_| CatchUpError::Refused)?;
        let tip = *self.engine.tip_digest();
        self.chain = Chain::new(ledger, tip);
        // The blocks skipped never pass through `settle_own`: what of ours
        // they executed is at or below our last sequence in the state, and
        // done. Signed again, it would only take our pool's room.
        let last = self
            .index()
            .and_then(|index| self.chain.ledger.validator(index))
            .map(|state| state.sequence);
        match last {
            Some(last) => self.own.retain(|own| own.signed.sequence > last),
            None => self.own.clear(),
        }
        self.history.clear();
        self.history_bytes = 0;
        self.complete_from = height.saturating_add(1);
        self.after(now);
        Ok(())
    }

    /// A valid payload for the block at `height` on `parent`, stamped
    /// `timestamp`, other than a leader's: no operations and no votes,
    /// executed on the parent's state. For simulations of validators that
    /// propose two blocks in one view; `None` if this validator does not
    /// hold the parent's state.
    #[cfg(feature = "sim")]
    #[must_use]
    pub fn empty_payload(
        &self,
        parent: &crate::digest::Digest,
        height: u64,
        timestamp: Millis,
    ) -> Option<Vec<u8>> {
        let base = self.chain.base(parent)?;
        let executed = Chain::execute(base, height, timestamp, Vec::new(), Vec::new())?;
        Some(executed.payload.to_vec())
    }

    /// The final state.
    #[must_use]
    pub const fn ledger(&self) -> &Ledger {
        &self.chain.ledger
    }

    /// The consensus engine.
    #[must_use]
    pub const fn engine(&self) -> &Engine {
        &self.engine
    }

    /// When [`tick`](Self::tick) is due next: the engine's next timer, or,
    /// once we declared the hub stalled, when we pass our last final block
    /// on again.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<u64> {
        let pass = self.engine.is_stalled().then_some(self.pass_tip_at);
        [self.engine.poll_timeout(), pass]
            .into_iter()
            .flatten()
            .min()
    }

    /// The next thing for the host to do.
    pub fn poll(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }

    /// What the host persists before it carries out the actions of a step.
    pub fn take_batches(&mut self) -> Vec<Batch> {
        core::mem::take(&mut self.batches)
    }

    /// Takes a message from `from`, a validator or a member. `node` is the
    /// node key the transport authenticated the sender by, if any: only a
    /// validator of a set we know is served catch-up.
    pub fn handle(&mut self, now: Millis, from: PeerId, node: Option<&PublicKey>, bytes: &[u8]) {
        match Message::decode_exact(bytes) {
            Ok(Message::Consensus(inner)) => {
                if is_fetch(&inner) && !node.is_some_and(|node| self.is_validator(node)) {
                    self.after(now);
                    return;
                }
                self.engine
                    .handle_message(now, &mut self.chain, from, &inner);
            }
            Ok(Message::Operation(operation)) => {
                self.chain.mempool.admit(&self.chain.ledger, operation);
            }
            Ok(Message::Request(request)) => self.answer(from, request),
            Ok(Message::Share { sequence, signer }) => self.take_share(sequence, signer),
            Ok(Message::Statements(_) | Message::Payout(_)) | Err(_) => {}
        }
        self.after(now);
    }

    /// Whether `node` is a validator of the set in force or of the one
    /// scheduled to take over.
    fn is_validator(&self, node: &PublicKey) -> bool {
        let ledger = &self.chain.ledger;
        let pending = ledger.pending_rules.as_ref().map(|p| &p.rules);
        core::iter::once(ledger.rules())
            .chain(pending)
            .flat_map(|rules| &rules.validators)
            .any(|acceptance| acceptance.node == *node)
    }

    /// Does what is due.
    pub fn tick(&mut self, now: Millis) {
        self.engine.handle_timeout(now, &mut self.chain);
        let stall = self.chain.ledger.terms().timers.stall;
        if !self.engine.is_stalled() && self.engine.since_finalized_ms(now) > stall {
            // spec/designs/settlement-hubs.md#9-shutdown: it never votes
            // again, and signs the payout set of the last final block.
            self.engine.declare_stalled(now);
        }
        if self.engine.is_stalled() && now >= self.pass_tip_at {
            self.pass_tip_at = now.saturating_add(PASS_TIP_EVERY_MS);
            self.pass_tip();
        }
        self.after(now);
    }

    /// Passes the finalization of our last final block to the other
    /// validators, once we declared the hub stalled: we vote no more, so
    /// nothing else would tell a validator that missed it, and validators
    /// that stopped at different blocks would sign payout sets no quorum
    /// shares. It proves itself; one that holds the block finalizes it, and
    /// signs the payout set of that block in turn.
    fn pass_tip(&mut self) {
        let Some(finalization) = self.engine.tip().finalization.clone() else {
            return;
        };
        let inner = crate::consensus::wire::Message::Certificate(finalization).to_vec();
        self.actions
            .push_back(Action::Validators(Message::Consensus(inner).to_vec()));
    }

    /// Signs and passes on one of our own operations: what we saw of the
    /// escrow, or a vote. Its sequence is the time, or one more than the
    /// last, so it grows across restarts.
    ///
    /// Whether it was taken: an operation our own pool has no room for
    /// ([`mempool::VALIDATOR_WINDOW`]) is not passed on, and the host
    /// attests it again later.
    ///
    /// One that a later one of ours passed before any block took it is
    /// signed again, after the later one (`attest_passed`): an older vote
    /// can then execute after a newer one. A caller must not rely on the
    /// order of its value votes (`AttestEscrow`, `Reconfigure`,
    /// `SetEndpoint`): the last to execute is the one the ledger keeps.
    pub fn attest(&mut self, now: Millis, operation: ValidatorOperation) -> bool {
        let taken = self.pass_on(now, operation, 0);
        self.after(now);
        taken
    }

    /// Signs our `operation` with the next sequence, and passes it on if our
    /// pool takes it, following it until a block does: whether it took it.
    fn pass_on(&mut self, now: Millis, operation: ValidatorOperation, again: u8) -> bool {
        // A validator no longer in the set attests nothing.
        let Some(index) = self.index() else {
            return false;
        };
        self.sequence = self.sequence.saturating_add(1).max(now);
        let signed = SignedValidatorOperation::sign(
            &self.hub,
            index,
            self.sequence,
            operation,
            &self.consensus_key,
        );
        let message = Message::Operation(Operation::Validator(signed.clone())).to_vec();
        let taken = self
            .chain
            .mempool
            .admit(&self.chain.ledger, Operation::Validator(signed.clone()));
        if taken {
            self.actions.push_back(Action::Validators(message));
            if self.own.len() >= MAX_OWN {
                self.own.pop_front();
            }
            self.own.push_back(Own { signed, again });
        }
        taken
    }

    /// Our operations each final block took, as applied, failed or refused
    /// for what they said, are done; one refused for its sequence was not
    /// taken.
    ///
    /// Each is known as we signed it, not by our place now: a change of
    /// validators the block made may have moved us.
    fn settle_own(&mut self, block: &FinalBlock) {
        if self.index().is_none() {
            self.own.clear();
            return;
        }
        for (operation, outcome) in block.operations.iter().zip(&block.outcomes) {
            if let Operation::Validator(signed) = operation
                && *outcome != crate::ledger::Outcome::Refused(crate::ledger::Refusal::Sequence)
            {
                let ours = self.own.iter().any(|own| own.signed == *signed);
                self.own.retain(|own| own.signed != *signed);
                if ours {
                    self.keep_if_full(&signed.operation, outcome);
                }
            }
        }
    }

    /// Keeps our attestation of a deposit that failed for want of room to
    /// attest again ([`Self::attest_waiting`]). Of our attestations, only
    /// these does a freeze make fail: the hub credits no deposit while
    /// frozen, so our share of the pending table fills. Its other
    /// attestations, of withdrawals' outcomes among them, are recorded and
    /// wait in the ledger for the thaw (spec/decisions/token-hubs.md).
    fn keep_if_full(&mut self, operation: &ValidatorOperation, outcome: &crate::ledger::Outcome) {
        let full = matches!(
            outcome,
            crate::ledger::Outcome::Failed {
                reason: crate::ledger::Failure::Full,
                ..
            }
        );
        if let ValidatorOperation::AttestDeposit(deposit) = operation
            && full
            && self.waiting_deposits.len() < MAX_WAITING_DEPOSITS
            && !self.waiting_deposits.contains(deposit)
        {
            self.waiting_deposits.push_back(*deposit);
        }
    }

    /// Attests again, oldest first, the deposits whose attestations failed
    /// for want of room, as many as our share of the pending table has
    /// room for beside those of ours still on their way: none while the
    /// issuer holds the escrow, since the hub credits no deposit until the
    /// thaw and the room would only fill again.
    fn attest_waiting(&mut self, now: Millis) {
        if self.waiting_deposits.is_empty() || self.chain.ledger.issuer().holds() {
            return;
        }
        let Some(index) = self.index() else {
            self.waiting_deposits.clear();
            return;
        };
        let on_the_way = self
            .own
            .iter()
            .filter(|own| matches!(own.signed.operation, ValidatorOperation::AttestDeposit(_)))
            .count();
        let room = self
            .chain
            .ledger
            .deposit_room(index)
            .saturating_sub(on_the_way);
        for _ in 0..room {
            let Some(deposit) = self.waiting_deposits.pop_front() else {
                break;
            };
            if !self.pass_on(now, ValidatorOperation::AttestDeposit(deposit), 0) {
                // Our pool has no room: it goes again after the next block.
                self.waiting_deposits.push_front(deposit);
                break;
            }
        }
    }

    /// Attests again what a later operation of ours passed before any block
    /// took it. Our operations reach the other validators each on its own,
    /// in any order, and once one executed, none of ours with a lower
    /// sequence ever can: a leader that had only the later one would lose
    /// the earlier for good, a deposit's attestation among them. So each is
    /// signed again with a sequence above the last executed, a bounded
    /// number of times.
    fn attest_passed(&mut self, now: Millis) {
        let Some(last) = self
            .index()
            .and_then(|index| self.chain.ledger.validator(index))
            .map(|state| state.sequence)
        else {
            return;
        };
        let (passed, kept): (VecDeque<Own>, VecDeque<Own>) = self
            .own
            .drain(..)
            .partition(|own| own.signed.sequence <= last);
        self.own = kept;
        for own in passed {
            if own.again < MAX_AGAIN {
                self.pass_on(now, own.signed.operation, own.again.saturating_add(1));
            }
        }
    }

    fn answer(&mut self, from: PeerId, request: Request) {
        let statements = match request {
            Request::Submit(operation) => {
                self.chain.mempool.admit(&self.chain.ledger, operation);
                return;
            }
            Request::Account(key) => vec_of(Statement::account(&self.chain.ledger, key)),
            // The account and its holds as of one block, for a member that
            // reads its state anew (spec/areas/hubs.md#the-members-side).
            Request::Holds(account) => {
                let ledger = &self.chain.ledger;
                let Some(key) = ledger.account(account).map(|a| a.key) else {
                    return;
                };
                // Every page: the list is whole, and found by the ledger's
                // index of holds by account, not by looking at every hold.
                let mut statements = vec_of(Statement::account(ledger, key));
                statements.extend(Statement::holds(ledger, account));
                statements
            }
            Request::Since {
                account,
                from: height,
            } => self.since(account, height),
            // The payout set we signed, a page at a time, to anyone: its
            // signatures prove themselves. Nothing until we signed one.
            Request::Payout { from: first } => {
                if let Some(bundle) = &self.payout {
                    let page = wire::PayoutPage::of(bundle.height, &bundle.entries, first);
                    let message = Message::Payout(page).to_vec();
                    self.actions.push_back(Action::Peer(from, message));
                }
                return;
            }
        };
        self.send_statements(from, statements);
    }

    /// What was executed that touches `account` from `height` on, as far
    /// as it fits in one answer, then a token hub's issuer's hold as of the
    /// newest final block, and the final block the answer covers. Every
    /// answer states the issuer's hold, a short one and `Trimmed` too, so a
    /// member reads a freeze from what f + 1 validators sign alike, as it
    /// reads everything else (spec/areas/hubs.md#a-frozen-hub). It is as of
    /// the newest block even when the answer stops before it: the hold is
    /// known only as it is now.
    fn since(&self, account: AccountNumber, from: u64) -> Vec<Statement> {
        let ledger = &self.chain.ledger;
        let issuer = Statement::issuer(ledger);
        let tip = Statement::Final {
            height: ledger.height(),
            timestamp: ledger.timestamp(),
        };
        // What we no longer keep, we cannot vouch for: the member reads its
        // state anew (spec/areas/hubs.md#the-validators-service).
        if from < self.complete_from {
            let trimmed = Statement::Trimmed {
                height: ledger.height(),
                below: self.complete_from,
            };
            return issuer.into_iter().chain([trimmed]).collect();
        }
        let mut statements = Vec::new();
        let mut bytes = 0usize;
        for block in self.history.iter().filter(|block| block.height >= from) {
            let of_block: Vec<Statement> = touching(block, account).collect();
            let block_bytes: usize = of_block.iter().map(Encode::encoded_len).sum();
            // An answer too long, by count or bytes, stops before a block,
            // and vouches only for the blocks before it. A block is always
            // sent whole, however many messages it takes, so every answer
            // covers one more.
            let long = statements.len().saturating_add(of_block.len()) > MAX_ANSWER
                || bytes.saturating_add(block_bytes) > MAX_ANSWER_BYTES;
            bytes = bytes.saturating_add(block_bytes);
            if long && !statements.is_empty() {
                statements.extend(issuer);
                statements.push(Statement::Final {
                    height: block.height.saturating_sub(1),
                    timestamp: block.timestamp.saturating_sub(1),
                });
                return statements;
            }
            statements.extend(of_block);
        }
        statements.extend(issuer);
        statements.push(tip);
        statements
    }

    /// Our place in the set in force, which a change of validators can
    /// move; none once we left it.
    fn index(&self) -> Option<ValidatorIndex> {
        self.chain
            .ledger
            .validators()
            .index_of(&self.consensus_public)
    }

    fn send_statements(&mut self, to: PeerId, statements: Vec<Statement>) {
        let Some(index) = self.index() else {
            return;
        };
        let signed: Vec<SignedStatement> = statements
            .into_iter()
            .map(|statement| {
                SignedStatement::sign(&self.hub, index, statement, &self.consensus_key)
            })
            .collect();
        for chunk in wire::split_statements(signed) {
            self.actions
                .push_back(Action::Peer(to, Message::Statements(chunk).to_vec()));
        }
    }

    /// After every input: the engine's outputs and events, the statements
    /// of blocks that became final, and the escrow signatures they call for.
    fn after(&mut self, now: Millis) {
        // A block was final that our ledger cannot follow: more than f
        // validators are faulty, or there is a bug. We stop as at a stall:
        // never vote again, and sign the payout set of the last block we
        // executed (spec/designs/settlement-hubs.md#9-shutdown).
        if self.chain.halted() && !self.engine.is_stalled() {
            self.engine.declare_stalled(now);
        }
        while let Some(output) = self.engine.poll_output() {
            self.route(output);
        }
        while let Some(event) = self.engine.poll_event() {
            match event {
                Event::Proposed { view, block, .. } => self.chain.adopt_proposal(view, block),
                Event::Behind(finalized) => self.actions.push_back(Action::Behind(finalized)),
                Event::Evidence(evidence) => self.actions.push_back(Action::Evidence(evidence)),
                _ => {}
            }
        }
        self.batches.push(self.engine.take_batch());
        self.take_finals();
        self.attest_passed(now);
        self.attest_waiting(now);
        self.sign_withdrawals();
        // Once ended or stalled, we sign the payout set of our last final
        // block, and again whenever we learn of a later one, never of an
        // earlier one (spec/designs/settlement-hubs.md#9-shutdown). A later
        // one only once the hub stopped there too, its block older than the
        // stall: a hub that carries on without a validator that stalled
        // alone never has it sign a set, each a bundle written, at every
        // block it learns of. None while the issuer holds a token hub's
        // escrow as of that block (spec/decisions/token-hubs.md): its
        // Payments would fail on the frozen line, each using its sequence
        // and paying nothing. Only a later block can show the thaw.
        let held = self.chain.ledger.issuer().holds();
        let phase = self.chain.ledger.phase();
        let ended = phase == Phase::Ended || self.engine.is_stalled();
        let height = self.chain.ledger.height();
        let stall = self.chain.ledger.terms().timers.stall;
        let stopped = phase == Phase::Ended || self.engine.since_finalized_ms(now) > stall;
        let due = self
            .payout_height
            .is_none_or(|signed| signed < height && stopped);
        if ended && due && !held {
            self.sign_payout();
        }
        self.chain.check_invariants();
    }

    /// Settles our operations the blocks that became final took, and keeps
    /// those with operations to answer `Since`.
    fn take_finals(&mut self) {
        // Blocks the chain could not hold until now are not in our
        // history: we no longer vouch for what happened there.
        if let Some(lost) = self.chain.lost.take() {
            self.complete_from = self.complete_from.max(lost.saturating_add(1));
        }
        while let Some(block) = self.chain.finals.pop_front() {
            self.settle_own(&block);
            if !block.operations.is_empty() {
                self.keep_history(block);
            }
        }
    }

    /// Keeps `block` in the history, dropping the oldest while it holds
    /// more than [`KEPT_BLOCKS`] blocks or [`KEPT_BYTES`] bytes.
    fn keep_history(&mut self, block: FinalBlock) {
        let bytes = block.bytes();
        while self.history.len() >= KEPT_BLOCKS
            || self.history_bytes.saturating_add(bytes) > KEPT_BYTES
        {
            let Some(oldest) = self.history.pop_front() else {
                break;
            };
            self.history_bytes = self.history_bytes.saturating_sub(oldest.bytes());
            self.complete_from = self.complete_from.max(oldest.height.saturating_add(1));
        }
        self.history_bytes = self.history_bytes.saturating_add(bytes);
        self.history.push_back(block);
    }

    fn route(&mut self, output: Output) {
        // Consensus keeps its messages within MAX_MESSAGE_BYTES, which our
        // content's bound covers; one beyond it is dropped, never sent half
        // or encoded past its bound.
        if output.message.len() > wire::MAX_CONTENT {
            return;
        }
        let message = Message::Consensus(output.message).to_vec();
        self.actions.push_back(match output.to {
            Recipient::All => Action::Validators(message),
            Recipient::Validator(key) => Action::Validator(*key, message),
            Recipient::Peer(peer) => Action::Peer(peer, message),
        });
    }

    /// Signs each withdrawal the log gave a sequence, once, and shares the
    /// signature with the other validators; submits each a quorum signed.
    /// While the issuer holds a token hub's escrow, it signs and submits
    /// nothing (spec/decisions/token-hubs.md): a withdrawal fails on a
    /// frozen line, and fails at a cost, its sequence used. Shares of
    /// others still count, and once the issuer lets go it signs what is
    /// left and submits what has a quorum.
    fn sign_withdrawals(&mut self) {
        let held = self.chain.ledger.issuer().holds();
        let unsigned: Vec<(u32, dassie_xrpl::codec::Payment)> = self
            .chain
            .ledger
            .withdrawals()
            .filter(|(sequence, _)| !held && !self.signed.contains_key(sequence))
            .map(|(sequence, w)| {
                (
                    sequence,
                    withdrawal_payment(self.chain.ledger.rules(), sequence, w),
                )
            })
            .collect();
        for (sequence, payment) in unsigned {
            let signer = payment.multi_sign(self.keys.escrow.account(), &self.keys.escrow);
            self.signed.insert(sequence, ());
            self.actions.push_back(Action::Validators(
                Message::Share { sequence, signer }.to_vec(),
            ));
            self.take_share(sequence, signer);
        }
        let open: Vec<u32> = self.chain.ledger.withdrawals().map(|(s, _)| s).collect();
        self.signed.retain(|sequence, ()| open.contains(sequence));
        self.shares.retain(|sequence, _| open.contains(sequence));
        self.submitted
            .retain(|sequence, ()| open.contains(sequence));
        if !held {
            self.submit_signed();
        }
    }

    /// Gathers signatures of withdrawal `sequence`, until a quorum. Only a
    /// signature by the escrow key of a validator of the set in force
    /// counts, one per validator: anyone can sign the payment with keys of
    /// their own, and the XRP Ledger would refuse a transaction with such
    /// signers.
    fn take_share(&mut self, sequence: u32, signer: Signer) {
        let ledger = &self.chain.ledger;
        let Some((_, withdrawal)) = ledger.withdrawals().find(|(s, _)| *s == sequence) else {
            return;
        };
        let Some(index) = ledger.rules().validators.iter().position(|acceptance| {
            acceptance.escrow == *signer.signing_pub_key.as_bytes()
                && signer.account == signer.signing_pub_key.account()
        }) else {
            return;
        };
        let payment = withdrawal_payment(ledger.rules(), sequence, withdrawal);
        if !payment.verify_signer(&signer) {
            return;
        }
        let quorum = ledger.validators().quorum();
        let shares = self.shares.entry(sequence).or_default();
        if shares.contains_key(&index) || shares.len() >= quorum {
            return;
        }
        shares.insert(index, signer);
    }

    /// Submits, once each, every withdrawal a quorum signed.
    fn submit_signed(&mut self) {
        let ledger = &self.chain.ledger;
        let quorum = ledger.validators().quorum();
        for (sequence, withdrawal) in ledger.withdrawals() {
            let Some(shares) = self.shares.get(&sequence) else {
                continue;
            };
            if shares.len() < quorum || self.submitted.contains_key(&sequence) {
                continue;
            }
            let payment = withdrawal_payment(ledger.rules(), sequence, withdrawal);
            if let Ok(signed) = payment.with_signers(shares.values().copied().collect()) {
                self.submitted.insert(sequence, ());
                self.actions.push_back(Action::Submit(signed.blob));
            }
        }
    }

    fn sign_payout(&mut self) {
        let Some(set) = payout_set(&self.chain.ledger) else {
            return;
        };
        if let Some(signatures) = sign_payout_set(&set, &self.keys) {
            self.payout_height = Some(set.height);
            self.payout = Some(PayoutBundle::of(&set, &signatures));
            self.actions.push_back(Action::Payout(set, signatures));
        }
    }

    /// Asserts what holds between steps.
    pub fn check_invariants(&self) {
        self.engine.check_invariants();
        self.chain.check_invariants();
        assert!(
            self.history.len() <= KEPT_BLOCKS && self.history_bytes <= KEPT_BYTES,
            "history over its bound"
        );
        assert_eq!(
            self.history_bytes,
            self.history.iter().map(FinalBlock::bytes).sum::<usize>(),
            "history bytes out of step"
        );
        assert!(self.own.len() <= MAX_OWN, "our operations over their bound");
        assert!(
            self.waiting_deposits.len() <= MAX_WAITING_DEPOSITS,
            "deposits waiting for room over their bound"
        );
    }
}

/// Whether a consensus message asks for catch-up: what a delivery answers
/// can be up to a consensus message's bound, so it is for validators only.
fn is_fetch(inner: &[u8]) -> bool {
    // A request is a few bytes; anything longer is not one.
    inner.len() <= 64
        && matches!(
            crate::consensus::wire::Message::decode_message(inner),
            Ok(crate::consensus::wire::Message::Fetch(_))
        )
}

fn vec_of(statement: Statement) -> Vec<Statement> {
    alloc::vec![statement]
}

/// The statements of `block`'s operations that touch `account`: its own,
/// and those on holds it pays into or takes from.
fn touching(block: &FinalBlock, account: AccountNumber) -> impl Iterator<Item = Statement> + '_ {
    (0u32..)
        .zip(&block.operations)
        .zip(&block.outcomes)
        .zip(&block.touched)
        .filter(move |(_, touched)| touched.contains(&account))
        .map(
            move |(((index, operation), outcome), _)| Statement::Executed {
                height: block.height,
                timestamp: block.timestamp,
                index,
                operation: operation.clone(),
                outcome: *outcome,
            },
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Digest;
    use crate::ledger::{Detail, Outcome, Refusal};
    use crate::operation::{MemberOperation, SignedMemberOperation};

    /// A block of `count` copies of one transfer by account 5 to account
    /// 6, the first applied and the rest refused, as a faulty leader may
    /// build it.
    pub(super) fn block_of(height: u64, count: usize) -> FinalBlock {
        let key = NodeKey::from_bytes(&[5; 32]);
        let hub = HubId(Digest([1; 32]));
        let transfer = MemberOperation::Transfer {
            to: AccountNumber(6),
            amount: 1,
        };
        let operation = Operation::Member(SignedMemberOperation::sign(
            &hub,
            AccountNumber(5),
            0,
            10,
            transfer,
            &key,
        ));
        let outcomes = (0..count).map(|i| {
            if i == 0 {
                Outcome::Applied {
                    fee: 10,
                    detail: Detail::None,
                }
            } else {
                Outcome::Refused(Refusal::Sequence)
            }
        });
        FinalBlock {
            height,
            timestamp: height * 10,
            digest: Digest([2; 32]),
            operations: alloc::vec![operation; count],
            outcomes: outcomes.collect(),
            touched: alloc::vec![alloc::vec![AccountNumber(5), AccountNumber(6)]; count],
        }
    }

    // An operation of ours is known as we signed it: one signed under the
    // place we had before a change of validators moved us is done once a
    // block ran it, though our place is another by the time we read the
    // block, and is never signed again.
    #[test]
    fn an_operation_signed_under_an_earlier_place_is_settled_by_its_block() {
        let mut v = validator();
        let (_, mut keys) = crate::rules::tests::rules(4);
        let earlier = ValidatorIndex::new(2).unwrap();
        assert_ne!(v.index(), Some(earlier));
        let signed = SignedValidatorOperation::sign(
            &v.hub,
            earlier,
            7,
            ValidatorOperation::AttestEscrow { sequence: 5 },
            &keys.remove(0),
        );
        v.own.push_back(Own {
            signed: signed.clone(),
            again: 0,
        });
        let block = FinalBlock {
            height: 2,
            timestamp: 20,
            digest: Digest([2; 32]),
            operations: alloc::vec![Operation::Validator(signed)],
            outcomes: alloc::vec![Outcome::Applied {
                fee: 0,
                detail: Detail::None,
            }],
            touched: alloc::vec![Vec::new()],
        };
        v.settle_own(&block);
        assert!(v.own.is_empty());
    }

    #[test]
    fn an_operation_twice_in_a_block_makes_two_statements() {
        let statements: Vec<Statement> = touching(&block_of(3, 2), AccountNumber(6)).collect();
        let places: Vec<u32> = statements
            .iter()
            .filter_map(|s| match s {
                Statement::Executed { index, .. } => Some(*index),
                _ => None,
            })
            .collect();
        assert_eq!(places, [0, 1]);
        let hub = HubId(Digest([1; 32]));
        assert_ne!(statements[0].digest(&hub), statements[1].digest(&hub));
    }

    /// Validator 0 of a hub of four, at genesis.
    pub(super) fn validator() -> Validator {
        let (rules, mut keys) = crate::rules::tests::rules(4);
        let keys = ValidatorKeys {
            consensus: keys.remove(0),
            escrow: dassie_xrpl::keys::Wallet::from_seed([1; 16]),
            payout: crate::escrow::bip32::ExtendedPrivate::master(&[1; 32]).unwrap(),
        };
        Validator::genesis(rules, keys, 1_000, 1_000).unwrap()
    }

    /// The statements of the messages validator `v` sent `to`.
    fn sent(v: &mut Validator, to: PeerId) -> Vec<Vec<Statement>> {
        let mut messages = Vec::new();
        while let Some(action) = v.poll() {
            if let Action::Peer(peer, bytes) = action
                && peer == to
                && let Ok(Message::Statements(list)) = Message::decode_exact(&bytes)
            {
                messages.push(list.into_iter().map(|s| s.statement).collect());
            }
        }
        messages
    }

    #[test]
    fn a_block_too_long_for_one_answer_is_sent_whole() {
        let mut v = validator();
        let long = MAX_ANSWER + 44;
        v.history.push_back(block_of(2, long));
        v.history.push_back(block_of(3, 1));
        let member = PeerId(100);
        v.answer(
            member,
            Request::Since {
                account: AccountNumber(6),
                from: 1,
            },
        );
        let messages = sent(&mut v, member);
        assert!(messages.len() > 1, "over several messages");
        let statements: Vec<Statement> = messages.into_iter().flatten().collect();
        assert_eq!(statements.len(), long + 1);
        assert!(matches!(
            statements.last(),
            Some(Statement::Final { height: 2, .. })
        ));
    }

    #[test]
    fn an_executed_statement_adds_at_most_its_overhead() {
        let block = block_of(3, 1);
        let statement = touching(&block, AccountNumber(6)).next().unwrap();
        let operation = block.operations[0].encoded_len();
        assert!(statement.encoded_len() - operation <= EXECUTED_OVERHEAD);
    }

    // An answer is bounded by bytes as well as count, so a member can bound
    // what it holds of one validator's answer.
    #[test]
    fn an_answer_is_bounded_by_bytes() {
        let mut v = validator();
        let key = NodeKey::from_bytes(&[5; 32]);
        let hub = HubId(Digest([1; 32]));
        for height in 2..40u64 {
            let claim = Operation::Member(SignedMemberOperation::sign(
                &hub,
                AccountNumber(5),
                height,
                10,
                mempool::tests::claim(1024),
                &key,
            ));
            v.history.push_back(FinalBlock {
                height,
                timestamp: height * 10,
                digest: Digest([2; 32]),
                operations: alloc::vec![claim],
                outcomes: alloc::vec![Outcome::Refused(Refusal::Sequence)],
                touched: alloc::vec![alloc::vec![AccountNumber(6)]],
            });
        }
        let statements = v.since(AccountNumber(6), 1);
        let bytes: usize = statements.iter().map(Encode::encoded_len).sum();
        assert!(bytes <= MAX_SINCE_ANSWER_BYTES);
        assert!(statements.len() < 39, "it stopped before a block");
        assert!(matches!(statements.last(), Some(Statement::Final { .. })));
    }

    // Nothing is pushed: a member that asked hears of a later block only
    // once it asks again (spec/areas/hubs.md#the-validators-service).
    #[test]
    fn a_member_hears_only_what_it_asks() {
        let mut v = validator();
        let member = PeerId(100);
        let since = Request::Since {
            account: AccountNumber(6),
            from: 1,
        };
        v.answer(member, since.clone());
        assert_eq!(sent(&mut v, member).len(), 1, "it answers");
        v.chain.finals.push_back(block_of(2, 1));
        v.take_finals();
        assert!(sent(&mut v, member).is_empty(), "nothing unasked");
        v.answer(member, since);
        let statements: Vec<Statement> = sent(&mut v, member).into_iter().flatten().collect();
        assert!(matches!(
            statements[0],
            Statement::Executed { height: 2, .. }
        ));
        v.check_invariants();
    }

    // A token hub's answers to `Since` end with the issuer's hold as of the
    // newest block, then the final block, whether the answer runs to the
    // tip or stops before a block; an XRP hub's carry none
    // (spec/areas/hubs.md#a-frozen-hub).
    #[test]
    fn a_token_hubs_answers_carry_the_issuers_hold() {
        use crate::operation::Issuer;
        let tail = |statements: &[Statement]| -> Vec<Statement> {
            statements[statements.len().saturating_sub(2)..].to_vec()
        };
        let mut xrp = validator();
        xrp.history.push_back(block_of(2, 1));
        let statements = xrp.since(AccountNumber(6), 1);
        assert!(
            !statements
                .iter()
                .any(|s| matches!(s, Statement::Issuer { .. })),
            "none over an XRP hub"
        );

        let mut v = validator();
        let ledger = &mut v.chain.ledger;
        ledger.rules.terms = crate::rules::tests::token_terms(ledger.rules.terms.clone());
        ledger.issuer = Issuer {
            ledger: 5,
            frozen: true,
            fee: false,
        };
        ledger.frozen = Some(7);
        let height = v.chain.ledger.height();
        let timestamp = v.chain.ledger.timestamp();
        let hold = Statement::Issuer {
            height,
            timestamp,
            frozen: Some(7),
            held: true,
        };
        assert_eq!(hold.encoded_len(), ISSUER_STATEMENT_BYTES);
        let last = Statement::Final { height, timestamp };
        assert_eq!(last.encoded_len(), FINAL_STATEMENT_BYTES);

        v.history.push_back(block_of(2, 1));
        let statements = v.since(AccountNumber(6), 1);
        assert_eq!(tail(&statements), [hold.clone(), last]);

        // Stopped before a block: the final block is the one before it, the
        // hold still as of the newest.
        v.history.push_back(block_of(3, MAX_ANSWER));
        let statements = v.since(AccountNumber(6), 1);
        let ends = tail(&statements);
        assert_eq!(ends[0], hold);
        assert!(matches!(ends[1], Statement::Final { height: 2, .. }));
        let bytes: usize = statements.iter().map(Encode::encoded_len).sum();
        assert!(bytes <= MAX_SINCE_ANSWER_BYTES);
        assert!(statements.len() <= MAX_ANSWER + SINCE_TAIL);
    }

    #[test]
    fn a_final_block_the_ledger_cannot_follow_stops_the_validator() {
        use crate::consensus::Application;
        let mut v = validator();
        let (mut block, _) = chain::tests::empty_block(&v.chain);
        block.payload = alloc::vec![1, 2, 3];
        v.chain.finalized(&block, &Digest([9; 32]));
        v.tick(2_000);
        assert!(v.engine().is_stalled());
        let mut payout = false;
        while let Some(action) = v.poll() {
            payout |= matches!(action, Action::Payout(..));
        }
        assert!(
            payout,
            "it signs the payout set of the last block it executed"
        );
    }

    // spec/decisions/token-hubs.md: an ended token hub whose last state
    // has the issuer holding its escrow signs no payout set: its Payments
    // would fail on the frozen line. A later state that lets go of it is
    // signed.
    #[test]
    fn an_ended_hub_the_issuer_holds_signs_no_payout_set() {
        use crate::operation::Issuer;
        let mut v = validator();
        let ledger = &mut v.chain.ledger;
        ledger.rules.terms = crate::rules::tests::token_terms(ledger.rules.terms.clone());
        ledger.phase = Phase::Ended;
        ledger.issuer = Issuer {
            ledger: 5,
            frozen: false,
            fee: true,
        };
        let payouts = |v: &mut Validator| {
            let mut count = 0;
            while let Some(action) = v.poll() {
                count += usize::from(matches!(action, Action::Payout(..)));
            }
            count
        };
        v.tick(2_000);
        assert_eq!(payouts(&mut v), 0, "none while held");
        v.chain.ledger.issuer = Issuer {
            ledger: 6,
            frozen: false,
            fee: false,
        };
        v.tick(2_001);
        assert_eq!(payouts(&mut v), 1, "signed once let go");
    }

    // spec/areas/hubs.md#a-frozen-hub: a token hub's answers to `Since`
    // end with the issuer's hold as of the tip, before the final block,
    // whether short, long or trimmed; an XRP hub's never carry it. The end
    // of an answer takes at most what the bound on its bytes counts.
    #[test]
    fn a_token_hubs_answers_state_the_issuers_hold() {
        use crate::operation::Issuer;
        let mut v = validator();
        assert!(
            !v.since(AccountNumber(6), 1)
                .iter()
                .any(|s| matches!(s, Statement::Issuer { .. })),
            "an XRP hub has no issuer"
        );
        let ledger = &mut v.chain.ledger;
        ledger.rules.terms = crate::rules::tests::token_terms(ledger.rules.terms.clone());
        ledger.frozen = Some(ledger.timestamp());
        ledger.issuer = Issuer {
            ledger: 5,
            frozen: true,
            fee: false,
        };
        let held = Statement::issuer(&v.chain.ledger).unwrap();
        assert!(matches!(
            held,
            Statement::Issuer {
                frozen: Some(_),
                held: true,
                ..
            }
        ));
        let tail = [
            held.clone(),
            Statement::Final {
                height: v.chain.ledger.height(),
                timestamp: v.chain.ledger.timestamp(),
            },
        ];
        assert_eq!(tail.len(), SINCE_TAIL);
        assert_eq!(
            tail.iter().map(Encode::encoded_len).sum::<usize>(),
            FINAL_STATEMENT_BYTES + ISSUER_STATEMENT_BYTES
        );
        // Short: what touched the account, then the tail.
        v.history.push_back(block_of(2, 1));
        let short = v.since(AccountNumber(6), 1);
        assert_eq!(short.len(), 1 + SINCE_TAIL);
        assert_eq!(&short[1..], &tail);
        // Long: it stops before a block, the hold as of the tip before the
        // final block it covers.
        v.history.push_back(block_of(3, MAX_ANSWER));
        let long = v.since(AccountNumber(6), 1);
        assert_eq!(long.len(), 1 + SINCE_TAIL);
        assert_eq!(long[1], held);
        assert!(matches!(long[2], Statement::Final { height: 2, .. }));
        // Trimmed: the member reads its state anew, and the freeze with it.
        v.complete_from = 2;
        let trimmed = v.since(AccountNumber(6), 1);
        assert_eq!(trimmed[0], held);
        assert!(matches!(trimmed[1], Statement::Trimmed { below: 2, .. }));
    }

    // Our attestation of a deposit that failed for want of room waits, and
    // goes again once the issuer lets go and there is room; one that
    // failed otherwise, or that was not ours, does not.
    #[test]
    fn only_our_deposits_that_found_no_room_wait() {
        let mut v = validator();
        let index = v.index().unwrap();
        let deposit = |byte| Deposit {
            transaction: Digest([byte; 32]),
            ledger: 10,
            amount: 50_000_000,
            sender: [3; 20],
            tag: 0,
            key: None,
        };
        let full = Outcome::Failed {
            fee: 0,
            reason: crate::ledger::Failure::Full,
        };
        let stale = Outcome::Failed {
            fee: 0,
            reason: crate::ledger::Failure::Stale,
        };
        let (_, mut keys) = crate::rules::tests::rules(4);
        let mut operations = Vec::new();
        for (byte, sequence) in [(1u8, 7u64), (2, 8)] {
            let signed = SignedValidatorOperation::sign(
                &v.hub,
                index,
                sequence,
                ValidatorOperation::AttestDeposit(deposit(byte)),
                &keys[0],
            );
            v.own.push_back(Own {
                signed: signed.clone(),
                again: 0,
            });
            operations.push(Operation::Validator(signed));
        }
        let other = SignedValidatorOperation::sign(
            &v.hub,
            ValidatorIndex::new(1).unwrap(),
            9,
            ValidatorOperation::AttestDeposit(deposit(3)),
            &keys.remove(1),
        );
        operations.push(Operation::Validator(other));
        let block = FinalBlock {
            height: 2,
            timestamp: 20,
            digest: Digest([2; 32]),
            operations,
            outcomes: alloc::vec![full, stale, full],
            touched: alloc::vec![Vec::new(); 3],
        };
        v.settle_own(&block);
        assert!(v.own.is_empty());
        assert_eq!(v.waiting_deposits, [deposit(1)]);
        v.attest_waiting(2_000);
        assert!(v.waiting_deposits.is_empty(), "attested again");
        assert_eq!(v.own.len(), 1);
    }

    #[test]
    fn final_blocks_never_taken_raise_what_the_history_vouches_for() {
        use crate::consensus::{Application, MAX_BLOCKS};
        let mut v = validator();
        for i in 0..=MAX_BLOCKS {
            let (block, _) = chain::tests::empty_block(&v.chain);
            let digest = Digest::of(crate::digest::Domain::Block, &[&i.to_be_bytes()]);
            v.chain.finalized(&block, &digest);
        }
        v.take_finals();
        assert_eq!(v.complete_from, 2, "block 1 was dropped untaken");
    }

    #[test]
    fn the_history_is_bounded_by_bytes() {
        let mut v = validator();
        for height in 1..200 {
            v.keep_history(block_of(height, 4096));
        }
        assert!(v.history_bytes <= KEPT_BYTES);
        assert!(v.complete_from > 1);
        v.check_invariants();
    }
}

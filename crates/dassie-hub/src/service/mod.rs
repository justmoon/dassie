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
    Batch, Block, Config, Engine, Event, Finalized, Output, PeerId, Recipient, Stored,
};
use crate::escrow::{PayoutSet, ValidatorKeys, payout_set, sign_payout_set, withdrawal_payment};
use crate::ledger::{Ledger, Phase};
use crate::operation::{AccountNumber, Operation, SignedValidatorOperation, ValidatorOperation};
use crate::rules::{HubId, Millis, Rules, RulesError};
use crate::validators::ValidatorIndex;
use payload::Payload;
use wire::Message;

/// The most members one validator pushes statements to.
pub const MAX_SUBSCRIBERS: usize = 1024;

/// The most members one validator pushes one account's statements to.
pub const MAX_SUBSCRIBERS_PER_ACCOUNT: usize = 4;

/// The most final blocks with operations a validator keeps to answer
/// `Since`; blocks without any take no room.
pub const KEPT_BLOCKS: usize = 4096;

/// The most bytes of operations the blocks kept to answer `Since` hold.
pub const KEPT_BYTES: usize = 64 * 1024 * 1024;

/// The most statements one answer to `Since` carries, over several
/// messages, but for its first block, which is always whole: a block can
/// touch an account more often than this.
pub const MAX_ANSWER: usize = 4 * crate::client::MAX_STATEMENTS;

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
    /// Who gets pushes for which account, oldest first.
    subscribers: VecDeque<(PeerId, AccountNumber)>,
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
    payout_signed: bool,
    actions: VecDeque<Action>,
    batches: Vec<Batch>,
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
            subscribers: VecDeque::new(),
            history: VecDeque::new(),
            history_bytes: 0,
            complete_from: 1,
            shares: BTreeMap::new(),
            signed: BTreeMap::new(),
            payout_signed: false,
            actions: VecDeque::new(),
            batches: Vec::new(),
        })
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
            .jump(now, finalized, change.clone())
            .map_err(|_| CatchUpError::Refused)?;
        let tip = *self.engine.tip_digest();
        self.chain = Chain::resumed(ledger, tip, change.map(|(from, _)| from));
        self.history.clear();
        self.history_bytes = 0;
        self.complete_from = height.saturating_add(1);
        self.after(now);
        Ok(())
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

    /// When [`tick`](Self::tick) is due next.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<u64> {
        self.engine.poll_timeout()
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
            Ok(Message::Statements(_)) | Err(_) => {}
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
        self.after(now);
    }

    /// Signs and passes on one of our own operations: what we saw of the
    /// escrow, or a vote. Its sequence is the time, or one more than the
    /// last, so it grows across restarts.
    ///
    /// Whether it was taken: an operation our own pool has no room for
    /// ([`mempool::VALIDATOR_WINDOW`]) is not passed on, and the host
    /// attests it again later.
    pub fn attest(&mut self, now: Millis, operation: ValidatorOperation) -> bool {
        // A validator no longer in the set attests nothing.
        let Some(index) = self.index() else {
            return false;
        };
        self.sequence = self.sequence.saturating_add(1).max(now);
        let signed = Operation::Validator(SignedValidatorOperation::sign(
            &self.hub,
            index,
            self.sequence,
            operation,
            &self.consensus_key,
        ));
        let message = Message::Operation(signed.clone()).to_vec();
        let taken = self.chain.mempool.admit(&self.chain.ledger, signed);
        if taken {
            self.actions.push_back(Action::Validators(message));
        }
        self.after(now);
        taken
    }

    fn answer(&mut self, from: PeerId, request: Request) {
        let statements = match request {
            Request::Submit(operation) => {
                self.chain.mempool.admit(&self.chain.ledger, operation);
                return;
            }
            Request::Account(key) => vec_of(Statement::account(&self.chain.ledger, key)),
            Request::Hold(number) => vec_of(Statement::hold(&self.chain.ledger, number)),
            Request::Since {
                account,
                from: height,
            } => {
                self.subscribe(from, account);
                self.since(account, height)
            }
        };
        self.send_statements(from, statements);
    }

    /// Pushes to `peer` what touches `account` from now on, in place of
    /// what it subscribed to before. The oldest subscription goes when the
    /// account, or the validator, has too many: anyone may subscribe to any
    /// account, and members reconnect, so none may lock others out.
    fn subscribe(&mut self, peer: PeerId, account: AccountNumber) {
        self.subscribers.retain(|(p, _)| *p != peer);
        let of_account = self.subscribers.iter().filter(|(_, a)| *a == account);
        if of_account.count() >= MAX_SUBSCRIBERS_PER_ACCOUNT
            && let Some(oldest) = self.subscribers.iter().position(|(_, a)| *a == account)
        {
            self.subscribers.remove(oldest);
        }
        if self.subscribers.len() >= MAX_SUBSCRIBERS {
            self.subscribers.pop_front();
        }
        self.subscribers.push_back((peer, account));
    }

    /// What was executed that touches `account` from `height` on, and the
    /// newest final block, as far as they fit in one answer.
    fn since(&self, account: AccountNumber, from: u64) -> Vec<Statement> {
        let tip = Statement::Final {
            height: self.chain.ledger.height(),
            timestamp: self.chain.ledger.timestamp(),
        };
        // What we no longer keep, we cannot vouch for: the member reads its
        // state anew (spec/areas/hubs.md#the-validators-service).
        if from < self.complete_from {
            return alloc::vec![Statement::Trimmed {
                height: self.chain.ledger.height(),
                below: self.complete_from,
            }];
        }
        let mut statements = Vec::new();
        for block in self.history.iter().filter(|block| block.height >= from) {
            let of_block: Vec<Statement> = touching(block, account).collect();
            // An answer too long stops before a block, and vouches only for
            // the blocks before it. A block is always sent whole, however
            // many messages it takes, so every answer covers one more.
            let long = statements.len().saturating_add(of_block.len()) > MAX_ANSWER;
            if long && !statements.is_empty() {
                statements.push(Statement::Final {
                    height: block.height.saturating_sub(1),
                    timestamp: block.timestamp.saturating_sub(1),
                });
                return statements;
            }
            statements.extend(of_block);
        }
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
                _ => {}
            }
        }
        self.batches.push(self.engine.take_batch());
        self.take_finals();
        self.sign_withdrawals();
        let ended = self.chain.ledger.phase() == Phase::Ended || self.engine.is_stalled();
        if ended && !self.payout_signed {
            self.sign_payout();
        }
        self.chain.check_invariants();
    }

    /// Publishes the blocks that became final, and keeps those with
    /// operations to answer `Since`.
    fn take_finals(&mut self) {
        // Blocks the chain could not hold until now are not in our
        // history: we no longer vouch for what happened there.
        if let Some(lost) = self.chain.lost.take() {
            self.complete_from = self.complete_from.max(lost.saturating_add(1));
        }
        while let Some(block) = self.chain.finals.pop_front() {
            self.publish(&block);
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

    /// Pushes to each subscriber what the block did to its account.
    ///
    /// A push is early notice only: it carries no `Final`, since a push that
    /// is lost would make the validator vouch for blocks it never sent. A
    /// member learns what is covered from answers to `Since`, which list
    /// everything up to the `Final` they end with
    /// (spec/areas/hubs.md#the-validators-service).
    fn publish(&mut self, block: &FinalBlock) {
        let subscribers: Vec<(PeerId, AccountNumber)> = self.subscribers.iter().copied().collect();
        for (peer, account) in subscribers {
            let statements: Vec<Statement> = touching(block, account).collect();
            if !statements.is_empty() {
                self.send_statements(peer, statements);
            }
        }
    }

    /// Signs each withdrawal the log gave a sequence, once, and shares the
    /// signature with the other validators.
    fn sign_withdrawals(&mut self) {
        let unsigned: Vec<(u32, dassie_xrpl::codec::Payment)> = self
            .chain
            .ledger
            .withdrawals()
            .filter(|(sequence, _)| !self.signed.contains_key(sequence))
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
    }

    /// Gathers signatures of withdrawal `sequence`; with a quorum, the
    /// transaction is submitted. Only a signature by the escrow key of a
    /// validator of the set in force counts, one per validator: anyone can
    /// sign the payment with keys of their own, and the XRP Ledger would
    /// refuse a transaction with such signers.
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
        if shares.len() == quorum
            && let Ok(signed) = payment.with_signers(shares.values().copied().collect())
        {
            self.actions.push_back(Action::Submit(signed.blob));
        }
    }

    fn sign_payout(&mut self) {
        let Some(set) = payout_set(&self.chain.ledger) else {
            return;
        };
        if let Some(signatures) = sign_payout_set(&set, &self.keys) {
            self.payout_signed = true;
            self.actions.push_back(Action::Payout(set, signatures));
        }
    }

    /// Asserts what holds between steps.
    pub fn check_invariants(&self) {
        self.engine.check_invariants();
        self.chain.check_invariants();
        assert!(
            self.subscribers.len() <= MAX_SUBSCRIBERS
                && self.subscribers.iter().all(|(_, account)| {
                    let of_account = self.subscribers.iter().filter(|(_, a)| a == account);
                    of_account.count() <= MAX_SUBSCRIBERS_PER_ACCOUNT
                }),
            "subscribers over their bound"
        );
        assert!(
            self.history.len() <= KEPT_BLOCKS && self.history_bytes <= KEPT_BYTES,
            "history over its bound"
        );
        assert_eq!(
            self.history_bytes,
            self.history.iter().map(FinalBlock::bytes).sum::<usize>(),
            "history bytes out of step"
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
    fn a_push_carries_no_final() {
        let mut v = validator();
        let member = PeerId(100);
        v.subscribe(member, AccountNumber(6));
        v.publish(&block_of(2, 1));
        let statements: Vec<Statement> = sent(&mut v, member).into_iter().flatten().collect();
        assert_eq!(statements.len(), 1);
        assert!(matches!(statements[0], Statement::Executed { .. }));
    }

    #[test]
    fn new_subscribers_push_out_the_oldest() {
        let mut v = validator();
        // Many reconnects to one account, and many accounts.
        for peer in 0..10 {
            v.subscribe(PeerId(peer), AccountNumber(6));
        }
        for peer in 10..2_000 {
            v.subscribe(PeerId(peer), AccountNumber(u32::try_from(peer).unwrap()));
        }
        v.check_invariants();
        let latecomer = PeerId(5_000);
        v.subscribe(latecomer, AccountNumber(6));
        v.publish(&block_of(2, 1));
        assert_eq!(
            sent(&mut v, latecomer).len(),
            1,
            "the latecomer gets pushes"
        );
        v.check_invariants();
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

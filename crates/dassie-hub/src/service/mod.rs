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

use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::NodeKey;
use dassie_oer::Encode;
use dassie_xrpl::codec::{Signer, Transact};

pub use chain::{Chain, FinalBlock};
pub use mempool::{Mempool, SEQUENCE_WINDOW};

use crate::client::{Request, SignedStatement, Statement};
use crate::consensus::{Batch, Block, Config, Engine, Event, Output, PeerId, Recipient, Stored};
use crate::escrow::{PayoutSet, ValidatorKeys, payout_set, sign_payout_set, withdrawal_payment};
use crate::ledger::{Ledger, Phase};
use crate::operation::{AccountNumber, Operation, SignedValidatorOperation, ValidatorOperation};
use crate::rules::{HubId, Millis, Rules, RulesError};
use crate::validators::ValidatorIndex;
use wire::Message;

/// The most members one validator pushes statements to.
pub const MAX_SUBSCRIBERS: usize = 1024;

/// The most final blocks with operations a validator keeps to answer
/// `Since`; blocks without any take no room.
pub const KEPT_BLOCKS: usize = 4096;

/// The most statements one answer to `Since` carries, over several
/// messages.
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
}

/// A hub's validator.
pub struct Validator {
    hub: HubId,
    index: ValidatorIndex,
    consensus_key: Arc<NodeKey>,
    keys: ValidatorKeys,
    engine: Engine,
    chain: Chain,
    sequence: u64,
    subscribers: BTreeMap<PeerId, AccountNumber>,
    history: VecDeque<FinalBlock>,
    /// The lowest height from which `history` holds every block with
    /// operations.
    complete_from: u64,
    shares: BTreeMap<u32, Vec<Signer>>,
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
        let index = set
            .index_of(&keys.consensus.public_key())
            .ok_or(RulesError::Validators)?;
        let block = genesis_block(&rules, timestamp);
        let hub = *ledger.hub();
        let stored = Stored::genesis(block, set);
        let consensus_key = Arc::new(NodeKey::from_bytes(keys.consensus.as_bytes()));
        let engine = Engine::new(Config::DEFAULT, hub.0, consensus_key.clone(), stored, now);
        let tip = *engine.tip_digest();
        let capacity = usize::from(rules.terms.limits.block_operations).saturating_mul(4);
        Ok(Self {
            hub,
            index,
            consensus_key,
            keys,
            engine,
            chain: Chain::new(ledger, tip, capacity),
            sequence: now,
            subscribers: BTreeMap::new(),
            history: VecDeque::new(),
            complete_from: 1,
            shares: BTreeMap::new(),
            signed: BTreeMap::new(),
            payout_signed: false,
            actions: VecDeque::new(),
            batches: Vec::new(),
        })
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

    /// Takes a message from `from`, a validator or a member.
    pub fn handle(&mut self, now: Millis, from: PeerId, bytes: &[u8]) {
        match Message::decode_exact(bytes) {
            Ok(Message::Consensus(inner)) => {
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
    pub fn attest(&mut self, now: Millis, operation: ValidatorOperation) {
        self.sequence = self.sequence.saturating_add(1).max(now);
        let signed = Operation::Validator(SignedValidatorOperation::sign(
            &self.hub,
            self.index,
            self.sequence,
            operation,
            &self.consensus_key,
        ));
        self.actions.push_back(Action::Validators(
            Message::Operation(signed.clone()).to_vec(),
        ));
        self.chain.mempool.admit(&self.chain.ledger, signed);
        self.after(now);
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
                if self.subscribers.len() < MAX_SUBSCRIBERS || self.subscribers.contains_key(&from)
                {
                    self.subscribers.insert(from, account);
                }
                self.since(account, height)
            }
        };
        self.send_statements(from, statements);
    }

    /// What was executed that touches `account` from `height` on, and the
    /// newest final block, as far as they fit in one answer.
    fn since(&self, account: AccountNumber, from: u64) -> Vec<Statement> {
        let tip = Statement::Final {
            height: self.chain.ledger.height(),
            timestamp: self.chain.ledger.timestamp(),
        };
        // What we no longer keep, we cannot vouch for: the member reads its
        // state anew (spec/areas/hubs.md#the-members-side).
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
            // the blocks before it.
            if statements.len().saturating_add(of_block.len()) >= MAX_ANSWER {
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

    fn send_statements(&mut self, to: PeerId, statements: Vec<Statement>) {
        let signed: Vec<SignedStatement> = statements
            .into_iter()
            .map(|statement| {
                SignedStatement::sign(&self.hub, self.index, statement, &self.consensus_key)
            })
            .collect();
        for chunk in signed.chunks(crate::client::MAX_STATEMENTS) {
            self.actions.push_back(Action::Peer(
                to,
                Message::Statements(chunk.to_vec()).to_vec(),
            ));
        }
    }

    /// After every input: the engine's outputs and events, the statements
    /// of blocks that became final, and the escrow signatures they call for.
    fn after(&mut self, now: Millis) {
        while let Some(output) = self.engine.poll_output() {
            self.route(output);
        }
        while let Some(event) = self.engine.poll_event() {
            if let Event::Proposed { view, block, .. } = event {
                self.chain.adopt_proposal(view, block);
            }
        }
        self.batches.push(self.engine.take_batch());
        while let Some(block) = self.chain.finals.pop_front() {
            self.publish(&block);
            if block.operations.is_empty() {
                continue;
            }
            if self.history.len() >= KEPT_BLOCKS
                && let Some(oldest) = self.history.pop_front()
            {
                self.complete_from = oldest.height.saturating_add(1);
            }
            self.history.push_back(block);
        }
        self.sign_withdrawals();
        let ended = self.chain.ledger.phase() == Phase::Ended || self.engine.is_stalled();
        if ended && !self.payout_signed {
            self.sign_payout();
        }
        let _ = now;
        self.chain.check_invariants();
    }

    fn route(&mut self, output: Output) {
        let message = Message::Consensus(output.message).to_vec();
        self.actions.push_back(match output.to {
            Recipient::All => Action::Validators(message),
            Recipient::Validator(key) => Action::Validator(*key, message),
            Recipient::Peer(peer) => Action::Peer(peer, message),
        });
    }

    /// Pushes to each subscriber what the block did to its account, and the
    /// newest final block to all.
    fn publish(&mut self, block: &FinalBlock) {
        let subscribers: Vec<(PeerId, AccountNumber)> =
            self.subscribers.iter().map(|(p, a)| (*p, *a)).collect();
        for (peer, account) in subscribers {
            let mut statements: Vec<Statement> = touching(block, account).collect();
            statements.push(Statement::Final {
                height: block.height,
                timestamp: block.timestamp,
            });
            self.send_statements(peer, statements);
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
    /// transaction is submitted.
    fn take_share(&mut self, sequence: u32, signer: Signer) {
        let ledger = &self.chain.ledger;
        let Some((_, withdrawal)) = ledger.withdrawals().find(|(s, _)| *s == sequence) else {
            return;
        };
        let payment = withdrawal_payment(ledger.rules(), sequence, withdrawal);
        if !payment.verify_signer(&signer) {
            return;
        }
        let quorum = ledger.validators().quorum();
        let shares = self.shares.entry(sequence).or_default();
        if shares.iter().any(|s| s.account == signer.account) || shares.len() >= quorum {
            return;
        }
        shares.push(signer);
        if shares.len() == quorum
            && let Ok(signed) = payment.with_signers(shares.clone())
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
            self.subscribers.len() <= MAX_SUBSCRIBERS,
            "subscribers over their bound"
        );
        assert!(self.history.len() <= KEPT_BLOCKS, "history over its bound");
    }
}

fn vec_of(statement: Statement) -> Vec<Statement> {
    alloc::vec![statement]
}

/// The statements of `block`'s operations that touch `account`: its own,
/// and those on holds it pays into or takes from.
fn touching(block: &FinalBlock, account: AccountNumber) -> impl Iterator<Item = Statement> + '_ {
    block
        .operations
        .iter()
        .zip(&block.outcomes)
        .zip(&block.touched)
        .filter(move |(_, touched)| touched.contains(&account))
        .map(move |((operation, outcome), _)| Statement::Executed {
            height: block.height,
            timestamp: block.timestamp,
            operation: operation.clone(),
            outcome: *outcome,
        })
}

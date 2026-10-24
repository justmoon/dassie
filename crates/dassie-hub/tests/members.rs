//! Members peered over a settlement hub, end to end
//! (spec/areas/hubs.md#the-members-side,
//! spec/designs/settlement-hubs.md#4-peering-over-a-hub): a real ledger
//! whose four validators execute what members submit in blocks and answer
//! with statements each signs alone, and members that each run a
//! `HubAccount` and a `HubScheme` and book what the scheme says, as the
//! node would. Packets travel between members as the connector would carry
//! them; settlements are made as the node makes them on request.
//!
//! After every step the ledger, every account and every scheme check their
//! invariants, and each member's books hold what its scheme says it holds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dassie_hub::client::{MAX_STATEMENTS, Request, SignedStatement, Statement};
use dassie_hub::escrow::{ValidatorKeys, escrow_master};
use dassie_hub::ledger::{Block, Detail, Outcome};
use dassie_hub::member::{FIRST, HubAccount, HubScheme, SCHEME_ID, units};
use dassie_hub::operation::{
    AccountNumber, Deposit, Destination, MemberOperation, Operation, SignedMemberOperation,
    SignedValidatorOperation, ValidatorOperation,
};
use dassie_hub::promise::{Commitment, Fulfilled, HoldNumber, Signed};
use dassie_hub::rules::{Fees, Limits, Rules, Terms, Timers};
use dassie_hub::service::wire::Message;
use dassie_hub::validators::{Signers, ValidatorIndex};
use dassie_hub::{Digest, Ledger, Millis};
use dassie_identity::{NodeKey, PublicKey};
use dassie_ledger::Name;
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Encode, Reader};
use dassie_settlement::{
    AssetEffect, Decided, Effect, ExternalAsset, LedgerInput, LossCause, Promised, ReconcileCause,
    SchemeError, SettlementScheme,
};
use sha2::{Digest as _, Sha256};

const NONCE: [u8; 32] = [7; 32];
const START: Millis = 1_800_000_000_000;
/// How often the validators make a block.
const BLOCK_MS: u64 = 50;
/// How finely time moves.
const STEP_MS: u64 = 10;
/// Drops in an XRP.
const XRP: u64 = 1_000_000;
/// The largest packet, in internal units: 0.1 XRP.
const P: u64 = 100_000_000;
const FEE: u64 = 10;
const HOLD_RESERVE: u64 = 200_000;
const ACCOUNT_RESERVE: u64 = 2_000_000;
const MARGIN: Millis = 500;

fn terms(sponsor: &NodeKey, escrow: [u8; 20]) -> Terms {
    Terms {
        nonce: NONCE,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow,
        fees: Fees {
            operation: FEE,
            ledger: 100,
            reserve_share_ppm: 100_000,
            account_reserve: ACCOUNT_RESERVE,
            hold_reserve: HOLD_RESERVE,
            starting_balance: 1_000,
            least_payout: 1_000_000,
        },
        limits: Limits {
            accounts: 100,
            holds: 100,
            holds_per_account: 8,
            withdrawals: 16,
            pending_deposits: 64,
            recent_deposits: 256,
            block_operations: 64,
            deadline_operations: 16,
            claimed_per_hold: 1_100,
            commitments_per_claim: 16,
            vote_window: 32,
        },
        timers: Timers {
            hold_notice: 60_000,
            shutdown_notice: 60_000,
            reconfigure_notice: 120_000,
            absence: 86_400_000,
            stall: 3_600_000,
            precision: 100,
            margin: MARGIN,
            allowance: 300,
            view_timeout: 200,
        },
    }
}

/// What a validator does with what it is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Honest,
    /// Answers nothing.
    Silent,
    /// Claims to have covered blocks it has not, with other stamps, leaves
    /// out every operation, and says the account holds more.
    Liar,
}

/// An operation in a final block.
#[derive(Clone, Debug)]
struct Entry {
    height: u64,
    timestamp: Millis,
    /// Its place in its block.
    index: u32,
    operation: Operation,
    outcome: Outcome,
}

/// The hub: its ledger, run by four validators.
struct Validators {
    ledger: Ledger,
    keys: Vec<NodeKey>,
    faults: [Fault; 4],
    mempool: Vec<Operation>,
    executed: BTreeSet<Digest>,
    log: Vec<Entry>,
    parties: BTreeMap<HoldNumber, (AccountNumber, AccountNumber)>,
    stamps: BTreeMap<u64, Millis>,
    sequence: u64,
    deposits: u32,
    /// The lowest height whose operations the validators still keep.
    complete_from: u64,
}

impl Validators {
    /// The sponsor's hub, with its escrow attested.
    fn found(faults: [Fault; 4]) -> Self {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let master = escrow_master(&sponsor, &NONCE).unwrap();
        let terms = terms(&sponsor, master.account().0);
        let nodes: Vec<NodeKey> = (1..=4).map(|i| NodeKey::from_bytes(&[i; 32])).collect();
        let keys: Vec<ValidatorKeys> = nodes
            .iter()
            .map(|node| ValidatorKeys::derive(node, &NONCE).unwrap())
            .collect();
        let validators = keys
            .iter()
            .zip(&nodes)
            .enumerate()
            .map(|(i, (keys, node))| {
                keys.accept(node, &terms, format!("v{i}.example:443"))
                    .unwrap()
            })
            .collect();
        let rules = Rules { terms, validators };
        let mut hub = Self {
            ledger: Ledger::genesis(rules, START).unwrap(),
            keys: keys.into_iter().map(|k| k.consensus).collect(),
            faults,
            mempool: Vec::new(),
            executed: BTreeSet::new(),
            log: Vec::new(),
            parties: BTreeMap::new(),
            stamps: BTreeMap::from([(0, START)]),
            sequence: 0,
            deposits: 0,
            complete_from: 1,
        };
        hub.attest(&ValidatorOperation::AttestEscrow { sequence: 10 });
        hub.block(START + 1);
        assert!(hub.ledger.escrow().checked);
        hub
    }

    fn rules(&self) -> Rules {
        self.ledger.rules().clone()
    }

    /// A quorum attests `operation`, in the next block.
    fn attest(&mut self, operation: &ValidatorOperation) {
        for i in 0..self.ledger.validators().quorum() {
            self.sequence += 1;
            self.mempool
                .push(Operation::Validator(SignedValidatorOperation::sign(
                    self.ledger.hub(),
                    ValidatorIndex::new(i).unwrap(),
                    self.sequence,
                    operation.clone(),
                    &self.keys[i],
                )));
        }
    }

    /// A deposit that opens an account for `key`.
    fn deposit(&mut self, key: &PublicKey, drops: u64) {
        self.deposits += 1;
        let deposit = Deposit {
            transaction: Digest([u8::try_from(self.deposits).unwrap(); 32]),
            ledger: 1_000 + self.deposits,
            amount: drops,
            sender: [0xaa; 20],
            tag: 0,
            key: Some(*key),
        };
        self.attest(&ValidatorOperation::AttestDeposit(deposit));
    }

    /// A deposit of `drops` to account `number`, by its tag.
    fn top_up(&mut self, number: AccountNumber, drops: u64) {
        self.deposits += 1;
        let deposit = Deposit {
            transaction: Digest([u8::try_from(self.deposits).unwrap(); 32]),
            ledger: 1_000 + self.deposits,
            amount: drops,
            sender: [0xaa; 20],
            tag: number.0,
            key: None,
        };
        self.attest(&ValidatorOperation::AttestDeposit(deposit));
    }

    /// The escrow's payment of withdrawal `sequence` delivered, or failed.
    fn settle_withdrawal(&mut self, sequence: u32, delivered: bool) {
        self.attest(&ValidatorOperation::AttestWithdrawal {
            escrow_sequence: sequence,
            delivered,
        });
    }

    /// Takes an operation for a block, once.
    fn submit(&mut self, operation: Operation) {
        let hash = operation.hash();
        if !self.executed.contains(&hash) && !self.mempool.contains(&operation) {
            self.mempool.push(operation);
        }
    }

    /// Forgets the history of every block so far, as validators that keep a
    /// bounded history do.
    fn trim(&mut self) {
        self.complete_from = self.ledger.height() + 1;
        self.log.clear();
    }

    /// Executes what waits, deadlines first, as far as a block takes.
    fn block(&mut self, timestamp: Millis) {
        let limits = self.ledger.terms().limits;
        let waiting = std::mem::take(&mut self.mempool);
        let (mut deadlines, mut others): (Vec<Operation>, Vec<Operation>) =
            waiting.into_iter().partition(Operation::has_deadline);
        let keep = usize::from(limits.deadline_operations);
        let free = usize::from(limits.block_operations - limits.deadline_operations);
        self.mempool
            .extend(deadlines.split_off(deadlines.len().min(keep)));
        self.mempool
            .extend(others.split_off(others.len().min(free)));
        let operations: Vec<Operation> = deadlines.into_iter().chain(others).collect();
        let height = self.ledger.height() + 1;
        let votes = if height > 1 {
            vec![(height - 1, Signers::from_bits(0b1111))]
        } else {
            vec![]
        };
        let block = Block {
            height,
            timestamp,
            operations: &operations,
            votes: &votes,
        };
        let outcomes = self.ledger.apply(&block).unwrap();
        self.stamps.insert(height, timestamp);
        for ((index, operation), outcome) in (0u32..).zip(operations).zip(outcomes) {
            if let (
                Operation::Member(signed),
                Outcome::Applied {
                    detail: Detail::Hold(hold),
                    ..
                },
            ) = (&operation, outcome)
                && let MemberOperation::HoldOpen { payee, .. } = signed.operation
            {
                self.parties.insert(hold, (signed.account, payee));
            }
            self.executed.insert(operation.hash());
            self.log.push(Entry {
                height,
                timestamp,
                index,
                operation,
                outcome,
            });
        }
    }

    /// Whether an executed operation touches `account`.
    fn touches(&self, account: AccountNumber, operation: &Operation) -> bool {
        let Operation::Member(signed) = operation else {
            return false;
        };
        let party = |hold: &HoldNumber| {
            self.parties
                .get(hold)
                .is_some_and(|(payer, payee)| *payer == account || *payee == account)
        };
        signed.account == account
            || match &signed.operation {
                MemberOperation::HoldOpen { payee, .. } => *payee == account,
                MemberOperation::Transfer { to, .. } => *to == account,
                MemberOperation::HoldFund { hold, .. }
                | MemberOperation::HoldReduce { hold, .. }
                | MemberOperation::HoldRelease { hold }
                | MemberOperation::Claim { hold, .. } => party(hold),
                MemberOperation::Redeem(claim) => party(&claim.value.hold),
                _ => false,
            }
    }

    /// Everything executed from `from` on that touches `account`, and the
    /// newest final block the answer covers: an answer too long for one
    /// message stops before a block.
    fn since(&self, account: AccountNumber, from: u64) -> Vec<Statement> {
        if from < self.complete_from {
            return vec![Statement::Trimmed {
                height: self.ledger.height(),
                below: self.complete_from,
            }];
        }
        let mut statements: Vec<Statement> = Vec::new();
        let mut covered = self.ledger.height();
        let touching = self
            .log
            .iter()
            .filter(|entry| entry.height >= from && self.touches(account, &entry.operation));
        for entry in touching {
            if statements.len() == MAX_STATEMENTS - 1 {
                covered = entry.height - 1;
                statements.retain(|s| s.height() <= covered);
                break;
            }
            statements.push(Statement::Executed {
                height: entry.height,
                timestamp: entry.timestamp,
                index: entry.index,
                operation: entry.operation.clone(),
                outcome: entry.outcome,
            });
        }
        statements.push(Statement::Final {
            height: covered,
            timestamp: self.stamps[&covered],
        });
        statements
    }

    /// Validator `index`'s answer to `request`, if any.
    fn answer(&mut self, index: usize, request: &[u8]) -> Option<Vec<u8>> {
        if self.faults[index] == Fault::Silent {
            return None;
        }
        let Message::Request(request) = Message::decode_exact(request).unwrap() else {
            panic!("a member sends only requests");
        };
        let statements = match request {
            Request::Submit(operation) => {
                self.submit(operation);
                return None;
            }
            Request::Account(key) => vec![Statement::account(&self.ledger, key)],
            Request::Hold(number) => vec![Statement::hold(&self.ledger, number)],
            Request::Since { account, from } => self.since(account, from),
            Request::Holds(account) => {
                let key = self.ledger.account(account)?.key;
                let mut statements = vec![Statement::account(&self.ledger, key)];
                statements.extend(Statement::holds(&self.ledger, account));
                statements
            }
        };
        let statements = if self.faults[index] == Fault::Liar {
            statements.into_iter().filter_map(lie).collect()
        } else {
            statements
        };
        let signed: Vec<SignedStatement> = statements
            .into_iter()
            .map(|statement| {
                SignedStatement::sign(
                    self.ledger.hub(),
                    ValidatorIndex::new(index).unwrap(),
                    statement,
                    &self.keys[index],
                )
            })
            .collect();
        Some(Message::Statements(signed).to_vec())
    }
}

/// What a lying validator says instead.
fn lie(statement: Statement) -> Option<Statement> {
    match statement {
        Statement::Final { height, timestamp } => Some(Statement::Final {
            height: height + 5,
            timestamp: timestamp + 1_000_000,
        }),
        Statement::Account {
            height,
            key,
            account: Some((number, mut view)),
        } => {
            view.balance += 1;
            Some(Statement::Account {
                height,
                key,
                account: Some((number, view)),
            })
        }
        Statement::Executed { .. } => None,
        other => Some(other),
    }
}

/// A member's books, as the node books what its scheme says.
#[derive(Debug, Default)]
struct Books {
    settlement: i128,
    owner: i128,
    suspense: i128,
    reserve: i128,
    fees: i128,
    /// Per peer: what it settled with us, less what we settled with it.
    peers: BTreeMap<Name, i128>,
    pending: BTreeMap<Name, (Name, u64)>,
    finalized: Vec<Name>,
    cancelled: Vec<Name>,
    losses: Vec<(Name, u64, LossCause)>,
    ids: BTreeSet<Name>,
    /// The owner's payments held while on their way, as the node holds
    /// them: by ID, what each pays.
    held: BTreeMap<Name, u64>,
}

impl Books {
    fn apply(&mut self, effect: Effect) {
        match effect {
            Effect::Deposit { amount } => {
                self.settlement += i128::from(amount);
                self.owner += i128::from(amount);
            }
            Effect::Withdrawal { amount } => {
                self.settlement -= i128::from(amount);
                self.owner -= i128::from(amount);
            }
            Effect::OwnerPaid { id, applied } => {
                let amount = self.held.remove(&id).unwrap();
                if applied {
                    self.settlement -= i128::from(amount);
                    self.owner -= i128::from(amount);
                }
            }
            Effect::IncomingSettlement { peer, amount, id } => {
                assert!(self.ids.insert(id.unwrap()), "a settlement booked once");
                self.settlement += i128::from(amount);
                *self.peers.entry(peer).or_default() += i128::from(amount);
            }
            Effect::Finalize { settlement } => {
                let (peer, amount) = self.pending.remove(&settlement).unwrap();
                self.settlement -= i128::from(amount);
                *self.peers.entry(peer).or_default() -= i128::from(amount);
                self.finalized.push(settlement);
            }
            Effect::Cancel { settlement } => {
                self.pending.remove(&settlement).unwrap();
                self.cancelled.push(settlement);
            }
            Effect::Fee { amount, id } => {
                assert!(self.ids.insert(id), "a fee booked once");
                self.settlement -= i128::from(amount);
                self.fees += i128::from(amount);
            }
            Effect::Reconcile {
                amount,
                gain,
                cause,
            } => {
                let amount = if gain {
                    i128::from(amount)
                } else {
                    -i128::from(amount)
                };
                self.settlement += amount;
                match cause {
                    ReconcileCause::Reserve => self.reserve += amount,
                    ReconcileCause::Unexplained => self.suspense += amount,
                }
            }
            Effect::PeerLoss {
                peer,
                amount,
                cause,
                id,
            } => {
                assert!(self.ids.insert(id), "a loss booked once");
                if cause == LossCause::ClaimFee {
                    self.settlement -= i128::from(amount);
                }
                self.losses.push((peer, amount, cause));
            }
            Effect::SendMessage { .. } => {}
        }
    }

    fn lost(&self, cause: LossCause) -> u64 {
        self.losses
            .iter()
            .filter(|(_, _, c)| *c == cause)
            .map(|(_, amount, _)| amount)
            .sum()
    }
}

struct Member {
    name: Name,
    key: Arc<NodeKey>,
    disbursal: Destination,
    account: HubAccount,
    scheme: HubScheme,
    books: Books,
    peer_states: BTreeMap<Name, Vec<u8>>,
    decided: Vec<Decided>,
    /// It reaches no validator: its timers do not run.
    offline: bool,
}

/// The hub and its members, in simulated time.
struct World {
    now: u64,
    next_block: u64,
    validators: Validators,
    members: Vec<Member>,
    rng: SeededRng,
}

fn scheme_name() -> Name {
    Name::new(SCHEME_ID).unwrap()
}

impl World {
    fn new(faults: [Fault; 4]) -> Self {
        Self {
            now: START + 10,
            next_block: START + 10 + BLOCK_MS,
            validators: Validators::found(faults),
            members: Vec::new(),
            rng: SeededRng::new(1),
        }
    }

    /// A member that deposits `xrp` for a new account.
    fn join(&mut self, name: &str, seed: u8, xrp: u64) -> usize {
        let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
        self.validators.deposit(&key.public_key(), xrp * XRP);
        let disbursal = Destination {
            account: [seed; 20],
            tag: None,
        };
        let account = HubAccount::new(
            FIRST,
            self.validators.rules(),
            Arc::clone(&key),
            disbursal,
            scheme_name(),
            &[],
        )
        .unwrap();
        let scheme = HubScheme::new(&account, P, None, &[]).unwrap();
        self.members.push(Member {
            name: Name::new(name).unwrap(),
            key,
            disbursal,
            account,
            scheme,
            books: Books::default(),
            peer_states: BTreeMap::new(),
            decided: Vec::new(),
            offline: false,
        });
        self.members.len() - 1
    }

    /// Moves time on by `ms`: blocks, and every member's timers.
    fn run(&mut self, ms: u64) {
        let until = self.now + ms;
        while self.now < until {
            self.now += STEP_MS;
            if self.now >= self.next_block {
                self.validators.block(self.now);
                self.next_block += BLOCK_MS;
            }
            for i in 0..self.members.len() {
                let due = !self.members[i].offline
                    && self.members[i]
                        .account
                        .deadline()
                        .is_some_and(|at| at <= self.now);
                if due {
                    self.members[i].account.on_timer(self.now);
                    self.heard(i);
                }
            }
        }
    }

    /// After member `i`'s account heard something: its scheme is told and
    /// books it, then what the account asks goes to the validators, whose
    /// answers come back at once.
    fn heard(&mut self, i: usize) {
        let now = self.now;
        loop {
            let member = &mut self.members[i];
            let mut effects = Vec::new();
            member
                .scheme
                .on_asset(now, &mut member.account, &mut effects);
            for effect in effects {
                member.books.apply(effect);
            }
            member.account.told();
            self.check(i);
            let member = &mut self.members[i];
            let mut asked = Vec::new();
            member.account.take_effects(&mut asked);
            if asked.is_empty() {
                return;
            }
            for effect in asked {
                let AssetEffect::SendTo { endpoint, message } = effect else {
                    panic!("a hub account only sends to its validators: {effect:?}");
                };
                if let Some(reply) = self.validators.answer(endpoint, &message) {
                    self.members[i].account.on_ledger(
                        now,
                        LedgerInput::From {
                            endpoint,
                            message: &reply,
                        },
                    );
                }
            }
        }
    }

    /// Member `i` holds together: its account, its scheme and its books.
    fn check(&mut self, i: usize) {
        let member = &mut self.members[i];
        member.account.check_invariants();
        member.scheme.check_invariants();
        assert_eq!(
            member.books.settlement,
            i128::from(member.scheme.balance()),
            "{}: the books hold what the scheme says",
            member.name
        );
        let mut decided = Vec::new();
        if let Some(commitments) = member.scheme.commitments() {
            commitments.take_decided(&mut decided);
        }
        member.decided.extend(decided);
    }

    fn number(&self, i: usize) -> AccountNumber {
        self.members[i].account.number().unwrap()
    }

    /// What member `i`'s account holds in the hub.
    fn balance(&self, i: usize) -> u64 {
        self.validators
            .ledger
            .account(self.number(i))
            .unwrap()
            .balance
    }

    /// The hold member `i` pays to `j`.
    fn hold(&self, i: usize, j: usize) -> Option<HoldNumber> {
        let (payer, payee) = (self.number(i), self.number(j));
        self.validators
            .ledger
            .holds()
            .find(|(_, hold)| hold.payer == payer && hold.payee == payee)
            .map(|(number, _)| number)
    }

    /// What member `i`'s books should hold: its balance and the holds
    /// toward it from its peers, as the ledger says.
    fn truth(&self, i: usize) -> u64 {
        let number = self.number(i);
        let toward: u64 = self
            .validators
            .ledger
            .holds()
            .filter(|(_, hold)| hold.payee == number)
            .map(|(_, hold)| hold.amount)
            .sum();
        units(self.balance(i) + toward)
    }

    /// `a` and `b` peer over the hub, `a` asking.
    fn peer(&mut self, a: usize, b: usize) {
        let (name_a, name_b) = (self.members[a].name, self.members[b].name);
        let mut info = Vec::new();
        self.members[b].scheme.peering_info(&mut info);
        let mut request = Vec::new();
        self.members[a]
            .scheme
            .create_peering_request(name_b, &info, &mut request)
            .unwrap();
        let mut response = Vec::new();
        let state_at_b = self.members[b]
            .scheme
            .accept_peering_request(name_a, &request, &mut response)
            .unwrap();
        let state_at_a = self.members[a]
            .scheme
            .finalize_peering_request(name_b, &info, &response)
            .unwrap();
        self.members[a].scheme.on_peered(name_b, &state_at_a);
        self.members[a].peer_states.insert(name_b, state_at_a);
        self.members[b].scheme.on_peered(name_a, &state_at_b);
        self.members[b].peer_states.insert(name_a, state_at_b);
    }

    /// `a` settles `amount` with `b`, as the node does when asked.
    fn settle(&mut self, a: usize, b: usize, amount: u64) -> Result<Name, SchemeError> {
        let peer = self.members[b].name;
        let member = &mut self.members[a];
        let state = member.peer_states[&peer].clone();
        let prepared = member.scheme.prepare_settlement(
            &mut self.rng,
            Some(&mut member.account),
            peer,
            amount,
            &state,
        )?;
        member
            .books
            .pending
            .insert(prepared.id, (peer, prepared.amount));
        let mut effects = Vec::new();
        member
            .scheme
            .execute_settlement(Some(&mut member.account), prepared.id, &mut effects);
        for effect in effects {
            member.books.apply(effect);
        }
        self.heard(a);
        Ok(prepared.id)
    }

    /// `a` sends `b` a Prepare of `amount` that expires in `expires_in`,
    /// with its commitment, which `b` checks. Returns the commitment.
    fn prepare(
        &mut self,
        (a, b): (usize, usize),
        request_id: u32,
        amount: u64,
        expires_in: u64,
    ) -> Result<Vec<u8>, SchemeError> {
        let (name_a, name_b) = (self.members[a].name, self.members[b].name);
        let promised = promised(request_id, amount, self.now + expires_in);
        let mut commitment = Vec::new();
        self.members[a].scheme.commitments().unwrap().commit(
            self.now,
            name_b,
            request_id,
            promised,
            &mut commitment,
        )?;
        self.members[b]
            .scheme
            .commitments()
            .unwrap()
            .check_commitment(self.now, name_a, request_id, promised, &commitment)?;
        Ok(commitment)
    }

    /// `b` fulfills `a`'s Prepare; `a` takes the Fulfill if it arrives, and
    /// its claim reaches `b` if `claim_arrives`.
    fn fulfill(
        &mut self,
        (a, b): (usize, usize),
        request_id: u32,
        arrives: bool,
        claim_arrives: bool,
    ) {
        let (name_a, name_b) = (self.members[a].name, self.members[b].name);
        let now = self.now;
        self.members[b].scheme.commitments().unwrap().on_fulfilled(
            now,
            name_a,
            request_id,
            preimage(request_id),
        );
        if !arrives {
            return;
        }
        let mut claim = Vec::new();
        self.members[a]
            .scheme
            .commitments()
            .unwrap()
            .claim(now, name_b, request_id, true, &mut claim);
        if claim_arrives {
            self.members[b]
                .scheme
                .commitments()
                .unwrap()
                .on_claim(now, name_a, request_id, &claim);
        }
    }

    /// `b` rejects `a`'s Prepare, and `a` takes the Reject.
    fn reject(&mut self, (a, b): (usize, usize), request_id: u32) {
        let (name_a, name_b) = (self.members[a].name, self.members[b].name);
        self.members[b]
            .scheme
            .commitments()
            .unwrap()
            .on_released(name_a, request_id);
        self.members[a]
            .scheme
            .commitments()
            .unwrap()
            .on_answered(name_b, request_id);
    }

    /// `a`'s Prepare to `b` timed out: its Reject upstream waits for the
    /// hub.
    fn wait(&mut self, (a, b): (usize, usize), request_id: u32, backstop: u64) {
        let name_b = self.members[b].name;
        let condition = condition(request_id);
        let now = self.now;
        self.members[a]
            .scheme
            .commitments()
            .unwrap()
            .on_hub_wait(now, name_b, request_id, condition, backstop);
        self.check(a);
    }

    /// Member `i` submits `operation` itself, with its key and the
    /// account's sequence as the hub has it.
    fn raw(&mut self, i: usize, operation: MemberOperation) {
        let number = self.number(i);
        let sequence = self.validators.ledger.account(number).unwrap().sequence;
        let signed = SignedMemberOperation::sign(
            self.validators.ledger.hub(),
            number,
            sequence,
            FEE,
            operation,
            &self.members[i].key,
        );
        self.validators.submit(Operation::Member(signed));
    }

    /// After a restart, the node hands member `i`'s scheme the settlements
    /// its books still hold pending.
    fn reconcile(&mut self, i: usize, pending: &[(Name, u64)]) {
        let member = &mut self.members[i];
        let mut effects = Vec::new();
        member
            .scheme
            .reconcile(Some(&mut member.account), pending, &mut effects);
        for effect in effects {
            member.books.apply(effect);
        }
        self.heard(i);
    }

    /// Makes member `i` again from what its account and scheme keep.
    fn restart(&mut self, i: usize) {
        let rules = self.validators.rules();
        let member = &mut self.members[i];
        let account_state = member.account.state();
        let scheme_state = member.scheme.state();
        let balance = member.scheme.balance();
        member.account = HubAccount::new(
            FIRST,
            rules,
            Arc::clone(&member.key),
            member.disbursal,
            scheme_name(),
            &account_state,
        )
        .unwrap();
        member.scheme = HubScheme::new(&member.account, P, Some(balance), &scheme_state).unwrap();
        assert_eq!(
            member.account.state(),
            account_state,
            "the account's state round-trips"
        );
        assert_eq!(
            member.scheme.state(),
            scheme_state,
            "the scheme's state round-trips"
        );
    }

    /// Two members with 100 XRP each in the hub, `a` peered with `b` and
    /// a hold of `hold` XRP toward `b`.
    fn pair(faults: [Fault; 4], hold: u64) -> (Self, usize, usize) {
        let mut world = Self::new(faults);
        let a = world.join("alice", 31, 100);
        let b = world.join("bob", 32, 100);
        world.run(3_000);
        for i in [a, b] {
            assert!(world.members[i].scheme.has_synced(), "{i} synced");
            assert_eq!(world.balance(i), 100 * XRP - ACCOUNT_RESERVE - FEE);
        }
        world.peer(a, b);
        world.settle(a, b, units(hold * XRP)).unwrap();
        world.run(1_000);
        (world, a, b)
    }
}

fn preimage(request_id: u32) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[..4].copy_from_slice(&request_id.to_be_bytes());
    bytes
}

fn condition(request_id: u32) -> [u8; 32] {
    Sha256::digest(preimage(request_id)).into()
}

fn promised(request_id: u32, amount: u64, expires_at_ms: u64) -> Promised {
    Promised {
        amount,
        condition: condition(request_id),
        expires_at_ms,
    }
}

fn signed_commitment(bytes: &[u8]) -> Signed<Commitment> {
    let mut reader = Reader::new(bytes);
    let signed = Signed::<Commitment>::decode(&mut reader).unwrap();
    reader.finish().unwrap();
    signed
}

/// Opening a hold is `a`'s outgoing settlement and `b`'s incoming one;
/// the hold's reserve is the ledger's, never booked.
#[test]
fn a_hold_is_a_settlement_on_both_sides() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let books = &world.members[a].books;
    assert_eq!(books.finalized.len(), 1);
    assert_eq!(
        books.peers[&world.members[b].name],
        -i128::from(units(2 * XRP))
    );
    assert_eq!(books.reserve, -i128::from(units(HOLD_RESERVE)));
    let books = &world.members[b].books;
    assert_eq!(
        books.peers[&world.members[a].name],
        i128::from(units(2 * XRP))
    );
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0, "everything explained");
    }
    assert_eq!(
        world.balance(a),
        98 * XRP - ACCOUNT_RESERVE - HOLD_RESERVE - 2 * FEE
    );
    // Each side reports the hold, which the node's credit follows
    // (spec/areas/hubs.md#in-the-node), and the hub it is a member of.
    let (name_a, name_b) = (world.members[a].name, world.members[b].name);
    let holds = |world: &mut World, i: usize, peer: Name| {
        world.members[i].scheme.commitments().unwrap().holds(peer)
    };
    assert_eq!(
        holds(&mut world, a, name_b),
        dassie_settlement::HoldsWith {
            toward: units(2 * XRP),
            from: 0
        }
    );
    assert_eq!(
        holds(&mut world, b, name_a),
        dassie_settlement::HoldsWith {
            toward: 0,
            from: units(2 * XRP)
        }
    );
    for i in [a, b] {
        let membership = world.members[i]
            .scheme
            .commitments()
            .unwrap()
            .membership()
            .unwrap();
        assert_eq!(
            membership.account,
            world.members[i].account.number().unwrap().0
        );
    }
}

/// `a` pays `b` across the hold; `b` takes its claims and redeems them
/// once they add up to the threshold.
fn pays_across_the_hold(faults: [Fault; 4]) {
    let (mut world, a, b) = World::pair(faults, 2);
    let hold = world.hold(a, b).unwrap();
    for request_id in 1..=10 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, true);
        world.run(30);
    }
    world.run(2_000);
    let redeemed = world.validators.ledger.hold(hold).unwrap().total;
    assert!(redeemed >= XRP - 2 * P / 1_000, "redeemed {redeemed}");
    assert!(redeemed <= XRP);
    // What the payee took left the hold, as both sides see it.
    let (name_a, name_b) = (world.members[a].name, world.members[b].name);
    let left = units(2 * XRP - redeemed);
    let toward = world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .holds(name_b)
        .toward;
    let from = world.members[b]
        .scheme
        .commitments()
        .unwrap()
        .holds(name_a)
        .from;
    assert_eq!((toward, from), (left, left));
    // Its fees: setting its disbursal address, and each redeem.
    let redeems = world.members[b].books.fees / i128::from(units(FEE)) - 1;
    assert!(
        (1..10).contains(&redeems),
        "{redeems} redeems for ten packets"
    );
    assert_eq!(
        world.balance(b),
        100 * XRP - ACCOUNT_RESERVE - FEE + redeemed - FEE * u64::try_from(redeems).unwrap()
    );
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0, "everything explained");
        assert!(world.members[i].books.losses.is_empty());
    }
}

#[test]
fn a_payer_pays_across_its_hold_and_the_payee_redeems() {
    pays_across_the_hold([Fault::Honest; 4]);
}

/// With one validator silent, a quorum still covers every block.
#[test]
fn a_silent_validator_changes_nothing() {
    pays_across_the_hold([Fault::Honest, Fault::Honest, Fault::Honest, Fault::Silent]);
}

/// One validator that lies about what it covered, what blocks were stamped
/// and what the account holds, and leaves out every operation, is outvoted.
#[test]
fn a_lying_validator_changes_nothing() {
    pays_across_the_hold([Fault::Honest, Fault::Liar, Fault::Honest, Fault::Honest]);
}

/// The payee asks, and the payer funds the hold by what it is asked; the
/// payee books it once agreed statements show it.
#[test]
fn settling_again_funds_the_hold() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    world.settle(a, b, units(XRP) + 999).unwrap();
    world.run(1_000);
    let hold = world.hold(a, b).unwrap();
    assert_eq!(
        world.validators.ledger.hold(hold).unwrap().amount,
        3 * XRP,
        "whole drops"
    );
    assert_eq!(
        world.members[b].books.peers[&world.members[a].name],
        i128::from(units(3 * XRP))
    );
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// A payer that sends no claims: the payee claims on the hub as each
/// claim is late, books the claims' fees as the payer's doing, and still
/// takes the payer's later claim without being paid twice.
#[test]
fn a_payee_claims_on_the_hub_what_claims_left_out() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    let before = world.balance(b);
    for request_id in 1..=3 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, false);
    }
    world.run(2_000);
    let ledger_hold = world.validators.ledger.hold(hold).unwrap();
    assert_eq!(ledger_hold.claimed.len(), 3, "claimed on the hub");
    let claim_fees = world.members[b].books.lost(LossCause::ClaimFee);
    assert!(claim_fees >= units(FEE));
    assert_eq!(
        world.balance(b),
        before + 3 * P / 1_000 - claim_fees / 1_000
    );
    // The payer's next claims cover all three, and four more: redeeming
    // them pays only the four.
    for request_id in 4..=7 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, true);
    }
    world.run(1_000);
    let ledger_hold = world.validators.ledger.hold(hold).unwrap();
    assert_eq!(ledger_hold.total, 7 * P / 1_000, "redeemed through seven");
    assert!(ledger_hold.claimed.is_empty());
    assert_eq!(
        ledger_hold.amount,
        2 * XRP - 7 * P / 1_000,
        "paid once each"
    );
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
    assert!(
        world.members[a].books.losses.is_empty(),
        "the payer owed all four"
    );
}

/// spec/designs/settlement-hubs.md#4-peering-over-a-hub: the payer's
/// Reject of a packet that timed out waits for the hub, which decides it:
/// with the fulfillment of a claim the payee won, or without, once a final
/// block is stamped at or after the commitment's expiry.
#[test]
fn a_deferred_reject_is_decided_from_the_hub() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    // 1: the payee fulfills, and its Fulfill never reaches the payer.
    world.prepare((a, b), 1, P, 2_000).unwrap();
    world.fulfill((a, b), 1, false, false);
    // 2: the payee never answers.
    world.prepare((a, b), 2, P, 2_000).unwrap();
    world.run(2_000);
    world.wait((a, b), 1, world.now + 1_000);
    world.wait((a, b), 2, world.now + 1_000);
    world.run(500);
    let name_b = world.members[b].name;
    let decided = &world.members[a].decided;
    assert!(decided.contains(&Decided {
        peer: name_b,
        request_id: 1,
        fulfillment: Some(preimage(1)),
    }));
    assert!(decided.contains(&Decided {
        peer: name_b,
        request_id: 2,
        fulfillment: None,
    }));
    assert_eq!(decided.len(), 2);
    // The node fulfills the first upstream, and is done with both.
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_answered(name_b, 1);
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_answered(name_b, 2);
    let name_a = world.members[a].name;
    world.members[b]
        .scheme
        .commitments()
        .unwrap()
        .on_released(name_a, 2);
    world.run(500);
    assert!(world.members[a].books.losses.is_empty(), "decided in time");
}

/// A packet the payer gave up on, which the payee then won on the hub, is
/// a loss on the payee; one the payee rejected and then claimed is
/// evidence of cheating. A payee that claims twice, or late, gains
/// nothing.
#[test]
fn what_a_payee_takes_behind_its_answers_is_booked_against_it() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    // 1: the payee rejects it, then claims it anyway.
    let first = signed_commitment(&world.prepare((a, b), 1, P, 10_000).unwrap());
    world.reject((a, b), 1);
    let claim_one = |c: Signed<Commitment>, request_id: u32| MemberOperation::Claim {
        hold,
        fulfilled: vec![Fulfilled {
            commitment: c,
            fulfillment: preimage(request_id),
        }],
    };
    world.raw(b, claim_one(first, 1));
    world.run(300);
    let cheated = world.members[a].books.lost(LossCause::Cheated);
    assert_eq!(cheated, P, "a packet's worth, counted against the peer");
    let paid_once = world.balance(b);
    // Twice: the hub remembers what it paid.
    world.raw(b, claim_one(first, 1));
    world.run(300);
    assert_eq!(world.balance(b), paid_once - FEE, "only the fee");
    // 2: the payer waits for the hub and gives up before the payee claims.
    let second = signed_commitment(&world.prepare((a, b), 2, P, 1_000).unwrap());
    world.run(1_000);
    world.wait((a, b), 2, world.now);
    let name_b = world.members[b].name;
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_answered(name_b, 2);
    // Late: after the commitment expired, the hub pays nothing.
    world.raw(b, claim_one(second, 2));
    world.run(300);
    assert_eq!(
        world.balance(b),
        paid_once - 2 * FEE,
        "nothing for a late claim"
    );
    assert_eq!(world.members[a].books.lost(LossCause::PaidLate), 0);
    assert_eq!(
        world.validators.ledger.hold(hold).unwrap().amount,
        2 * XRP - P / 1_000
    );
    for i in [a, b] {
        world.members[i].account.check_invariants();
        world.members[i].scheme.check_invariants();
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
}

/// An operation refused for its sequence is signed again with the one the
/// hub says, and applies.
#[test]
fn a_refused_sequence_is_learnt_again() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let to = world.number(b);
    world.raw(a, MemberOperation::Transfer { to, amount: 5 });
    let settlement = world.settle(a, b, units(XRP)).unwrap();
    world.run(3_000);
    assert!(world.members[a].books.finalized.contains(&settlement));
    let hold = world.hold(a, b).unwrap();
    assert_eq!(world.validators.ledger.hold(hold).unwrap().amount, 3 * XRP);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
}

/// A member made again from what it keeps carries on: its holds, its
/// sequences and what it is owed.
#[test]
fn a_member_carries_on_after_a_restart() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    world.prepare((a, b), 1, P, 10_000).unwrap();
    world.fulfill((a, b), 1, true, false);
    world.restart(a);
    world.restart(b);
    world.run(2_000);
    // The payee claimed on the hub after its restart, and the payer's next
    // packet takes the next sequence.
    let hold = world.hold(a, b).unwrap();
    assert_eq!(world.validators.ledger.hold(hold).unwrap().claimed.len(), 1);
    let next = signed_commitment(&world.prepare((a, b), 2, P, 10_000).unwrap());
    assert_eq!(next.value.sequence, 2);
    world.fulfill((a, b), 2, true, true);
    world.settle(a, b, units(XRP)).unwrap();
    world.run(1_000);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// A settlement the hub fails is cancelled, and its fee booked.
#[test]
fn a_settlement_the_hub_fails_is_cancelled() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let to = world.number(b);
    let drain = world.balance(a) - FEE - 15;
    world.raw(a, MemberOperation::Transfer { to, amount: drain });
    let settlement = world.settle(a, b, units(XRP)).unwrap();
    world.run(3_000);
    assert_eq!(world.members[a].books.cancelled, [settlement]);
    assert_eq!(world.balance(a), 5, "the fee of the settlement that failed");
    let hold = world.hold(a, b).unwrap();
    assert_eq!(world.validators.ledger.hold(hold).unwrap().amount, 2 * XRP);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
}

/// After a restart, a settlement the account still has goes on and is
/// finalized; one it never had was never sent, and is cancelled.
#[test]
fn settlements_pending_at_a_restart_are_reconciled() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let settlement = world.settle(a, b, units(XRP)).unwrap();
    world.restart(a);
    let lost = Name::new("00112233445566778899aabbccddeeff").unwrap();
    let name_b = world.members[b].name;
    world.members[a]
        .books
        .pending
        .insert(lost, (name_b, units(XRP)));
    world.reconcile(a, &[(settlement, units(XRP)), (lost, units(XRP))]);
    assert_eq!(world.members[a].books.cancelled, [lost]);
    world.run(1_000);
    assert!(world.members[a].books.finalized.contains(&settlement));
    let hold = world.hold(a, b).unwrap();
    assert_eq!(world.validators.ledger.hold(hold).unwrap().amount, 3 * XRP);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// A hold covers only what is in it: the payer commits nothing more, and
/// the payee takes nothing more.
#[test]
fn nothing_goes_beyond_the_hold() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 1);
    for request_id in 1..=10 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
    }
    assert_eq!(
        world.prepare((a, b), 11, 1_000, 10_000),
        Err(SchemeError::Unavailable),
        "the payer's room is used up"
    );
    // A payee is not fooled by a payer that commits beyond its room.
    let name_a = world.members[a].name;
    let promised = promised(12, 1_000, world.now + 10_000);
    let forged = Signed::sign(
        Commitment {
            hold: world.hold(a, b).unwrap(),
            sequence: 99,
            amount: 1,
            condition: promised.condition,
            expiry: promised.expires_at_ms - MARGIN,
        },
        world.validators.ledger.hub(),
        &world.members[a].key,
    );
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&dassie_oer::Encode::to_vec(&forged));
    assert_eq!(
        world.members[b]
            .scheme
            .commitments()
            .unwrap()
            .check_commitment(world.now, name_a, 12, promised, &bytes),
        Err(SchemeError::Unavailable)
    );
}

/// A payee offline past what the validators keep reads its state anew,
/// and books what its payer added to its hold meanwhile as the payer's
/// settlement; the peering carries on.
#[test]
fn a_payee_offline_past_the_history_books_what_its_hold_grew() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    world.members[b].offline = true;
    world.settle(a, b, units(XRP)).unwrap();
    world.run(1_000);
    world.validators.trim();
    let trimmed = world.validators.ledger.height();
    world.members[b].offline = false;
    world.run(2_000);
    assert!(world.members[b].account.cursor() >= trimmed);
    assert!(!world.members[b].account.is_resyncing());
    let name_a = world.members[a].name;
    assert_eq!(
        world.members[b].books.peers[&name_a],
        i128::from(units(3 * XRP))
    );
    // It reads in order again: the next packets go through, and their
    // claims are redeemed.
    for request_id in 1..=3 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, true);
    }
    world.run(1_000);
    let hold = world.hold(a, b).unwrap();
    assert_eq!(
        world.validators.ledger.hold(hold).unwrap().total,
        3 * P / 1_000
    );
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0, "everything explained");
    }
}

/// A payer offline past what the validators keep, while the payee won a
/// claim on the hub for a packet the payer gave up on, books what the hub
/// paid as a loss once it reads its holds anew, and counts it in its
/// claims, so the payee still takes them.
#[test]
fn a_payer_offline_past_the_history_books_what_the_hub_paid() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    world.prepare((a, b), 1, P, 5_000).unwrap();
    world.fulfill((a, b), 1, false, false);
    world.members[a].offline = true;
    world.run(5_000);
    let hold = world.hold(a, b).unwrap();
    assert_eq!(world.validators.ledger.hold(hold).unwrap().claimed.len(), 1);
    // Its node gave up waiting for the hub.
    world.wait((a, b), 1, world.now);
    let name_b = world.members[b].name;
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_answered(name_b, 1);
    world.validators.trim();
    world.members[a].offline = false;
    world.run(2_000);
    assert_eq!(world.members[a].books.lost(LossCause::PaidLate), P);
    assert!(world.members[a].decided.is_empty(), "it gave up before");
    for request_id in 2..=3 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, true);
    }
    world.run(1_000);
    let ledger_hold = world.validators.ledger.hold(hold).unwrap();
    assert_eq!(ledger_hold.total, 3 * P / 1_000, "the payee took its claim");
    assert_eq!(
        ledger_hold.amount,
        2 * XRP - 3 * P / 1_000,
        "paid once each"
    );
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0, "everything explained");
    }
}

/// A hold opened toward a payee while it could not read its history is
/// found when it reads its holds anew, booked as the payer's settlement,
/// and taken once the hub confirms the key the payer gave at peering.
#[test]
fn a_hold_opened_while_the_payee_was_away_is_found() {
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let b = world.join("bob", 32, 100);
    world.run(3_000);
    world.peer(a, b);
    world.members[b].offline = true;
    world.settle(a, b, units(2 * XRP)).unwrap();
    world.run(1_000);
    world.validators.trim();
    world.members[b].offline = false;
    world.run(2_000);
    let name_a = world.members[a].name;
    assert_eq!(
        world.members[b].books.peers[&name_a],
        i128::from(units(2 * XRP))
    );
    world.prepare((a, b), 1, P, 10_000).unwrap();
    world.fulfill((a, b), 1, true, true);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// A member of another hub is refused as a peer.
#[test]
fn a_peer_in_another_hub_is_refused() {
    let (mut world, a, _) = World::pair([Fault::Honest; 4], 1);
    let mut info = Vec::new();
    world.members[a].scheme.peering_info(&mut info);
    info[0] ^= 1;
    let mut out = Vec::new();
    let name = Name::new("mallory").unwrap();
    assert_eq!(
        world.members[a]
            .scheme
            .create_peering_request(name, &info, &mut out),
        Err(SchemeError::InvalidData)
    );
    assert_eq!(
        world.members[a]
            .scheme
            .accept_peering_request(name, &info, &mut out),
        Err(SchemeError::InvalidData)
    );
}

/// The payee releases the payer's hold; the next settlement opens another,
/// whose records start at nothing on both sides, as the hub starts it: a
/// packet fulfilled on it is claimed with T from 0, and a redeem pays only
/// what was owed on it.
#[test]
fn a_hold_opened_again_starts_its_records_anew() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let first = world.hold(a, b).unwrap();
    for request_id in 1..=3 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, true);
    }
    world.run(1_000);
    world.raw(b, MemberOperation::HoldRelease { hold: first });
    world.run(1_000);
    assert!(world.validators.ledger.hold(first).is_none(), "released");
    world.settle(a, b, units(2 * XRP)).unwrap();
    world.run(1_000);
    let second = world.hold(a, b).unwrap();
    assert_ne!(second, first);
    for request_id in 4..=6 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, true);
    }
    world.run(2_000);
    let hold = world.validators.ledger.hold(second).unwrap();
    assert_eq!(hold.total, 3 * P / 1_000, "only what was owed on it");
    assert_eq!(hold.amount, 2 * XRP - 3 * P / 1_000);
    assert!(world.members[a].books.losses.is_empty());
}

/// A peer that comes back with another key for its account, or another
/// account, is refused while anything of its old membership is unsettled:
/// here, a hold with funds and packets fulfilled that no claim covers.
#[test]
fn peering_again_as_another_waits_for_the_old_to_settle() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    for request_id in 1..=2 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, false);
    }
    let hub = *world.validators.ledger.hub();
    let membership = |account: AccountNumber, key: PublicKey| {
        [
            hub.0.as_bytes().as_slice(),
            &account.0.to_be_bytes(),
            key.as_bytes(),
        ]
        .concat()
    };
    let name_a = world.members[a].name;
    let other_key = NodeKey::from_bytes(&[77; 32]).public_key();
    let own_key = world.members[a].key.public_key();
    let mut out = Vec::new();
    for bytes in [
        membership(world.number(a), other_key),
        membership(AccountNumber(99), own_key),
    ] {
        let scheme = &mut world.members[b].scheme;
        assert_eq!(
            scheme.accept_peering_request(name_a, &bytes, &mut out),
            Err(SchemeError::Unavailable)
        );
        assert_eq!(
            scheme.finalize_peering_request(name_a, &bytes, &[]),
            Err(SchemeError::Unavailable)
        );
    }
    // As before, it may.
    let same = membership(world.number(a), own_key);
    assert!(
        world.members[b]
            .scheme
            .accept_peering_request(name_a, &same, &mut out)
            .is_ok()
    );
}

/// A member asked to settle funds its hold with at most what its account
/// can spend: what is left once the operation's fee is set aside
/// (spec/areas/hubs.md#in-the-node).
#[test]
fn a_hold_is_funded_with_at_most_what_the_account_can_spend() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let spendable = world.balance(a) - FEE;
    let settlement = world.settle(a, b, units(1_000 * XRP)).unwrap();
    assert_eq!(
        world.members[a].books.pending[&settlement].1,
        units(spendable)
    );
    world.run(1_000);
    assert!(world.members[a].books.finalized.contains(&settlement));
    let hold = world.hold(a, b).unwrap();
    assert_eq!(
        world.validators.ledger.hold(hold).unwrap().amount,
        2 * XRP + spendable
    );
    assert_eq!(world.balance(a), 0);
}

/// The owner's deposits and withdrawals are booked as theirs, from how far
/// the account's counters grew, never through suspense
/// (spec/areas/hubs.md#the-members-side): a deposit by the account's tag, a
/// withdrawal, and a withdrawal the escrow could not pay, which comes back
/// as a deposit. The fee of the operation is a fee.
#[test]
fn deposits_and_withdrawals_are_the_owners() {
    let (mut world, a, _) = World::pair([Fault::Honest; 4], 2);
    let owner = world.members[a].books.owner;
    let number = world.number(a);
    world.validators.top_up(number, 7 * XRP);
    world.run(3_000);
    assert_eq!(
        world.members[a].books.owner,
        owner + i128::from(units(7 * XRP))
    );
    let to = Destination {
        account: [9; 20],
        tag: None,
    };
    let withdrawal = |amount| MemberOperation::Withdraw { amount, to };
    world.raw(a, withdrawal(3 * XRP));
    world.run(3_000);
    let ledger_fee = world.validators.ledger.terms().fees.ledger;
    let taken = i128::from(units(3 * XRP + ledger_fee));
    assert_eq!(
        world.members[a].books.owner,
        owner + i128::from(units(7 * XRP)) - taken
    );
    let (sequence, _) = world.validators.ledger.withdrawals().next().unwrap();
    world.validators.settle_withdrawal(sequence, true);
    world.raw(a, withdrawal(2 * XRP));
    world.run(3_000);
    let (sequence, _) = world.validators.ledger.withdrawals().next().unwrap();
    world.validators.settle_withdrawal(sequence, false);
    world.run(3_000);
    let back = i128::from(units(2 * XRP));
    let books = &world.members[a].books;
    assert_eq!(
        books.owner,
        owner + i128::from(units(7 * XRP)) - taken - (back + i128::from(units(ledger_fee))) + back
    );
    assert_eq!(books.suspense, 0, "nothing left to explain");
    assert_eq!(books.reserve, -i128::from(units(HOLD_RESERVE)));
    assert_eq!(world.members[a].scheme.balance(), world.truth(a));
    let account = world.validators.ledger.account(number).unwrap();
    assert_eq!(account.deposited, 100 * XRP + 7 * XRP);
    assert_eq!(account.returned, 2 * XRP, "the withdrawal that came back");
    assert_eq!(account.withdrawn, 5 * XRP + 2 * ledger_fee);
    world.restart(a);
    world.run(3_000);
    assert_eq!(world.members[a].books.suspense, 0, "the counters are kept");
}

/// The owner's withdrawal through the scheme (`pay_owner`): one at a time,
/// at least the hub's least payout, to an address the escrow can pay. Once
/// the hub executed it, what it paid out is booked as the owner's
/// withdrawal, and the operation's fee and the ledger's as fees; neither
/// twice across a restart, nor the counter's growth as a withdrawal.
#[test]
fn the_owner_withdraws_through_the_scheme() {
    use dassie_settlement::{OwnerPayment, OwnerPaymentState};

    let (mut world, a, _) = World::pair([Fault::Honest; 4], 2);
    let owner = world.members[a].books.owner;
    let fees = world.members[a].books.fees;
    let payment = |destination: [u8; 20], drops: u64| OwnerPayment {
        destination: dassie_xrpl::AccountId(destination).to_address(),
        tag: Some(3),
        memo: None,
        amount: units(drops),
    };
    let escrow = world.validators.ledger.terms().escrow;
    let member = &mut world.members[a];
    for refused in [payment(escrow, 2 * XRP), payment([9; 20], XRP / 2)] {
        assert_eq!(
            member.scheme.pay_owner(Some(&mut member.account), &refused),
            Err(SchemeError::InvalidData)
        );
    }
    let id = member
        .scheme
        .pay_owner(Some(&mut member.account), &payment([9; 20], 2 * XRP))
        .unwrap();
    member.books.held.insert(id, units(2 * XRP));
    assert_eq!(member.scheme.owner_payment(id), OwnerPaymentState::InFlight);
    assert_eq!(
        member
            .scheme
            .pay_owner(Some(&mut member.account), &payment([9; 20], 2 * XRP)),
        Err(SchemeError::Unavailable),
        "one at a time"
    );
    world.heard(a);
    // Executed, and then a restart before an agreed state shows what it
    // took: its hold is posted and its ledger fee booked, once, the counter
    // still to show them.
    world.run(500);
    let member = &world.members[a];
    assert_eq!(member.scheme.owner_payment(id), OwnerPaymentState::Applied);
    assert!(member.books.held.is_empty(), "the hold is posted");
    assert_eq!(member.books.owner, owner - i128::from(units(2 * XRP)));
    world.restart(a);
    world.run(3_000);
    let member = &world.members[a];
    assert_eq!(member.scheme.owner_payment(id), OwnerPaymentState::Applied);
    let ledger_fee = world.validators.ledger.terms().fees.ledger;
    assert_eq!(member.books.owner, owner - i128::from(units(2 * XRP)));
    assert_eq!(
        member.books.fees,
        fees + i128::from(units(FEE + ledger_fee))
    );
    assert_eq!(member.books.suspense, 0);
    let (_, withdrawal) = world.validators.ledger.withdrawals().next().unwrap();
    assert_eq!(
        withdrawal.to,
        Destination {
            account: [9; 20],
            tag: Some(3)
        }
    );
    assert_eq!(member.scheme.balance(), world.truth(a));
}

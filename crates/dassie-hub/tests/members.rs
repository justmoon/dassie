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
//!
//! A token hub's validators also attest its issuer's freezes and thaws, as
//! a quorum's `AttestIssuer` (spec/decisions/token-hubs.md), and answer
//! with the issuer's hold on the escrow: members pay each other until a
//! freeze, take no packet over it while it lasts, still claim what they
//! took before it, and carry on after the thaw.

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
use dassie_hub::ledger::{Block, Detail, Outcome, Refusal, TESTNET_NETWORK};
use dassie_hub::member::{ACCOUNT_ID, HubAccount, HubScheme, SCHEME_ID, units};
use dassie_hub::operation::{
    AccountNumber, Deposit, Destination, Issuer, MemberOperation, Operation, SignedMemberOperation,
    SignedValidatorOperation, ValidatorOperation,
};
use dassie_hub::promise::{Commitment, Fulfilled, HoldNumber, Release, Signed};
use dassie_hub::rules::{Asset, Fees, LEAST_TOKEN_GAS_DROPS, Limits, Rules, Terms, Timers};
use dassie_hub::service::wire::Message;
use dassie_hub::validators::{Signers, ValidatorIndex};
use dassie_hub::{Digest, Ledger, Millis};
use dassie_identity::{IdentityKey, NodeKey, PublicKey};
use dassie_kit::sim::SeededRng;
use dassie_ledger::Name;
use dassie_oer::{Encode, Reader};
use dassie_settlement::{
    AssetEffect, Decided, Effect, ExternalAsset, LedgerInput, LedgerStatus, LossCause, Promised,
    ReconcileCause, SchemeError, SettlementScheme,
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
const MARGIN: Millis = 600;

/// A token hub's asset: DSUSD.
const DSUSD: Asset = Asset::Token {
    currency: dassie_xrpl::dsusd::CURRENCY,
    issuer: dassie_xrpl::dsusd::ISSUER,
};

fn terms(sponsor: &IdentityKey, escrow: [u8; 20], asset: Asset, version: u16) -> Terms {
    let token = asset != Asset::Xrp;
    Terms {
        version,
        nonce: NONCE,
        sponsor: sponsor.identity(),
        network: 1,
        escrow,
        asset,
        fees: Fees {
            operation: FEE,
            ledger: if token { 0 } else { 100 },
            gas: if token { LEAST_TOKEN_GAS_DROPS } else { 0 },
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
            margin: MARGIN,
            allowance: 300,
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
    /// The XRP Ledger ledger of the issuer's last hold a quorum attested.
    issuer_ledger: u32,
}

impl Validators {
    /// The sponsor's hub of `asset`, of `version` of the hub's protocol,
    /// with its escrow attested.
    fn found(faults: [Fault; 4], asset: Asset, version: u16) -> Self {
        let sponsor = IdentityKey::from_bytes(&[100; 32]);
        let master = escrow_master(&sponsor, TESTNET_NETWORK, &NONCE);
        let terms = terms(&sponsor, master.account().0, asset, version);
        let nodes: Vec<IdentityKey> = (1..=4).map(|i| IdentityKey::from_bytes(&[i; 32])).collect();
        let keys: Vec<ValidatorKeys> = nodes
            .iter()
            .map(|node| ValidatorKeys::derive(node, TESTNET_NETWORK, &NONCE).unwrap())
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
            issuer_ledger: 0,
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

    /// The issuer freezes the escrow's line, or thaws it, as a quorum
    /// attests it in the next block.
    fn freeze(&mut self, frozen: bool) {
        self.issuer_ledger += 1;
        self.attest(&ValidatorOperation::AttestIssuer(Issuer {
            ledger: 2_000 + self.issuer_ledger,
            frozen,
            fee: false,
        }));
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
        self.ledger.check_invariants();
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
            // One a frozen hub refused used no sequence: it may come again
            // after the thaw, and apply then.
            if outcome.used_sequence() {
                self.executed.insert(operation.hash());
            }
            self.log.push(Entry {
                height,
                timestamp,
                index,
                operation,
                outcome,
            });
        }
    }

    /// Whether an executed operation touches `account`: a `pay` touches
    /// the account it opened.
    fn touches(&self, account: AccountNumber, entry: &Entry) -> bool {
        let Operation::Member(signed) = &entry.operation else {
            return false;
        };
        if let (
            MemberOperation::Pay { .. },
            Outcome::Applied {
                detail: Detail::Account(opened),
                ..
            },
        ) = (&signed.operation, entry.outcome)
            && opened == account
        {
            return true;
        }
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
                | MemberOperation::Claim { hold, .. }
                | MemberOperation::Dispute { hold, .. } => party(hold),
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
            .filter(|entry| entry.height >= from && self.touches(account, entry));
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
        // A token hub's issuer's hold, and the version past version 1, as
        // of the newest block.
        statements.extend(Statement::issuer(&self.ledger));
        statements.extend(Statement::protocol(&self.ledger));
        statements.push(Statement::Final {
            height: covered,
            timestamp: self.stamps[&covered],
        });
        statements
    }

    /// How many times `operation` came to a block.
    fn times_in_blocks(&self, operation: &SignedMemberOperation) -> usize {
        let hash = Operation::Member(operation.clone()).hash();
        self.log
            .iter()
            .filter(|entry| entry.operation.hash() == hash)
            .count()
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
            Request::Payout { .. } => return None,
            Request::Account(key) => vec![Statement::account(&self.ledger, key)],
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
        // That the hub froze when it did not, or the other way around.
        Statement::Issuer {
            height,
            timestamp,
            frozen,
            held,
        } => Some(Statement::Issuer {
            height,
            timestamp,
            frozen: if frozen.is_some() {
                None
            } else {
                Some(timestamp)
            },
            held: !held,
        }),
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
                if cause.is_fee() {
                    self.settlement -= i128::from(amount);
                }
                self.losses.push((peer, amount, cause));
            }
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
        Self::of(faults, Asset::Xrp)
    }

    /// A hub of `asset`.
    fn of(faults: [Fault; 4], asset: Asset) -> Self {
        Self::of_version(faults, asset, dassie_hub::rules::HUB_VERSION)
    }

    /// A hub of `asset`, of `version` of the hub's protocol.
    fn of_version(faults: [Fault; 4], asset: Asset, version: u16) -> Self {
        Self {
            now: START + 10,
            next_block: START + 10 + BLOCK_MS,
            validators: Validators::found(faults, asset, version),
            members: Vec::new(),
            rng: SeededRng::new(1),
        }
    }

    /// A member that deposits `xrp` for a new account.
    fn join(&mut self, name: &str, seed: u8, xrp: u64) -> usize {
        let i = self.newcomer(name, seed);
        let key = self.members[i].key.public_key();
        self.validators.deposit(&key, xrp * XRP);
        i
    }

    /// A member the hub has no account for, to be paid in.
    fn newcomer(&mut self, name: &str, seed: u8) -> usize {
        let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
        let disbursal = Destination {
            account: [seed; 20],
            tag: None,
        };
        let account = HubAccount::new(
            ACCOUNT_ID,
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
            .map(|(_, hold)| hold.amount + hold.pending_amount())
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
        let state_at_b = self.members[b]
            .scheme
            .accept_peering_request(name_a, &request)
            .unwrap();
        let state_at_a = self.members[a]
            .scheme
            .finalize_peering_request(name_b, &info)
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
        // Its message is a bare notice, which the payee takes and books
        // nothing on: the hub says what arrived
        // (spec/areas/settlement.md#settlement-hubs).
        assert!(prepared.message.is_empty(), "a bare notice carries no data");
        let name_a = self.members[a].name;
        let payee = &mut self.members[b];
        let state = payee.peer_states[&name_a].clone();
        let mut effects = Vec::new();
        payee.scheme.handle_settlement(
            name_a,
            prepared.amount,
            &prepared.message,
            &state,
            &mut effects,
        )?;
        assert!(effects.is_empty(), "the notice booked {effects:?}");
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

    /// `b` rejects `a`'s Prepare, with its release of the commitment where
    /// the hub takes one, and `a` takes the Reject. Returns the release.
    fn reject(&mut self, (a, b): (usize, usize), request_id: u32) -> Vec<u8> {
        let (name_a, name_b) = (self.members[a].name, self.members[b].name);
        let mut release = Vec::new();
        self.members[b]
            .scheme
            .commitments()
            .unwrap()
            .on_released(name_a, request_id, &mut release);
        let stands = self.members[a]
            .scheme
            .commitments()
            .unwrap()
            .on_rejected(name_b, request_id, &release);
        assert!(stands, "a Reject with its release stands");
        release
    }

    /// Moves time on past the hold's notice, by which what claims took is
    /// paid (spec/decisions/signed-rejects.md).
    fn past_the_notice(&mut self) {
        let notice = self.validators.ledger.terms().timers.hold_notice;
        self.run(notice + 1_000);
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
            ACCOUNT_ID,
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
        Self::pair_in(Self::new(faults), hold)
    }

    /// [`pair`](Self::pair), in a hub of version 1, whose claims pay at
    /// once and which takes no releases.
    fn pair_of_version_1(faults: [Fault; 4], hold: u64) -> (Self, usize, usize) {
        Self::pair_in(Self::of_version(faults, Asset::Xrp, 1), hold)
    }

    /// [`pair`](Self::pair), in a DSUSD hub: amounts in millionths of a
    /// dollar, as the hub counts them.
    fn token_pair(faults: [Fault; 4], hold: u64) -> (Self, usize, usize) {
        Self::pair_in(Self::of(faults, DSUSD), hold)
    }

    /// Whether members `who` know the hub frozen, and say its method is
    /// unavailable, exactly while it is.
    fn see_the_freeze(&self, who: &[usize]) {
        let frozen = self.validators.ledger.frozen();
        for i in who {
            let account = &self.members[*i].account;
            let name = self.members[*i].name;
            assert_eq!(account.frozen(), frozen, "{name} knows the freeze");
            assert_eq!(account.escrow_held(), frozen.is_some());
            let status = if frozen.is_some() {
                LedgerStatus::Syncing
            } else {
                LedgerStatus::Synced
            };
            assert_eq!(account.status(), status);
        }
    }

    fn pair_in(mut world: Self, hold: u64) -> (Self, usize, usize) {
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
    // What the claims took waits for the hold's notice: the books count
    // it as in the hold until then (spec/decisions/signed-rejects.md).
    assert_eq!(world.balance(b), before - claim_fees / 1_000);
    assert_eq!(world.members[b].scheme.balance(), world.truth(b));
    world.past_the_notice();
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
        .on_released(name_a, 2, &mut Vec::new());
    world.run(500);
    assert!(world.members[a].books.losses.is_empty(), "decided in time");
}

/// In a hub of version 1: a packet the payer gave up on, which the payee
/// then won on the hub, is a loss on the payee; one the payee rejected and
/// then claimed is evidence of cheating. A payee that claims twice, or
/// late, gains nothing.
#[test]
fn what_a_payee_takes_behind_its_answers_is_booked_against_it() {
    let (mut world, a, b) = World::pair_of_version_1([Fault::Honest; 4], 2);
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
        world.members[a].scheme.accept_peering_request(name, &info),
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
    for bytes in [
        membership(world.number(a), other_key),
        membership(AccountNumber(99), own_key),
    ] {
        let scheme = &mut world.members[b].scheme;
        assert_eq!(
            scheme.accept_peering_request(name_a, &bytes),
            Err(SchemeError::Unavailable)
        );
        assert_eq!(
            scheme.finalize_peering_request(name_a, &bytes),
            Err(SchemeError::Unavailable)
        );
    }
    // As before, it may.
    let same = membership(world.number(a), own_key);
    assert!(
        world.members[b]
            .scheme
            .accept_peering_request(name_a, &same)
            .is_ok()
    );
}

/// A member asked to settle funds its hold with at most what its account
/// can spend: what is left once the operation's fee and a redeem's are set
/// aside, so that it can still take what its peers' holds owe it
/// (spec/areas/hubs.md#in-the-node).
#[test]
fn a_hold_is_funded_with_at_most_what_the_account_can_spend() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let spendable = world.balance(a) - 2 * FEE;
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
    assert_eq!(world.balance(a), FEE, "a redeem's fee is left");
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

/// The hub's minimum for a new account, in drops: the account reserve and
/// the starting balance.
const MINIMUM: u64 = ACCOUNT_RESERVE + 1_000;

impl World {
    /// Whether member `i`'s packets with `j` carry promises.
    fn commits_with(&mut self, i: usize, j: usize) -> bool {
        let peer = self.members[j].name;
        self.members[i]
            .scheme
            .commitments()
            .unwrap()
            .commits_with(peer)
    }

    /// The least member `i` must settle with `j` to pay it in, if `j` has
    /// no account.
    fn opening_minimum(&mut self, i: usize, j: usize) -> Option<u64> {
        let peer = self.members[j].name;
        self.members[i]
            .scheme
            .commitments()
            .unwrap()
            .opening_minimum(peer)
    }

    /// What member `i`, without an account, is to be paid in with by `j`,
    /// which has one.
    fn paid_in_minimum(&mut self, i: usize, j: usize) -> Option<u64> {
        let peer = self.members[j].name;
        self.members[i]
            .scheme
            .commitments()
            .unwrap()
            .paid_in_minimum(peer)
    }

    /// Member `i`'s account as the ledger has it.
    fn ledger_account(&self, i: usize) -> &dassie_hub::ledger::Account {
        self.validators.ledger.account(self.number(i)).unwrap()
    }

    /// Whether the hub has an account for member `i`'s key.
    fn has_account(&self, i: usize) -> bool {
        let key = self.members[i].key.public_key();
        self.validators.ledger.account_of(&key).is_some()
    }
}

/// A member the hub has no account for runs, presents its key, and is paid
/// in by a peer with an account once the peer settles at least the hub's
/// minimum for a new account, never before; until then neither side
/// promises anything (spec/designs/settlement-hubs.md#5-joining). Its new
/// account sets its disbursal address first, and the `pay` is a settlement
/// on both sides, the account reserve the reserve's. From then on the
/// peering goes on as any over the hub.
#[test]
fn a_newcomer_is_paid_in_at_the_minimum_and_not_before() {
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    let (name_a, name_c) = (world.members[a].name, world.members[c].name);
    let member = &mut world.members[c];
    assert_eq!(member.account.number(), None);
    assert_eq!(member.account.status(), LedgerStatus::Unfunded);
    assert!(member.scheme.has_synced(), "it runs without a deposit");
    assert_eq!(member.scheme.balance(), 0);
    // Advertised with no account, so that a member with one pays it in.
    let membership = member.scheme.commitments().unwrap().membership();
    assert_eq!(
        membership.map(|m| m.account),
        Some(dassie_settlement::Membership::NO_ACCOUNT)
    );
    world.peer(a, c);
    assert!(!world.commits_with(a, c));
    assert!(!world.commits_with(c, a));
    assert_eq!(world.opening_minimum(a, c), Some(units(MINIMUM)));
    assert_eq!(world.opening_minimum(c, a), None);
    assert_eq!(world.paid_in_minimum(c, a), Some(units(MINIMUM)));
    assert_eq!(world.paid_in_minimum(a, c), None);
    assert_eq!(
        world.prepare((a, c), 1, P, 10_000),
        Err(SchemeError::Unavailable),
        "nothing extended to it"
    );
    assert_eq!(
        world.settle(c, a, units(XRP)),
        Err(SchemeError::Unavailable),
        "it has nothing to pay with"
    );
    // Short of the minimum, nothing is paid.
    let before = world.balance(a);
    assert_eq!(
        world.settle(a, c, units(MINIMUM - 1)),
        Err(SchemeError::Unavailable)
    );
    world.run(1_000);
    assert_eq!(world.balance(a), before);
    assert!(!world.has_account(c));
    // At the minimum or more, what the payer settles opens it.
    let paid = MINIMUM + 3 * XRP;
    let settlement = world.settle(a, c, units(paid)).unwrap();
    world.run(3_000);
    let account = world.ledger_account(c);
    assert_eq!(account.disbursal, Some(world.members[c].disbursal));
    assert_eq!(account.sequence, 1, "the disbursal address first, once");
    assert_eq!(world.balance(c), paid - ACCOUNT_RESERVE - FEE);
    assert_eq!(world.balance(a), before - paid - FEE);
    let books = &world.members[a].books;
    assert_eq!(books.finalized, [settlement]);
    assert_eq!(books.peers[&name_c], -i128::from(units(paid)));
    let books = &world.members[c].books;
    assert_eq!(books.peers[&name_a], i128::from(units(paid)));
    assert_eq!(books.owner, 0, "nothing of it the owner's");
    assert_eq!(books.reserve, -i128::from(units(ACCOUNT_RESERVE)));
    assert_eq!(books.fees, i128::from(units(FEE)));
    for i in [a, c] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
    // An ordinary peering over the hub from then on.
    assert!(world.commits_with(a, c));
    assert!(world.commits_with(c, a));
    assert_eq!(world.opening_minimum(a, c), None);
    assert_eq!(world.paid_in_minimum(c, a), None);
    assert_eq!(
        world.members[a].scheme.peer_account(name_c),
        Some(world.number(c))
    );
    assert_eq!(
        world.members[a].scheme.peer_hub_account(name_c),
        Some(world.number(c).0)
    );
    let membership = world.members[c].scheme.commitments().unwrap().membership();
    assert_eq!(membership.map(|m| m.account), Some(world.number(c).0));
    world.settle(a, c, units(2 * XRP)).unwrap();
    world.run(1_000);
    assert!(world.hold(a, c).is_some());
    world.prepare((a, c), 1, P, 10_000).unwrap();
    world.fulfill((a, c), 1, true, true);
    assert_eq!(
        world.members[c].books.peers[&name_a],
        i128::from(units(paid + 2 * XRP))
    );
    for i in [a, c] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// Two members without an account do not peer: neither could pay the
/// other in.
#[test]
fn two_newcomers_do_not_peer() {
    let mut world = World::new([Fault::Honest; 4]);
    let c = world.newcomer("carol", 33);
    let d = world.newcomer("dave", 34);
    world.run(3_000);
    let mut info = Vec::new();
    world.members[d].scheme.peering_info(&mut info);
    assert!(!info.is_empty());
    let name_d = world.members[d].name;
    let mut out = Vec::new();
    assert_eq!(
        world.members[c]
            .scheme
            .create_peering_request(name_d, &info, &mut out),
        Err(SchemeError::Unavailable)
    );
    assert_eq!(
        world.members[c]
            .scheme
            .accept_peering_request(name_d, &info),
        Err(SchemeError::Unavailable)
    );
}

/// Restarts of either side, after every step while the payee is paid in
/// and joins, neither lose nor repeat anything: the `pay` executes once, is
/// booked once on each side, and the disbursal address is set once.
#[test]
fn a_pay_in_is_booked_once_on_each_side_across_restarts() {
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    world.peer(a, c);
    let (name_a, name_c) = (world.members[a].name, world.members[c].name);
    let before = world.balance(a);
    let settlement = world.settle(a, c, units(MINIMUM)).unwrap();
    for _ in 0..200 {
        world.run(STEP_MS);
        for i in [a, c] {
            world.restart(i);
            let pending: Vec<(Name, u64)> = world.members[i]
                .books
                .pending
                .iter()
                .map(|(id, (_, amount))| (*id, *amount))
                .collect();
            world.reconcile(i, &pending);
        }
    }
    world.run(3_000);
    assert_eq!(world.ledger_account(c).sequence, 1);
    assert_eq!(world.balance(a), before - MINIMUM - FEE, "paid once");
    assert_eq!(world.members[a].books.finalized, [settlement]);
    assert_eq!(
        world.members[a].books.peers[&name_c],
        -i128::from(units(MINIMUM))
    );
    let books = &world.members[c].books;
    assert_eq!(books.peers[&name_a], i128::from(units(MINIMUM)));
    assert_eq!(books.reserve, -i128::from(units(ACCOUNT_RESERVE)));
    for i in [a, c] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// A newcomer that opened its account itself, by a deposit, before its
/// peer's `pay` executed: the `pay` fails and is cancelled, the payer asks
/// the hub for the account of the key the peer gave, and its next
/// settlement funds a hold toward it.
#[test]
fn a_newcomer_that_deposited_is_found_by_its_key() {
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    world.peer(a, c);
    let (name_a, name_c) = (world.members[a].name, world.members[c].name);
    let key = world.members[c].key.public_key();
    world.validators.deposit(&key, 10 * XRP);
    world.run(200);
    assert!(world.has_account(c));
    let settlement = world.settle(a, c, units(MINIMUM)).unwrap();
    world.run(3_000);
    assert_eq!(world.members[a].books.cancelled, [settlement]);
    assert_eq!(
        world.members[a].scheme.peer_account(name_c),
        Some(world.number(c))
    );
    assert!(world.commits_with(a, c));
    let books = &world.members[c].books;
    assert_eq!(books.owner, i128::from(units(10 * XRP - ACCOUNT_RESERVE)));
    assert!(!books.peers.contains_key(&name_a));
    world.settle(a, c, units(XRP)).unwrap();
    world.run(1_000);
    assert!(world.hold(a, c).is_some());
    assert_eq!(
        world.members[c].books.peers[&name_a],
        i128::from(units(XRP))
    );
    for i in [a, c] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// A node that cycles fresh keys through a member costs the member
/// nothing (spec/decisions/hubs-only.md): the member promises a peer
/// without an account nothing, so extends it nothing, and pays it in only
/// with what it settles, at least the minimum. A peer paid in that comes
/// back under a fresh key, once nothing of its old membership is left to
/// settle, starts again without an account.
#[test]
fn fresh_keys_cost_the_payer_nothing() {
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let newcomers: Vec<usize> = (0..4)
        .map(|n| world.newcomer(&format!("fresh{n}"), 40 + n))
        .collect();
    world.run(3_000);
    let before = world.balance(a);
    let sequence = world.ledger_account(a).sequence;
    for c in newcomers {
        world.peer(a, c);
        assert!(!world.commits_with(a, c));
        assert_eq!(
            world.prepare((a, c), 1, P, 10_000),
            Err(SchemeError::Unavailable)
        );
        let name = world.members[c].name;
        assert_eq!(
            world.members[a].scheme.commitments().unwrap().holds(name),
            dassie_settlement::HoldsWith::default()
        );
        assert_eq!(
            world.settle(a, c, units(MINIMUM - 1)),
            Err(SchemeError::Unavailable)
        );
    }
    world.run(3_000);
    assert_eq!(world.balance(a), before, "nothing paid");
    assert_eq!(
        world.ledger_account(a).sequence,
        sequence,
        "nothing submitted"
    );
    // Paid in with what it was owed, then back under a fresh key.
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    world.peer(a, c);
    world.settle(a, c, units(MINIMUM)).unwrap();
    world.run(3_000);
    assert!(world.commits_with(a, c));
    let again = world.newcomer("carol", 34);
    world.run(3_000);
    world.peer(a, again);
    assert!(!world.commits_with(a, again), "a fresh key has no account");
    assert_eq!(world.opening_minimum(a, again), Some(units(MINIMUM)));
    assert_eq!(world.balance(a), before - MINIMUM - FEE);
}

/// A payer whose `pay` executed while it could not read its history books
/// it once the hub says it has an account for the key it paid in, f + 1
/// agreeing: as made, never cancelled, so the newcomer is paid in once and
/// the next settlement funds a hold toward it. The `pay`'s fee goes
/// through suspense, as any unread operation's.
#[test]
fn a_pay_the_payer_never_read_is_booked_as_made() {
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    world.peer(a, c);
    let (name_a, name_c) = (world.members[a].name, world.members[c].name);
    let before = world.balance(a);
    let settlement = world.settle(a, c, units(MINIMUM)).unwrap();
    world.members[a].offline = true;
    world.run(5_000);
    assert!(world.has_account(c), "the `pay` executed");
    world.validators.trim();
    world.members[a].offline = false;
    world.run(3_000);
    assert!(!world.members[a].account.is_resyncing());
    let books = &world.members[a].books;
    assert_eq!(books.finalized, [settlement]);
    assert!(books.cancelled.is_empty());
    assert_eq!(books.peers[&name_c], -i128::from(units(MINIMUM)));
    assert_eq!(books.suspense, -i128::from(units(FEE)));
    assert_eq!(
        world.members[a].scheme.peer_account(name_c),
        Some(world.number(c))
    );
    assert!(world.commits_with(a, c));
    assert_eq!(world.opening_minimum(a, c), None);
    world.settle(a, c, units(XRP)).unwrap();
    world.run(1_000);
    assert!(world.hold(a, c).is_some(), "not paid in again");
    assert_eq!(
        world.members[c].books.peers[&name_a],
        i128::from(units(MINIMUM + XRP))
    );
    assert_eq!(
        world.balance(a),
        before - MINIMUM - XRP - HOLD_RESERVE - 2 * FEE
    );
    for i in [a, c] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
    assert_eq!(world.members[c].books.suspense, 0);
}

/// A `pay` the payer never read that did not apply, as another operation
/// signed with its key took its sequence: the hub says, f + 1 agreeing as
/// of a block at or after the one the payer read anew at, that the key has
/// no account, and the settlement is cancelled; what the payer's books
/// counted of it meanwhile comes back out of suspense. Paid in later, the
/// newcomer is paid in once.
#[test]
fn a_pay_the_payer_never_read_that_failed_is_cancelled() {
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let b = world.join("bob", 32, 100);
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    world.peer(a, c);
    let name_c = world.members[c].name;
    let before = world.balance(a);
    let to = world.number(b);
    world.raw(a, MemberOperation::Transfer { to, amount: 1 });
    let settlement = world.settle(a, c, units(MINIMUM)).unwrap();
    world.members[a].offline = true;
    world.run(2_000);
    assert!(!world.has_account(c), "refused for its sequence");
    world.validators.trim();
    world.members[a].offline = false;
    world.run(3_000);
    assert!(!world.members[a].account.is_resyncing());
    let books = &world.members[a].books;
    assert_eq!(books.cancelled, [settlement]);
    assert!(books.finalized.is_empty());
    assert!(!books.peers.contains_key(&name_c));
    assert_eq!(
        books.suspense,
        -i128::from(units(1 + FEE)),
        "only the transfer it never read"
    );
    assert_eq!(world.members[a].scheme.balance(), world.truth(a));
    assert_eq!(world.opening_minimum(a, c), Some(units(MINIMUM)));
    let again = world.settle(a, c, units(MINIMUM)).unwrap();
    world.run(3_000);
    assert_eq!(world.members[a].books.finalized, [again]);
    assert_eq!(world.balance(a), before - 1 - FEE - MINIMUM - FEE);
    assert_eq!(world.balance(c), MINIMUM - ACCOUNT_RESERVE - FEE);
    for i in [a, c] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
}

/// A payer asks the hub for the account of a peer without one now and
/// then, so it learns of one the peer's own deposit opened without paying
/// it in: both ends then promise, and nothing was submitted for it.
#[test]
fn a_payer_learns_a_deposit_opened_its_peers_account() {
    use dassie_hub::member::scheme::LOOK_UP_EVERY_MS;
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    world.peer(a, c);
    // Asked once at peering: none yet.
    world.run(3_000);
    let name_c = world.members[c].name;
    let sequence = world.ledger_account(a).sequence;
    let key = world.members[c].key.public_key();
    world.validators.deposit(&key, 10 * XRP);
    world.run(3_000);
    assert!(world.commits_with(c, a), "it knows its own account");
    assert!(!world.commits_with(a, c), "not asked again yet");
    world.run(LOOK_UP_EVERY_MS);
    assert_eq!(
        world.members[a].scheme.peer_account(name_c),
        Some(world.number(c))
    );
    assert!(world.commits_with(a, c));
    assert_eq!(world.opening_minimum(a, c), None);
    assert_eq!(world.ledger_account(a).sequence, sequence, "nothing paid");
}

/// A `pay` that failed is not tried again at once: each try costs the
/// payer a fee.
#[test]
fn a_failed_pay_is_not_tried_again_at_once() {
    use dassie_hub::member::scheme::PAY_AGAIN_MS;
    let mut world = World::new([Fault::Honest; 4]);
    let a = world.join("alice", 31, 100);
    let b = world.join("bob", 32, 100);
    let c = world.newcomer("carol", 33);
    world.run(3_000);
    world.peer(a, c);
    let to = world.number(b);
    let drain = world.balance(a) - FEE - 15;
    world.raw(a, MemberOperation::Transfer { to, amount: drain });
    let settlement = world.settle(a, c, units(MINIMUM)).unwrap();
    world.run(3_000);
    assert_eq!(world.members[a].books.cancelled, [settlement]);
    assert!(!world.has_account(c));
    let number = world.number(a);
    world.validators.top_up(number, 100 * XRP);
    world.run(1_000);
    assert_eq!(
        world.settle(a, c, units(MINIMUM)),
        Err(SchemeError::Unavailable),
        "not again so soon"
    );
    world.run(PAY_AGAIN_MS);
    world.settle(a, c, units(MINIMUM)).unwrap();
    world.run(3_000);
    assert!(world.has_account(c));
    for i in [a, c] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
}

// spec/decisions/token-hubs.md: a frozen hub's method is unavailable from
// the freeze's block until the thaw: no commitment is signed or taken and
// nothing is settled over it, and the peering carries on after the thaw.
// One lying validator neither freezes the hub nor thaws it.
#[test]
fn a_frozen_hubs_method_is_unavailable_until_the_thaw() {
    let faults = [Fault::Honest, Fault::Liar, Fault::Honest, Fault::Honest];
    let (mut world, a, b) = World::token_pair(faults, 2);
    let (name_a, name_b) = (world.members[a].name, world.members[b].name);
    world.see_the_freeze(&[a, b]);
    world.prepare((a, b), 1, P, 10_000).unwrap();
    world.fulfill((a, b), 1, true, true);
    // A commitment signed before the freeze, checked after it.
    let late = promised(2, P, world.now + 10_000);
    let mut commitment = Vec::new();
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .commit(world.now, name_b, 2, late, &mut commitment)
        .unwrap();
    world.validators.freeze(true);
    world.run(1_000);
    assert!(world.validators.ledger.frozen().is_some());
    world.see_the_freeze(&[a, b]);
    assert_eq!(
        world.prepare((a, b), 3, P, 10_000),
        Err(SchemeError::Unavailable),
        "no commitment signed"
    );
    let now = world.now;
    assert_eq!(
        world.members[b]
            .scheme
            .commitments()
            .unwrap()
            .check_commitment(now, name_a, 2, late, &commitment),
        Err(SchemeError::Unavailable),
        "none taken"
    );
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_answered(name_b, 2);
    assert_eq!(
        world.settle(a, b, units(XRP)),
        Err(SchemeError::Unavailable),
        "nothing settled"
    );
    world.validators.freeze(false);
    world.run(1_000);
    world.see_the_freeze(&[a, b]);
    for request_id in 4..=6 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, true);
    }
    world.settle(a, b, units(XRP)).unwrap();
    world.run(2_000);
    let hold = world.hold(a, b).unwrap();
    let ledger_hold = world.validators.ledger.hold(hold).unwrap();
    assert_eq!(ledger_hold.total, 4 * P / 1_000, "redeemed after the thaw");
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
        assert!(world.members[i].books.losses.is_empty());
    }
}

// An operation a frozen hub refused waits for the thaw: it is not counted
// as failed or sent again while the hub is frozen, and applies once after
// it.
#[test]
fn an_operation_refused_while_frozen_goes_again_after_the_thaw_and_applies_once() {
    let (mut world, a, b) = World::token_pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    // The freeze takes effect in the block that takes the settlement, which
    // the payer sent before it knew.
    world.validators.freeze(true);
    let settlement = world.settle(a, b, units(XRP)).unwrap();
    world.run(5_000);
    let refused: Vec<SignedMemberOperation> = world
        .validators
        .log
        .iter()
        .filter(|entry| entry.outcome == Outcome::Refused(Refusal::Frozen))
        .filter_map(|entry| match &entry.operation {
            Operation::Member(signed) => Some(signed.clone()),
            Operation::Validator(_) => None,
        })
        .collect();
    assert_eq!(refused.len(), 1, "refused once, and not sent again");
    let refused = &refused[0];
    let books = &world.members[a].books;
    assert!(books.pending.contains_key(&settlement), "never failed");
    assert!(books.cancelled.is_empty() && books.finalized.len() == 1);
    assert!(
        world.members[a]
            .account
            .has(dassie_hub::member::Purpose::Settle(settlement))
    );
    world.validators.freeze(false);
    world.run(3_000);
    assert_eq!(world.validators.times_in_blocks(refused), 2, "applied once");
    let books = &world.members[a].books;
    assert!(books.finalized.contains(&settlement));
    assert!(books.pending.is_empty() && books.cancelled.is_empty());
    assert_eq!(world.validators.ledger.hold(hold).unwrap().amount, 3 * XRP);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

// spec/decisions/token-hubs.md: a frozen hub still pays claims for the
// packets a payee took before the freeze, whose payer's claims do not come.
// Across the freeze and the thaw, neither member loses anything on the
// other but the fees of its own claims.
#[test]
fn claims_for_packets_taken_before_the_freeze_go_on() {
    let (mut world, a, b) = World::token_pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    let before = world.balance(b);
    for request_id in 1..=3 {
        world.prepare((a, b), request_id, P, 10_000).unwrap();
        world.fulfill((a, b), request_id, true, false);
    }
    world.validators.freeze(true);
    world.run(3_000);
    assert!(world.validators.ledger.frozen().is_some());
    let ledger_hold = world.validators.ledger.hold(hold).unwrap();
    assert_eq!(ledger_hold.claimed.len(), 3, "claimed while frozen");
    let claim_fees = world.members[b].books.lost(LossCause::ClaimFee);
    // What they took waits for the hold's notice, by the hub's clock,
    // which stands still while frozen (spec/decisions/signed-rejects.md).
    world.past_the_notice();
    assert_eq!(world.balance(b), before - claim_fees / 1_000);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
    world.validators.freeze(false);
    world.run(3_000);
    assert!(world.validators.ledger.frozen().is_none());
    world.past_the_notice();
    assert_eq!(
        world.balance(b),
        before + 3 * P / 1_000 - claim_fees / 1_000
    );
    for i in [a, b] {
        for cause in [LossCause::PaidLate, LossCause::Cheated] {
            assert_eq!(world.members[i].books.lost(cause), 0, "{i}: {cause:?}");
        }
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
    assert!(world.members[a].books.losses.is_empty());
}

// A hold's reduction pending at a freeze waits for the thaw, as its notice
// does, and the payee redeems before it: it loses nothing it had not
// redeemed.
#[test]
fn a_reduction_pending_at_a_freeze_takes_nothing_the_payee_is_owed() {
    let (mut world, a, b) = World::token_pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    // One packet: its claim is below the threshold to redeem.
    world.prepare((a, b), 1, P, 10_000).unwrap();
    world.fulfill((a, b), 1, true, true);
    world.run(1_000);
    assert_eq!(world.validators.ledger.hold(hold).unwrap().total, 0);
    // The payer takes it all back, and the issuer freezes in that block.
    world.raw(
        a,
        MemberOperation::HoldReduce {
            hold,
            amount: 2 * XRP,
        },
    );
    world.validators.freeze(true);
    let notice = world.validators.ledger.terms().timers.hold_notice;
    world.run(notice + 1_000);
    let ledger_hold = world.validators.ledger.hold(hold).unwrap();
    assert_eq!(ledger_hold.amount, 2 * XRP, "nothing reduced while frozen");
    assert_eq!(ledger_hold.total, 0, "the redeem waits for the thaw");
    let mirror = world.members[b].account.hold(hold).unwrap();
    assert_eq!(mirror.amount, 2 * XRP, "nor in the payee's records");
    world.validators.freeze(false);
    world.run(1_000);
    let ledger_hold = world.validators.ledger.hold(hold).unwrap();
    assert_eq!(ledger_hold.total, P / 1_000, "redeemed after the thaw");
    assert!(ledger_hold.reduction.is_some(), "before the reduction");
    for i in [a, b] {
        let mirror = world.members[i].account.hold(hold).unwrap();
        assert!(mirror.reduction.is_some(), "its notice moved by the freeze");
    }
    world.run(notice + 5_000);
    assert!(world.validators.ledger.hold(hold).is_none(), "reduced");
    for i in [a, b] {
        assert!(world.members[i].account.hold(hold).is_none());
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert!(world.members[i].books.losses.is_empty());
    }
}

// spec/decisions/token-hubs.md: a payee refuses a commitment, and its
// Prepare, that expires further ahead of its clock than a connector holds
// a packet and the precision: over any hub.
#[test]
fn a_commitment_past_the_bound_is_refused() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let name_b = world.members[b].name;
    let ahead = dassie_hub::rules::MAX_HOLD_MS + MARGIN;
    world.prepare((a, b), 1, P, ahead).unwrap();
    assert_eq!(
        world.prepare((a, b), 2, P, ahead + 10_000),
        Err(SchemeError::InvalidData)
    );
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_answered(name_b, 2);
    world.fulfill((a, b), 1, true, true);
    world.run(1_000);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
    }
}

/// Bob's claim on `hold` of Alice's `commitment` for Prepare `request_id`,
/// submitted by Bob itself.
fn claim_of(hold: HoldNumber, commitment: Signed<Commitment>, request_id: u32) -> MemberOperation {
    MemberOperation::Claim {
        hold,
        fulfilled: vec![Fulfilled {
            commitment,
            fulfillment: preimage(request_id),
        }],
    }
}

/// spec/decisions/signed-rejects.md: the payee's Reject comes with its
/// release; a claim it makes all the same waits, the payer disputes it with
/// the release, and the hub never pays it. The dispute's fee is the payee's
/// cheating, booked against it.
#[test]
fn a_claim_of_what_the_payee_released_is_disputed_and_never_paid() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    let before = world.balance(b);
    let commitment = signed_commitment(&world.prepare((a, b), 1, P, 10_000).unwrap());
    let release = world.reject((a, b), 1);
    let release = Signed::<Release>::decode(&mut dassie_oer::Reader::new(&release)).unwrap();
    assert_eq!(release.value, Release::of(&commitment.value));
    world.raw(b, claim_of(hold, commitment, 1));
    world.run(2_000);
    assert_eq!(
        world.validators.ledger.hold(hold).unwrap().amount,
        2 * XRP,
        "the dispute put it back"
    );
    world.past_the_notice();
    assert_eq!(world.balance(b), before - FEE, "only its claim's fee");
    let books = &world.members[a].books;
    assert_eq!(books.lost(LossCause::Cheated), 0, "nothing paid");
    assert_eq!(books.lost(LossCause::DisputeFee), units(FEE));
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// A payer away for longer than the hold's notice cannot dispute in time:
/// the claim of what its payee released is paid, and booked as the payee's
/// cheating once the payer reads it.
#[test]
fn a_payer_away_past_the_notice_books_the_claim_it_could_not_dispute() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    let commitment = signed_commitment(&world.prepare((a, b), 1, P, 10_000).unwrap());
    world.reject((a, b), 1);
    world.members[a].offline = true;
    world.raw(b, claim_of(hold, commitment, 1));
    world.past_the_notice();
    assert!(
        world
            .validators
            .ledger
            .hold(hold)
            .unwrap()
            .pending
            .is_empty(),
        "paid"
    );
    world.members[a].offline = false;
    world.run(3_000);
    assert_eq!(world.members[a].books.lost(LossCause::Cheated), P);
    for i in [a, b] {
        assert_eq!(world.members[i].scheme.balance(), world.truth(i));
        assert_eq!(world.members[i].books.suspense, 0);
    }
}

/// From version 2 on, a Reject without the payee's release does not stand:
/// the payee could still claim the packet, so it waits for the hub as one
/// that expired, which decides it. In a hub of version 1 it stands, and
/// no release goes with it.
#[test]
fn a_reject_without_its_release_waits_for_the_hub() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let name_b = world.members[b].name;
    world.prepare((a, b), 1, P, 2_000).unwrap();
    let stands = world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_rejected(name_b, 1, &[]);
    assert!(!stands, "no release");
    // A release of another packet's commitment is no release of this one.
    world.prepare((a, b), 2, P, 2_000).unwrap();
    let other = world.reject((a, b), 2);
    let stands = world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .on_rejected(name_b, 1, &other);
    assert!(!stands, "another packet's release");
    world.run(2_000);
    world.wait((a, b), 1, world.now + 1_000);
    world.run(1_000);
    assert!(world.members[a].decided.contains(&Decided {
        peer: name_b,
        request_id: 1,
        fulfillment: None,
    }));

    let (mut world, a, b) = World::pair_of_version_1([Fault::Honest; 4], 2);
    world.prepare((a, b), 1, P, 2_000).unwrap();
    assert!(
        world.reject((a, b), 1).is_empty(),
        "no release in version 1"
    );
}

/// A payee never takes again a commitment it released: the payer could
/// otherwise dispute its claim of the packet with the release. It releases
/// a Prepare it refused too, if the payer signed its commitment, and none
/// it never could have taken.
#[test]
fn a_payee_takes_no_commitment_it_released() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let (name_a, name_b, now) = (world.members[a].name, world.members[b].name, world.now);
    let bytes = world.prepare((a, b), 1, P, 10_000).unwrap();
    world.reject((a, b), 1);
    let promised = promised(1, P, now + 10_000);
    let payee = world.members[b].scheme.commitments().unwrap();
    assert!(
        payee
            .check_commitment(now, name_a, 7, promised, &bytes)
            .is_err(),
        "taken again"
    );
    // Refused before it was taken: released, and never taken after.
    let refused = world.members[a].scheme.commitments().unwrap().commit(
        now,
        name_b,
        2,
        promised,
        &mut Vec::new(),
    );
    assert!(refused.is_ok());
    let mut fresh = Vec::new();
    world.members[a]
        .scheme
        .commitments()
        .unwrap()
        .commit(now, name_b, 3, promised, &mut fresh)
        .unwrap();
    let payee = world.members[b].scheme.commitments().unwrap();
    let mut release = Vec::new();
    payee.release_refused(now, name_a, &fresh, &mut release);
    assert!(!release.is_empty());
    assert!(
        payee
            .check_commitment(now, name_a, 8, promised, &fresh)
            .is_err()
    );
    // One the payer did not sign is not released.
    let mut forged = fresh.clone();
    let last = forged.len() - 1;
    forged[last] ^= 1;
    payee.release_refused(now, name_a, &forged, &mut release);
    assert!(release.is_empty());
}

/// A payee forgets a release once its commitment expired, and may then
/// take another commitment its payer signs under the same sequence: its
/// release names the first commitment's expiry, so the payer's dispute of
/// the payee's claim of the second puts nothing back, and the payee is
/// paid (spec/decisions/signed-rejects.md).
#[test]
fn an_old_release_never_takes_back_a_claim_of_a_new_commitment() {
    let (mut world, a, b) = World::pair([Fault::Honest; 4], 2);
    let hold = world.hold(a, b).unwrap();
    let name_a = world.members[a].name;
    world.prepare((a, b), 1, P, 2_000).unwrap();
    let release = world.reject((a, b), 1);
    let release = Signed::<Release>::decode(&mut dassie_oer::Reader::new(&release)).unwrap();
    // Past the first commitment's expiry, the payee forgot its release.
    world.run(5_000);
    // The payer signs another commitment under sequence 1.
    let now = world.now;
    let promise = promised(2, P, now + 10_000);
    let again = Signed::sign(
        Commitment {
            hold,
            sequence: 1,
            amount: P / 1_000,
            condition: condition(2),
            expiry: now + 10_000 - MARGIN,
        },
        world.validators.ledger.hub(),
        &world.members[a].key,
    );
    world.members[b]
        .scheme
        .commitments()
        .unwrap()
        .check_commitment(now, name_a, 2, promise, &again.to_vec())
        .unwrap();
    world.members[b]
        .scheme
        .commitments()
        .unwrap()
        .on_fulfilled(now, name_a, 2, preimage(2));
    let before = world.balance(b);
    // The payee claims it, and the payer disputes with the old release.
    world.run(2_000);
    assert_eq!(world.validators.ledger.hold(hold).unwrap().pending.len(), 1);
    world.raw(
        a,
        MemberOperation::Dispute {
            hold,
            releases: vec![release],
        },
    );
    world.past_the_notice();
    assert_eq!(
        world.validators.ledger.hold(hold).unwrap().claimed[&1],
        P / 1_000,
        "the dispute put nothing back"
    );
    let fees = world.members[b].books.lost(LossCause::ClaimFee) / 1_000;
    assert!(
        world.balance(b) + fees >= before + P / 1_000,
        "the payee was paid"
    );
}

//! The `hub` simulation's ledger run (spec/areas/hubs.md#the-ledger): a
//! hub's ledger executing blocks of members' and validators' operations,
//! in three seeds of four a token hub whose issuer freezes the escrow's
//! line, thaws it, sets and removes a transfer fee and claws back
//! (spec/decisions/token-hubs.md), and otherwise an XRP hub, where every
//! such attestation is refused.
//!
//! Members join by deposit, pay each other over holds with commitments and
//! claims, reduce holds, transfer and withdraw. Honest validators attest
//! what the underlying ledger did, each after a delay of its own, some
//! long, so their votes come late; up to f faulty ones attest made-up
//! states and clawbacks, replay old states, and some never vote, so their
//! absence counts. The escrow pays a withdrawal only while the issuer holds
//! nothing against it, and stops once the hub shuts down, leaving the rest
//! to the payout set. The run ends with a shutdown, by clawback or by the
//! validators' votes, and the payout set of the last block.
//!
//! Oracles, after every block, from a model of the hub's clock kept apart
//! from the ledger's: while frozen, only claims of commitments within the
//! freeze's window execute, everything else is refused as waiting, no
//! deposit is credited and no withdrawal settled; no reduction, shutdown or
//! absence falls due but by the model's clock, which stands still during a
//! freeze, so one pending at a freeze falls due only after the thaw, its
//! notice moved by the freeze; the issuer's state in force is one the
//! issuer was in, and only true clawbacks count, each once. At the end, the
//! ledger holds what the escrow holds, to the unit, and the payout set
//! pays the withdrawals already sequenced in full and each account its
//! share, rounded down, never more than the escrow holds.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{IdentityKey, NodeKey};
use dassie_kit::sim::{Outcome as SimOutcome, SeededRng};
use sha2::{Digest as _, Sha256};

use crate::consensus::Config;
use crate::digest::Digest;
use crate::escrow::payout::{PayoutTransaction, payout_set};
use crate::escrow::{ValidatorKeys, escrow_master};
use crate::founding::starting_terms;
use crate::ledger::{Block, Failure, Ledger, Outcome, Phase, Refusal};
use crate::operation::{
    AccountNumber, Clawback, Deposit, Destination, Issuer, MemberOperation, Operation,
    SignedMemberOperation, SignedValidatorOperation, ValidatorOperation,
};
use crate::promise::{Claim, Commitment, Fulfilled, HoldNumber, Signed};
use crate::rules::{
    Amount, Asset, LEAST_TOKEN_GAS_DROPS, MAX_HOLD_MS, Millis, Rules, least_margin_ms,
};
use crate::validators::{Signers, ValidatorIndex};

/// The network the run's hub is on: the XRP Ledger testnet.
const NETWORK: u32 = 1;

/// When the run's hub starts.
const GENESIS: Millis = super::START;

/// The most a member pays its fees with in one operation.
const MAX_FEE: Amount = 10;

/// How many steps members act for, before the hub winds down.
const STEPS: u64 = 240;

/// The most steps the wind-down may take: blocks every step, the
/// shutdown's notice passed in jumps once nothing is on its way.
const WIND_DOWN_STEPS: u64 = 400;

/// What a seed draws.
struct Setup {
    token: bool,
    validators: usize,
    faulty: usize,
    /// Whether the faulty validators never vote, so their absence shuts
    /// the hub down once the hub's clock passed it.
    silent: bool,
    members: usize,
}

fn setup(seed: u64) -> (SeededRng, Setup) {
    let mut rng = SeededRng::new(seed ^ 0x746f_6b65_6e5f_6875);
    let token = rng.below(4) != 0;
    let validators = 4 + usize::try_from(rng.below(4)).unwrap_or(0);
    let faults = (validators - 1) / 3;
    let faulty = usize::try_from(rng.below(faults as u64 + 1)).unwrap_or(0);
    let silent = faulty > 0 && rng.below(3) == 0;
    let members = 3 + usize::try_from(rng.below(3)).unwrap_or(0);
    (
        rng,
        Setup {
            token,
            validators,
            faulty,
            silent,
            members,
        },
    )
}

/// What the seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, setup) = setup(seed);
    format!(
        "a{} hub's ledger: {} validators, {} faulty{}, {} members",
        if setup.token { " token" } else { "n XRP" },
        setup.validators,
        setup.faulty,
        if setup.silent { " and silent" } else { "" },
        setup.members
    )
}

/// The rules of the run's hub: the starting terms, as a token hub's if
/// `token`, with the margin `validators` call for.
fn rules(token: bool, validators: usize) -> (Rules, Vec<NodeKey>) {
    let nonce = [0x5a; 32];
    let sponsor = IdentityKey::from_bytes(&[0x50; 32]);
    let escrow = escrow_master(&sponsor, NETWORK, &nonce);
    let mut terms = starting_terms(nonce, sponsor.identity(), NETWORK, escrow.account().0);
    terms.timers.margin = least_margin_ms(validators);
    if token {
        terms.asset = Asset::Token {
            currency: dassie_xrpl::dsusd::CURRENCY,
            issuer: dassie_xrpl::dsusd::ISSUER,
        };
        terms.fees.gas = LEAST_TOKEN_GAS_DROPS;
        terms.fees.ledger = 0;
        terms.fees.least_payout = 1;
    }
    let mut consensus = Vec::new();
    let mut acceptances = Vec::new();
    for index in 0..validators {
        let node = IdentityKey::from_bytes(&[0x60 + u8::try_from(index).unwrap(); 32]);
        let keys = ValidatorKeys::derive(&node, NETWORK, &nonce).unwrap();
        acceptances.push(
            keys.accept(&node, &terms, format!("v{index}.example:443"))
                .unwrap(),
        );
        consensus.push(keys.consensus);
    }
    (
        Rules {
            terms,
            validators: acceptances,
        },
        consensus,
    )
}

/// A commitment a payee holds, with its fulfillment.
#[derive(Clone)]
struct Held {
    commitment: Signed<Commitment>,
    fulfillment: [u8; 32],
}

/// What the payer of a hold signed: its commitments' last sequence and
/// their total.
#[derive(Clone, Copy, Default)]
struct Signing {
    sequence: u64,
    total: Amount,
}

/// A member: its account and key.
struct Member {
    number: AccountNumber,
    key: NodeKey,
}

/// The issuer and the escrow as the underlying ledger has them.
#[derive(Default)]
struct Truth {
    /// The underlying ledger's index.
    ledger: u32,
    /// The issuer's hold, and every state it was in.
    issuer: Issuer,
    history: Vec<Issuer>,
    /// What the escrow holds of the hub's asset, credited or not.
    escrow: Amount,
    /// Every clawback.
    clawbacks: BTreeMap<Digest, Amount>,
    /// Withdrawals the escrow applied, by sequence, with whether they
    /// delivered.
    applied: BTreeMap<u32, bool>,
    /// The next escrow sequence the escrow applies.
    next: Option<u32>,
}

/// The model of the hub's clock: the time frozen so far, and the
/// deadlines it runs toward, in the hub's time.
#[derive(Default)]
struct Clock {
    paused: Millis,
    /// Each hold's reduction, due at this hub time.
    reductions: BTreeMap<HoldNumber, Millis>,
    /// Each validator's last vote recorded, in hub time.
    recorded: Vec<Millis>,
    /// The end of the shutdown's notice, in hub time.
    ends: Option<Millis>,
    /// Holds whose reduction was pending at a freeze.
    caught: BTreeSet<HoldNumber>,
}

struct Sim {
    rng: SeededRng,
    ledger: Ledger,
    validators: Vec<NodeKey>,
    sequences: Vec<u64>,
    /// Faulty validators are the last ones.
    faulty: usize,
    /// Whether their votes are never recorded.
    silent: bool,
    members: Vec<Member>,
    held: BTreeMap<HoldNumber, Vec<Held>>,
    signing: BTreeMap<HoldNumber, Signing>,
    /// Attestations on their way, by the step they are due at.
    queue: Vec<(u64, usize, ValidatorOperation)>,
    truth: Truth,
    clock: Clock,
    step: u64,
    deposits: u64,
    trace: Vec<String>,
    counts: BTreeMap<&'static str, u64>,
    winding_down: bool,
}

/// Runs the ledger part of seed `seed`: its outcome, trace and counts.
#[must_use]
pub fn run(seed: u64) -> (SimOutcome, Vec<String>, BTreeMap<&'static str, u64>) {
    let (rng, setup) = setup(seed);
    let (rules, validators) = rules(setup.token, setup.validators);
    let ledger = Ledger::genesis(rules, GENESIS).unwrap();
    let mut sim = Sim {
        rng,
        ledger,
        sequences: alloc::vec![0; validators.len()],
        validators,
        faulty: setup.faulty,
        silent: setup.silent,
        members: Vec::new(),
        held: BTreeMap::new(),
        signing: BTreeMap::new(),
        queue: Vec::new(),
        truth: Truth::default(),
        clock: Clock {
            recorded: alloc::vec![GENESIS; setup.validators],
            ..Clock::default()
        },
        step: 0,
        deposits: 0,
        trace: Vec::new(),
        counts: BTreeMap::new(),
        winding_down: false,
    };
    sim.trace.push(describe(seed));
    let outcome = match sim.run(setup.members) {
        Ok(()) => SimOutcome::Pass,
        Err(outcome) => outcome,
    };
    (outcome, sim.trace, sim.counts)
}

type Checked = Result<(), SimOutcome>;

fn fail(check: &'static str, message: String) -> SimOutcome {
    SimOutcome::safety(check, message)
}

impl Sim {
    /// Whether no honest attestation is on its way.
    fn calm(&self) -> bool {
        self.queue
            .iter()
            .all(|(_, index, _)| *index >= self.honest())
    }

    fn token(&self) -> bool {
        !matches!(self.ledger.terms().asset, Asset::Xrp)
    }

    fn honest(&self) -> usize {
        self.validators.len() - self.faulty
    }

    fn count(&mut self, name: &'static str) {
        *self.counts.entry(name).or_insert(0) += 1;
    }

    fn run(&mut self, members: usize) -> Checked {
        let escrow = (0..self.honest())
            .map(|i| (i, ValidatorOperation::AttestEscrow { sequence: 10 }))
            .collect();
        self.block(500, Vec::new(), escrow)?;
        self.truth.next = Some(10);
        for seed in 0..members {
            self.join(seed)?;
        }
        while self.step < STEPS && self.ledger.phase() != Phase::Ended {
            self.act()?;
        }
        self.wind_down()
    }

    /// A member joins by deposit, and sets its disbursal address.
    fn join(&mut self, seed: usize) -> Checked {
        let key = NodeKey::from_bytes(&[0x70 + u8::try_from(seed).unwrap(); 32]);
        let amount = 20_000_000 + self.rng.below(30_000_000);
        self.deposit(0, Some(&key), amount, None);
        let due = self.due();
        self.block(100, Vec::new(), due)?;
        let Some(number) = self.ledger.account_of(&key.public_key()) else {
            return Err(fail("join", format!("member {seed} has no account")));
        };
        let to = Destination {
            account: [0x80 + u8::try_from(seed).unwrap(); 20],
            tag: None,
        };
        let op = self.member_op(number, &key, MemberOperation::SetDisbursal(to));
        self.block(100, alloc::vec![op], Vec::new())?;
        self.members.push(Member { number, key });
        Ok(())
    }

    /// A deposit lands in the escrow; honest validators attest it within
    /// `delay` steps or so, or at once.
    fn deposit(&mut self, tag: u32, key: Option<&NodeKey>, amount: Amount, delay: Option<u64>) {
        self.deposits += 1;
        self.truth.ledger += 1;
        let mut hash = [0xde; 32];
        hash[..8].copy_from_slice(&self.deposits.to_be_bytes());
        let deposit = Deposit {
            transaction: Digest(hash),
            ledger: self.truth.ledger,
            amount,
            sender: [0xaa; 20],
            tag,
            key: key.map(NodeKey::public_key),
        };
        self.truth.escrow += amount;
        let operation = ValidatorOperation::AttestDeposit(deposit);
        match delay {
            Some(delay) => self.attest_honestly(operation, delay),
            None => {
                for index in 0..self.honest() {
                    self.queue.push((self.step, index, operation.clone()));
                }
            }
        }
    }

    /// Queues `operation` for every honest validator, each after a delay
    /// of its own: mostly within `delay` steps, now and then long after.
    fn attest_honestly(&mut self, operation: ValidatorOperation, delay: u64) {
        for index in 0..self.honest() {
            let wait = if self.rng.below(8) == 0 {
                10 + self.rng.below(30)
            } else {
                self.rng.below(delay + 1)
            };
            self.queue
                .push((self.step + wait, index, operation.clone()));
        }
    }

    fn member_op(
        &self,
        number: AccountNumber,
        key: &NodeKey,
        operation: MemberOperation,
    ) -> Operation {
        let sequence = self.ledger.account(number).map_or(0, |a| a.sequence);
        Operation::Member(SignedMemberOperation::sign(
            self.ledger.hub(),
            number,
            sequence,
            MAX_FEE,
            operation,
            key,
        ))
    }

    /// One step: time passes, the issuer may act, the escrow may apply a
    /// withdrawal, members act, faulty validators make things up, and a
    /// block takes it all.
    fn act(&mut self) -> Checked {
        self.step += 1;
        let frozen = self.ledger.frozen().is_some();
        // Now and then a long stretch, longer while frozen, so notices and
        // absences have time to fall due, or not.
        let mut dt = match self.rng.below(30) {
            0 if frozen => 3_600_000 + self.rng.below(2 * 86_400_000),
            0 => 60_000 + self.rng.below(86_400_000),
            1 => 10_000 + self.rng.below(60_000),
            _ => 50 + self.rng.below(1_500),
        };
        // Once the hub shuts down, what is on its way gets in before its
        // end: the escrow's last state is what the payout set pays.
        if self.ledger.phase() != Phase::Running && !self.calm() {
            dt = dt.min(1_500);
        }
        if self.token() && self.rng.below(12) == 0 {
            self.issuer_acts();
        }
        if !self.token() && self.rng.below(20) == 0 {
            // Made up: an XRP hub takes no issuer's act.
            self.truth.ledger += 1;
            let operation = ValidatorOperation::AttestIssuer(Issuer {
                ledger: self.truth.ledger,
                frozen: true,
                fee: false,
            });
            self.queue.push((self.step, 0, operation));
        }
        self.escrow_applies();
        self.faulty_acts();
        let members = self.member_ops();
        let validators = self.due();
        self.block(dt, members, validators)
    }

    /// The issuer freezes or thaws, sets or removes a fee, or claws back.
    fn issuer_acts(&mut self) {
        self.truth.ledger += 1 + u32::try_from(self.rng.below(4)).unwrap_or(0);
        let mut state = self.truth.issuer;
        state.ledger = self.truth.ledger;
        match self.rng.below(10) {
            0..=4 => state.frozen = !state.frozen,
            5..=7 => state.fee = !state.fee,
            _ => {
                // A clawback ends the hub: one late in the run, now and
                // then, so the freezes before it have their time.
                if self.truth.clawbacks.len() >= 2
                    || self.truth.escrow == 0
                    || self.step < STEPS / 2
                    || self.rng.below(3) != 0
                {
                    return;
                }
                let amount = 1 + self.rng.below(self.truth.escrow);
                let mut hash = [0xcb; 32];
                hash[..4].copy_from_slice(&self.truth.ledger.to_be_bytes());
                let clawback = Clawback {
                    transaction: Digest(hash),
                    ledger: self.truth.ledger,
                    amount,
                };
                self.truth.escrow -= amount;
                self.truth.clawbacks.insert(clawback.transaction, amount);
                self.trace.push(format!(
                    "step {}: the issuer claws back {amount}",
                    self.step
                ));
                self.attest_honestly(ValidatorOperation::AttestClawback(clawback), 3);
                return;
            }
        }
        self.trace.push(format!(
            "step {}: the issuer's hold at ledger {}: frozen {}, fee {}",
            self.step, state.ledger, state.frozen, state.fee
        ));
        self.truth.issuer = state;
        self.truth.history.push(state);
        self.attest_honestly(ValidatorOperation::AttestIssuer(state), 3);
    }

    /// The escrow applies the next withdrawal the hub sequenced, while the
    /// issuer holds nothing against it and the hub has not begun to shut
    /// down: it delivers if the escrow holds enough, and fails otherwise.
    fn escrow_applies(&mut self) {
        if self.truth.issuer.holds()
            || self.ledger.phase() != Phase::Running
            || self.rng.below(3) != 0
        {
            return;
        }
        let Some(next) = self.truth.next else {
            return;
        };
        let Some(withdrawal) = self
            .ledger
            .withdrawals()
            .find(|(s, _)| *s == next)
            .map(|(_, w)| w.clone())
        else {
            return;
        };
        let cost = withdrawal.amount + withdrawal.ledger_fee;
        let delivered = self.truth.escrow >= cost;
        self.truth.escrow -= if delivered {
            cost
        } else {
            withdrawal.ledger_fee.min(self.truth.escrow)
        };
        self.truth.applied.insert(next, delivered);
        self.truth.next = Some(next + 1);
        self.attest_honestly(
            ValidatorOperation::AttestWithdrawal {
                escrow_sequence: next,
                delivered,
            },
            3,
        );
    }

    /// Faulty validators attest made-up states and clawbacks, and replay
    /// old states.
    fn faulty_acts(&mut self) {
        for index in self.honest()..self.validators.len() {
            if self.rng.below(6) != 0 {
                continue;
            }
            let operation = match self.rng.below(3) {
                0 => ValidatorOperation::AttestIssuer(Issuer {
                    ledger: u32::try_from(self.rng.below(u64::from(self.truth.ledger) + 20))
                        .unwrap_or(0),
                    frozen: self.rng.below(2) == 0,
                    fee: self.rng.below(2) == 0,
                }),
                1 => {
                    let old = self.truth.history.len() as u64;
                    if old == 0 {
                        continue;
                    }
                    let pick = usize::try_from(self.rng.below(old)).unwrap_or(0);
                    ValidatorOperation::AttestIssuer(self.truth.history[pick])
                }
                _ => {
                    let mut hash = [0xfa; 32];
                    hash[..8].copy_from_slice(&self.rng.next_u64().to_be_bytes());
                    ValidatorOperation::AttestClawback(Clawback {
                        transaction: Digest(hash),
                        ledger: self.truth.ledger,
                        amount: self.rng.below(1_000_000_000),
                    })
                }
            };
            self.queue.push((self.step, index, operation));
        }
    }

    /// The attestations due by this step, oldest first.
    fn due(&mut self) -> Vec<(usize, ValidatorOperation)> {
        let step = self.step;
        let (due, later): (Vec<_>, Vec<_>) =
            self.queue.drain(..).partition(|(at, _, _)| *at <= step);
        self.queue = later;
        let mut due: Vec<(usize, ValidatorOperation)> =
            due.into_iter().map(|(_, index, op)| (index, op)).collect();
        // A block holds either acts that may freeze or thaw the hub or
        // acts that may begin its shutdown, never both, so the model knows
        // which clock a shutdown began by: the rest wait a step.
        let freezes = |op: &ValidatorOperation| matches!(op, ValidatorOperation::AttestIssuer(_));
        let ends = |op: &ValidatorOperation| {
            matches!(
                op,
                ValidatorOperation::AttestClawback(_) | ValidatorOperation::Shutdown
            )
        };
        if due.iter().any(|(_, op)| freezes(op)) && due.iter().any(|(_, op)| ends(op)) {
            let (keep, defer): (Vec<_>, Vec<_>) = due.into_iter().partition(|(_, op)| !ends(op));
            for (index, op) in defer {
                self.queue.push((step + 1, index, op));
            }
            due = keep;
        }
        due
    }

    /// One operation for some members: open, fund or reduce a hold, sign
    /// commitments and claims, claim, redeem, transfer, withdraw, deposit.
    fn member_ops(&mut self) -> Vec<Operation> {
        let mut deadlines = Vec::new();
        let mut others = Vec::new();
        let count = self.members.len();
        for me in 0..count {
            if self.rng.below(3) != 0 {
                continue;
            }
            let other =
                (me + 1 + usize::try_from(self.rng.below(count as u64 - 1)).unwrap_or(0)) % count;
            let (number, payee) = (self.members[me].number, self.members[other].number);
            if self.ledger.account(number).is_none() {
                continue;
            }
            let operation = match self.rng.below(12) {
                0 => match self.hold(number, payee) {
                    None => MemberOperation::HoldOpen {
                        payee,
                        amount: 1_000_000 + self.rng.below(4_000_000),
                    },
                    Some(hold) => MemberOperation::HoldFund {
                        hold,
                        amount: self.rng.below(1_000_000),
                    },
                },
                1 => {
                    let Some(hold) = self.hold(number, payee) else {
                        continue;
                    };
                    // An honest payer asks again only once its last
                    // reduction took effect.
                    if self
                        .ledger
                        .hold(hold)
                        .is_some_and(|h| h.reduction.is_some())
                    {
                        continue;
                    }
                    MemberOperation::HoldReduce {
                        hold,
                        amount: 1 + self.rng.below(800_000),
                    }
                }
                2..=4 => {
                    self.sign_commitments(number, payee);
                    continue;
                }
                5 | 6 => {
                    let Some(claim) = self.claim(payee, number) else {
                        continue;
                    };
                    deadlines.push(self.member_op(payee, &self.members[other].key, claim));
                    continue;
                }
                7 => {
                    let Some(redeem) = self.redeem(number, payee) else {
                        continue;
                    };
                    deadlines.push(self.member_op(payee, &self.members[other].key, redeem));
                    continue;
                }
                8 => MemberOperation::Transfer {
                    to: payee,
                    amount: self.rng.below(200_000),
                },
                9 => MemberOperation::Withdraw {
                    amount: self.ledger.terms().fees.least_payout + self.rng.below(2_000_000),
                    to: Destination {
                        account: [0x90; 20],
                        tag: Some(number.0),
                    },
                },
                10 => {
                    // No member pays into a hub that shuts down, or one it
                    // knows is frozen: its method is unavailable then.
                    // Deposits on their way at a freeze land during it.
                    if !self.winding_down
                        && self.ledger.phase() == Phase::Running
                        && self.ledger.frozen().is_none()
                    {
                        let amount = 1_000 + self.rng.below(5_000_000);
                        self.deposit(number.0, None, amount, Some(4));
                    }
                    continue;
                }
                _ => continue,
            };
            others.push(self.member_op(number, &self.members[me].key, operation));
        }
        deadlines.extend(others);
        deadlines
    }

    fn hold(&self, payer: AccountNumber, payee: AccountNumber) -> Option<HoldNumber> {
        self.ledger
            .holds_of(payer)
            .find(|(_, h)| h.payer == payer && h.payee == payee)
            .map(|(n, _)| n)
    }

    /// The payer signs a few commitments, as for packets it sends; the
    /// payee keeps them with their fulfillments. Mostly they expire within
    /// the longest a connector holds a packet, now and then much later.
    fn sign_commitments(&mut self, payer: AccountNumber, payee: AccountNumber) {
        let Some(hold) = self.hold(payer, payee) else {
            return;
        };
        let Some(key) = self
            .members
            .iter()
            .find(|m| m.number == payer)
            .map(|m| NodeKey::from_bytes(m.key.as_bytes()))
        else {
            return;
        };
        let now = self.ledger.timestamp();
        for _ in 0..=self.rng.below(3) {
            let signing = self.signing.entry(hold).or_default();
            signing.sequence += 1;
            let amount = 100 + self.rng.below(5_000);
            signing.total += amount;
            let sequence = signing.sequence;
            let expiry = if self.rng.below(10) == 0 {
                now + MAX_HOLD_MS + self.rng.below(10 * MAX_HOLD_MS)
            } else {
                now + 1 + self.rng.below(MAX_HOLD_MS)
            };
            let mut fulfillment = [0; 32];
            fulfillment[..8].copy_from_slice(&self.rng.next_u64().to_be_bytes());
            let commitment = Commitment {
                hold,
                sequence,
                amount,
                condition: Sha256::digest(fulfillment).into(),
                expiry,
            };
            let signed = Signed::sign(commitment, self.ledger.hub(), &key);
            self.held.entry(hold).or_default().push(Held {
                commitment: signed,
                fulfillment,
            });
        }
    }

    /// A claim of up to three commitments the payee holds that have not
    /// expired.
    fn claim(&mut self, payee: AccountNumber, payer: AccountNumber) -> Option<MemberOperation> {
        let hold = self.hold(payer, payee)?;
        let now = self.ledger.timestamp();
        let through = self.ledger.hold(hold)?.through;
        let list = self.held.get_mut(&hold)?;
        list.retain(|h| h.commitment.value.expiry > now && h.commitment.value.sequence > through);
        if list.is_empty() {
            return None;
        }
        let take = 1 + usize::try_from(self.rng.below(3)).unwrap_or(0);
        let start = usize::try_from(self.rng.below(list.len() as u64)).unwrap_or(0);
        let fulfilled: Vec<Fulfilled> = list
            .iter()
            .skip(start)
            .take(take)
            .map(|h| Fulfilled {
                commitment: h.commitment,
                fulfillment: h.fulfillment,
            })
            .collect();
        Some(MemberOperation::Claim { hold, fulfilled })
    }

    /// The payer's claim over every commitment it signed, which the payee
    /// redeems.
    fn redeem(&mut self, payer: AccountNumber, payee: AccountNumber) -> Option<MemberOperation> {
        let hold = self.hold(payer, payee)?;
        let signing = *self.signing.get(&hold)?;
        let key = self.members.iter().find(|m| m.number == payer)?;
        let claim = Claim {
            hold,
            total: signing.total,
            through: signing.sequence,
        };
        Some(MemberOperation::Redeem(Signed::sign(
            claim,
            self.ledger.hub(),
            &key.key,
        )))
    }

    /// Ends the run: the issuer lifts its hold, what is on its way is
    /// attested, the hub shuts down, by a clawback in some token runs or
    /// by the validators' votes, and runs until it ended; then the payout
    /// set is checked.
    fn wind_down(&mut self) -> Checked {
        self.winding_down = true;
        if self.truth.issuer.holds() {
            self.truth.ledger += 1;
            let state = Issuer {
                ledger: self.truth.ledger,
                frozen: false,
                fee: false,
            };
            self.truth.issuer = state;
            self.truth.history.push(state);
            self.attest_honestly(ValidatorOperation::AttestIssuer(state), 3);
        }
        let mut shutdown = false;
        for _ in 0..WIND_DOWN_STEPS {
            if self.ledger.phase() == Phase::Ended {
                break;
            }
            self.step += 1;
            let calm = self.calm() && self.ledger.frozen().is_none();
            if calm && !shutdown && self.ledger.phase() == Phase::Running {
                shutdown = true;
                if self.token() && self.truth.escrow > 0 && self.rng.below(2) == 0 {
                    self.truth.ledger += 1;
                    let amount = 1 + self.rng.below(self.truth.escrow);
                    let clawback = Clawback {
                        transaction: Digest([0xce; 32]),
                        ledger: self.truth.ledger,
                        amount,
                    };
                    self.truth.escrow -= amount;
                    self.truth.clawbacks.insert(clawback.transaction, amount);
                    self.attest_honestly(ValidatorOperation::AttestClawback(clawback), 2);
                } else {
                    for index in 0..self.honest() {
                        self.queue
                            .push((self.step, index, ValidatorOperation::Shutdown));
                    }
                }
            }
            self.escrow_applies();
            let dt = if calm && self.ledger.phase() != Phase::Running {
                600_000
            } else {
                1_000
            };
            let validators = self.due();
            self.block(dt, Vec::new(), validators)?;
        }
        if self.ledger.phase() != Phase::Ended {
            return Err(SimOutcome::liveness(
                "ended",
                format!(
                    "the hub has not ended: {:?} frozen {:?} issuer {:?} truth {:?} queue {:?} votes {:?}",
                    self.ledger.phase(),
                    self.ledger.frozen(),
                    self.ledger.issuer(),
                    self.truth.issuer,
                    self.queue
                        .iter()
                        .map(|(a, i, _)| (*a, *i))
                        .collect::<Vec<_>>(),
                    self.ledger.issuer_votes
                ),
            ));
        }
        self.check_payout()
    }

    /// Executes a block `dt` after the last, with `members`' operations
    /// first and then `validators`', and checks it.
    fn block(
        &mut self,
        dt: Millis,
        members: Vec<Operation>,
        validators: Vec<(usize, ValidatorOperation)>,
    ) -> Checked {
        let mut operations = members;
        let split = operations.len();
        for (index, operation) in validators {
            self.sequences[index] += 1;
            operations.push(Operation::Validator(SignedValidatorOperation::sign(
                self.ledger.hub(),
                ValidatorIndex::new(index).unwrap(),
                self.sequences[index],
                operation,
                &self.validators[index],
            )));
        }
        let height = self.ledger.height() + 1;
        // Honest validators' votes for the last block are recorded; a
        // faulty one's only now and then, or never, so its absence counts.
        let mut voters = Signers::NONE;
        if height > 1 {
            for index in 0..self.validators.len() {
                if index < self.honest() || (!self.silent && self.rng.below(50) == 0) {
                    voters = voters.with(ValidatorIndex::new(index).unwrap());
                }
            }
        }
        let votes = if height > 1 {
            alloc::vec![(height - 1, voters)]
        } else {
            Vec::new()
        };
        let timestamp = self.ledger.timestamp() + dt.max(1);
        let before = self.ledger.clone();
        let outcomes = self
            .ledger
            .apply(&Block {
                height,
                timestamp,
                operations: &operations,
                votes: &votes,
            })
            .map_err(|error| fail("block", format!("block {height} not executed: {error:?}")))?;
        let (member_ops, validator_ops) = operations.split_at(split);
        let (member_outcomes, validator_outcomes) = outcomes.split_at(split);
        self.check_members(&before, member_ops, member_outcomes)?;
        self.check_validators(&before, validator_ops, validator_outcomes)?;
        self.check_clock(&before, voters, validator_ops)?;
        self.learn(member_ops, member_outcomes);
        Ok(())
    }

    /// What the members learn from their outcomes.
    fn learn(&mut self, operations: &[Operation], outcomes: &[Outcome]) {
        for (operation, outcome) in operations.iter().zip(outcomes) {
            let Operation::Member(signed) = operation else {
                continue;
            };
            match (&signed.operation, outcome) {
                (
                    MemberOperation::Claim { hold, fulfilled },
                    Outcome::Applied { .. } | Outcome::Failed { .. },
                ) => {
                    let claimed: BTreeSet<u64> = fulfilled
                        .iter()
                        .map(|f| f.commitment.value.sequence)
                        .collect();
                    if let Some(list) = self.held.get_mut(hold) {
                        list.retain(|h| !claimed.contains(&h.commitment.value.sequence));
                    }
                }
                (MemberOperation::Redeem(claim), Outcome::Applied { .. }) => {
                    let through = claim.value.through;
                    if let Some(list) = self.held.get_mut(&claim.value.hold) {
                        list.retain(|h| h.commitment.value.sequence > through);
                    }
                }
                _ => {}
            }
        }
    }

    fn bad(&self, check: &'static str, message: &str) -> SimOutcome {
        fail(check, format!("block {}: {message}", self.ledger.height()))
    }

    /// A frozen hub takes only claims of commitments within the freeze's
    /// window, refusing the rest as waiting; a hub not frozen refuses
    /// nothing as waiting. Members' operations come first in a block, so
    /// they run by whether the hub was frozen before it.
    fn check_members(
        &mut self,
        before: &Ledger,
        operations: &[Operation],
        outcomes: &[Outcome],
    ) -> Checked {
        let frozen = before.frozen();
        let window = frozen.map(|since| {
            since + before.terms().timers.margin + MAX_HOLD_MS + Config::DEFAULT.precision_ms
        });
        for (operation, outcome) in operations.iter().zip(outcomes) {
            let Operation::Member(signed) = operation else {
                continue;
            };
            let waited = *outcome == Outcome::Refused(Refusal::Frozen);
            let waits = match (&signed.operation, window) {
                (_, None) => false,
                (MemberOperation::Claim { fulfilled, .. }, Some(end)) => {
                    fulfilled.iter().any(|f| f.commitment.value.expiry > end)
                }
                (_, Some(_)) => true,
            };
            // Refused for another reason first, a fee it cannot pay, is no
            // wait.
            if (waits && !matches!(outcome, Outcome::Refused(_))) || (waited && !waits) {
                return Err(self.bad(
                    "frozen",
                    &format!(
                        "{:?} came to {outcome:?}, the hub frozen since {frozen:?}",
                        signed.operation
                    ),
                ));
            }
            if waited {
                self.count("waited");
            } else if frozen.is_some() && matches!(outcome, Outcome::Applied { .. }) {
                self.count("window-claim");
            }
        }
        // While frozen throughout, nothing is credited or settled. A block
        // may thaw the hub and freeze it again: then it was not frozen
        // throughout.
        if frozen.is_some() && self.ledger.frozen() == frozen {
            let deposited = |l: &Ledger| l.accounts().map(|(_, a)| a.deposited).sum::<Amount>();
            let sequences = |l: &Ledger| l.withdrawals().map(|(s, _)| s).collect::<Vec<_>>();
            if deposited(before) != deposited(&self.ledger)
                || sequences(before) != sequences(&self.ledger)
            {
                return Err(self.bad("frozen", "a deposit or withdrawal settled while frozen"));
            }
        }
        Ok(())
    }

    /// An XRP hub refuses the issuer's acts; a token hub's state in force
    /// is one its issuer was in, and only true clawbacks count, each once.
    fn check_validators(
        &mut self,
        before: &Ledger,
        operations: &[Operation],
        outcomes: &[Outcome],
    ) -> Checked {
        for (operation, outcome) in operations.iter().zip(outcomes) {
            let Operation::Validator(signed) = operation else {
                continue;
            };
            let issuers = matches!(
                signed.operation,
                ValidatorOperation::AttestIssuer(_) | ValidatorOperation::AttestClawback(_)
            );
            if issuers && !self.token() && *outcome != Outcome::Refused(Refusal::NotAllowed) {
                return Err(self.bad("xrp", &format!("an XRP hub took {:?}", signed.operation)));
            }
            if matches!(
                outcome,
                Outcome::Failed {
                    reason: Failure::Stale,
                    ..
                }
            ) {
                self.count("stale-vote");
            }
        }
        let issuer = *self.ledger.issuer();
        if issuer != Issuer::default() && !self.truth.history.contains(&issuer) {
            return Err(self.bad(
                "issuer",
                &format!("{issuer:?} is no state the issuer was in"),
            ));
        }
        if issuer.fee && !before.issuer().fee {
            self.count("fee");
        }
        let true_clawed = self
            .ledger
            .recent_clawbacks
            .keys()
            .map(|hash| self.truth.clawbacks.get(hash).copied())
            .sum::<Option<Amount>>();
        if true_clawed != Some(self.ledger.escrow().clawed_back) {
            return Err(self.bad(
                "clawback",
                &format!(
                    "{} clawed back, the true clawbacks applied make {true_clawed:?}",
                    self.ledger.escrow().clawed_back
                ),
            ));
        }
        if self.ledger.escrow().clawed_back > before.escrow().clawed_back {
            self.count("clawback");
        }
        Ok(())
    }

    /// The model of the hub's clock: the block's votes, what fell due at
    /// its start, what its operations began, then a freeze or a thaw; and
    /// the deadlines the ledger keeps are the model's, in the ledger's
    /// time.
    fn check_clock(
        &mut self,
        before: &Ledger,
        voters: Signers,
        validators: &[Operation],
    ) -> Checked {
        let frozen = before.frozen();
        let hub_time = frozen.unwrap_or(self.ledger.timestamp()) - self.clock.paused;
        for index in voters.iter() {
            self.clock.recorded[index.get()] = hub_time;
        }
        self.check_reductions(before, hub_time)?;
        let voted = validators.iter().any(|op| {
            matches!(op, Operation::Validator(signed)
                if signed.operation == ValidatorOperation::Shutdown)
        });
        self.check_phase(before, hub_time, voted)?;
        match (frozen, self.ledger.frozen()) {
            (None, Some(since)) => {
                if since != self.ledger.timestamp() {
                    return Err(self.bad("frozen", "frozen from another block"));
                }
                self.count("freeze");
                self.clock.caught = self.clock.reductions.keys().copied().collect();
            }
            (Some(since), None) => {
                self.clock.paused += self.ledger.timestamp() - since;
                self.count("thaw");
            }
            (Some(since), Some(again)) if again != since => {
                // Thawed and frozen again within the block.
                self.clock.paused += self.ledger.timestamp() - since;
                self.count("thaw");
                self.count("freeze");
                self.clock.caught = self.clock.reductions.keys().copied().collect();
            }
            _ => {}
        }
        let paused = self.clock.paused;
        for (number, hold) in self.ledger.holds() {
            let Some(reduction) = hold.reduction else {
                continue;
            };
            let model = self.clock.reductions.get(&number).map(|due| due + paused);
            if model != Some(reduction.at) {
                return Err(self.bad(
                    "reduction",
                    &format!(
                        "hold {} reduced at {}, the model says {model:?}",
                        number.0, reduction.at
                    ),
                ));
            }
        }
        if let Phase::ShuttingDown { ends } = self.ledger.phase()
            && self.clock.ends.map(|model| model + paused) != Some(ends)
        {
            return Err(self.bad(
                "shutdown",
                &format!("it ends at {ends}, the model says {:?}", self.clock.ends),
            ));
        }
        Ok(())
    }

    /// Reductions fall due by the model's clock only, so one pending at a
    /// freeze only after the thaw. A new one starts the model's notice:
    /// asked by a member, so while the hub was not frozen.
    fn check_reductions(&mut self, before: &Ledger, hub_time: Millis) -> Checked {
        let notice = self.ledger.terms().timers.hold_notice;
        for (number, hold) in before.holds() {
            let Some(reduction) = hold.reduction else {
                continue;
            };
            // A thaw moves it; only its taking effect ends it, since an
            // honest payer asks again only once it did.
            let _ = reduction;
            let applied = self
                .ledger
                .hold(number)
                .is_none_or(|h| h.reduction.is_none());
            let due = self.clock.reductions.get(&number).copied().unwrap_or(0);
            if applied != (hub_time >= due) {
                return Err(self.bad(
                    "reduction",
                    &format!(
                        "hold {} due at hub time {due}, applied {applied} at {hub_time}",
                        number.0
                    ),
                ));
            }
            if applied {
                self.clock.reductions.remove(&number);
                if self.clock.caught.remove(&number) {
                    self.count("reduction-after-thaw");
                }
            }
        }
        let now = self.ledger.timestamp() - self.clock.paused;
        for (number, hold) in self.ledger.holds() {
            if hold.reduction.is_some() && !self.clock.reductions.contains_key(&number) {
                self.clock.reductions.insert(number, now + notice);
            }
        }
        Ok(())
    }

    /// A shutdown begins by an absence the model counts, at the block's
    /// start, or by a clawback or a quorum's votes during it, and ends once
    /// the model's clock passed its notice.
    fn check_phase(&mut self, before: &Ledger, hub_time: Millis, voted: bool) -> Checked {
        let timers = self.ledger.terms().timers;
        match (before.phase(), self.ledger.phase()) {
            (Phase::ShuttingDown { .. }, after) => {
                let due = self.clock.ends.is_some_and(|ends| hub_time >= ends);
                if due != (after == Phase::Ended) {
                    return Err(self.bad(
                        "shutdown",
                        &format!("its end due {due} at hub time {hub_time}, {after:?}"),
                    ));
                }
            }
            (Phase::Running, after) => {
                let absent = self
                    .clock
                    .recorded
                    .iter()
                    .any(|last| hub_time.saturating_sub(*last) > timers.absence);
                let clawed = self.ledger.escrow().clawed_back > before.escrow().clawed_back;
                let shutting = matches!(after, Phase::ShuttingDown { .. });
                if (absent || clawed) && !shutting || shutting && !(absent || clawed || voted) {
                    return Err(self.bad(
                        "shutdown",
                        &format!("absent {absent}, clawed back {clawed}, voted {voted}: {after:?}"),
                    ));
                }
                if shutting {
                    if absent {
                        self.count("absence-shutdown");
                    }
                    // A block's acts never both freeze and end the hub,
                    // so it began by the clock the block began with.
                    self.clock.ends = Some(hub_time + timers.shutdown_notice);
                }
            }
            (Phase::Ended, _) => return Err(self.bad("ended", "a block after the end")),
        }
        Ok(())
    }

    /// The payout set of the last block: the withdrawals not settled
    /// first, in full, then each account with a disbursal address its share
    /// of what the escrow holds beyond them, pro rata to what the hub owes
    /// it, rounded down, and never more than the escrow holds.
    fn check_payout(&mut self) -> Checked {
        let at = |message: String| fail("payout", message);
        let ledger = &self.ledger;
        if ledger.escrow().held() != self.truth.escrow {
            return Err(at(format!(
                "the hub holds {} by its books, the escrow {}",
                ledger.escrow().held(),
                self.truth.escrow
            )));
        }
        let terms = ledger.terms().clone();
        let set = payout_set(ledger).ok_or_else(|| at("no payout set".into()))?;
        let sequenced: Vec<(u32, Amount, Amount)> = ledger
            .withdrawals()
            .map(|(s, w)| (s, w.amount, w.ledger_fee))
            .collect();
        let paid_withdrawals: Vec<(u32, Amount)> = set
            .transactions
            .iter()
            .filter_map(|t| match t {
                PayoutTransaction::Withdrawal(p) => {
                    Some((p.sequence, terms.hub_amount(&p.amount)?))
                }
                _ => None,
            })
            .collect();
        let expected: Vec<(u32, Amount)> = sequenced.iter().map(|(s, a, _)| (*s, *a)).collect();
        if paid_withdrawals != expected {
            return Err(at(format!(
                "withdrawals {paid_withdrawals:?}, sequenced {expected:?}"
            )));
        }
        let withdrawn: Amount = sequenced.iter().map(|(_, a, f)| a + f).sum();
        let have = self.truth.escrow.saturating_sub(withdrawn);
        let due = ledger.supply() - withdrawn;
        let mut owed: BTreeMap<AccountNumber, u128> = ledger
            .accounts()
            .map(|(n, a)| (n, u128::from(a.balance + a.reserve)))
            .collect();
        for (_, hold) in ledger.holds() {
            *owed.entry(hold.payer).or_default() += u128::from(hold.amount + hold.reserve);
        }
        let fees = terms.fees;
        let mut expected = Vec::new();
        for (number, total) in &owed {
            let share = if have >= due {
                *total
            } else {
                total * u128::from(have) / u128::from(due)
            };
            let share = Amount::try_from(share).unwrap();
            let Some(destination) = ledger.account(*number).and_then(|a| a.disbursal) else {
                continue;
            };
            let amount = share.saturating_sub(fees.ledger);
            if share < fees.ledger || amount < fees.least_payout {
                continue;
            }
            expected.push((destination.account, amount));
        }
        let paid: Vec<([u8; 20], Amount)> = set
            .transactions
            .iter()
            .filter_map(|t| match t {
                PayoutTransaction::Payout(p) => {
                    Some((p.destination.0, terms.hub_amount(&p.amount)?))
                }
                _ => None,
            })
            .collect();
        if paid != expected {
            return Err(at(format!(
                "paid {paid:?}, each account's share is {expected:?}"
            )));
        }
        let out: Amount = paid.iter().map(|(_, a)| a + fees.ledger).sum();
        if out > have {
            return Err(at(format!(
                "the set pays {out} beyond the withdrawals, the escrow holds {have} beyond them"
            )));
        }
        if have < due {
            self.count("pro-rata");
        }
        self.trace.push(format!(
            "the payout set pays {} withdrawals and {} accounts, {out} of {}",
            sequenced.len(),
            paid.len(),
            self.truth.escrow
        ));
        Ok(())
    }
}

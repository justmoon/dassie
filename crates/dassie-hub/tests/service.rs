//! Four validators of one hub, on a network without loss: they attest the
//! escrow and a deposit, take a member's operations, agree on every block,
//! and tell the member what happened in statements it trusts once f + 1
//! of them signed the same.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use std::collections::VecDeque;

use dassie_hub::client::{Agreement, Request, Statement};
use dassie_hub::consensus::{PeerId, Stored};
use dassie_hub::digest::Digest;
use dassie_hub::escrow::{ValidatorKeys, escrow_master};
use dassie_hub::ledger::Ledger;
use dassie_hub::ledger::Outcome;
use dassie_hub::operation::{
    AccountNumber, Deposit, Destination, MemberOperation, Operation, SignedMemberOperation,
    ValidatorOperation,
};
use dassie_hub::rules::{Acceptance, Fees, Limits, Rules, Terms, Timers};
use dassie_hub::service::payload::Payload;
use dassie_hub::service::wire::Message;
use dassie_hub::service::{Action, ResumeError, Validator, genesis_block};
use dassie_identity::{NodeKey, PublicKey};
use dassie_oer::{Decode, Encode};

const NONCE: [u8; 32] = [7; 32];
const START: u64 = 1_700_000_000_000;
const MEMBER: PeerId = PeerId(100);

fn terms(sponsor: &NodeKey) -> Terms {
    Terms {
        nonce: NONCE,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow: escrow_master(sponsor, &NONCE).unwrap().account().0,
        asset: dassie_hub::rules::Asset::Xrp,
        fees: Fees {
            operation: 10,
            ledger: 100,
            gas: 0,
            reserve_share_ppm: 100_000,
            account_reserve: 2_000_000,
            hold_reserve: 200_000,
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
            margin: 500,
            allowance: 300,
            view_timeout: 200,
        },
    }
}

struct Network {
    validators: Vec<Validator>,
    /// What each validator's host persisted: its consensus state, and the
    /// state after its last final block.
    stored: Vec<(Stored, Vec<u8>)>,
    node_keys: Vec<NodeKey>,
    nodes: Vec<dassie_identity::PublicKey>,
    /// Each validator's consensus key, by its place in the network.
    consensus: Vec<dassie_identity::PublicKey>,
    rules: Rules,
    queue: VecDeque<(usize, PeerId, Vec<u8>)>,
    member: Vec<Vec<u8>>,
    submitted: Vec<(usize, Vec<u8>)>,
    /// Whether shares to validator 0 wait in `held` until released.
    hold_shares: bool,
    held: Vec<(PeerId, Vec<u8>)>,
    payouts: usize,
    /// A validator cut off from the others: nothing reaches it or leaves it.
    cut: Option<usize>,
    /// Validators that are behind, with the final block whose state they
    /// asked for.
    behind: Vec<(usize, Box<dassie_hub::consensus::Finalized>)>,
    /// States taken that way.
    caught_up: usize,
    /// Evidence each validator reported.
    evidence: Vec<(usize, dassie_hub::consensus::Evidence)>,
    /// How many operations of its own each validator passed on.
    operations: Vec<usize>,
    /// The height of each payout set each validator signed, in order.
    payout_heights: Vec<(usize, u64)>,
    /// Whether every vote to finalize, finalization and catch-up is lost,
    /// so blocks are notarized and none becomes final.
    no_finality: bool,
    now: u64,
}

impl Network {
    fn new() -> Self {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let terms = terms(&sponsor);
        let nodes: Vec<NodeKey> = (1..=4).map(|i| NodeKey::from_bytes(&[i; 32])).collect();
        let acceptances: Vec<Acceptance> = nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                ValidatorKeys::derive(node, &NONCE)
                    .unwrap()
                    .accept(node, &terms, format!("v{i}.example:443"))
                    .unwrap()
            })
            .collect();
        let rules = Rules {
            terms,
            validators: acceptances,
        };
        let validators: Vec<Validator> = nodes
            .iter()
            .map(|node| {
                let keys = ValidatorKeys::derive(node, &NONCE).unwrap();
                Validator::genesis(rules.clone(), keys, START, START).unwrap()
            })
            .collect();
        let set = rules.validator_set().unwrap();
        let genesis = Stored::genesis(genesis_block(&rules, START), set);
        let stored = validators
            .iter()
            .map(|v| (genesis.clone(), v.ledger().snapshot()))
            .collect();
        Self {
            validators,
            stored,
            node_keys: nodes
                .iter()
                .map(|n| NodeKey::from_bytes(n.as_bytes()))
                .collect(),
            nodes: nodes.iter().map(NodeKey::public_key).collect(),
            consensus: rules.validators.iter().map(|a| a.consensus).collect(),
            rules,
            queue: VecDeque::new(),
            member: Vec::new(),
            submitted: Vec::new(),
            hold_shares: false,
            held: Vec::new(),
            payouts: 0,
            cut: None,
            behind: Vec::new(),
            caught_up: 0,
            evidence: Vec::new(),
            operations: vec![0; 4],
            payout_heights: Vec::new(),
            no_finality: false,
            now: START,
        }
    }

    fn collect(&mut self, from: usize) {
        while let Some(action) = self.validators[from].poll() {
            let sender = PeerId(u64::try_from(from).unwrap());
            match action {
                Action::Validators(bytes) => {
                    if matches!(Message::decode_exact(&bytes), Ok(Message::Operation(_))) {
                        self.operations[from] += 1;
                    }
                    for to in (0..self.validators.len()).filter(|to| *to != from) {
                        self.queue.push_back((to, sender, bytes.clone()));
                    }
                }
                Action::Validator(key, bytes) => {
                    let to = self.consensus.iter().position(|k| *k == key).unwrap();
                    self.queue.push_back((to, sender, bytes));
                }
                Action::Peer(peer, bytes) if peer == MEMBER => self.member.push(bytes),
                Action::Peer(peer, bytes) => {
                    self.queue
                        .push_back((usize::try_from(peer.0).unwrap(), sender, bytes));
                }
                Action::Submit(blob) => self.submitted.push((from, blob)),
                Action::Payout(set, _) => {
                    self.payouts += 1;
                    self.payout_heights.push((from, set.height));
                }
                Action::Behind(finalized) => self.behind.push((from, finalized)),
                Action::Evidence(evidence) => self.evidence.push((from, *evidence)),
            }
        }
        self.persist(from);
    }

    /// Persists a step as a host does: its batches folded into the stored
    /// consensus state, and the ledger after each final block.
    fn persist(&mut self, from: usize) {
        let (stored, snapshot) = &mut self.stored[from];
        for batch in self.validators[from].take_batches() {
            stored.apply(batch);
        }
        let ledger = self.validators[from].ledger();
        assert_eq!(
            ledger.height(),
            stored.tip.block.height,
            "the ledger and the stored tip are at one height after a step"
        );
        *snapshot = ledger.snapshot();
    }

    /// Restarts validator `index` from what its host persisted, its stored
    /// state read back from its encoding, and its ledger restored against
    /// the root its tip names.
    fn restart(&mut self, index: usize) {
        let (stored, snapshot) = &self.stored[index];
        let stored = Stored::decode_exact(&stored.to_vec()).unwrap();
        assert_eq!(
            &stored, &self.stored[index].0,
            "the stored state round-trips"
        );
        let keys = ValidatorKeys::derive(&self.node_keys[index], &NONCE).unwrap();
        let ledger = if stored.tip.block.height == 0 {
            Ledger::genesis(self.rules.clone(), START).unwrap()
        } else {
            let root = Payload::decode_exact(&stored.tip.block.payload)
                .unwrap()
                .state;
            Ledger::restore(snapshot, &root).unwrap()
        };
        self.validators[index] = Validator::resume(keys, stored, ledger, self.now).unwrap();
        self.collect(index);
    }

    /// Runs the network for `ms` of time, a millisecond at a time.
    fn run(&mut self, ms: u64) {
        for _ in 0..ms {
            self.now += 1;
            for i in 0..self.validators.len() {
                if self.validators[i]
                    .poll_timeout()
                    .is_some_and(|at| at <= self.now)
                {
                    self.validators[i].tick(self.now);
                }
                self.collect(i);
            }
            while let Some((to, from, bytes)) = self.queue.pop_front() {
                let sender = usize::try_from(from.0).ok();
                if self.cut.is_some() && (self.cut == Some(to) || self.cut == sender) {
                    continue;
                }
                if self.no_finality && carries_finality(&bytes) {
                    continue;
                }
                let share = matches!(Message::decode_exact(&bytes), Ok(Message::Share { .. }));
                if self.hold_shares && to == 0 && share {
                    self.held.push((from, bytes));
                    continue;
                }
                let node = self.nodes.get(usize::try_from(from.0).unwrap()).copied();
                self.validators[to].handle(self.now, from, node.as_ref(), &bytes);
                self.collect(to);
            }
        }
        for validator in &self.validators {
            validator.check_invariants();
        }
    }

    /// Hands each validator that is behind the state it asked for, from a
    /// validator whose final state is at that block, as a host fetches it.
    fn serve_states(&mut self) {
        for (to, finalized) in core::mem::take(&mut self.behind) {
            let height = finalized.block.height;
            let from = (0..self.validators.len())
                .find(|i| *i != to && self.validators[*i].ledger().height() == height);
            if let Some(from) = from {
                let snapshot = self.validators[from].ledger().snapshot();
                self.validators[to]
                    .catch_up(self.now, *finalized, &snapshot)
                    .unwrap();
                self.caught_up += 1;
                self.collect(to);
            }
        }
    }

    fn attest_all(&mut self, operation: &ValidatorOperation) {
        for i in 0..self.validators.len() {
            self.validators[i].attest(self.now, operation.clone());
            self.collect(i);
        }
    }

    fn request(&mut self, request: &Request) {
        let bytes = Message::Request(request.clone()).to_vec();
        for i in 0..self.validators.len() {
            self.validators[i].handle(self.now, MEMBER, None, &bytes);
            self.collect(i);
        }
    }
}

#[test]
fn validators_agree_and_tell_the_member() {
    let mut net = Network::new();
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(2_000);
    assert!(net.validators.iter().all(|v| v.ledger().escrow().checked));
    let alice = NodeKey::from_bytes(&[31; 32]);
    let deposit = Deposit {
        transaction: Digest([1; 32]),
        ledger: 10,
        amount: 50_000_000,
        sender: [3; 20],
        tag: 0,
        key: Some(alice.public_key()),
    };
    net.attest_all(&ValidatorOperation::AttestDeposit(deposit));
    net.run(2_000);
    let number = net.validators[0]
        .ledger()
        .account_of(&alice.public_key())
        .unwrap();
    assert_eq!(number, AccountNumber(5));

    // Alice subscribes and sets her disbursal address.
    net.request(&Request::Since {
        account: number,
        from: 1,
    });
    let hub = *net.validators[0].ledger().hub();
    let disbursal = Operation::Member(SignedMemberOperation::sign(
        &hub,
        number,
        0,
        10,
        MemberOperation::SetDisbursal(Destination {
            account: [9; 20],
            tag: None,
        }),
        &alice,
    ));
    net.request(&Request::Submit(disbursal.clone()));
    net.run(2_000);

    // Every validator executed it, and validators at one height agree.
    for validator in &net.validators {
        let account = validator.ledger().account(number).unwrap();
        assert!(account.disbursal.is_some(), "the operation was executed");
        assert!(validator.ledger().height() > 3, "blocks keep coming");
    }
    let top = net
        .validators
        .iter()
        .map(|v| v.ledger().height())
        .min()
        .unwrap();
    let roots: Vec<Digest> = net
        .validators
        .iter()
        .filter(|v| v.ledger().height() == top)
        .map(|v| v.ledger().state_root())
        .collect();
    assert!(
        roots.windows(2).all(|w| w[0] == w[1]),
        "validators disagree"
    );

    // Alice asks again, as her account polls; what three validators
    // signed, she believes.
    net.request(&Request::Since {
        account: number,
        from: 1,
    });
    let set = net.validators[0].ledger().validators().clone();
    let mut agreement = Agreement::new(hub, set, 4096);
    let mut agreed = Vec::new();
    for bytes in &net.member {
        let Ok(Message::Statements(list)) = Message::decode_exact(bytes) else {
            panic!("not statements");
        };
        for signed in list {
            if let Some(statement) = agreement.add(signed) {
                agreed.push(statement);
            }
        }
    }
    let executed = agreed.iter().any(|statement| {
        matches!(statement, Statement::Executed { operation, outcome: Outcome::Applied { .. }, .. }
            if *operation == disbursal)
    });
    assert!(executed, "the member learned its operation applied");
    assert!(agreed.iter().any(|s| matches!(s, Statement::Final { .. })));
}

/// A funded member with a disbursal address.
fn member(net: &mut Network, seed: u8) -> (AccountNumber, NodeKey) {
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(1_000);
    let key = NodeKey::from_bytes(&[seed; 32]);
    let deposit = Deposit {
        transaction: Digest([seed; 32]),
        ledger: 10,
        amount: 50_000_000,
        sender: [3; 20],
        tag: 0,
        key: Some(key.public_key()),
    };
    net.attest_all(&ValidatorOperation::AttestDeposit(deposit));
    net.run(1_000);
    let number = net.validators[0]
        .ledger()
        .account_of(&key.public_key())
        .unwrap();
    let hub = *net.validators[0].ledger().hub();
    let disbursal = MemberOperation::SetDisbursal(Destination {
        account: [9; 20],
        tag: None,
    });
    let op = Operation::Member(SignedMemberOperation::sign(
        &hub, number, 0, 10, disbursal, &key,
    ));
    net.request(&Request::Submit(op));
    net.run(1_000);
    (number, key)
}

// spec/designs/settlement-hubs.md#10-consensus: what the engine proves
// against validators that broke the rules reaches the host, to publish. A
// validator that votes for two blocks in one view is named, and no other.
#[test]
fn a_validator_reports_the_evidence_its_engine_finds() {
    use dassie_hub::consensus::wire::Message as Consensus;
    use dassie_hub::consensus::{Vote, VoteKind};
    use dassie_hub::validators::ValidatorIndex;

    let mut net = Network::new();
    let hub = net.rules.id().0;
    let liar = ValidatorKeys::derive(&net.node_keys[2], &NONCE)
        .unwrap()
        .consensus;
    let signer = ValidatorIndex::new(2).unwrap();
    for block in [Digest([1; 32]), Digest([2; 32])] {
        let vote = Vote::sign(&hub, &liar, signer, VoteKind::Notarize, (0, 1), block);
        let bytes = Message::Consensus(Consensus::Vote(vote).to_vec()).to_vec();
        let node = net.nodes[2];
        net.validators[0].handle(net.now, PeerId(2), Some(&node), &bytes);
        net.collect(0);
    }
    let named: Vec<_> = net
        .evidence
        .iter()
        .map(|(by, evidence)| (*by, evidence.culprits()))
        .collect();
    assert_eq!(
        named,
        [(0, dassie_hub::validators::Signers::NONE.with(signer))]
    );
}

// A validator's operations reach the others each on its own, in any order.
// Should a leader take a later one of a validator's alone, the earlier one,
// a deposit's attestation here, can never execute: each validator attests
// it again, above its last executed sequence, and the deposit is credited.
#[test]
fn an_attestation_a_later_one_passed_is_attested_again() {
    let mut net = Network::new();
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(2_000);
    let deposit = |seed: u8| Deposit {
        transaction: Digest([seed; 32]),
        ledger: 10,
        amount: 50_000_000,
        sender: [3; 20],
        tag: 0,
        key: Some(NodeKey::from_bytes(&[seed; 32]).public_key()),
    };
    // Every validator's attestation of the first deposit is lost on its way
    // to the others; that of the second is not.
    for i in 0..net.validators.len() {
        let now = net.now;
        assert!(net.validators[i].attest(now, ValidatorOperation::AttestDeposit(deposit(41))));
        while net.validators[i].poll().is_some() {}
        net.persist(i);
        net.validators[i].attest(now, ValidatorOperation::AttestDeposit(deposit(42)));
        net.collect(i);
    }
    net.run(4_000);
    let ledger = net.validators[0].ledger();
    for seed in [41, 42] {
        let key = NodeKey::from_bytes(&[seed; 32]).public_key();
        assert!(
            ledger.account_of(&key).is_some(),
            "the deposit of {seed} was credited"
        );
    }
}

/// A deposit to attest, crediting the account of `seed`'s key.
fn deposit_of(seed: u8) -> Deposit {
    Deposit {
        transaction: Digest([seed; 32]),
        ledger: 10,
        amount: 50_000_000,
        sender: [3; 20],
        tag: 0,
        key: Some(NodeKey::from_bytes(&[seed; 32]).public_key()),
    }
}

// A validator that takes a peer's state skips the blocks below it, and
// with them what of its own they executed: done, it is not attested
// again, where it would only take the validator's room in its own pool.
#[test]
fn what_a_caught_up_validator_s_skipped_blocks_executed_is_not_attested_again() {
    let mut net = Network::new();
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(1_000);
    for i in 0..4 {
        let now = net.now;
        assert!(net.validators[i].attest(now, ValidatorOperation::AttestDeposit(deposit_of(51))));
        net.collect(i);
    }
    // Every attestation reaches the others but validator 3, which hears
    // nothing more.
    while let Some((to, from, bytes)) = net.queue.pop_front() {
        if to != 3 {
            let node = net.nodes[usize::try_from(from.0).unwrap()];
            net.validators[to].handle(net.now, from, Some(&node), &bytes);
            net.collect(to);
        }
    }
    net.cut = Some(3);
    let mut runs = 0;
    while net.validators[0].ledger().height() < net.validators[3].ledger().height() + 200 {
        net.run(500);
        runs += 1;
        assert!(runs < 100, "the three others go on alone");
    }
    assert!(
        net.validators[0]
            .ledger()
            .account_of(&NodeKey::from_bytes(&[51; 32]).public_key())
            .is_some(),
        "the others executed its attestation"
    );
    let before = net.operations[3];
    net.cut = None;
    let mut runs = 0;
    while net.validators[3].ledger().height() + 2 < net.validators[0].ledger().height() {
        net.run(100);
        net.serve_states();
        runs += 1;
        assert!(runs < 200, "it catches up");
    }
    assert!(net.caught_up > 0, "through a peer's state");
    net.run(1_000);
    assert_eq!(net.operations[3], before, "nothing of its own signed again");
}

#[test]
fn a_validator_whose_place_moves_still_attests() {
    let mut net = Network::new();
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(1_000);
    // The same validators, in the reverse order: each one's index moves.
    let mut rules = net.rules.clone();
    rules.validators.reverse();
    net.attest_all(&ValidatorOperation::Reconfigure(rules.clone()));
    net.run(1_000);
    let notice = net.validators[0].ledger().terms().timers.reconfigure_notice;
    net.run(notice + 2_000);
    for validator in &net.validators {
        assert_eq!(validator.ledger().rules(), &rules, "the change is in force");
    }
    let key = NodeKey::from_bytes(&[31; 32]);
    let deposit = Deposit {
        transaction: Digest([31; 32]),
        ledger: 10,
        amount: 50_000_000,
        sender: [3; 20],
        tag: 0,
        key: Some(key.public_key()),
    };
    net.attest_all(&ValidatorOperation::AttestDeposit(deposit));
    net.run(1_000);
    for validator in &net.validators {
        assert!(
            validator.ledger().account_of(&key.public_key()).is_some(),
            "attestations count after the change"
        );
    }
}

#[test]
fn shares_from_anyone_but_the_escrow_keys_count_for_nothing() {
    use dassie_xrpl::codec::{Payment, Transact};
    use dassie_xrpl::keys::Wallet;
    let mut net = Network::new();
    let (number, key) = member(&mut net, 31);
    let hub = *net.validators[0].ledger().hub();
    let withdraw = MemberOperation::Withdraw {
        amount: 5_000_000,
        to: Destination {
            account: [9; 20],
            tag: None,
        },
    };
    let op = Operation::Member(SignedMemberOperation::sign(
        &hub, number, 1, 10, withdraw, &key,
    ));
    net.hold_shares = true;
    net.request(&Request::Submit(op));
    net.run(1_000);
    // The withdrawal is final; only validator 0's own share is in. Anyone
    // can sign the payment: q − 1 throwaway keys do.
    let ledger = net.validators[0].ledger();
    let (sequence, withdrawal) = ledger.withdrawals().next().unwrap();
    let payment = dassie_hub::escrow::withdrawal_payment(ledger.rules(), sequence, withdrawal);
    let quorum = ledger.validators().quorum();
    for seed in 0..quorum - 1 {
        let wallet = Wallet::from_seed([u8::try_from(seed).unwrap() + 1; 16]);
        let signer = payment.multi_sign(wallet.account(), &wallet);
        let bytes = Message::Share { sequence, signer }.to_vec();
        net.validators[0].handle(net.now, MEMBER, None, &bytes);
        net.collect(0);
    }
    net.hold_shares = false;
    for (from, bytes) in std::mem::take(&mut net.held) {
        net.queue.push_back((0, from, bytes));
    }
    net.run(100);
    let escrow_keys: Vec<[u8; 33]> = net.validators[0]
        .ledger()
        .rules()
        .validators
        .iter()
        .map(|a| a.escrow)
        .collect();
    let mine: Vec<&Vec<u8>> = net
        .submitted
        .iter()
        .filter(|(from, _)| *from == 0)
        .map(|(_, blob)| blob)
        .collect();
    assert_eq!(mine.len(), 1, "validator 0 gathered a quorum once");
    let payment = Payment::decode(mine[0]).unwrap();
    assert!(payment.signatures_valid());
    let dassie_xrpl::codec::Signing::Multi(signers) = payment.signing() else {
        panic!("not multi-signed");
    };
    assert_eq!(signers.len(), quorum);
    assert!(
        signers
            .iter()
            .all(|s| escrow_keys.contains(s.signing_pub_key.as_bytes())),
        "only validators' escrow keys sign"
    );
}

#[test]
fn a_withdrawal_is_signed_by_a_quorum_and_the_hub_pays_out_at_its_end() {
    let mut net = Network::new();
    let (number, key) = member(&mut net, 31);
    let hub = *net.validators[0].ledger().hub();
    let withdraw = MemberOperation::Withdraw {
        amount: 5_000_000,
        to: Destination {
            account: [9; 20],
            tag: None,
        },
    };
    let op = Operation::Member(SignedMemberOperation::sign(
        &hub, number, 1, 10, withdraw, &key,
    ));
    net.request(&Request::Submit(op));
    net.run(1_000);
    assert!(
        !net.submitted.is_empty(),
        "no validator gathered a quorum of signatures"
    );
    // Validators that gathered different quorums submit the same
    // transaction with different signers; the ledger applies one.
    use dassie_xrpl::codec::{Payment, Transact};
    let payloads: Vec<Vec<u8>> = net
        .submitted
        .iter()
        .map(|(_, blob)| {
            let payment = Payment::decode(blob).unwrap();
            assert!(payment.signatures_valid());
            payment.multi_signing_payload(dassie_xrpl::AccountId([0; 20]))
        })
        .collect();
    assert!(payloads.windows(2).all(|w| w[0] == w[1]));

    net.attest_all(&ValidatorOperation::Shutdown);
    net.run(1_000);
    let notice = net.validators[0].ledger().terms().timers.shutdown_notice;
    net.run(notice + 1_000);
    assert_eq!(
        net.validators[0].ledger().phase(),
        dassie_hub::ledger::Phase::Ended
    );
    assert_eq!(net.payouts, 4, "every validator signs the payout set once");
}

/// How many messages validator 0 sends back to `from` for `bytes`.
fn answers(net: &mut Network, from: PeerId, node: Option<&PublicKey>, bytes: &[u8]) -> usize {
    net.validators[0].handle(net.now, from, node, bytes);
    let mut count = 0;
    while let Some(action) = net.validators[0].poll() {
        if matches!(action, Action::Peer(to, _) if to == from) {
            count += 1;
        }
    }
    count
}

#[test]
fn only_validators_are_served_catch_up() {
    let mut net = Network::new();
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(1_000);
    let fetch = dassie_hub::consensus::wire::Message::Fetch(dassie_hub::consensus::wire::Fetch {
        height: 0,
        epoch: 0,
        view: 1,
    });
    let bytes = Message::Consensus(fetch.to_vec()).to_vec();
    let stranger = NodeKey::from_bytes(&[99; 32]).public_key();
    assert_eq!(answers(&mut net, MEMBER, None, &bytes), 0);
    assert_eq!(answers(&mut net, MEMBER, Some(&stranger), &bytes), 0);
    let validator = net.nodes[1];
    assert!(answers(&mut net, PeerId(1), Some(&validator), &bytes) > 0);
}

// A validator further behind than peers keep blocks takes the state after
// a final block from a peer, checked against the block's root, and goes on
// from there (spec/areas/hubs.md#the-validators-service).
#[test]
fn a_validator_far_behind_takes_a_peers_state_and_goes_on() {
    let mut net = Network::new();
    net.run(100);
    net.cut = Some(3);
    // The others finalize more blocks than they keep for catching up.
    let mut runs = 0;
    while net.validators[0].ledger().height() < net.validators[3].ledger().height() + 200 {
        net.run(500);
        runs += 1;
        assert!(runs < 100, "the three others go on alone");
    }
    let behind_at = net.validators[3].ledger().height();
    net.cut = None;
    let mut runs = 0;
    while net.validators[3].ledger().height() + 2 < net.validators[0].ledger().height() {
        net.run(100);
        net.serve_states();
        runs += 1;
        assert!(runs < 200, "it catches up");
    }
    assert!(net.validators[3].ledger().height() > behind_at + 200);
    assert!(net.caught_up > 0, "through a peer's state");
    // It takes part again: blocks keep becoming final with it in step.
    let height = net.validators[0].ledger().height();
    net.cut = Some(0);
    net.run(3_000);
    assert!(
        net.validators[3].ledger().height() > height,
        "a quorum with it"
    );
}

// spec/designs/settlement-hubs.md#9-shutdown, "Passing views around": a
// validator that stalled behind the others learns of their later final
// block from them, though none of them votes any more, and signs that
// block's payout set too, so that they all come to sign one.
#[test]
fn validators_that_stalled_apart_come_to_sign_one_payout_set() {
    let mut net = Network::new();
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(2_000);
    // Validator 3 falls behind; the others go on without it.
    net.cut = Some(3);
    net.run(2_000);
    let behind = net.validators[3].ledger().height();
    let ahead = net.validators[0].ledger().height();
    assert!(ahead > behind + 3, "{behind} {ahead}");
    // Then the hub stalls: nothing is final for longer than the rules'
    // stall, and every validator declares it.
    let stall = net.rules.terms.timers.stall;
    net.no_finality = true;
    net.now += stall + 1;
    net.run(1);
    assert!(net.validators.iter().all(|v| v.engine().is_stalled()));
    assert!(net.payout_heights.contains(&(3, behind)));
    // Once validator 3 hears from the others again, it learns of their
    // last final block, and signs that block's payout set too.
    net.cut = None;
    net.no_finality = false;
    let mut runs = 0;
    while net.validators[3].ledger().height() < ahead {
        net.run(500);
        runs += 1;
        assert!(runs < 100, "it learns of the later final block");
    }
    net.run(100);
    let top = net.validators[0].ledger().height();
    assert_eq!(net.validators[3].ledger().height(), top);
    let signed: Vec<u64> = net
        .payout_heights
        .iter()
        .filter(|(index, _)| *index == 3)
        .map(|(_, height)| *height)
        .collect();
    assert_eq!(signed, [behind, top], "never an earlier one after a later");
    for v in &net.validators {
        v.check_invariants();
    }
}

// A validator that stalled alone and hears again from a hub that carries
// on without it signs no later payout set while the hub goes on: each
// would be a whole set, and a bundle written, at every block it learns of.
// Once the hub stops too, it signs the set of the hub's last block, once.
#[test]
fn a_validator_that_stalled_alone_signs_no_set_while_the_hub_goes_on() {
    let mut net = Network::new();
    net.attest_all(&ValidatorOperation::AttestEscrow { sequence: 5 });
    net.run(2_000);
    net.cut = Some(3);
    net.run(1_000);
    // Its clock alone runs past the stall: it declares the hub stalled.
    let stall = net.rules.terms.timers.stall;
    let later = net.now + stall + 1;
    net.validators[3].tick(later);
    net.collect(3);
    assert!(net.validators[3].engine().is_stalled());
    let signed = |net: &Network| net.payout_heights.iter().filter(|(v, _)| *v == 3).count();
    assert_eq!(signed(&net), 1);
    // The others carry on, and it hears from them again.
    net.cut = None;
    let height = net.validators[0].ledger().height();
    net.run(5_000);
    assert!(
        net.validators[0].ledger().height() > height + 10,
        "the hub goes on"
    );
    assert!(
        net.validators[3].ledger().height() > height + 10,
        "it learns of their blocks"
    );
    assert_eq!(signed(&net), 1, "no set while the hub goes on");
    // The hub stops: its last block grows older than the stall.
    net.no_finality = true;
    net.now += stall + 1;
    net.run(1);
    let top = net.validators[3].ledger().height();
    assert_eq!(
        net.payout_heights.iter().rfind(|(v, _)| *v == 3),
        Some(&(3, top)),
        "the set of the hub's last block"
    );
    assert_eq!(signed(&net), 2);
}

// A state that is not the one the block names is refused, and nothing
// changes.
#[test]
fn a_state_that_is_not_the_blocks_is_refused() {
    let mut net = Network::new();
    net.run(100);
    net.cut = Some(3);
    let mut runs = 0;
    while net.validators[0].ledger().height() < net.validators[3].ledger().height() + 200 {
        net.run(500);
        runs += 1;
        assert!(runs < 100);
    }
    net.cut = None;
    let mut runs = 0;
    while net.behind.is_empty() {
        net.run(100);
        runs += 1;
        assert!(runs < 200, "it learns it is behind");
    }
    let (to, finalized) = net.behind.remove(0);
    let before = net.validators[to].ledger().clone();
    // The state of another height.
    let wrong = before.snapshot();
    assert_eq!(
        net.validators[to].catch_up(net.now, *finalized, &wrong),
        Err(dassie_hub::service::CatchUpError::State)
    );
    assert_eq!(net.validators[to].ledger(), &before);
}

/// Whether a message is a vote to finalize, a finalization, or catch-up,
/// which carries finalizations.
fn carries_finality(bytes: &[u8]) -> bool {
    use dassie_hub::consensus::VoteKind;
    use dassie_hub::consensus::wire::Message as Consensus;
    let Ok(Message::Consensus(inner)) = Message::decode_exact(bytes) else {
        return false;
    };
    match Consensus::decode_message(&inner) {
        Ok(Consensus::Vote(vote)) => vote.kind == VoteKind::Finalize,
        Ok(Consensus::Certificate(certificate)) => certificate.kind == VoteKind::Finalize,
        Ok(Consensus::Fetch(_) | Consensus::Delivery(_)) => true,
        _ => false,
    }
}

// Validators keep the states of a bounded number of blocks not yet final
// (`MAX_EXECUTED`), and the engine verifies again what they let go of: a
// long run of notarized blocks none of which became final ends once votes
// to finalize arrive again (spec/areas/hubs.md#consensus).
#[test]
fn finality_returns_after_more_blocks_than_validators_keep_states_of() {
    let mut net = Network::new();
    net.run(100);
    let final_height = net.validators[0].ledger().height();
    let view = net.validators[0].engine().view();
    net.no_finality = true;
    // A hundred views: more than the states a validator keeps, fewer than
    // the rounds the engine keeps.
    net.run(10_000);
    assert_eq!(
        net.validators[0].ledger().height(),
        final_height,
        "nothing became final"
    );
    let views = net.validators[0].engine().view() - view;
    assert!(
        views > 70,
        "more blocks notarized than states kept: {views} views"
    );
    net.no_finality = false;
    net.run(5_000);
    for validator in &net.validators {
        assert!(
            validator.ledger().height() > final_height + 70,
            "final again: {}",
            validator.ledger().height()
        );
    }
}

#[test]
fn a_validator_resumes_from_what_its_host_persisted() {
    let mut net = Network::new();
    let (number, key) = member(&mut net, 31);
    let before = net.validators[2].ledger().height();
    assert!(before > 3, "blocks became final");
    // Validator 2 crashes: whatever it had not persisted is gone. It comes
    // back from its stored state and its last snapshot.
    net.restart(2);
    assert_eq!(net.validators[2].ledger().height(), before);
    net.run(1_000);
    let hub = *net.validators[0].ledger().hub();
    let disbursal = MemberOperation::SetDisbursal(Destination {
        account: [8; 20],
        tag: None,
    });
    let op = Operation::Member(SignedMemberOperation::sign(
        &hub, number, 1, 10, disbursal, &key,
    ));
    net.request(&Request::Submit(op));
    net.run(1_000);
    for validator in &net.validators {
        let account = validator.ledger().account(number).unwrap();
        assert_eq!(account.disbursal.unwrap().account, [8; 20]);
    }
    assert!(net.validators[2].ledger().height() > before + 3);

    // Every validator restarts at once, from what each persisted, and the
    // hub goes on.
    for index in 0..4 {
        net.restart(index);
    }
    let height = net.validators[0].ledger().height();
    net.run(2_000);
    assert!(
        net.validators
            .iter()
            .all(|v| v.ledger().height() > height + 3)
    );

    // A restarted validator vouches for nothing from before it: a member
    // reads its state anew.
    net.member.clear();
    net.request(&Request::Since {
        account: number,
        from: 1,
    });
    assert_eq!(net.member.len(), 4);
    for bytes in &net.member {
        let Ok(Message::Statements(list)) = Message::decode_exact(bytes) else {
            panic!("not statements");
        };
        assert!(
            list.iter()
                .all(|s| matches!(s.statement, Statement::Trimmed { .. }))
        );
    }
}

#[test]
fn what_a_host_did_not_write_together_does_not_resume() {
    let mut net = Network::new();
    member(&mut net, 31);
    let (stored, snapshot) = net.stored[1].clone();
    let root = Payload::decode_exact(&stored.tip.block.payload)
        .unwrap()
        .state;
    let ledger = Ledger::restore(&snapshot, &root).unwrap();
    let keys = || ValidatorKeys::derive(&net.node_keys[1], &NONCE).unwrap();
    // A snapshot of another height than the tip's.
    let mut ahead = stored.clone();
    ahead.tip.block.height += 1;
    assert_eq!(
        Validator::resume(keys(), ahead, ledger.clone(), net.now).err(),
        Some(ResumeError::Height)
    );
    // Of the same height, but another state.
    let mut other = stored.clone();
    other.tip.block.payload = Vec::new();
    assert_eq!(
        Validator::resume(keys(), other, ledger.clone(), net.now).err(),
        Some(ResumeError::State)
    );
    // Keys that are no validator's of the hub.
    let stranger = ValidatorKeys::derive(&NodeKey::from_bytes(&[99; 32]), &NONCE).unwrap();
    assert_eq!(
        Validator::resume(stranger, stored, ledger, net.now).err(),
        Some(ResumeError::NotAValidator)
    );
}

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
use dassie_hub::consensus::PeerId;
use dassie_hub::digest::Digest;
use dassie_hub::escrow::{ValidatorKeys, escrow_master};
use dassie_hub::ledger::Outcome;
use dassie_hub::operation::{
    AccountNumber, Deposit, Destination, MemberOperation, Operation, SignedMemberOperation,
    ValidatorOperation,
};
use dassie_hub::rules::{Acceptance, Fees, Limits, Rules, Terms, Timers};
use dassie_hub::service::wire::Message;
use dassie_hub::service::{Action, Validator};
use dassie_identity::NodeKey;
use dassie_oer::Encode;

const NONCE: [u8; 32] = [7; 32];
const START: u64 = 1_700_000_000_000;
const MEMBER: PeerId = PeerId(100);

fn terms(sponsor: &NodeKey) -> Terms {
    Terms {
        nonce: NONCE,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow: escrow_master(sponsor, &NONCE).unwrap().account().0,
        fees: Fees {
            operation: 10,
            ledger: 100,
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
    queue: VecDeque<(usize, PeerId, Vec<u8>)>,
    member: Vec<Vec<u8>>,
    submitted: Vec<Vec<u8>>,
    payouts: usize,
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
        let validators = nodes
            .iter()
            .map(|node| {
                let keys = ValidatorKeys::derive(node, &NONCE).unwrap();
                Validator::genesis(rules.clone(), keys, START, START).unwrap()
            })
            .collect();
        Self {
            validators,
            queue: VecDeque::new(),
            member: Vec::new(),
            submitted: Vec::new(),
            payouts: 0,
            now: START,
        }
    }

    fn collect(&mut self, from: usize) {
        while let Some(action) = self.validators[from].poll() {
            let sender = PeerId(u64::try_from(from).unwrap());
            match action {
                Action::Validators(bytes) => {
                    for to in (0..self.validators.len()).filter(|to| *to != from) {
                        self.queue.push_back((to, sender, bytes.clone()));
                    }
                }
                Action::Validator(key, bytes) => {
                    let set = self.validators[0].ledger().validators();
                    let to = set.index_of(&key).unwrap().get();
                    self.queue.push_back((to, sender, bytes));
                }
                Action::Peer(peer, bytes) if peer == MEMBER => self.member.push(bytes),
                Action::Peer(peer, bytes) => {
                    self.queue
                        .push_back((usize::try_from(peer.0).unwrap(), sender, bytes));
                }
                Action::Submit(blob) => self.submitted.push(blob),
                Action::Payout(..) => self.payouts += 1,
            }
        }
        self.validators[from].take_batches();
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
                self.validators[to].handle(self.now, from, &bytes);
                self.collect(to);
            }
        }
        for validator in &self.validators {
            validator.check_invariants();
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
            self.validators[i].handle(self.now, MEMBER, &bytes);
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
        .map(|blob| {
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

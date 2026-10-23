//! A hub's escrow on the fake XRP Ledger, from founding to payout
//! (spec/designs/settlement-hubs.md#2-founding-a-hub,
//! spec/designs/settlement-hubs.md#6-deposits-and-withdrawals,
//! spec/designs/settlement-hubs.md#9-shutdown): the sponsor sets it up and
//! the validators check it; a member deposits and withdraws; the hub shuts
//! down and the escrow pays out exactly the final balances, with
//! signatures the validators give alone; and a payout set for another
//! block can no longer pay once one has.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use dassie_hub::escrow::{
    EscrowFault, PayoutTransaction, ValidatorKeys, check_escrow, deposit_of, escrow_master,
    outcome_of, payout_set, setup, sign_payout_set, signer_list_set, withdrawal_outcome,
    withdrawal_payment,
};
use dassie_hub::ledger::Failure;
use dassie_hub::ledger::{Block, Detail, Outcome, Phase};
use dassie_hub::operation::{
    AccountNumber, Destination, MemberOperation, Operation, SignedMemberOperation,
    SignedValidatorOperation, ValidatorOperation,
};
use dassie_hub::rules::{Acceptance, Fees, Limits, Rules, Terms, Timers};
use dassie_hub::validators::{Signers, ValidatorIndex};
use dassie_hub::{Ledger, Millis};
use dassie_identity::NodeKey;
use dassie_xrpl::api::{self, TxRecord};
use dassie_xrpl::codec::{Amount, Memo, Payment, Signer, Signing, Transact};
use dassie_xrpl::fake::{FEE_BASE, FakeLedger};
use dassie_xrpl::{AccountId, Wallet};
use serde_json::Value;

const CONNECTION: u64 = 1;
const NONCE: [u8; 32] = [7; 32];
const LEDGER_FEE: u64 = 100;

fn request(ledger: &mut FakeLedger, bytes: &[u8]) -> Value {
    match api::parse(&ledger.request(CONNECTION, bytes)) {
        api::Message::Response { outcome, .. } => outcome.unwrap(),
        other => panic!("not a response: {other:?}"),
    }
}

fn sequence(ledger: &mut FakeLedger, account: AccountId) -> u32 {
    let result = request(ledger, &api::account_info(1, account));
    api::account_info_result(&result).unwrap().sequence
}

fn submit(ledger: &mut FakeLedger, blob: &[u8]) -> String {
    let result = request(ledger, &api::submit(3, blob));
    api::submit_result(&result).unwrap()
}

fn record(ledger: &mut FakeLedger, hash: &[u8; 32]) -> TxRecord {
    let result = request(ledger, &api::tx_anywhere(4, hash));
    api::tx_result(&result).unwrap()
}

fn terms(sponsor: &NodeKey, escrow: AccountId) -> Terms {
    Terms {
        nonce: NONCE,
        sponsor: sponsor.public_key(),
        network: 1,
        escrow: escrow.0,
        fees: Fees {
            operation: 10,
            ledger: LEDGER_FEE,
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

/// A founded hub: its validators' keys, its ledger and the XRP Ledger.
struct Hub {
    xrpl: FakeLedger,
    validators: Vec<ValidatorKeys>,
    ledger: Ledger,
    rules: Rules,
    sequence: u64,
}

impl Hub {
    /// The sponsor founds the hub and sets up its escrow; the validators
    /// check it and attest its next sequence.
    fn found() -> Self {
        let mut xrpl = FakeLedger::new();
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let master = escrow_master(&sponsor, &NONCE).unwrap();
        let terms = terms(&sponsor, master.account());
        let nodes: Vec<NodeKey> = (1..=4).map(|i| NodeKey::from_bytes(&[i; 32])).collect();
        let validators: Vec<ValidatorKeys> = nodes
            .iter()
            .map(|node| ValidatorKeys::derive(node, &NONCE).unwrap())
            .collect();
        let acceptances: Vec<Acceptance> = validators
            .iter()
            .zip(&nodes)
            .enumerate()
            .map(|(i, (keys, node))| {
                keys.accept(node, &terms, format!("v{i}.example:443"))
                    .unwrap()
            })
            .collect();
        let rules = Rules {
            terms,
            validators: acceptances,
        };
        rules.check().unwrap();
        // The escrow is created by funding it, then set up by its master
        // key, which it disables last.
        xrpl.fund(master.account());
        xrpl.close();
        let first = sequence(&mut xrpl, master.account());
        for transaction in setup(&rules, &master, first, FEE_BASE) {
            assert_eq!(submit(&mut xrpl, transaction.blob()), "tesSUCCESS");
        }
        xrpl.close();
        let escrow = master.account();
        let settings =
            api::account_settings_result(&request(&mut xrpl, &api::account_info(1, escrow)))
                .unwrap();
        let objects = api::account_objects_result(&request(
            &mut xrpl,
            &api::account_objects(2, escrow, None),
        ))
        .unwrap();
        check_escrow(&rules, &settings, &objects).unwrap();
        let ledger = Ledger::genesis(rules.clone(), 1_000_000).unwrap();
        let mut hub = Self {
            xrpl,
            validators,
            ledger,
            rules,
            sequence: 0,
        };
        let next = sequence(&mut hub.xrpl, escrow);
        hub.attest(|_| ValidatorOperation::AttestEscrow { sequence: next });
        assert!(hub.ledger.escrow().checked);
        hub
    }

    fn escrow(&self) -> AccountId {
        AccountId(self.rules.terms.escrow)
    }

    fn block_at(&mut self, timestamp: Millis, operations: Vec<Operation>) -> Vec<Outcome> {
        let height = self.ledger.height() + 1;
        let votes: Vec<(u64, Signers)> = if height > 1 {
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
        self.ledger.apply(&block).unwrap()
    }

    fn block(&mut self, operations: Vec<Operation>) -> Vec<Outcome> {
        let timestamp = self.ledger.timestamp() + 10;
        self.block_at(timestamp, operations)
    }

    /// Three of the four validators attest what `operation` says.
    fn attest(&mut self, operation: impl Fn(usize) -> ValidatorOperation) {
        let ops = (0..3)
            .map(|i| {
                self.sequence += 1;
                Operation::Validator(SignedValidatorOperation::sign(
                    self.ledger.hub(),
                    ValidatorIndex::new(i).unwrap(),
                    self.sequence,
                    operation(i),
                    &self.validators[i].consensus,
                ))
            })
            .collect();
        self.block(ops);
    }

    fn member(
        &mut self,
        account: AccountNumber,
        key: &NodeKey,
        operation: MemberOperation,
    ) -> Outcome {
        let sequence = self.ledger.account(account).unwrap().sequence;
        let op = Operation::Member(SignedMemberOperation::sign(
            self.ledger.hub(),
            account,
            sequence,
            10,
            operation,
            key,
        ));
        self.block(vec![op])[0]
    }

    /// Submits a transaction the validators multi-signed, with the
    /// signatures of the first three.
    fn submit_signed<T: Transact>(&mut self, transaction: T, signers: Vec<Signer>) -> [u8; 32] {
        let signed = transaction.with_signers(signers).unwrap();
        assert_eq!(submit(&mut self.xrpl, &signed.blob), "tesSUCCESS");
        signed.hash
    }
}

/// Alice deposits from her own XRP Ledger account, naming her hub key in
/// the memo, with tag 0 for a new account.
fn deposit(hub: &mut Hub, alice: &Wallet, alice_key: &NodeKey, drops: u64) -> AccountNumber {
    let payment = Payment {
        account: alice.account(),
        destination: hub.escrow(),
        destination_tag: Some(0),
        amount: Amount::Xrp(drops),
        send_max: None,
        fee: FEE_BASE,
        sequence: sequence(&mut hub.xrpl, alice.account()),
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: vec![Memo {
            memo_type: b"dassie/hub-account".to_vec(),
            memo_data: alice_key.public_key().as_bytes().to_vec(),
        }],
        signing: Signing::by(alice.public_key()),
    };
    let signed = payment.sign(alice);
    assert_eq!(submit(&mut hub.xrpl, &signed.blob), "tesSUCCESS");
    hub.xrpl.close();
    let seen = record(&mut hub.xrpl, &signed.hash);
    let deposit = deposit_of(&seen, hub.escrow()).unwrap();
    hub.attest(|_| ValidatorOperation::AttestDeposit(deposit));
    hub.ledger.account_of(&alice_key.public_key()).unwrap()
}

#[test]
fn a_hub_from_founding_to_payout() {
    let mut hub = Hub::found();
    let alice = Wallet::from_seed([31; 16]);
    hub.xrpl.fund(alice.account());
    hub.xrpl.close();
    let alice_key = NodeKey::from_bytes(&[32; 32]);
    let number = deposit(&mut hub, &alice, &alice_key, 50_000_000);
    let to = Destination {
        account: alice.account().0,
        tag: None,
    };
    hub.member(number, &alice_key, MemberOperation::SetDisbursal(to));

    // A withdrawal: each validator builds the same Payment and signs alone.
    let outcome = hub.member(
        number,
        &alice_key,
        MemberOperation::Withdraw {
            amount: 5_000_000,
            to,
        },
    );
    let Outcome::Applied {
        detail: Detail::Withdrawal(sequence),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    let (_, withdrawal) = hub.ledger.withdrawals().next().unwrap();
    let payment = withdrawal_payment(&hub.rules, sequence, withdrawal);
    let signers: Vec<Signer> = hub.validators[..3]
        .iter()
        .map(|keys| payment.multi_sign(keys.escrow.account(), &keys.escrow))
        .collect();
    let before = hub.xrpl.balance(alice.account()).unwrap();
    let hash = hub.submit_signed(payment, signers);
    hub.xrpl.close();
    let (seen, delivered) = outcome_of(&record(&mut hub.xrpl, &hash), hub.escrow()).unwrap();
    assert_eq!((seen, delivered), (sequence, true));
    hub.attest(|_| ValidatorOperation::AttestWithdrawal {
        escrow_sequence: sequence,
        delivered: true,
    });
    assert_eq!(hub.ledger.withdrawals().count(), 0);
    assert_eq!(
        hub.xrpl.balance(alice.account()).unwrap(),
        before + 5_000_000
    );

    // Shutdown by vote, then the last block once its notice passed.
    hub.attest(|_| ValidatorOperation::Shutdown);
    let Phase::ShuttingDown { ends } = hub.ledger.phase() else {
        panic!("not shutting down");
    };
    let earlier = hub.ledger.clone();
    hub.block_at(ends, vec![]);
    assert_eq!(hub.ledger.phase(), Phase::Ended);

    // Each validator builds the payout set alone, and signs it alone.
    let set = payout_set(&hub.ledger).unwrap();
    assert_eq!(
        set,
        payout_set(&hub.ledger.clone()).unwrap(),
        "built the same by everyone"
    );
    let signatures: Vec<Vec<Signer>> = hub.validators[1..]
        .iter()
        .map(|keys| sign_payout_set(&set, keys).unwrap())
        .collect();
    let owed = hub.ledger.account(number).unwrap();
    let alice_total = owed.balance + owed.reserve - LEDGER_FEE;
    let before = hub.xrpl.balance(alice.account()).unwrap();
    let mut alice_paid = false;
    for (i, transaction) in set.transactions.iter().enumerate() {
        let signers: Vec<Signer> = signatures.iter().map(|s| s[i]).collect();
        match transaction {
            PayoutTransaction::Payout(payment) => {
                alice_paid |= payment.destination == alice.account();
                hub.submit_signed(payment.clone(), signers);
            }
            PayoutTransaction::Selector(list) | PayoutTransaction::SignerList(list) => {
                hub.submit_signed(list.clone(), signers);
            }
            PayoutTransaction::Withdrawal(payment) => {
                hub.submit_signed(payment.clone(), signers);
            }
        }
        hub.xrpl.close();
    }
    assert!(alice_paid);
    assert_eq!(
        hub.xrpl.balance(alice.account()).unwrap(),
        before + alice_total
    );

    // A set for an earlier block was signed too, by validators that had
    // not heard of the last one: its selector's sequence is used, and its
    // payouts are signed by keys no longer on the list.
    let stale = payout_set(&earlier).unwrap();
    let stale_signatures: Vec<Vec<Signer>> = hub.validators[..3]
        .iter()
        .map(|keys| sign_payout_set(&stale, keys).unwrap())
        .collect();
    let escrow_before = hub.xrpl.balance(hub.escrow());
    for (i, transaction) in stale.transactions.iter().enumerate() {
        if let PayoutTransaction::Payout(payment) = transaction {
            let signers: Vec<Signer> = stale_signatures.iter().map(|s| s[i]).collect();
            let signed = payment.clone().with_signers(signers).unwrap();
            let result = submit(&mut hub.xrpl, &signed.blob);
            assert_ne!(result, "tesSUCCESS", "a stale set paid out");
        }
    }
    hub.xrpl.close();
    assert_eq!(hub.xrpl.balance(hub.escrow()), escrow_before);
}

#[test]
fn a_deposit_without_a_key_attests_no_key_and_is_paid_back() {
    let mut hub = Hub::found();
    hub.xrpl.deposit_tagged(hub.escrow(), 3_000_000, Some(0));
    hub.xrpl.close();
    let escrow = hub.escrow();
    let result = request(
        &mut hub.xrpl,
        &api::account_tx(5, escrow, 0, u32::MAX, None),
    );
    let records = api::account_tx_result(&result).unwrap();
    let deposit = records
        .transactions
        .iter()
        .find_map(|record| deposit_of(record, hub.escrow()))
        .unwrap();
    assert_eq!(deposit.key, None);
    hub.attest(|_| ValidatorOperation::AttestDeposit(deposit));
    assert_eq!(hub.ledger.withdrawals().count(), 1, "paid back");
}

#[test]
fn a_payout_set_keeps_withdrawals_still_out_in_sequence() {
    let mut hub = Hub::found();
    let alice = Wallet::from_seed([31; 16]);
    hub.xrpl.fund(alice.account());
    hub.xrpl.close();
    let alice_key = NodeKey::from_bytes(&[32; 32]);
    let number = deposit(&mut hub, &alice, &alice_key, 50_000_000);
    let to = Destination {
        account: alice.account().0,
        tag: None,
    };
    hub.member(number, &alice_key, MemberOperation::SetDisbursal(to));
    hub.member(
        number,
        &alice_key,
        MemberOperation::Withdraw {
            amount: 5_000_000,
            to,
        },
    );
    hub.attest(|_| ValidatorOperation::Shutdown);
    let Phase::ShuttingDown { ends } = hub.ledger.phase() else {
        panic!("not shutting down");
    };
    hub.block_at(ends, vec![]);
    let set = payout_set(&hub.ledger).unwrap();
    assert!(matches!(
        set.transactions[0],
        PayoutTransaction::Withdrawal(_)
    ));
    assert!(matches!(
        set.transactions[1],
        PayoutTransaction::Selector(_)
    ));
    let first = set.transactions[0].sequence();
    for (i, transaction) in set.transactions.iter().enumerate() {
        assert_eq!(transaction.sequence(), first + u32::try_from(i).unwrap());
    }
}

impl Hub {
    fn settings_and_objects(&mut self) -> (api::AccountSettings, api::AccountObjects) {
        let escrow = self.escrow();
        let settings =
            api::account_settings_result(&request(&mut self.xrpl, &api::account_info(1, escrow)))
                .unwrap();
        let objects = api::account_objects_result(&request(
            &mut self.xrpl,
            &api::account_objects(2, escrow, None),
        ))
        .unwrap();
        (settings, objects)
    }
}

#[test]
fn a_change_of_validators_reaches_the_escrows_signer_list() {
    let mut hub = Hub::found();
    let newcomer = NodeKey::from_bytes(&[9; 32]);
    let keys = ValidatorKeys::derive(&newcomer, &NONCE).unwrap();
    let mut rules = hub.rules.clone();
    rules.validators[3] = keys
        .accept(&newcomer, &rules.terms, "v9.example:443".into())
        .unwrap();
    rules.check().unwrap();
    let votes = rules.clone();
    hub.attest(move |_| ValidatorOperation::Reconfigure(votes.clone()));
    let at = hub.ledger.timestamp() + hub.rules.terms.timers.reconfigure_notice;
    hub.block_at(at, vec![]);
    // The new set takes over two blocks after its notice passed.
    hub.block(vec![]);
    hub.block(vec![]);
    let sequence = hub.ledger.escrow().signer_list.unwrap();
    // The old set signs the change of signers it owes.
    let list = signer_list_set(&rules, sequence);
    let signers: Vec<Signer> = hub.validators[..3]
        .iter()
        .map(|keys| list.multi_sign(keys.escrow.account(), &keys.escrow))
        .collect();
    let hash = hub.submit_signed(list, signers);
    hub.xrpl.close();
    let (seen, applied) = outcome_of(&record(&mut hub.xrpl, &hash), hub.escrow()).unwrap();
    assert_eq!((seen, applied), (sequence, true));
    let reserve = hub.ledger.reserve_account();
    hub.attest(move |_| ValidatorOperation::AttestSignerList { sequence });
    assert_eq!(hub.ledger.escrow().signer_list, None);
    assert_eq!(
        hub.ledger.reserve_account(),
        reserve.saturating_sub(LEDGER_FEE)
    );
    // The escrow now checks against the new rules.
    let (settings, objects) = hub.settings_and_objects();
    check_escrow(&rules, &settings, &objects).unwrap();
    assert_eq!(
        check_escrow(&hub.rules, &settings, &objects),
        Err(EscrowFault::SignerList)
    );
}

#[test]
fn the_check_refuses_an_escrow_with_a_way_in() {
    let mut hub = Hub::found();
    let (settings, objects) = hub.settings_and_objects();
    check_escrow(&hub.rules, &settings, &objects).unwrap();
    let rules = hub.rules.clone();
    let check = |settings: &api::AccountSettings, objects: &api::AccountObjects| {
        check_escrow(&rules, settings, objects)
    };
    let mut master = settings;
    master.flags &= !api::account_flags::DISABLE_MASTER;
    assert_eq!(check(&master, &objects), Err(EscrowFault::MasterKey));
    let mut regular = settings;
    regular.regular_key = Some(AccountId([5; 20]));
    assert_eq!(check(&regular, &objects), Err(EscrowFault::RegularKey));
    let mut open = settings;
    open.flags &= !api::account_flags::REQUIRE_DEST_TAG;
    assert_eq!(check(&open, &objects), Err(EscrowFault::NoRequireDest));
    let mut more = objects.clone();
    more.others = 1;
    assert_eq!(check(&settings, &more), Err(EscrowFault::Objects));
    let mut list = objects.clone();
    list.signer_list.as_mut().unwrap().quorum = 2;
    assert_eq!(check(&settings, &list), Err(EscrowFault::SignerList));
    let mut earlier = objects.clone();
    earlier.ledger_index = settings.ledger_index - 1;
    assert_eq!(check(&settings, &earlier), Err(EscrowFault::NotValidated));
    let mut unvalidated = settings;
    unvalidated.validated = false;
    assert_eq!(
        check(&unvalidated, &objects),
        Err(EscrowFault::NotValidated)
    );
}

#[test]
fn only_what_reaches_the_escrow_is_a_deposit() {
    let mut hub = Hub::found();
    let alice = Wallet::from_seed([31; 16]);
    hub.xrpl.fund(alice.account());
    hub.xrpl.close();
    let alice_key = NodeKey::from_bytes(&[32; 32]);
    deposit(&mut hub, &alice, &alice_key, 50_000_000);
    let escrow = hub.escrow();
    let result = request(
        &mut hub.xrpl,
        &api::account_tx(5, escrow, 0, u32::MAX, None),
    );
    let records = api::account_tx_result(&result).unwrap();
    let real = records
        .transactions
        .into_iter()
        .find(|record| deposit_of(record, escrow).is_some())
        .unwrap();
    let not = |change: &dyn Fn(&mut TxRecord)| {
        let mut record = real.clone();
        change(&mut record);
        deposit_of(&record, escrow)
    };
    assert_eq!(not(&|r| r.validated = false), None);
    assert_eq!(not(&|r| r.result = "tecPATH_PARTIAL".into()), None);
    assert_eq!(not(&|r| r.destination = Some(AccountId([1; 20]))), None);
    assert_eq!(not(&|r| r.destination_tag = None), None);
    assert_eq!(
        not(&|r| r.delivered = None),
        None,
        "a token is not the hub's asset"
    );
    assert_eq!(
        not(&|r| r.account = escrow),
        None,
        "the escrow paying itself"
    );
    // A partial payment counts for what it delivered.
    let partial = not(&|r| r.delivered = Some(7)).unwrap();
    assert_eq!(partial.amount, 7);
}

#[test]
fn the_escrow_and_the_zero_account_are_no_destinations() {
    let mut hub = Hub::found();
    let alice = Wallet::from_seed([31; 16]);
    hub.xrpl.fund(alice.account());
    hub.xrpl.close();
    let alice_key = NodeKey::from_bytes(&[32; 32]);
    let number = deposit(&mut hub, &alice, &alice_key, 50_000_000);
    let refused = Outcome::Failed {
        fee: 10,
        reason: Failure::Destination,
    };
    for account in [hub.rules.terms.escrow, [0; 20]] {
        let to = Destination {
            account,
            tag: Some(1),
        };
        assert_eq!(
            hub.member(number, &alice_key, MemberOperation::SetDisbursal(to)),
            refused
        );
    }
    let home = Destination {
        account: alice.account().0,
        tag: None,
    };
    hub.member(number, &alice_key, MemberOperation::SetDisbursal(home));
    for account in [hub.rules.terms.escrow, [0; 20]] {
        let to = Destination { account, tag: None };
        let withdraw = MemberOperation::Withdraw {
            amount: 5_000_000,
            to,
        };
        assert_eq!(hub.member(number, &alice_key, withdraw), refused);
        let close = MemberOperation::Close(dassie_hub::operation::CloseTo::Ledger(to));
        assert_eq!(hub.member(number, &alice_key, close), refused);
    }
}

#[test]
fn a_withdrawals_outcome_is_its_own_payment() {
    let mut hub = Hub::found();
    let alice = Wallet::from_seed([31; 16]);
    hub.xrpl.fund(alice.account());
    hub.xrpl.close();
    let alice_key = NodeKey::from_bytes(&[32; 32]);
    let number = deposit(&mut hub, &alice, &alice_key, 50_000_000);
    let to = Destination {
        account: alice.account().0,
        tag: Some(5),
    };
    hub.member(number, &alice_key, MemberOperation::SetDisbursal(to));
    let outcome = hub.member(
        number,
        &alice_key,
        MemberOperation::Withdraw {
            amount: 5_000_000,
            to,
        },
    );
    let Outcome::Applied {
        detail: Detail::Withdrawal(sequence),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    let (_, withdrawal) = hub.ledger.withdrawals().next().unwrap();
    let withdrawal = withdrawal.clone();
    let payment = withdrawal_payment(&hub.rules, sequence, &withdrawal);
    let signers: Vec<Signer> = hub.validators[..3]
        .iter()
        .map(|keys| payment.multi_sign(keys.escrow.account(), &keys.escrow))
        .collect();
    let hash = hub.submit_signed(payment, signers);
    hub.xrpl.close();
    let seen = record(&mut hub.xrpl, &hash);
    assert_eq!(
        withdrawal_outcome(&seen, hub.escrow(), sequence, &withdrawal),
        Some(true)
    );
    let mut elsewhere = seen.clone();
    elsewhere.destination_tag = Some(6);
    assert_eq!(
        withdrawal_outcome(&elsewhere, hub.escrow(), sequence, &withdrawal),
        None
    );
    let mut short = seen;
    short.delivered = Some(1);
    assert_eq!(
        withdrawal_outcome(&short, hub.escrow(), sequence, &withdrawal),
        None
    );
}

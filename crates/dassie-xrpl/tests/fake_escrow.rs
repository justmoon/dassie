//! An account several keys control, on the fake ledger
//! (spec/areas/xrpl.md#keys-in-the-fake-ledger): set up as a settlement
//! hub's escrow is, checked from outside as its validators check it, paid
//! to with a destination tag, and paid out multi-signed; and who may sign,
//! checked when a transaction is submitted and again when it applies.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use dassie_xrpl::api::{self, AccountObjects, AccountSettings, TxRecord, account_flags};
use dassie_xrpl::codec::{
    ASF_DISABLE_MASTER, ASF_REQUIRE_DEST, AccountSet, Amount, Payment, SetRegularKey, Signed,
    Signer, SignerEntry, SignerListSet, Signing, TF_OPTIONAL_DEST_TAG, TF_REQUIRE_DEST_TAG,
    Transact, multi_signed_fee,
};
use dassie_xrpl::fake::{FEE_BASE, FakeLedger, RESERVE_BASE, RESERVE_INC};
use dassie_xrpl::keys::{Key, Secp256k1Key};
use dassie_xrpl::{AccountId, Wallet, hex_upper};
use serde_json::{Value, json};

/// The connection every test reads the ledger over.
const CONNECTION: u64 = 1;

fn request(ledger: &mut FakeLedger, bytes: &[u8]) -> Result<Value, api::ApiError> {
    match api::parse(&ledger.request(CONNECTION, bytes)) {
        api::Message::Response { outcome, .. } => outcome,
        other => panic!("not a response: {other:?}"),
    }
}

fn sequence(ledger: &mut FakeLedger, account: AccountId) -> u32 {
    let result = request(ledger, &api::account_info(1, account)).unwrap();
    api::account_info_result(&result).unwrap().sequence
}

fn settings(ledger: &mut FakeLedger, account: AccountId) -> AccountSettings {
    let result = request(ledger, &api::account_info(1, account)).unwrap();
    api::account_settings_result(&result).unwrap()
}

fn objects(ledger: &mut FakeLedger, account: AccountId) -> AccountObjects {
    let result = request(ledger, &api::account_objects(2, account, None)).unwrap();
    api::account_objects_result(&result).unwrap()
}

/// Submits a signed blob; what `submit` says at once.
fn submit(ledger: &mut FakeLedger, blob: &[u8]) -> String {
    let result = request(ledger, &api::submit(3, blob)).unwrap();
    api::submit_result(&result).unwrap()
}

/// The validated transaction `hash`.
fn tx(ledger: &mut FakeLedger, hash: &[u8; 32]) -> TxRecord {
    let result = request(ledger, &api::tx_anywhere(4, hash)).unwrap();
    api::tx_result(&result).unwrap()
}

/// Closes a ledger, and checks that no XRP was made or lost.
fn close(ledger: &mut FakeLedger) {
    ledger.close();
    assert_eq!(ledger.total(), dassie_xrpl::MAX_DROPS);
}

/// A wallet the faucet funded, with its first sequence.
fn funded(ledger: &mut FakeLedger, seed: u8) -> Wallet {
    let wallet = Wallet::from_seed([seed; 16]);
    ledger.fund(wallet.account());
    close(ledger);
    wallet
}

fn secp(seed: u8) -> Secp256k1Key {
    Secp256k1Key::from_secret(&[seed; 32]).unwrap()
}

/// What every kind of transaction here has.
struct Base {
    account: AccountId,
    fee: u64,
    sequence: u32,
    signing: Signing,
}

fn base(ledger: &mut FakeLedger, account: AccountId, signing: Signing) -> Base {
    Base {
        account,
        fee: FEE_BASE,
        sequence: sequence(ledger, account),
        signing,
    }
}

fn account_set(base: Base, set_flag: Option<u32>, clear_flag: Option<u32>) -> AccountSet {
    AccountSet {
        account: base.account,
        set_flag,
        clear_flag,
        fee: base.fee,
        sequence: base.sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: base.signing,
    }
}

fn set_regular_key(base: Base, regular_key: Option<AccountId>) -> SetRegularKey {
    SetRegularKey {
        account: base.account,
        regular_key,
        fee: base.fee,
        sequence: base.sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: base.signing,
    }
}

fn signer_list_set(base: Base, quorum: u32, accounts: &[AccountId]) -> SignerListSet {
    SignerListSet {
        account: base.account,
        signer_quorum: quorum,
        signer_entries: accounts
            .iter()
            .map(|account| SignerEntry {
                account: *account,
                weight: 1,
            })
            .collect(),
        fee: base.fee,
        sequence: base.sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: base.signing,
    }
}

fn payment(base: Base, to: AccountId, drops: u64, tag: Option<u32>) -> Payment {
    Payment {
        account: base.account,
        destination: to,
        destination_tag: tag,
        amount: Amount::Xrp(drops),
        send_max: None,
        fee: base.fee,
        sequence: base.sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: base.signing,
    }
}

/// `tx` multi-signed by `keys`, each as its own account.
fn multi_signed<T: Transact + Clone>(tx: T, keys: &[&dyn Key]) -> Signed<T> {
    let signers: Vec<Signer> = keys
        .iter()
        .map(|key| tx.multi_sign(key.account(), *key))
        .collect();
    tx.with_signers(signers).unwrap()
}

/// An escrow as a hub's sponsor sets one up: a signer list of `signers`,
/// each of weight 1, with `quorum`; destination tags required; no regular
/// key; the master key disabled, last.
fn escrow(ledger: &mut FakeLedger, master: &Wallet, signers: &[AccountId], quorum: u32) {
    let id = master.account();
    let by_master = || Signing::by(master.public_key());
    let list = signer_list_set(base(ledger, id, by_master()), quorum, signers).sign(master);
    assert_eq!(submit(ledger, &list.blob), "tesSUCCESS");
    let next = Base {
        sequence: list.tx.sequence + 1,
        ..base(ledger, id, by_master())
    };
    let require = account_set(next, Some(ASF_REQUIRE_DEST), None).sign(master);
    assert_eq!(submit(ledger, &require.blob), "tesSUCCESS");
    let last = Base {
        sequence: require.tx.sequence + 1,
        ..base(ledger, id, by_master())
    };
    let disable = account_set(last, Some(ASF_DISABLE_MASTER), None).sign(master);
    assert_eq!(submit(ledger, &disable.blob), "tesSUCCESS");
    close(ledger);
    for hash in [list.hash, require.hash, disable.hash] {
        assert_eq!(tx(ledger, &hash).result, "tesSUCCESS");
    }
}

#[test]
fn an_escrow_is_set_up_and_checked_as_a_hubs_validators_check_it() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let id = master.account();
    let keys = [secp(11), secp(12), secp(13), secp(14)];
    let mut signers: Vec<AccountId> = keys.iter().map(Secp256k1Key::account).collect();
    escrow(&mut ledger, &master, &signers, 3);
    // Master key disabled, no regular key, destination tags required.
    let settings = settings(&mut ledger, id);
    assert!(settings.master_disabled() && settings.requires_destination_tag());
    assert_eq!(settings.regular_key, None);
    // The signer list exactly, sorted by account, and no other objects.
    let objects = objects(&mut ledger, id);
    let list = objects.signer_list.unwrap();
    signers.sort_unstable();
    assert_eq!(list.quorum, 3);
    assert_eq!(
        list.entries.iter().map(|e| e.account).collect::<Vec<_>>(),
        signers
    );
    assert!(list.entries.iter().all(|e| e.weight == 1));
    assert_eq!((objects.others, objects.marker), (0, None));
    assert!(objects.validated);
    // One object, whatever its entries: one owner reserve.
    assert_eq!(ledger.owner_count(id), Some(1));
    assert_eq!(ledger.signer_list(id), Some(&list));
    // The master key signs nothing more.
    let blocked = account_set(
        base(&mut ledger, id, Signing::by(master.public_key())),
        None,
        Some(ASF_REQUIRE_DEST),
    )
    .sign(&master);
    assert_eq!(submit(&mut ledger, &blocked.blob), "tefMASTER_DISABLED");
}

#[test]
fn the_master_key_is_disabled_only_by_itself_and_with_another_way_to_sign() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let id = master.account();
    let by_master = || Signing::by(master.public_key());
    // No regular key and no signer list: it would lock the account.
    let alone = account_set(
        base(&mut ledger, id, by_master()),
        Some(ASF_DISABLE_MASTER),
        None,
    )
    .sign(&master);
    assert_eq!(submit(&mut ledger, &alone.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &alone.hash).result, "tecNO_ALTERNATIVE_KEY");
    assert_eq!(ledger.account_flags(id), Some(0));
    // A regular key may sign for it, but not disable the master key.
    let regular = secp(21);
    let set =
        set_regular_key(base(&mut ledger, id, by_master()), Some(regular.account())).sign(&master);
    assert_eq!(submit(&mut ledger, &set.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(ledger.regular_key(id), Some(regular.account()));
    let by_regular = account_set(
        base(&mut ledger, id, Signing::by(regular.public_key())),
        Some(ASF_DISABLE_MASTER),
        None,
    )
    .sign(&regular);
    assert_eq!(submit(&mut ledger, &by_regular.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(
        tx(&mut ledger, &by_regular.hash).result,
        "tecNEED_MASTER_KEY"
    );
    let disable = account_set(
        base(&mut ledger, id, by_master()),
        Some(ASF_DISABLE_MASTER),
        None,
    )
    .sign(&master);
    assert_eq!(submit(&mut ledger, &disable.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(
        ledger.account_flags(id),
        Some(account_flags::DISABLE_MASTER)
    );
    // The regular key is now the only way in: it cannot be removed.
    let remove = set_regular_key(
        base(&mut ledger, id, Signing::by(regular.public_key())),
        None,
    )
    .sign(&regular);
    assert_eq!(submit(&mut ledger, &remove.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(
        tx(&mut ledger, &remove.hash).result,
        "tecNO_ALTERNATIVE_KEY"
    );
    // It enables the master key again, and the master key signs.
    let enable = account_set(
        base(&mut ledger, id, Signing::by(regular.public_key())),
        None,
        Some(ASF_DISABLE_MASTER),
    )
    .sign(&regular);
    assert_eq!(submit(&mut ledger, &enable.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(ledger.account_flags(id), Some(0));
    // Another key is neither: refused at once, and nothing used.
    let other = secp(22);
    let stranger = account_set(
        base(&mut ledger, id, Signing::by(other.public_key())),
        None,
        None,
    )
    .sign(&other);
    assert_eq!(submit(&mut ledger, &stranger.blob), "tefBAD_AUTH");
    let own = set_regular_key(base(&mut ledger, id, by_master()), Some(id)).sign(&master);
    assert_eq!(submit(&mut ledger, &own.blob), "temBAD_REGKEY");
}

#[test]
fn a_payment_to_an_account_that_requires_a_tag_names_one() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let payer = funded(&mut ledger, 2);
    let id = master.account();
    escrow(
        &mut ledger,
        &master,
        &[secp(11).account(), secp(12).account()],
        2,
    );
    let by_payer = || Signing::by(payer.public_key());
    let before = ledger.balance(payer.account()).unwrap();
    // No tag: refused, with its fee and sequence spent.
    let untagged = payment(
        base(&mut ledger, payer.account(), by_payer()),
        id,
        5_000_000,
        None,
    )
    .sign(&payer);
    assert_eq!(submit(&mut ledger, &untagged.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &untagged.hash).result, "tecDST_TAG_NEEDED");
    assert_eq!(ledger.balance(payer.account()), Some(before - FEE_BASE));
    // With a tag: paid, and the tag is in what the ledger says of it.
    let subscribe = json!({ "id": 9, "command": "subscribe", "accounts": [id.to_address()] });
    request_ok(&mut ledger, &subscribe);
    let tagged = payment(
        base(&mut ledger, payer.account(), by_payer()),
        id,
        5_000_000,
        Some(77),
    )
    .sign(&payer);
    assert_eq!(tagged.tx.sequence, untagged.tx.sequence + 1);
    assert_eq!(submit(&mut ledger, &tagged.blob), "tesSUCCESS");
    let streamed: Vec<TxRecord> = ledger
        .close()
        .into_iter()
        .filter_map(|(_, bytes)| match api::parse(&bytes) {
            api::Message::Transaction(record) => Some(*record),
            _ => None,
        })
        .collect();
    let record = tx(&mut ledger, &tagged.hash);
    assert_eq!(record.result, "tesSUCCESS");
    assert_eq!(
        (record.destination_tag, record.delivered),
        (Some(77), Some(5_000_000))
    );
    assert_eq!(streamed.len(), 1);
    assert_eq!(streamed[0].destination_tag, Some(77));
    let history = request(
        &mut ledger,
        &api::account_tx(5, id, dassie_xrpl::fake::FIRST_LEDGER, u32::MAX, None),
    )
    .unwrap();
    let history = api::account_tx_result(&history).unwrap().transactions;
    assert_eq!(history.last().unwrap().destination_tag, Some(77));
    // The fake's own deposits are held to it too.
    ledger.deposit(id, 1_000_000);
    ledger.deposit_tagged(id, 1_000_000, Some(3));
    let balance = ledger.balance(id).unwrap();
    close(&mut ledger);
    assert_eq!(ledger.balance(id), Some(balance + 1_000_000));
}

fn request_ok(ledger: &mut FakeLedger, value: &Value) -> Value {
    request(ledger, &serde_json::to_vec(value).unwrap()).unwrap()
}

#[test]
fn signers_on_the_list_pay_out_once_they_reach_the_quorum() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let member = funded(&mut ledger, 2);
    let id = master.account();
    let ed = Wallet::from_seed([31; 16]);
    let keys = [secp(11), secp(12), secp(13)];
    let accounts = [
        keys[0].account(),
        keys[1].account(),
        keys[2].account(),
        ed.account(),
    ];
    escrow(&mut ledger, &master, &accounts, 3);
    let payout = |ledger: &mut FakeLedger, fee: u64| Payment {
        fee,
        ..payment(
            base(ledger, id, Signing::multi()),
            member.account(),
            2_000_000,
            Some(1),
        )
    };
    let three: [&dyn Key; 3] = [&keys[0], &ed, &keys[2]];
    // One base fee for each signer, and once more.
    let cheap = multi_signed(
        payout(&mut ledger, multi_signed_fee(FEE_BASE, 3) - 1),
        &three,
    );
    assert_eq!(submit(&mut ledger, &cheap.blob), "telINSUF_FEE_P");
    // Below the quorum, or a signer not on the list.
    let fee = multi_signed_fee(FEE_BASE, 3);
    let two = multi_signed(payout(&mut ledger, fee), &[&keys[0], &keys[1]]);
    assert_eq!(submit(&mut ledger, &two.blob), "tefBAD_QUORUM");
    let stranger = secp(19);
    let outsider = multi_signed(payout(&mut ledger, fee), &[&keys[0], &keys[1], &stranger]);
    assert_eq!(submit(&mut ledger, &outsider.blob), "tefBAD_SIGNATURE");
    // Unsorted, or a signature on something else: refused before anything.
    let mut unsorted = three_signers(&payout(&mut ledger, fee), &three);
    unsorted.sort_unstable_by_key(|signer| std::cmp::Reverse(signer.account));
    let blob = Payment {
        signing: Signing::Multi(unsorted),
        ..payout(&mut ledger, fee)
    }
    .blob();
    assert_eq!(submit(&mut ledger, &blob), "temBAD_SIGNATURE");
    let mut forged = three_signers(&payout(&mut ledger, fee), &three);
    forged[0].signature = forged[1].signature;
    forged.sort_unstable_by_key(|signer| signer.account);
    let blob = Payment {
        signing: Signing::Multi(forged),
        ..payout(&mut ledger, fee)
    }
    .blob();
    assert_eq!(submit(&mut ledger, &blob), "temBAD_SIGNATURE");
    // Three of four: paid, with no last ledger, many ledgers on.
    for _ in 0..30 {
        close(&mut ledger);
    }
    let before = ledger.balance(member.account()).unwrap();
    let paid = multi_signed(payout(&mut ledger, fee), &three);
    assert_eq!(submit(&mut ledger, &paid.blob), "tesSUCCESS");
    close(&mut ledger);
    let record = tx(&mut ledger, &paid.hash);
    assert_eq!((record.result.as_str(), record.fee), ("tesSUCCESS", fee));
    assert_eq!(ledger.balance(member.account()), Some(before + 2_000_000));
}

fn three_signers(tx: &Payment, keys: &[&dyn Key]) -> Vec<Signer> {
    keys.iter()
        .map(|key| tx.multi_sign(key.account(), *key))
        .collect()
}

#[test]
fn signers_no_longer_on_the_list_when_it_applies_apply_nothing() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let member = funded(&mut ledger, 2);
    let id = master.account();
    let old = [secp(11), secp(12), secp(13)];
    let new = [secp(41), secp(42), secp(43)];
    let old_accounts: Vec<AccountId> = old.iter().map(Secp256k1Key::account).collect();
    let new_accounts: Vec<AccountId> = new.iter().map(Secp256k1Key::account).collect();
    escrow(&mut ledger, &master, &old_accounts, 2);
    let start = sequence(&mut ledger, id);
    let fee = multi_signed_fee(FEE_BASE, 2);
    // A selector replaces the signers; a payment signed by the old ones
    // takes the next sequence. Both are taken when submitted.
    let selector = multi_signed(
        SignerListSet {
            fee,
            ..signer_list_set(base(&mut ledger, id, Signing::multi()), 2, &new_accounts)
        },
        &[&old[0], &old[1]],
    );
    let stale = multi_signed(
        Payment {
            fee,
            sequence: start + 1,
            ..payment(
                base(&mut ledger, id, Signing::multi()),
                member.account(),
                1_000_000,
                Some(5),
            )
        },
        &[&old[1], &old[2]],
    );
    assert_eq!(submit(&mut ledger, &selector.blob), "tesSUCCESS");
    assert_eq!(submit(&mut ledger, &stale.blob), "tesSUCCESS");
    let before = ledger.balance(id).unwrap();
    close(&mut ledger);
    // The selector applied; the payment did not, cost nothing and left
    // its sequence for the new signers.
    assert_eq!(tx(&mut ledger, &selector.hash).result, "tesSUCCESS");
    let lookup = request(&mut ledger, &api::tx_anywhere(6, &stale.hash));
    assert_eq!(lookup.unwrap_err().error, "txnNotFound");
    assert_eq!(sequence(&mut ledger, id), start + 1);
    assert_eq!(ledger.balance(id), Some(before - fee));
    let mut listed = new_accounts.clone();
    listed.sort_unstable();
    let list = ledger.signer_list(id).unwrap();
    assert_eq!(
        list.entries.iter().map(|e| e.account).collect::<Vec<_>>(),
        listed
    );
    // Submitted again, it is refused at once; the new signers pay out.
    assert_eq!(submit(&mut ledger, &stale.blob), "tefBAD_SIGNATURE");
    let fresh = multi_signed(
        Payment {
            fee,
            ..payment(
                base(&mut ledger, id, Signing::multi()),
                member.account(),
                1_000_000,
                Some(5),
            )
        },
        &[&new[0], &new[2]],
    );
    assert_eq!(fresh.tx.sequence, start + 1);
    assert_eq!(submit(&mut ledger, &fresh.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &fresh.hash).result, "tesSUCCESS");
    // The selector's metadata shows the list it replaced.
    let record = request(&mut ledger, &api::tx_anywhere(7, &selector.hash)).unwrap();
    let nodes = record["meta"]["AffectedNodes"].as_array().unwrap();
    let list = nodes
        .iter()
        .find(|node| node["ModifiedNode"]["LedgerEntryType"] == "SignerList")
        .unwrap();
    assert_eq!(list["ModifiedNode"]["FinalFields"]["SignerQuorum"], 2);
    assert_eq!(record["Signers"].as_array().unwrap().len(), 2);
    assert_eq!(record["SigningPubKey"], "");
}

#[test]
fn a_signer_signs_with_its_accounts_regular_key_never_a_disabled_master() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let signer = funded(&mut ledger, 3);
    let id = master.account();
    let phantom = secp(11);
    escrow(
        &mut ledger,
        &master,
        &[signer.account(), phantom.account()],
        2,
    );
    // The signer's account names a regular key, which signs for it.
    let regular = secp(51);
    let set = set_regular_key(
        base(
            &mut ledger,
            signer.account(),
            Signing::by(signer.public_key()),
        ),
        Some(regular.account()),
    )
    .sign(&signer);
    assert_eq!(submit(&mut ledger, &set.blob), "tesSUCCESS");
    close(&mut ledger);
    let fee = multi_signed_fee(FEE_BASE, 2);
    let pay = |ledger: &mut FakeLedger| Payment {
        fee,
        ..payment(
            base(ledger, id, Signing::multi()),
            signer.account(),
            1_000_000,
            Some(9),
        )
    };
    let tx_by_regular = pay(&mut ledger);
    let signers = vec![
        tx_by_regular.multi_sign(signer.account(), &regular),
        tx_by_regular.multi_sign(phantom.account(), &phantom),
    ];
    let by_regular = tx_by_regular.with_signers(signers).unwrap();
    assert_eq!(submit(&mut ledger, &by_regular.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &by_regular.hash).result, "tesSUCCESS");
    // Another key as the signer's: not its regular key.
    let wrong_tx = pay(&mut ledger);
    let other = secp(52);
    let wrong = wrong_tx
        .clone()
        .with_signers(vec![
            wrong_tx.multi_sign(signer.account(), &other),
            wrong_tx.multi_sign(phantom.account(), &phantom),
        ])
        .unwrap();
    assert_eq!(submit(&mut ledger, &wrong.blob), "tefBAD_SIGNATURE");
    // The signer disables its master key: its master key no longer signs.
    let disable = account_set(
        base(
            &mut ledger,
            signer.account(),
            Signing::by(signer.public_key()),
        ),
        Some(ASF_DISABLE_MASTER),
        None,
    )
    .sign(&signer);
    assert_eq!(submit(&mut ledger, &disable.blob), "tesSUCCESS");
    close(&mut ledger);
    let master_tx = pay(&mut ledger);
    let by_master = master_tx
        .clone()
        .with_signers(vec![
            master_tx.multi_sign(signer.account(), &signer),
            master_tx.multi_sign(phantom.account(), &phantom),
        ])
        .unwrap();
    assert_eq!(submit(&mut ledger, &by_master.blob), "tefMASTER_DISABLED");
}

#[test]
fn signer_lists_are_refused_as_the_ledger_refuses_them() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let id = master.account();
    let by_master = || Signing::by(master.public_key());
    let a = secp(11).account();
    let b = secp(12).account();
    let refused =
        |ledger: &mut FakeLedger, tx: SignerListSet| submit(ledger, &tx.sign(&master).blob);
    let list = |ledger: &mut FakeLedger, quorum, accounts: &[AccountId]| {
        signer_list_set(base(ledger, id, by_master()), quorum, accounts)
    };
    let set = list(&mut ledger, 3, &[a, b]);
    assert_eq!(refused(&mut ledger, set), "temBAD_QUORUM");
    let set = list(&mut ledger, 1, &[a, id]);
    assert_eq!(refused(&mut ledger, set), "temBAD_SIGNER");
    let set = list(&mut ledger, 1, &[a, a]);
    assert_eq!(refused(&mut ledger, set), "temBAD_SIGNER");
    let mut weightless = list(&mut ledger, 1, &[a, b]);
    weightless.signer_entries[1].weight = 0;
    assert_eq!(refused(&mut ledger, weightless), "temBAD_WEIGHT");
    let set = list(&mut ledger, 1, &[]);
    assert_eq!(refused(&mut ledger, set), "temMALFORMED");
    let set = list(&mut ledger, 0, &[a]);
    assert_eq!(refused(&mut ledger, set), "temMALFORMED");
    let flagged = SignerListSet {
        flags: 0x0001_0000,
        ..list(&mut ledger, 1, &[a])
    };
    assert_eq!(refused(&mut ledger, flagged), "temINVALID_FLAG");
    // Deleting a list that is the only way to sign is refused.
    escrow(&mut ledger, &master, &[a, b], 2);
    let remove = signer_list_set(base(&mut ledger, id, Signing::multi()), 0, &[]);
    let remove = multi_signed(
        SignerListSet {
            fee: multi_signed_fee(FEE_BASE, 2),
            ..remove
        },
        &[&secp(11), &secp(12)],
    );
    assert_eq!(submit(&mut ledger, &remove.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(
        tx(&mut ledger, &remove.hash).result,
        "tecNO_ALTERNATIVE_KEY"
    );
    assert_eq!(ledger.owner_count(id), Some(1));
}

#[test]
fn a_signer_list_holds_one_reserve_and_deleting_it_frees_it() {
    let mut ledger = FakeLedger::new();
    let poor = Wallet::from_seed([7; 16]);
    // Enough for the base reserve and fees, not for one more object.
    ledger.deposit(poor.account(), RESERVE_BASE + RESERVE_INC - 1);
    close(&mut ledger);
    let id = poor.account();
    let by_poor = || Signing::by(poor.public_key());
    let a = secp(11).account();
    let list = signer_list_set(base(&mut ledger, id, by_poor()), 1, &[a]).sign(&poor);
    assert_eq!(submit(&mut ledger, &list.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(
        tx(&mut ledger, &list.hash).result,
        "tecINSUFFICIENT_RESERVE"
    );
    assert_eq!(ledger.owner_count(id), Some(0));
    ledger.deposit(id, RESERVE_INC);
    close(&mut ledger);
    let list = signer_list_set(base(&mut ledger, id, by_poor()), 1, &[a]).sign(&poor);
    assert_eq!(submit(&mut ledger, &list.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &list.hash).result, "tesSUCCESS");
    assert_eq!(ledger.owner_count(id), Some(1));
    // Replacing it takes no more reserve; deleting it frees it.
    let b = secp(12).account();
    let again = signer_list_set(base(&mut ledger, id, by_poor()), 2, &[a, b]).sign(&poor);
    assert_eq!(submit(&mut ledger, &again.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &again.hash).result, "tesSUCCESS");
    assert_eq!(ledger.owner_count(id), Some(1));
    let delete = signer_list_set(base(&mut ledger, id, by_poor()), 0, &[]).sign(&poor);
    assert_eq!(submit(&mut ledger, &delete.blob), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &delete.hash).result, "tesSUCCESS");
    assert_eq!(ledger.owner_count(id), Some(0));
    assert_eq!(ledger.signer_list(id), None);
    assert_eq!(objects(&mut ledger, id).signer_list, None);
    // Without a list, multi-signing signs nothing.
    let unlisted = multi_signed(
        Payment {
            fee: multi_signed_fee(FEE_BASE, 1),
            ..payment(base(&mut ledger, id, Signing::multi()), a, 1, None)
        },
        &[&secp(11)],
    );
    assert_eq!(submit(&mut ledger, &unlisted.blob), "tefNOT_MULTI_SIGNING");
}

#[test]
fn account_settings_the_fake_does_not_model_are_refused() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let id = master.account();
    let by_master = || Signing::by(master.public_key());
    let submitted =
        |ledger: &mut FakeLedger, set: AccountSet| submit(ledger, &set.sign(&master).blob);
    // asfDefaultRipple (8): the ledger has it, the fake does not.
    let set = account_set(base(&mut ledger, id, by_master()), Some(8), None);
    assert_eq!(submitted(&mut ledger, set), "temINVALID_FLAG");
    let set = account_set(
        base(&mut ledger, id, by_master()),
        Some(ASF_REQUIRE_DEST),
        Some(ASF_REQUIRE_DEST),
    );
    assert_eq!(submitted(&mut ledger, set), "temINVALID_FLAG");
    let both = AccountSet {
        flags: TF_REQUIRE_DEST_TAG | TF_OPTIONAL_DEST_TAG,
        ..account_set(base(&mut ledger, id, by_master()), None, None)
    };
    assert_eq!(submitted(&mut ledger, both), "temINVALID_FLAG");
    // The transaction flag requires a tag as the setting does.
    let flag = AccountSet {
        flags: TF_REQUIRE_DEST_TAG,
        ..account_set(base(&mut ledger, id, by_master()), None, None)
    };
    assert_eq!(submitted(&mut ledger, flag), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(
        ledger.account_flags(id),
        Some(account_flags::REQUIRE_DEST_TAG)
    );
    let optional = AccountSet {
        flags: TF_OPTIONAL_DEST_TAG,
        ..account_set(base(&mut ledger, id, by_master()), None, None)
    };
    assert_eq!(submitted(&mut ledger, optional), "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(ledger.account_flags(id), Some(0));
}

#[test]
fn a_reset_takes_the_signer_lists_with_the_accounts() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let id = master.account();
    escrow(
        &mut ledger,
        &master,
        &[secp(11).account(), secp(12).account()],
        2,
    );
    assert!(ledger.signer_list(id).is_some());
    ledger.reset();
    assert_eq!(ledger.signer_list(id), None);
    let result = request(&mut ledger, &api::account_objects(8, id, None));
    assert_eq!(result.unwrap_err().error, "actNotFound");
    // Made again at the same address, it is set up again from scratch.
    let master = funded(&mut ledger, 1);
    escrow(
        &mut ledger,
        &master,
        &[secp(11).account(), secp(12).account()],
        2,
    );
    assert_eq!(ledger.owner_count(id), Some(1));
}

#[test]
fn account_objects_come_a_page_at_a_time() {
    let mut ledger = FakeLedger::new();
    let master = funded(&mut ledger, 1);
    let id = master.account();
    escrow(
        &mut ledger,
        &master,
        &[secp(11).account(), secp(12).account()],
        2,
    );
    // A trust line is another object, after the list.
    let line = dassie_xrpl::codec::TrustSet {
        account: id,
        limit: dassie_xrpl::fake::Token::RLUSD.amount(dassie_xrpl::codec::TokenValue::MAX),
        fee: multi_signed_fee(FEE_BASE, 2),
        sequence: sequence(&mut ledger, id),
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::multi(),
    };
    let line = multi_signed(line, &[&secp(11), &secp(12)]);
    assert_eq!(submit(&mut ledger, &line.blob), "tesSUCCESS");
    close(&mut ledger);
    let all = objects(&mut ledger, id);
    assert!(all.signer_list.is_some());
    assert_eq!((all.others, all.marker), (1, None));
    let page = |ledger: &mut FakeLedger, marker: Option<&Value>| {
        let mut params: Value =
            serde_json::from_slice(&api::account_objects(9, id, marker)).unwrap();
        params["limit"] = 1.into();
        let result = request(ledger, &serde_json::to_vec(&params).unwrap()).unwrap();
        api::account_objects_result(&result).unwrap()
    };
    let first = page(&mut ledger, None);
    assert!(first.signer_list.is_some() && first.others == 0);
    let second = page(&mut ledger, first.marker.as_ref());
    assert!(second.signer_list.is_none() && second.others == 1);
    assert_eq!(second.marker, None);
    assert_eq!(hex_upper(&line.hash), line.hash_hex());
}

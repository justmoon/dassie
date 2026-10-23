#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;
use crate::keys::Secp256k1Key;

fn payment(wallet: &Wallet) -> Payment {
    Payment {
        account: wallet.account(),
        destination: AccountId([9; 20]),
        destination_tag: None,
        amount: Amount::Xrp(1_000),
        send_max: None,
        fee: 12,
        sequence: 5,
        last_ledger_sequence: Some(25),
        flags: 0,
        network_id: None,
        memos: alloc::vec![Memo {
            memo_type: alloc::vec![1; 200],
            memo_data: alloc::vec![2, 3],
        }],
        signing: Signing::by(wallet.public_key()),
    }
}

#[test]
fn signed_payments_decode_to_themselves() {
    let wallet = Wallet::from_seed([1; 16]);
    let signed = payment(&wallet).sign(&wallet);
    assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
    assert_eq!(hash(&signed.blob), signed.hash);
    let unsigned = Payment {
        signing: Signing::by(wallet.public_key()),
        ..signed.tx
    };
    assert_eq!(
        Payment::decode(unsigned.signing_payload().get(4..).unwrap()),
        Ok(unsigned)
    );
}

#[test]
fn other_transactions_are_refused() {
    let wallet = Wallet::from_seed([1; 16]);
    let signed = payment(&wallet).sign(&wallet);
    let mut other = signed.blob.clone();
    // A TransactionType other than Payment.
    other[2] = 1;
    assert_eq!(Payment::decode(&other), Err(DecodeError::Value));
    assert_eq!(Transaction::decode(&other), Err(DecodeError::Value));
    let mut longer = signed.blob.clone();
    longer.push(0);
    assert!(Payment::decode(&longer).is_err());
    // A cut blob is no Payment, or one without the memos: optional
    // fields come last.
    for cut in 0..signed.blob.len() {
        match Payment::decode(&signed.blob[..cut]) {
            Err(_) => {}
            Ok(shorter) => assert!(shorter.memos.is_empty(), "{cut}"),
        }
    }
}

fn rlusd(value: &str) -> TokenAmount {
    TokenAmount {
        value: TokenValue::parse(value).unwrap(),
        currency: Currency::parse("524C555344000000000000000000000000000000").unwrap(),
        issuer: AccountId([7; 20]),
    }
}

fn trust_set(wallet: &Wallet) -> TrustSet {
    TrustSet {
        account: wallet.account(),
        limit: rlusd("1"),
        fee: 12,
        sequence: 6,
        last_ledger_sequence: Some(26),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
}

#[test]
fn token_payments_and_trust_sets_decode_to_themselves() {
    let wallet = Wallet::from_seed([1; 16]);
    let token = Payment {
        amount: Amount::Token(rlusd("1234567890.123456")),
        ..payment(&wallet)
    };
    let signed = token.sign(&wallet);
    assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
    assert_eq!(
        Transaction::decode(&signed.blob),
        Ok(Transaction::Payment(signed.tx.clone()))
    );
    let trust_set = TrustSet {
        limit: TokenAmount {
            value: TokenValue::MAX,
            ..rlusd("0")
        },
        flags: TF_SET_NO_RIPPLE,
        network_id: Some(21_338),
        ..trust_set(&wallet)
    };
    let signed = trust_set.sign(&wallet);
    assert_eq!(TrustSet::decode(&signed.blob), Ok(signed.tx.clone()));
    assert_eq!(
        Transaction::decode(&signed.blob),
        Ok(Transaction::TrustSet(signed.tx.clone()))
    );
    assert_eq!(
        Transaction::decode(&signed.blob).unwrap().blob(),
        signed.blob
    );
    // Each kind refuses the other.
    assert_eq!(Payment::decode(&signed.blob), Err(DecodeError::Value));
}

#[test]
fn partial_payments_and_negative_amounts_are_refused() {
    let wallet = Wallet::from_seed([1; 16]);
    let token = Payment {
        amount: Amount::Token(rlusd("1")),
        ..payment(&wallet)
    };
    let blob = token.blob();
    // Flags are bytes 4 to 7 of the blob: TransactionType, then Flags.
    let mut partial = blob.clone();
    partial[4..8].copy_from_slice(&TF_PARTIAL_PAYMENT.to_be_bytes());
    assert_eq!(Payment::decode(&partial), Err(DecodeError::Value));
    // The sign bit is the second of the amount's first byte, after
    // Sequence and LastLedgerSequence.
    let amount_at = blob.iter().position(|b| *b == 0x61).unwrap() + 1;
    let mut negative = blob.clone();
    negative[amount_at] &= !0x40;
    assert_eq!(Payment::decode(&negative), Err(DecodeError::Value));
    // A token of the zero issuer is none.
    let mut zero_issuer = blob;
    zero_issuer[amount_at + 28..amount_at + 48].fill(0);
    assert_eq!(Payment::decode(&zero_issuer), Err(DecodeError::Value));
}

#[test]
#[should_panic(expected = "never a partial payment")]
fn the_node_never_signs_a_partial_payment() {
    let wallet = Wallet::from_seed([1; 16]);
    let partial = Payment {
        flags: TF_PARTIAL_PAYMENT,
        ..payment(&wallet)
    };
    let _ = partial.sign(&wallet);
}

#[test]
#[should_panic(expected = "signed by the key it names")]
fn a_transaction_is_signed_by_the_key_it_names() {
    let wallet = Wallet::from_seed([1; 16]);
    let _ = payment(&wallet).sign(&Wallet::from_seed([2; 16]));
}

fn terms(amount: Amount, send_max: Amount) -> SendMaxTerms {
    SendMaxTerms {
        destination: AccountId([9; 20]),
        amount,
        send_max,
        fee: 12,
        sequence: 5,
        last_ledger_sequence: 25,
        network_id: None,
        memos: Vec::new(),
    }
}

#[test]
fn send_max_payments_decode_to_themselves_either_way() {
    let wallet = Wallet::from_seed([1; 16]);
    for (amount, send_max) in [
        (Amount::Token(rlusd("25")), Amount::Xrp(30_000_000)),
        (Amount::Xrp(1_000_000), Amount::Token(rlusd("0.75"))),
        (Amount::Token(rlusd("1")), Amount::Token(rlusd("1.002"))),
    ] {
        let signed = sign_send_max(&wallet, terms(amount, send_max)).unwrap();
        assert_eq!(signed.tx.send_max, Some(send_max));
        assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
        assert_eq!(
            Transaction::decode(&signed.blob).unwrap().blob(),
            signed.blob
        );
        assert_eq!(hash(&signed.blob), signed.hash);
    }
}

#[test]
fn send_max_is_refused_where_the_ledger_refuses_it() {
    let wallet = Wallet::from_seed([1; 16]);
    let xrp = |drops| Amount::Xrp(drops);
    let refused = [
        (terms(xrp(1), xrp(2)), TermsError::SendMax),
        (terms(xrp(0), Amount::Token(rlusd("1"))), TermsError::Amount),
        (
            terms(xrp(1), Amount::Token(rlusd("0"))),
            TermsError::SendMax,
        ),
        (
            terms(xrp(crate::MAX_DROPS + 1), Amount::Token(rlusd("1"))),
            TermsError::Amount,
        ),
        (
            terms(Amount::Token(rlusd("1")), xrp(crate::MAX_DROPS + 1)),
            TermsError::SendMax,
        ),
        (
            terms(Amount::Token(rlusd("-1")), xrp(1)),
            TermsError::Amount,
        ),
        (
            SendMaxTerms {
                fee: crate::MAX_DROPS + 1,
                ..terms(xrp(1), Amount::Token(rlusd("1")))
            },
            TermsError::Fee,
        ),
        (
            SendMaxTerms {
                memos: alloc::vec![
                    Memo {
                        memo_type: Vec::new(),
                        memo_data: Vec::new(),
                    };
                    MAX_MEMOS + 1
                ],
                ..terms(xrp(1), Amount::Token(rlusd("1")))
            },
            TermsError::Memos,
        ),
        (
            SendMaxTerms {
                destination: wallet.account(),
                ..terms(Amount::Token(rlusd("1")), Amount::Token(rlusd("2")))
            },
            TermsError::Redundant,
        ),
    ];
    for (terms, error) in refused {
        assert_eq!(
            sign_send_max(&wallet, terms.clone()),
            Err(error),
            "{terms:?}"
        );
    }
    // A swap to oneself is no redundant Payment.
    let swap = SendMaxTerms {
        destination: wallet.account(),
        ..terms(xrp(1), Amount::Token(rlusd("1")))
    };
    assert!(sign_send_max(&wallet, swap).is_ok());
}

#[test]
fn the_decoder_refuses_what_the_encoder_never_writes() {
    let wallet = Wallet::from_seed([1; 16]);
    let token = Amount::Token(rlusd("1"));
    let blob = sign_send_max(&wallet, terms(token, Amount::Xrp(5)))
        .unwrap()
        .blob;
    // XRP for XRP: the Amount's eight bytes after its header, 0x61.
    let at = blob.iter().position(|b| *b == 0x61).unwrap() + 1;
    let mut xrp_for_xrp = blob[..at].to_vec();
    xrp_for_xrp.extend_from_slice(&wire::xrp(7));
    xrp_for_xrp.extend_from_slice(&blob[at + 48..]);
    assert_eq!(Payment::decode(&xrp_for_xrp), Err(DecodeError::Value));
    // A negative SendMax.
    let mut negative = sign_send_max(&wallet, terms(Amount::Xrp(5), token))
        .unwrap()
        .blob;
    let at = negative.iter().position(|b| *b == 0x69).unwrap() + 1;
    negative[at] &= !0x40;
    assert_eq!(Payment::decode(&negative), Err(DecodeError::Value));
    // A TrustSet has no SendMax.
    let trust_set = trust_set(&wallet).blob();
    let at = trust_set.iter().position(|b| *b == 0x68).unwrap() + 9;
    let mut with_send_max = trust_set[..at].to_vec();
    with_send_max.push(0x69);
    with_send_max.extend_from_slice(&wire::xrp(5));
    with_send_max.extend_from_slice(&trust_set[at..]);
    assert_eq!(TrustSet::decode(&with_send_max), Err(DecodeError::Field));
}

#[test]
#[should_panic(expected = "a SendMax the ledger takes")]
fn the_node_never_signs_xrp_for_xrp() {
    let wallet = Wallet::from_seed([1; 16]);
    let payment = Payment {
        send_max: Some(Amount::Xrp(2_000)),
        ..payment(&wallet)
    };
    let _ = payment.sign(&wallet);
}

#[test]
fn fields_out_of_order_or_twice_are_refused() {
    let wallet = Wallet::from_seed([1; 16]);
    let blob = payment(&wallet).blob();
    // Flags (0x22) and Sequence (0x24), each with four bytes, swapped.
    assert_eq!(&blob[3..4], &[0x22]);
    assert_eq!(&blob[8..9], &[0x24]);
    let mut swapped = blob[..3].to_vec();
    swapped.extend_from_slice(&blob[8..13]);
    swapped.extend_from_slice(&blob[3..8]);
    swapped.extend_from_slice(&blob[13..]);
    assert_eq!(Payment::decode(&swapped), Err(DecodeError::Field));
    // Sequence twice.
    let mut twice = blob[..13].to_vec();
    twice.extend_from_slice(&blob[8..]);
    assert_eq!(Payment::decode(&twice), Err(DecodeError::Field));
    // Sequence's header in two bytes, where one does.
    let mut long = blob[..8].to_vec();
    long.extend_from_slice(&[0x20, 0x04]);
    long.extend_from_slice(&blob[9..]);
    assert_eq!(Payment::decode(&long), Err(DecodeError::Field));
}

#[test]
fn optional_fields_are_left_out_and_read_back() {
    let wallet = Wallet::from_seed([1; 16]);
    let tagged = Payment {
        destination_tag: Some(0xFFFF_FFFF),
        last_ledger_sequence: None,
        memos: Vec::new(),
        ..payment(&wallet)
    };
    let signed = tagged.sign(&wallet);
    assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
    // DestinationTag (2, 14) right after Sequence, in one byte: 0x2E.
    assert_eq!(&signed.blob[13..18], &[0x2E, 0xFF, 0xFF, 0xFF, 0xFF]);
    // An empty Memos array is never written, so never read.
    let mut empty_memos = signed.blob.clone();
    empty_memos.extend_from_slice(&[0xF9, 0xF1]);
    assert_eq!(Payment::decode(&empty_memos), Err(DecodeError::Value));
}

fn signer_list_set(account: AccountId, entries: Vec<SignerEntry>) -> SignerListSet {
    SignerListSet {
        account,
        signer_quorum: 3,
        signer_entries: entries,
        fee: 10,
        sequence: 7,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::multi(),
    }
}

fn entries(accounts: &[AccountId]) -> Vec<SignerEntry> {
    accounts
        .iter()
        .map(|account| SignerEntry {
            account: *account,
            weight: 1,
        })
        .collect()
}

#[test]
fn account_settings_decode_to_themselves() {
    let wallet = Wallet::from_seed([1; 16]);
    let key = Secp256k1Key::from_secret(&[0x22; 32]).unwrap();
    let set = AccountSet {
        account: wallet.account(),
        set_flag: Some(ASF_DISABLE_MASTER),
        clear_flag: Some(ASF_REQUIRE_DEST),
        fee: 10,
        sequence: 1,
        last_ledger_sequence: Some(30),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    };
    let regular = SetRegularKey {
        account: wallet.account(),
        regular_key: Some(key.account()),
        fee: 10,
        sequence: 2,
        last_ledger_sequence: None,
        flags: 0,
        network_id: Some(21_338),
        memos: Vec::new(),
        signing: Signing::by(key.public_key()),
    };
    let list = SignerListSet {
        signing: Signing::by(wallet.public_key()),
        ..signer_list_set(
            wallet.account(),
            entries(&[AccountId([3; 20]), key.account()]),
        )
    };
    let set = Transaction::AccountSet(set).sign(&wallet);
    let regular = Transaction::SetRegularKey(regular).sign(&key);
    let list = Transaction::SignerListSet(list).sign(&wallet);
    for signed in [set, regular, list] {
        assert_eq!(Transaction::decode(&signed.blob), Ok(signed.tx.clone()));
        assert!(signed.tx.signatures_valid());
        assert_eq!(hash(&signed.blob), signed.hash);
        assert_eq!(Payment::decode(&signed.blob), Err(DecodeError::Value));
    }
    // A list of no entries leaves the array out: a deletion.
    let delete = SignerListSet {
        signer_quorum: 0,
        ..signer_list_set(wallet.account(), Vec::new())
    };
    let blob = delete.blob();
    assert_eq!(SignerListSet::decode(&blob), Ok(delete));
}

#[test]
fn signers_sign_alone_and_are_put_together_in_order() {
    let payer = AccountId([0x55; 20]);
    let keys = [
        Secp256k1Key::from_secret(&[0x31; 32]).unwrap(),
        Secp256k1Key::from_secret(&[0x32; 32]).unwrap(),
        Secp256k1Key::from_secret(&[0x33; 32]).unwrap(),
    ];
    let ed = Wallet::from_seed([4; 16]);
    let payment = Payment {
        account: payer,
        last_ledger_sequence: None,
        destination_tag: Some(7),
        memos: Vec::new(),
        fee: multi_signed_fee(10, 4),
        signing: Signing::multi(),
        ..payment(&ed)
    };
    // Each signs the fields with SigningPubKey empty, and its account.
    let payload = payment.multi_signing_payload(ed.account());
    assert_eq!(&payload[..4], b"SMT\0");
    assert_eq!(&payload[payload.len() - 20..], &ed.account().0);
    let mut signers: Vec<Signer> = keys
        .iter()
        .map(|key| payment.multi_sign(key.account(), key))
        .collect();
    signers.push(payment.multi_sign(ed.account(), &ed));
    assert!(signers.iter().all(|signer| payment.verify_signer(signer)));
    // In any order; they come out sorted.
    signers.reverse();
    let signed = payment.clone().with_signers(signers.clone()).unwrap();
    let Signing::Multi(sorted) = &signed.tx.signing else {
        panic!("multi-signed");
    };
    assert!(
        sorted
            .windows(2)
            .all(|pair| pair[0].account < pair[1].account)
    );
    assert!(signed.tx.signatures_valid());
    assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
    assert_eq!(hash(&signed.blob), signed.hash);
    // Its signing payload is the unsigned one's: signers are not signed.
    assert_eq!(
        signed.tx.multi_signing_payload(ed.account()),
        payment.multi_signing_payload(ed.account())
    );
    // A signature on another account, twice the same, the sender, none.
    let mut wrong = signers.clone();
    wrong[0].account = AccountId([1; 20]);
    assert_eq!(
        payment.clone().with_signers(wrong),
        Err(SignersError::Signature)
    );
    let mut twice = signers.clone();
    twice.push(signers[0]);
    assert_eq!(
        payment.clone().with_signers(twice),
        Err(SignersError::Duplicate)
    );
    let as_sender = payment.multi_sign(payer, &keys[0]);
    assert_eq!(
        payment.clone().with_signers(alloc::vec![as_sender]),
        Err(SignersError::Sender)
    );
    assert_eq!(
        payment.clone().with_signers(Vec::new()),
        Err(SignersError::Count)
    );
    assert_eq!(
        payment.with_signers(alloc::vec![signers[0]; MAX_SIGNERS + 1]),
        Err(SignersError::Count)
    );
}

#[test]
fn signatures_that_are_not_valid_are_told_apart() {
    let wallet = Wallet::from_seed([1; 16]);
    let key = Secp256k1Key::from_secret(&[0x41; 32]).unwrap();
    let other = Secp256k1Key::from_secret(&[0x42; 32]).unwrap();
    let unsigned = payment(&wallet);
    assert!(!unsigned.signatures_valid());
    let mut signed = unsigned.clone().sign(&wallet).tx;
    assert!(signed.signatures_valid());
    signed.sequence += 1;
    assert!(!signed.signatures_valid());
    // Unsorted signers are not valid, though each signature is.
    let multi = Payment {
        signing: Signing::multi(),
        ..unsigned
    };
    let mut signers = alloc::vec![
        multi.multi_sign(key.account(), &key),
        multi.multi_sign(other.account(), &other),
    ];
    signers.sort_unstable_by_key(|signer| core::cmp::Reverse(signer.account));
    let unsorted = Payment {
        signing: Signing::Multi(signers.clone()),
        ..multi.clone()
    };
    assert!(!unsorted.signatures_valid());
    signers.reverse();
    let sorted = Payment {
        signing: Signing::Multi(signers),
        ..multi.clone()
    };
    assert!(sorted.signatures_valid());
    assert!(!multi.signatures_valid());
}

#[test]
fn single_and_multi_signing_never_mix_in_a_blob() {
    let wallet = Wallet::from_seed([1; 16]);
    let key = Secp256k1Key::from_secret(&[0x41; 32]).unwrap();
    let multi = Payment {
        signing: Signing::multi(),
        ..payment(&wallet)
    };
    let signed = multi
        .clone()
        .with_signers(alloc::vec![multi.multi_sign(key.account(), &key)])
        .unwrap();
    // SigningPubKey is empty: 0x73 0x00. Give it a key instead.
    let at = signed
        .blob
        .windows(2)
        .position(|pair| pair == [0x73, 0x00])
        .unwrap();
    let mut keyed = signed.blob[..at].to_vec();
    keyed.push(0x73);
    keyed.push(33);
    keyed.extend_from_slice(wallet.public_key().as_bytes());
    keyed.extend_from_slice(&signed.blob[at + 2..]);
    assert_eq!(Payment::decode(&keyed), Err(DecodeError::Value));
    // A TxnSignature with an empty SigningPubKey.
    let single = payment(&wallet).sign(&wallet).blob;
    let at = single.iter().position(|b| *b == 0x73).unwrap();
    let mut keyless = single[..at].to_vec();
    keyless.extend_from_slice(&[0x73, 0x00]);
    keyless.extend_from_slice(&single[at + 2 + 33..]);
    assert_eq!(Payment::decode(&keyless), Err(DecodeError::Value));
    // A key of no kind the ledger knows.
    let mut unknown = single.clone();
    unknown[at + 2] = 0x04;
    assert_eq!(Payment::decode(&unknown), Err(DecodeError::Value));
}

#[test]
fn signer_lists_are_checked_as_the_ledger_checks_them() {
    let account = AccountId([1; 20]);
    let list = entries(&[AccountId([2; 20]), AccountId([3; 20]), AccountId([4; 20])]);
    assert_eq!(check_signer_list(account, 3, &list), Ok(()));
    assert_eq!(
        check_signer_list(account, 4, &list),
        Err(SignerListError::Quorum)
    );
    assert_eq!(
        check_signer_list(account, 0, &list),
        Err(SignerListError::Quorum)
    );
    assert_eq!(
        check_signer_list(account, 1, &[]),
        Err(SignerListError::Count)
    );
    let many: Vec<AccountId> = (0..=32_u8).map(|n| AccountId([n + 10; 20])).collect();
    assert_eq!(
        check_signer_list(account, 1, &entries(&many)),
        Err(SignerListError::Count)
    );
    assert_eq!(check_signer_list(account, 1, &entries(&many[..32])), Ok(()));
    let mut zero = list.clone();
    zero[1].weight = 0;
    assert_eq!(
        check_signer_list(account, 1, &zero),
        Err(SignerListError::Weight)
    );
    let mut twice = list.clone();
    twice[2].account = twice[0].account;
    assert_eq!(
        check_signer_list(account, 1, &twice),
        Err(SignerListError::Signer)
    );
    let mut own = list;
    own[0].account = account;
    assert_eq!(
        check_signer_list(account, 1, &own),
        Err(SignerListError::Signer)
    );
    assert_eq!(SignerListError::Quorum.code(), "temBAD_QUORUM");
}

#[test]
fn signer_arrays_are_bounded() {
    let account = AccountId([1; 20]);
    let many: Vec<AccountId> = (0..32_u8).map(|n| AccountId([n + 10; 20])).collect();
    let list = signer_list_set(account, entries(&many));
    let blob = list.blob();
    assert_eq!(SignerListSet::decode(&blob), Ok(list));
    // A 33rd entry, before the array's end (0xF1).
    let mut more = blob[..blob.len() - 1].to_vec();
    let entry_at = more.iter().position(|b| *b == 0xEB).unwrap();
    let entry = more[entry_at..entry_at + 27].to_vec();
    more.extend_from_slice(&entry);
    more.push(0xF1);
    assert_eq!(SignerListSet::decode(&more), Err(DecodeError::Value));
}

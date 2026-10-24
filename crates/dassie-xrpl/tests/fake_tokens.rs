//! Tokens in the fake ledger (spec/areas/xrpl.md#the-fake-ledger): trust
//! lines and their reserve, token Payments through the issuer with the
//! ledger's failures, and an issuer that mints, freezes, sets a transfer
//! fee and claws back. Everything is driven as the node drives it, with
//! transactions signed by the node's codec and answers read by its API
//! parser.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_xrpl::api::{self, TxRecord, account_flags, line_flags};
use dassie_xrpl::codec::{
    Amount, Payment, Signing, TF_CLEAR_FREEZE, TF_SET_DEEP_FREEZE, TF_SET_FREEZE, TF_SET_NO_RIPPLE,
    TokenAmount, TokenValue, Transact, TrustSet,
};
use dassie_xrpl::fake::{
    DSUSD_CURRENCY, DSUSD_ISSUER, ExactSum, FEE_BASE, FakeLedger, RESERVE_BASE, RESERVE_INC, Token,
};
use dassie_xrpl::{AccountId, Wallet, hex_upper};
use proptest::prelude::*;
use serde_json::{Value, json};

/// The connection every test reads the ledger over.
const CONNECTION: u64 = 1;

fn value(text: &str) -> TokenValue {
    TokenValue::parse(text).unwrap()
}

fn dsusd(text: &str) -> TokenAmount {
    Token::DSUSD.amount(value(text))
}

fn wallet(seed: u8) -> Wallet {
    Wallet::from_seed([seed; 16])
}

fn request(ledger: &mut FakeLedger, request: &Value) -> Result<Value, api::ApiError> {
    match api::parse(&ledger.request(CONNECTION, &serde_json::to_vec(request).unwrap())) {
        api::Message::Response { outcome, .. } => outcome,
        other => panic!("not a response: {other:?}"),
    }
}

fn sequence(ledger: &mut FakeLedger, account: AccountId) -> u32 {
    let result = request(
        ledger,
        &json!({ "id": 1, "command": "account_info", "account": account.to_address() }),
    )
    .unwrap();
    api::account_info_result(&result).unwrap().sequence
}

/// Submits a signed blob; what `submit` says at once.
fn submit(ledger: &mut FakeLedger, blob: &[u8]) -> String {
    let result = request(
        ledger,
        &json!({ "id": 2, "command": "submit", "tx_blob": hex_upper(blob) }),
    )
    .unwrap();
    api::submit_result(&result).unwrap()
}

/// The validated transaction `hash`.
fn tx(ledger: &mut FakeLedger, hash: &[u8; 32]) -> TxRecord {
    let result = request(
        ledger,
        &json!({ "id": 3, "command": "tx", "transaction": hex_upper(hash) }),
    )
    .unwrap();
    api::tx_result(&result).unwrap()
}

/// `account`'s transactions, oldest first.
fn history(ledger: &mut FakeLedger, account: AccountId) -> Vec<TxRecord> {
    let result = request(
        ledger,
        &json!({ "id": 4, "command": "account_tx", "account": account.to_address() }),
    )
    .unwrap();
    api::account_tx_result(&result).unwrap().transactions
}

/// `account`'s last transaction.
fn last(ledger: &mut FakeLedger, account: AccountId) -> TxRecord {
    history(ledger, account).pop().unwrap()
}

/// Closes a ledger, and checks what never changes.
fn close(ledger: &mut FakeLedger) -> Vec<TxRecord> {
    let out = ledger.close();
    assert_eq!(ledger.total(), dassie_xrpl::MAX_DROPS);
    assert!(ledger.tokens_conserved());
    out.into_iter()
        .filter_map(|(_, bytes)| match api::parse(&bytes) {
            api::Message::Transaction(record) => Some(*record),
            _ => None,
        })
        .collect()
}

/// A TrustSet from `wallet` to `token`'s issuer, signed and submitted;
/// what `submit` said, and its hash.
fn trust(
    ledger: &mut FakeLedger,
    wallet: &Wallet,
    limit: TokenAmount,
    flags: u32,
) -> (String, [u8; 32]) {
    let sequence = sequence(ledger, wallet.account());
    let signed = TrustSet {
        account: wallet.account(),
        limit,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(wallet);
    (submit(ledger, &signed.blob), signed.hash)
}

/// A Payment from `wallet`, signed and submitted.
fn pay(ledger: &mut FakeLedger, wallet: &Wallet, to: AccountId, amount: Amount) -> [u8; 32] {
    let sequence = sequence(ledger, wallet.account());
    let signed = Payment {
        account: wallet.account(),
        destination: to,
        destination_tag: None,
        amount,
        send_max: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(wallet);
    assert_eq!(submit(ledger, &signed.blob), "tesSUCCESS");
    signed.hash
}

/// The result of a validated Payment from `wallet`.
fn paid(ledger: &mut FakeLedger, wallet: &Wallet, to: AccountId, amount: &str) -> TxRecord {
    let hash = pay(ledger, wallet, to, Amount::Token(dsusd(amount)));
    close(ledger);
    tx(ledger, &hash)
}

/// Accounts funded with XRP, each with a line to DSUSD's issuer of the
/// largest limit, with NoRipple, as the scheme makes it.
fn holders(ledger: &mut FakeLedger, seeds: &[u8]) -> Vec<Wallet> {
    let wallets: Vec<Wallet> = seeds.iter().map(|seed| wallet(*seed)).collect();
    for wallet in &wallets {
        ledger.deposit(wallet.account(), 100 * RESERVE_BASE);
    }
    close(ledger);
    for wallet in &wallets {
        let limit = Token::DSUSD.amount(TokenValue::MAX);
        assert_eq!(
            trust(ledger, wallet, limit, TF_SET_NO_RIPPLE).0,
            "tesSUCCESS"
        );
    }
    close(ledger);
    wallets
}

fn balance(ledger: &FakeLedger, holder: &Wallet) -> Option<TokenValue> {
    ledger.token_balance(holder.account(), Token::DSUSD)
}

/// Mints `amount` of DSUSD to `to` and closes.
fn mint(ledger: &mut FakeLedger, to: AccountId, amount: &str) -> TxRecord {
    ledger.issue(to, dsusd(amount)).unwrap();
    close(ledger);
    last(ledger, DSUSD_ISSUER)
}

// spec/areas/xrpl.md#tokens-in-the-fake-ledger: the testnet's fake issues
// DSUSD at its testnet address, mainnet's RLUSD at its mainnet address, and
// neither the other's; test RLUSD, whose scheme is disabled, is on neither.
#[test]
fn each_network_issues_its_dollar_at_its_address() {
    use dassie_xrpl::fake::{RLUSD_CURRENCY, RLUSD_ISSUER};

    assert_eq!(
        DSUSD_ISSUER.to_address(),
        "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M"
    );
    assert_eq!(
        DSUSD_CURRENCY.to_string(),
        "4453555344000000000000000000000000000000"
    );
    assert_eq!(
        RLUSD_ISSUER.to_address(),
        "rMxCKbEDwqr76QuheSUMdEGf4B9xJ8m5De"
    );
    assert_eq!(
        RLUSD_CURRENCY.to_string(),
        "524C555344000000000000000000000000000000"
    );
    let info = |ledger: &mut FakeLedger, issuer: AccountId| {
        request(
            ledger,
            &json!({ "id": 1, "command": "account_info", "account": issuer.to_address() }),
        )
    };
    let test_rlusd = dassie_xrpl::rlusd::TESTNET_ISSUER;
    for (mut ledger, issuer, others) in [
        (FakeLedger::new(), DSUSD_ISSUER, [RLUSD_ISSUER, test_rlusd]),
        (
            FakeLedger::mainnet(),
            RLUSD_ISSUER,
            [DSUSD_ISSUER, test_rlusd],
        ),
    ] {
        let result = info(&mut ledger, issuer).unwrap();
        let settings = api::account_settings_result(&result).unwrap();
        assert_eq!(
            settings.flags,
            account_flags::DEFAULT_RIPPLE | account_flags::ALLOW_TRUST_LINE_CLAWBACK
        );
        assert!(!settings.global_freeze() && !settings.charges_transfer_fee());
        for other in others {
            assert!(info(&mut ledger, other).is_err(), "{other:?}");
        }
        assert_eq!(ledger.total(), dassie_xrpl::MAX_DROPS);
    }
}

#[test]
fn a_line_is_the_holders_and_holds_its_reserve() {
    let mut ledger = FakeLedger::new();
    let holder = wallet(1);
    ledger.deposit(holder.account(), 100 * RESERVE_BASE);
    close(&mut ledger);
    let limit = Token::DSUSD.amount(TokenValue::MAX);
    let (code, hash) = trust(&mut ledger, &holder, limit, TF_SET_NO_RIPPLE);
    assert_eq!(code, "tesSUCCESS");
    let streamed = close(&mut ledger);
    assert_eq!(ledger.owner_count(holder.account()), Some(1));
    assert_eq!(ledger.owner_count(DSUSD_ISSUER), Some(0));
    // The metadata says the line was made, and the holder's side holds
    // the reserve and disabled rippling; the issuer's ripples by default.
    let record = tx(&mut ledger, &hash);
    assert!(record.succeeded());
    let change = record
        .line(holder.account(), DSUSD_ISSUER, DSUSD_CURRENCY)
        .unwrap();
    assert!(change.before().is_none());
    let view = change.after().unwrap().view(holder.account()).unwrap();
    assert_eq!(view.limit, TokenValue::MAX);
    assert!(view.ours.reserve && view.ours.no_ripple);
    assert!(!view.theirs.reserve && !view.theirs.no_ripple);
    assert_eq!(
        record.change_of(holder.account()).unwrap().owner_count,
        Some(1)
    );
    // `ledger_entry` reads the same line, from either side.
    let entry = request(
        &mut ledger,
        &serde_json::from_slice(&api::ripple_state(
            5,
            holder.account(),
            DSUSD_ISSUER,
            DSUSD_CURRENCY,
        ))
        .unwrap(),
    )
    .unwrap();
    let entry = api::ripple_state_result(&entry).unwrap();
    assert_eq!(
        Some(entry.line),
        ledger.line(DSUSD_ISSUER, holder.account(), DSUSD_CURRENCY)
    );
    assert_eq!(
        (entry.ledger_index, entry.validated),
        (ledger.ledger(), true)
    );
    // The stream that follows the holder carried nothing: no one listens.
    assert!(streamed.is_empty());
    let other = request(
        &mut ledger,
        &serde_json::from_slice(&api::ripple_state(
            6,
            wallet(2).account(),
            DSUSD_ISSUER,
            DSUSD_CURRENCY,
        ))
        .unwrap(),
    );
    assert_eq!(other.unwrap_err().error, "entryNotFound");
}

#[test]
fn the_first_two_lines_need_no_reserve_and_the_third_does() {
    let mut ledger = FakeLedger::new();
    let issuers = [AccountId([0x11; 20]), AccountId([0x22; 20])];
    for issuer in issuers {
        ledger.add_issuer(issuer);
    }
    let holder = wallet(1);
    // The base reserve and the fees: no owner reserve to spare.
    ledger.deposit(holder.account(), RESERVE_BASE + 3 * FEE_BASE);
    close(&mut ledger);
    let token = |issuer| Token {
        issuer,
        currency: DSUSD_CURRENCY,
    };
    for issuer in [issuers[0], issuers[1]] {
        let limit = token(issuer).amount(value("100"));
        assert_eq!(trust(&mut ledger, &holder, limit, 0).0, "tesSUCCESS");
        close(&mut ledger);
        assert!(last(&mut ledger, holder.account()).succeeded());
    }
    let (_, hash) = trust(&mut ledger, &holder, dsusd("100"), 0);
    close(&mut ledger);
    let record = tx(&mut ledger, &hash);
    assert_eq!(record.result, "tecNO_LINE_INSUF_RESERVE");
    assert!(record.lines.is_empty());
    assert_eq!(ledger.owner_count(holder.account()), Some(2));
    assert_eq!(ledger.balance(holder.account()), Some(RESERVE_BASE));
    // With the reserve of a third object, it goes.
    ledger.deposit(holder.account(), 3 * RESERVE_INC);
    close(&mut ledger);
    assert_eq!(trust(&mut ledger, &holder, dsusd("100"), 0).0, "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(ledger.owner_count(holder.account()), Some(3));
}

#[test]
fn trust_sets_are_refused_as_the_ledger_refuses_them() {
    let mut ledger = FakeLedger::new();
    let holder = wallet(1);
    ledger.deposit(holder.account(), 100 * RESERVE_BASE);
    close(&mut ledger);
    // To oneself, or with flags that contradict, nothing is queued.
    let own = TokenAmount {
        issuer: holder.account(),
        ..dsusd("1")
    };
    assert_eq!(trust(&mut ledger, &holder, own, 0).0, "temDST_IS_SRC");
    assert_eq!(
        trust(
            &mut ledger,
            &holder,
            dsusd("1"),
            TF_SET_FREEZE | TF_CLEAR_FREEZE
        )
        .0,
        "temINVALID_FLAG"
    );
    assert_eq!(ledger.queued(), 0);
    // A line to no one, and a line in its default state, are not made.
    let nobody = TokenAmount {
        issuer: AccountId([0x33; 20]),
        ..dsusd("1")
    };
    let (_, to_nobody) = trust(&mut ledger, &holder, nobody, 0);
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &to_nobody).result, "tecNO_DST");
    let (_, redundant) = trust(&mut ledger, &holder, dsusd("0"), TF_SET_NO_RIPPLE);
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &redundant).result, "tecNO_LINE_REDUNDANT");
    // A deep freeze only on a frozen line, and no freeze lifted under one.
    let (_, deep) = trust(&mut ledger, &holder, dsusd("5"), TF_SET_DEEP_FREEZE);
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &deep).result, "tecNO_PERMISSION");
    let both = TF_SET_FREEZE | TF_SET_DEEP_FREEZE;
    assert_eq!(
        trust(&mut ledger, &holder, dsusd("5"), both).0,
        "tesSUCCESS"
    );
    close(&mut ledger);
    let (_, lift) = trust(&mut ledger, &holder, dsusd("5"), TF_CLEAR_FREEZE);
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &lift).result, "tecNO_PERMISSION");
    // Each cost its fee, and only the one line was made.
    assert_eq!(ledger.owner_count(holder.account()), Some(1));
}

#[test]
fn a_line_back_in_its_default_state_is_deleted() {
    let mut ledger = FakeLedger::new();
    let holder = wallet(1);
    ledger.deposit(holder.account(), 100 * RESERVE_BASE);
    close(&mut ledger);
    assert_eq!(trust(&mut ledger, &holder, dsusd("10"), 0).0, "tesSUCCESS");
    close(&mut ledger);
    mint(&mut ledger, holder.account(), "4");
    // A limit of zero while it holds keeps the line, and its reserve.
    assert_eq!(
        trust(&mut ledger, &holder, dsusd("0"), TF_SET_NO_RIPPLE).0,
        "tesSUCCESS"
    );
    close(&mut ledger);
    assert_eq!(balance(&ledger, &holder), Some(value("4")));
    assert_eq!(ledger.owner_count(holder.account()), Some(1));
    // Paying it all back leaves it in its default state: it goes.
    let record = paid(&mut ledger, &holder, DSUSD_ISSUER, "4");
    assert!(record.succeeded());
    let change = record
        .line(holder.account(), DSUSD_ISSUER, DSUSD_CURRENCY)
        .unwrap();
    assert!(change.after().is_none());
    assert_eq!(
        change.balance_of(holder.account()),
        Some((value("4"), TokenValue::ZERO))
    );
    assert_eq!(balance(&ledger, &holder), None);
    assert_eq!(ledger.owner_count(holder.account()), Some(0));
}

#[test]
fn tokens_ripple_through_the_issuer_and_both_streams_carry_them() {
    let mut ledger = FakeLedger::new();
    let wallets = holders(&mut ledger, &[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    let subscribe = json!({
        "id": 9, "command": "subscribe",
        "accounts": [a.account().to_address(), b.account().to_address()],
    });
    request(&mut ledger, &subscribe).unwrap();
    // Minting is the issuer paying a holder.
    let minted = mint(&mut ledger, a.account(), "100.5");
    assert!(minted.succeeded());
    assert_eq!(minted.delivered_token, Some(dsusd("100.5")));
    assert_eq!(balance(&ledger, a), Some(value("100.5")));
    // A pays B: through the issuer, both lines change.
    let hash = pay(&mut ledger, a, b.account(), Amount::Token(dsusd("40.25")));
    let streamed = close(&mut ledger);
    assert_eq!(streamed.len(), 1, "one message for the one connection");
    let record = tx(&mut ledger, &hash);
    assert_eq!(streamed[0], record);
    assert!(record.succeeded());
    assert_eq!(record.delivered_token, Some(dsusd("40.25")));
    assert_eq!(record.delivered, None);
    let line_of = |wallet: &Wallet| {
        record
            .line(wallet.account(), DSUSD_ISSUER, DSUSD_CURRENCY)
            .unwrap()
            .balance_of(wallet.account())
            .unwrap()
    };
    assert_eq!(line_of(a), (value("100.5"), value("60.25")));
    assert_eq!(line_of(b), (TokenValue::ZERO, value("40.25")));
    // Each side's history has it, and the issuer's.
    for account in [a.account(), b.account(), DSUSD_ISSUER] {
        assert_eq!(last(&mut ledger, account).hash, record.hash, "{account}");
    }
    // Only its fee left in XRP.
    assert_eq!(
        record.change_of(a.account()).map(|c| c.before - c.after),
        Some(FEE_BASE)
    );
}

#[test]
fn a_payment_that_cannot_deliver_it_all_fails_and_costs_its_fee() {
    let mut ledger = FakeLedger::new();
    let wallets = holders(&mut ledger, &[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    let stranger = wallet(3);
    ledger.deposit(stranger.account(), 100 * RESERVE_BASE);
    close(&mut ledger);
    // Nothing held: dry. Less than asked: partial.
    assert_eq!(paid(&mut ledger, a, b.account(), "1").result, "tecPATH_DRY");
    mint(&mut ledger, a.account(), "5");
    let before = ledger.balance(a.account()).unwrap();
    let short = paid(&mut ledger, a, b.account(), "10");
    assert_eq!(short.result, "tecPATH_PARTIAL");
    assert!(short.lines.is_empty() && short.delivered_token.is_none());
    assert_eq!(ledger.balance(a.account()), Some(before - FEE_BASE));
    assert_eq!(balance(&ledger, a), Some(value("5")));
    // No line to the receiver: dry. No receiver at all: no destination.
    assert_eq!(
        paid(&mut ledger, a, stranger.account(), "1").result,
        "tecPATH_DRY"
    );
    let nobody = wallet(4).account();
    assert_eq!(paid(&mut ledger, a, nobody, "1").result, "tecNO_DST");
    // Never partial, never to oneself, never zero: refused at once.
    let mut partial = Payment {
        account: a.account(),
        destination: b.account(),
        destination_tag: None,
        amount: Amount::Token(dsusd("1")),
        send_max: None,
        fee: FEE_BASE,
        sequence: sequence(&mut ledger, a.account()),
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(a.public_key()),
    }
    .sign(a)
    .blob;
    partial[4..8].copy_from_slice(&dassie_xrpl::codec::TF_PARTIAL_PAYMENT.to_be_bytes());
    assert_eq!(submit(&mut ledger, &partial), "temMALFORMED");
    assert_eq!(ledger.queued(), 0);
}

#[test]
fn a_limit_stops_what_would_go_above_it() {
    let mut ledger = FakeLedger::new();
    let holder = wallet(1);
    ledger.deposit(holder.account(), 100 * RESERVE_BASE);
    close(&mut ledger);
    assert_eq!(
        trust(&mut ledger, &holder, dsusd("10"), TF_SET_NO_RIPPLE).0,
        "tesSUCCESS"
    );
    close(&mut ledger);
    assert_eq!(
        mint(&mut ledger, holder.account(), "15").result,
        "tecPATH_PARTIAL"
    );
    assert!(mint(&mut ledger, holder.account(), "10").succeeded());
    assert_eq!(
        mint(&mut ledger, holder.account(), "1").result,
        "tecPATH_DRY"
    );
    assert_eq!(balance(&ledger, &holder), Some(value("10")));
    // So does a holder's limit for what another holder pays it.
    let payer = &holders(&mut ledger, &[2])[0];
    mint(&mut ledger, payer.account(), "3");
    assert_eq!(
        paid(&mut ledger, payer, holder.account(), "1").result,
        "tecPATH_DRY"
    );
}

#[test]
fn a_frozen_line_pays_only_the_issuer_and_still_receives() {
    let mut ledger = FakeLedger::new();
    let wallets = holders(&mut ledger, &[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    mint(&mut ledger, a.account(), "10");
    mint(&mut ledger, b.account(), "10");
    request(
        &mut ledger,
        &json!({ "id": 9, "command": "subscribe", "accounts": [a.account().to_address()] }),
    )
    .unwrap();
    ledger.freeze(a.account(), Token::DSUSD, true).unwrap();
    // The holder's stream carries the issuer's freeze of its line.
    let streamed = close(&mut ledger);
    assert_eq!(streamed.len(), 1);
    let view = streamed[0]
        .line(a.account(), DSUSD_ISSUER, DSUSD_CURRENCY)
        .unwrap()
        .after()
        .unwrap()
        .view(a.account())
        .unwrap();
    assert!(view.theirs.freeze && !view.theirs.deep_freeze && view.theirs.reserve);
    assert_eq!(paid(&mut ledger, a, b.account(), "1").result, "tecPATH_DRY");
    assert!(paid(&mut ledger, b, a.account(), "1").succeeded());
    assert!(paid(&mut ledger, a, DSUSD_ISSUER, "1").succeeded());
    assert!(mint(&mut ledger, a.account(), "1").succeeded());
    assert_eq!(balance(&ledger, a), Some(value("11")));
    // Lifted, it pays anyone again.
    ledger.freeze(a.account(), Token::DSUSD, false).unwrap();
    close(&mut ledger);
    assert!(paid(&mut ledger, a, b.account(), "1").succeeded());
    let line = ledger
        .line(a.account(), DSUSD_ISSUER, DSUSD_CURRENCY)
        .unwrap();
    assert!(!line.view(a.account()).unwrap().theirs.reserve);
}

#[test]
fn a_deep_frozen_line_neither_pays_nor_receives_but_with_the_issuer() {
    let mut ledger = FakeLedger::new();
    let wallets = holders(&mut ledger, &[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    mint(&mut ledger, a.account(), "10");
    mint(&mut ledger, b.account(), "10");
    ledger.deep_freeze(a.account(), Token::DSUSD, true).unwrap();
    close(&mut ledger);
    let view = ledger
        .line(a.account(), DSUSD_ISSUER, DSUSD_CURRENCY)
        .unwrap()
        .view(a.account())
        .unwrap();
    assert!(view.theirs.freeze && view.theirs.deep_freeze);
    assert_eq!(paid(&mut ledger, b, a.account(), "1").result, "tecPATH_DRY");
    assert_eq!(paid(&mut ledger, a, b.account(), "1").result, "tecPATH_DRY");
    assert!(mint(&mut ledger, a.account(), "1").succeeded());
    assert!(paid(&mut ledger, a, DSUSD_ISSUER, "2").succeeded());
    assert_eq!(balance(&ledger, a), Some(value("9")));
    // The deep freeze lifted, the freeze stays: it receives, and pays
    // only the issuer.
    ledger
        .deep_freeze(a.account(), Token::DSUSD, false)
        .unwrap();
    close(&mut ledger);
    assert!(paid(&mut ledger, b, a.account(), "1").succeeded());
    assert_eq!(paid(&mut ledger, a, b.account(), "1").result, "tecPATH_DRY");
}

#[test]
fn a_global_freeze_stops_every_holder_but_toward_the_issuer() {
    let mut ledger = FakeLedger::new();
    let wallets = holders(&mut ledger, &[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    mint(&mut ledger, a.account(), "10");
    request(
        &mut ledger,
        &json!({ "id": 9, "command": "subscribe", "accounts": [a.account().to_address()] }),
    )
    .unwrap();
    ledger.global_freeze(DSUSD_ISSUER, true).unwrap();
    // Only the issuer's account changes: the holder's stream says nothing.
    assert!(close(&mut ledger).is_empty());
    let set = last(&mut ledger, DSUSD_ISSUER);
    assert!(set.succeeded() && set.lines.is_empty());
    let info = request(
        &mut ledger,
        &json!({ "id": 1, "command": "account_info", "account": DSUSD_ISSUER.to_address() }),
    )
    .unwrap();
    assert!(api::account_settings_result(&info).unwrap().global_freeze());
    assert_eq!(paid(&mut ledger, a, b.account(), "1").result, "tecPATH_DRY");
    assert!(paid(&mut ledger, a, DSUSD_ISSUER, "1").succeeded());
    assert!(mint(&mut ledger, b.account(), "1").succeeded());
    ledger.global_freeze(DSUSD_ISSUER, false).unwrap();
    close(&mut ledger);
    assert!(paid(&mut ledger, a, b.account(), "1").succeeded());
}

#[test]
fn a_transfer_fee_fails_payments_between_holders() {
    let mut ledger = FakeLedger::new();
    let wallets = holders(&mut ledger, &[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    mint(&mut ledger, a.account(), "10");
    assert_eq!(
        ledger.set_transfer_rate(DSUSD_ISSUER, 999_999_999),
        Err("temBAD_TRANSFER_RATE")
    );
    ledger
        .set_transfer_rate(DSUSD_ISSUER, 1_002_000_000)
        .unwrap();
    close(&mut ledger);
    let info = request(
        &mut ledger,
        &json!({ "id": 1, "command": "account_info", "account": DSUSD_ISSUER.to_address() }),
    )
    .unwrap();
    let settings = api::account_settings_result(&info).unwrap();
    assert_eq!(settings.transfer_rate, 1_002_000_000);
    assert!(settings.charges_transfer_fee());
    // Without `SendMax` the fee is not covered; the issuer charges itself
    // nothing.
    assert_eq!(
        paid(&mut ledger, a, b.account(), "1").result,
        "tecPATH_PARTIAL"
    );
    assert!(paid(&mut ledger, a, DSUSD_ISSUER, "1").succeeded());
    assert!(mint(&mut ledger, b.account(), "1").succeeded());
    ledger.set_transfer_rate(DSUSD_ISSUER, 0).unwrap();
    close(&mut ledger);
    assert!(paid(&mut ledger, a, b.account(), "1").succeeded());
}

#[test]
fn a_clawback_lowers_the_line_without_a_payment() {
    let mut ledger = FakeLedger::new();
    let holder = &holders(&mut ledger, &[1])[0];
    mint(&mut ledger, holder.account(), "10");
    request(
        &mut ledger,
        &json!({ "id": 9, "command": "subscribe", "accounts": [holder.account().to_address()] }),
    )
    .unwrap();
    ledger.claw_back(holder.account(), dsusd("2.5")).unwrap();
    let streamed = close(&mut ledger);
    assert_eq!(streamed.len(), 1);
    let record = &streamed[0];
    assert!(record.succeeded() && !record.is_payment);
    assert_eq!(record.account, DSUSD_ISSUER);
    assert_eq!(record.delivered_token, None);
    assert_eq!(
        record
            .line(holder.account(), DSUSD_ISSUER, DSUSD_CURRENCY)
            .unwrap()
            .balance_of(holder.account()),
        Some((value("10"), value("7.5")))
    );
    // More than it holds takes what it holds; from nothing, nothing.
    ledger.claw_back(holder.account(), dsusd("100")).unwrap();
    close(&mut ledger);
    assert_eq!(balance(&ledger, holder), Some(TokenValue::ZERO));
    ledger.claw_back(holder.account(), dsusd("1")).unwrap();
    close(&mut ledger);
    assert_eq!(last(&mut ledger, DSUSD_ISSUER).result, "tecNO_LINE");
    assert_eq!(
        ledger.claw_back(holder.account(), dsusd("0")),
        Err("temBAD_AMOUNT")
    );
    assert_eq!(
        ledger.claw_back(DSUSD_ISSUER, dsusd("1")),
        Err("temBAD_AMOUNT")
    );
}

#[test]
fn only_the_fakes_issuers_act_without_a_signature() {
    let mut ledger = FakeLedger::new();
    let other = Token {
        issuer: wallet(9).account(),
        currency: DSUSD_CURRENCY,
    };
    let holder = wallet(1).account();
    assert_eq!(
        ledger.issue(holder, other.amount(value("1"))),
        Err("terNO_ACCOUNT")
    );
    assert_eq!(ledger.freeze(holder, other, true), Err("terNO_ACCOUNT"));
    assert_eq!(ledger.issue(holder, dsusd("0")), Err("temBAD_AMOUNT"));
    assert_eq!(ledger.issue(DSUSD_ISSUER, dsusd("1")), Err("temREDUNDANT"));
    assert_eq!(
        ledger.freeze(DSUSD_ISSUER, Token::DSUSD, true),
        Err("temDST_IS_SRC")
    );
    assert_eq!(ledger.queued(), 0);
}

#[test]
fn a_line_reads_the_same_whichever_side_is_low() {
    let mut ledger = FakeLedger::new();
    // An issuer below every holder, and one above every holder.
    let (low_issuer, high_issuer) = (AccountId([0x01; 20]), AccountId([0xFE; 20]));
    ledger.add_issuer(low_issuer);
    ledger.add_issuer(high_issuer);
    let token = Token {
        issuer: low_issuer,
        currency: DSUSD_CURRENCY,
    };
    let high = Token {
        issuer: high_issuer,
        ..token
    };
    let holder = wallet(1);
    assert!(low_issuer < holder.account() && holder.account() < high_issuer);
    ledger.deposit(holder.account(), 100 * RESERVE_BASE);
    close(&mut ledger);
    for issuer in [low_issuer, high_issuer] {
        let limit = TokenAmount {
            issuer,
            ..token.amount(value("50"))
        };
        assert_eq!(
            trust(&mut ledger, &holder, limit, TF_SET_NO_RIPPLE).0,
            "tesSUCCESS"
        );
        close(&mut ledger);
    }
    ledger
        .issue(holder.account(), token.amount(value("7")))
        .unwrap();
    ledger
        .issue(holder.account(), high.amount(value("7")))
        .unwrap();
    close(&mut ledger);
    for issuer in [low_issuer, high_issuer] {
        let line = ledger
            .line(holder.account(), issuer, DSUSD_CURRENCY)
            .unwrap();
        let low_holds = line.low == holder.account();
        assert_eq!(
            line.balance,
            if low_holds { value("7") } else { value("-7") }
        );
        let view = line.view(holder.account()).unwrap();
        assert_eq!(view.balance, value("7"));
        assert_eq!(view.limit, value("50"));
        assert!(view.ours.reserve && view.ours.no_ripple);
        let flags = if low_holds {
            line_flags::LOW_RESERVE | line_flags::LOW_NO_RIPPLE
        } else {
            line_flags::HIGH_RESERVE | line_flags::HIGH_NO_RIPPLE
        };
        assert_eq!(line.flags, flags);
        let record = last(&mut ledger, issuer);
        let change = record
            .line(holder.account(), issuer, DSUSD_CURRENCY)
            .unwrap();
        assert_eq!(
            change.balance_of(holder.account()),
            Some((TokenValue::ZERO, value("7")))
        );
    }
}

#[test]
fn balances_the_ledger_could_not_hold_exactly_are_refused() {
    let mut ledger = FakeLedger::new();
    let holder = &holders(&mut ledger, &[1])[0];
    mint(&mut ledger, holder.account(), "1000000000000000");
    assert_eq!(
        mint(&mut ledger, holder.account(), "0.1").result,
        "tecPRECISION_LOSS"
    );
    assert!(mint(&mut ledger, holder.account(), "1").succeeded());
    assert_eq!(balance(&ledger, holder), Some(value("1000000000000001")));
}

#[test]
fn a_reset_takes_every_line_and_keeps_the_issuer() {
    let mut ledger = FakeLedger::new();
    let holder = &holders(&mut ledger, &[1])[0];
    mint(&mut ledger, holder.account(), "10");
    ledger.freeze(holder.account(), Token::DSUSD, true).unwrap();
    close(&mut ledger);
    assert_eq!(ledger.owner_count(DSUSD_ISSUER), Some(1));
    ledger.reset();
    assert!(ledger.tokens_conserved());
    assert_eq!(ledger.balance(holder.account()), None);
    assert_eq!(balance(&ledger, holder), None);
    assert_eq!(ledger.owner_count(DSUSD_ISSUER), Some(0));
    assert_eq!(
        ledger.account_flags(DSUSD_ISSUER),
        Some(account_flags::DEFAULT_RIPPLE | account_flags::ALLOW_TRUST_LINE_CLAWBACK)
    );
    // The line is made again once the account is.
    let wallets = holders(&mut ledger, &[1]);
    assert!(mint(&mut ledger, wallets[0].account(), "1").succeeded());
}

#[test]
fn exact_sums_say_their_sign() {
    let sum = |terms: &[&str]| {
        let mut sum = ExactSum::default();
        for term in terms {
            sum.add(value(term));
        }
        sum.signum()
    };
    use core::cmp::Ordering::{Equal, Greater, Less};
    assert_eq!(sum(&[]), Equal);
    assert_eq!(sum(&["1e80", "-1e80"]), Equal);
    assert_eq!(sum(&["9999999999999999e80", "1e-81"]), Greater);
    assert_eq!(sum(&["-9999999999999999e80", "1e-81"]), Less);
    assert_eq!(sum(&["1e-81", "-1e-80"]), Less);
    assert_eq!(sum(&["0.1", "0.2", "-0.3"]), Equal);
    assert_eq!(sum(&["1e10", "-1e10", "1e-10"]), Greater);
    assert_eq!(sum(&["-1", "0.9999999999999999"]), Less);
}

/// What a holder or the issuer does, in a property test.
#[derive(Clone, Debug)]
enum Op {
    Mint(usize, u64),
    Pay(usize, usize, u64),
    Redeem(usize, u64),
    Freeze(usize, bool),
    DeepFreeze(usize, bool),
    GlobalFreeze(bool),
    TransferFee(bool),
    ClawBack(usize, u64),
    Limit(usize, u64),
}

/// Millionths of DSUSD: what the scheme's ledger counts in.
fn micros(units: u64) -> TokenValue {
    TokenValue::from_units(units, 6).unwrap()
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    let holder = 0..3_usize;
    let units = 1..2_000_000_u64;
    let op = prop_oneof![
        (holder.clone(), units.clone()).prop_map(|(h, u)| Op::Mint(h, u)),
        (holder.clone(), holder.clone(), units.clone()).prop_map(|(a, b, u)| Op::Pay(a, b, u)),
        (holder.clone(), units.clone()).prop_map(|(h, u)| Op::Redeem(h, u)),
        (holder.clone(), any::<bool>()).prop_map(|(h, on)| Op::Freeze(h, on)),
        (holder.clone(), any::<bool>()).prop_map(|(h, on)| Op::DeepFreeze(h, on)),
        any::<bool>().prop_map(Op::GlobalFreeze),
        any::<bool>().prop_map(Op::TransferFee),
        (holder.clone(), units).prop_map(|(h, u)| Op::ClawBack(h, u)),
        (holder, 0..3_000_000_u64).prop_map(|(h, u)| Op::Limit(h, u)),
    ];
    prop::collection::vec(op, 1..40)
}

/// Applies `op`; a payment to oneself is skipped, as the ledger refuses
/// it before it applies.
fn run(ledger: &mut FakeLedger, wallets: &[Wallet], op: &Op) {
    let token = |units| Token::DSUSD.amount(micros(units));
    match *op {
        Op::Mint(h, u) => ledger.issue(wallets[h].account(), token(u)).unwrap(),
        Op::Pay(a, b, u) if a != b => {
            pay(
                ledger,
                &wallets[a],
                wallets[b].account(),
                Amount::Token(token(u)),
            );
        }
        Op::Pay(..) => {}
        Op::Redeem(h, u) => {
            pay(ledger, &wallets[h], DSUSD_ISSUER, Amount::Token(token(u)));
        }
        Op::Freeze(h, on) => ledger
            .freeze(wallets[h].account(), Token::DSUSD, on)
            .unwrap(),
        Op::DeepFreeze(h, on) => {
            ledger
                .deep_freeze(wallets[h].account(), Token::DSUSD, on)
                .unwrap();
        }
        Op::GlobalFreeze(on) => ledger.global_freeze(DSUSD_ISSUER, on).unwrap(),
        Op::TransferFee(on) => {
            let rate = if on { 1_002_000_000 } else { 0 };
            ledger.set_transfer_rate(DSUSD_ISSUER, rate).unwrap();
        }
        Op::ClawBack(h, u) => ledger.claw_back(wallets[h].account(), token(u)).unwrap(),
        Op::Limit(h, u) => {
            trust(ledger, &wallets[h], token(u), TF_SET_NO_RIPPLE);
        }
    }
}

proptest! {
    /// Whatever holders and the issuer do, no XRP and no token is made or
    /// lost (checked at every close), a transaction that fails changes no
    /// line, no holder holds less than nothing, and each owns the reserve
    /// of the line it keeps.
    #[test]
    fn tokens_are_conserved_and_failures_change_nothing(ops in ops()) {
        let mut ledger = FakeLedger::new();
        let wallets = holders(&mut ledger, &[1, 2, 3]);
        let mut accounts: Vec<String> = wallets.iter().map(|w| w.account().to_address()).collect();
        accounts.push(DSUSD_ISSUER.to_address());
        let subscribe = json!({ "id": 9, "command": "subscribe", "accounts": accounts });
        request(&mut ledger, &subscribe).unwrap();
        for op in &ops {
            run(&mut ledger, &wallets, op);
            for record in close(&mut ledger) {
                prop_assert!(record.succeeded() || record.lines.is_empty(), "{op:?}: {record:?}");
            }
            for wallet in &wallets {
                let line = ledger.line(wallet.account(), DSUSD_ISSUER, DSUSD_CURRENCY);
                let view = line.and_then(|line| line.view(wallet.account()));
                let owned = u32::from(view.is_some_and(|view| view.ours.reserve));
                prop_assert_eq!(ledger.owner_count(wallet.account()), Some(owned));
                prop_assert!(view.is_none_or(|view| view.balance >= TokenValue::ZERO));
            }
        }
    }
}

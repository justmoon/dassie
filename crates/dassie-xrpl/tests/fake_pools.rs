//! AMM pools in the fake ledger (spec/areas/xrpl.md#pools-in-the-fake-ledger):
//! a pool of XRP and RLUSD that its issuer sets, read with `amm_info`, and
//! Payments with `SendMax` that cross it either way, with the ledger's
//! failures. Everything is driven as a client drives it, with transactions
//! signed by the codec and answers read by the API parser; after every
//! ledger the XRP and the tokens still add up, and no swap lowers a pool's
//! product.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_xrpl::amm::{self, TRADING_FEE_DENOMINATOR};
use dassie_xrpl::amount::Asset;
use dassie_xrpl::api::{self, AmmInfo, TxRecord};
use dassie_xrpl::codec::{
    Amount, Payment, SendMaxTerms, TF_SET_NO_RIPPLE, TokenAmount, TokenValue, TrustSet,
    sign_send_max,
};
use dassie_xrpl::fake::{
    FEE_BASE, FakeLedger, POOL_ACCOUNT_FLAGS, PoolState, RESERVE_BASE, RLUSD_ISSUER, Token,
    pool_account,
};
use dassie_xrpl::{AccountId, DROPS_PER_XRP, MAX_DROPS, Wallet, hex_upper};
use proptest::prelude::*;
use serde_json::{Value, json};

const CONNECTION: u64 = 1;
/// The pool the tests start from: 1000 XRP and 500 RLUSD, at 0.5 %.
const POOL_DROPS: u64 = 1_000 * DROPS_PER_XRP;
const POOL_RLUSD: &str = "500";
const POOL_FEE: u32 = 500;

fn value(text: &str) -> TokenValue {
    TokenValue::parse(text).unwrap()
}

fn rlusd(text: &str) -> TokenAmount {
    Token::RLUSD.amount(value(text))
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

fn submit(ledger: &mut FakeLedger, blob: &[u8]) -> String {
    let result = request(
        ledger,
        &json!({ "id": 2, "command": "submit", "tx_blob": hex_upper(blob) }),
    )
    .unwrap();
    api::submit_result(&result).unwrap()
}

fn tx(ledger: &mut FakeLedger, hash: &[u8; 32]) -> TxRecord {
    let result = request(
        ledger,
        &json!({ "id": 3, "command": "tx", "transaction": hex_upper(hash) }),
    )
    .unwrap();
    api::tx_result(&result).unwrap()
}

/// Closes a ledger, and checks what never changes: all the XRP there is,
/// and every token its issuer's.
fn close(ledger: &mut FakeLedger) -> Vec<(u64, TxRecord)> {
    let out = ledger.close();
    assert_eq!(ledger.total(), MAX_DROPS);
    assert!(ledger.tokens_conserved());
    out.into_iter()
        .filter_map(|(connection, bytes)| match api::parse(&bytes) {
            api::Message::Transaction(record) => Some((connection, *record)),
            _ => None,
        })
        .collect()
}

/// Accounts funded with XRP, each with a line to RLUSD's issuer.
fn holders(ledger: &mut FakeLedger, seeds: &[u8]) -> Vec<Wallet> {
    let wallets: Vec<Wallet> = seeds.iter().map(|seed| wallet(*seed)).collect();
    for wallet in &wallets {
        ledger.deposit(wallet.account(), 100 * RESERVE_BASE);
    }
    close(ledger);
    for wallet in &wallets {
        let sequence = sequence(ledger, wallet.account());
        let signed = TrustSet {
            account: wallet.account(),
            limit: Token::RLUSD.amount(TokenValue::MAX),
            fee: FEE_BASE,
            sequence,
            last_ledger_sequence: ledger.ledger() + 10,
            flags: TF_SET_NO_RIPPLE,
            network_id: None,
            memos: Vec::new(),
            signing_pub_key: *wallet.public_key(),
            signature: None,
        }
        .sign(wallet);
        assert_eq!(submit(ledger, &signed.blob), "tesSUCCESS");
    }
    close(ledger);
    wallets
}

/// A ledger with the tests' pool, and holders of `seeds`.
fn with_pool(seeds: &[u8]) -> (FakeLedger, Vec<Wallet>) {
    let mut ledger = FakeLedger::new();
    let wallets = holders(&mut ledger, seeds);
    ledger
        .set_pool(Token::RLUSD, POOL_DROPS, value(POOL_RLUSD), POOL_FEE)
        .unwrap();
    close(&mut ledger);
    (ledger, wallets)
}

fn pool(ledger: &FakeLedger) -> PoolState {
    ledger.pool(Token::RLUSD).unwrap()
}

/// A Payment from `wallet` of `amount`, spending at most `send_max`,
/// signed and submitted; its hash.
fn swap(
    ledger: &mut FakeLedger,
    wallet: &Wallet,
    to: AccountId,
    amount: Amount,
    send_max: Amount,
) -> [u8; 32] {
    let sequence = sequence(ledger, wallet.account());
    let terms = SendMaxTerms {
        destination: to,
        amount,
        send_max,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: ledger.ledger() + 10,
        network_id: None,
        memos: Vec::new(),
    };
    let signed = sign_send_max(wallet, terms).unwrap();
    assert_eq!(submit(ledger, &signed.blob), "tesSUCCESS");
    signed.hash
}

/// The validated result of [`swap`].
fn swapped(
    ledger: &mut FakeLedger,
    wallet: &Wallet,
    to: AccountId,
    amount: Amount,
    send_max: Amount,
) -> TxRecord {
    let hash = swap(ledger, wallet, to, amount, send_max);
    close(ledger);
    tx(ledger, &hash)
}

/// What the pool's own price for `out` is now, as [`amm::swap_in`] makes
/// it.
fn price(ledger: &FakeLedger, out: Amount) -> Amount {
    let pool = pool(ledger);
    let (xrp, token) = (
        Amount::Xrp(pool.drops),
        Amount::Token(Token::RLUSD.amount(pool.value)),
    );
    let (pool_in, pool_out) = match out {
        Amount::Xrp(_) => (token, xrp),
        Amount::Token(_) => (xrp, token),
    };
    amm::swap_in(pool_in, pool_out, pool.trading_fee, out).unwrap()
}

/// What the fake charges `payer` for `out` now: the pool's price, and for
/// RLUSD rounded to what both lines hold exactly.
fn quote(ledger: &FakeLedger, payer: &Wallet, out: Amount) -> Amount {
    let quoted = ledger.quote(payer.account(), Token::RLUSD, out).unwrap();
    // Never below the pool's price, and the same in XRP.
    match (quoted, price(ledger, out)) {
        (Amount::Xrp(quoted), Amount::Xrp(price)) => assert_eq!(quoted, price),
        (Amount::Token(quoted), Amount::Token(price)) => assert!(quoted.value >= price.value),
        _ => panic!("a price in the other asset"),
    }
    quoted
}

fn drops(amount: Amount) -> u64 {
    amount.drops().unwrap()
}

fn token_value(amount: Amount) -> TokenValue {
    amount.token().unwrap().value
}

fn amm_info(ledger: &mut FakeLedger, asset: Asset, asset2: Asset) -> Result<AmmInfo, String> {
    let bytes = api::amm_info(5, asset, asset2);
    let request_value: Value = serde_json::from_slice(&bytes).unwrap();
    request(ledger, &request_value)
        .map(|result| api::amm_info_result(&result).unwrap())
        .map_err(|error| error.error)
}

fn rlusd_asset() -> Asset {
    Asset::Token {
        currency: Token::RLUSD.currency,
        issuer: RLUSD_ISSUER,
    }
}

#[test]
fn the_issuer_sets_a_pool_that_amm_info_reads() {
    let mut ledger = FakeLedger::new();
    assert_eq!(
        amm_info(&mut ledger, Asset::Xrp, rlusd_asset()),
        Err("actNotFound".into())
    );
    ledger
        .set_pool(Token::RLUSD, POOL_DROPS, value(POOL_RLUSD), POOL_FEE)
        .unwrap();
    close(&mut ledger);
    let state = pool(&ledger);
    assert_eq!(state.account, pool_account(Token::RLUSD));
    assert_eq!(
        (state.drops, state.value, state.trading_fee),
        (POOL_DROPS, value(POOL_RLUSD), POOL_FEE)
    );
    assert_eq!(
        ledger.account_flags(state.account),
        Some(POOL_ACCOUNT_FLAGS)
    );
    let info = amm_info(&mut ledger, Asset::Xrp, rlusd_asset()).unwrap();
    assert_eq!(info.account, state.account);
    assert_eq!(info.amount, Amount::Xrp(POOL_DROPS));
    assert_eq!(info.amount2, Amount::Token(rlusd(POOL_RLUSD)));
    assert_eq!(info.trading_fee, POOL_FEE);
    assert_eq!((info.ledger_index, info.validated), (ledger.ledger(), true));
    assert!(!info.asset_frozen && !info.asset2_frozen);
    assert_eq!(
        info.lp_token.currency,
        amm::lp_token_currency(Asset::Xrp, rlusd_asset())
    );
    assert_eq!(info.lp_token.issuer, state.account);
    // sqrt(10^9 drops × 500).
    assert_eq!(info.lp_token.value, value("707106.7811865475"));
    // Either order, answered in the order asked; and by its account.
    let reversed = amm_info(&mut ledger, rlusd_asset(), Asset::Xrp).unwrap();
    assert_eq!(
        (reversed.amount, reversed.amount2),
        (info.amount2, info.amount)
    );
    let by_account = request(
        &mut ledger,
        &json!({ "id": 6, "command": "amm_info", "amm_account": state.account.to_address() }),
    )
    .unwrap();
    assert_eq!(api::amm_info_result(&by_account), Some(info));
    // Set again: more XRP, less RLUSD, another fee.
    ledger
        .set_pool(Token::RLUSD, 2 * POOL_DROPS, value("250"), 0)
        .unwrap();
    close(&mut ledger);
    let info = amm_info(&mut ledger, Asset::Xrp, rlusd_asset()).unwrap();
    assert_eq!(info.amount, Amount::Xrp(2 * POOL_DROPS));
    assert_eq!(info.amount2, Amount::Token(rlusd("250")));
    assert_eq!(info.trading_fee, 0);
}

#[test]
fn only_the_fakes_issuers_set_pools_the_ledger_could_hold() {
    let mut ledger = FakeLedger::new();
    let stranger = Token {
        issuer: AccountId([9; 20]),
        ..Token::RLUSD
    };
    assert_eq!(
        ledger.set_pool(stranger, 1, value("1"), 0),
        Err("terNO_ACCOUNT")
    );
    assert_eq!(
        ledger.set_pool(Token::RLUSD, 0, value("1"), 0),
        Err("temBAD_AMOUNT")
    );
    assert_eq!(
        ledger.set_pool(Token::RLUSD, 1, value("-1"), 0),
        Err("temBAD_AMOUNT")
    );
    assert_eq!(
        ledger.set_pool(Token::RLUSD, 1, value("1"), 1_001),
        Err("temBAD_FEE")
    );
    assert_eq!(ledger.queued(), 0);
}

#[test]
fn xrp_buys_the_token_across_the_pool() {
    let (mut ledger, wallets) = with_pool(&[1, 2]);
    let (buyer, seller) = (&wallets[0], &wallets[1]);
    ledger.request(
        CONNECTION,
        &serde_json::to_vec(&json!({
            "id": 7, "command": "subscribe", "accounts": [seller.account().to_address()],
        }))
        .unwrap(),
    );
    let out = Amount::Token(rlusd("10"));
    let cost = drops(quote(&ledger, buyer, out));
    // 10^9 × 10 / (490 × 0.995) drops, rounded up.
    assert_eq!(cost, 20_510_717);
    let before = ledger.balance(buyer.account()).unwrap();
    let hash = swap(&mut ledger, buyer, seller.account(), out, Amount::Xrp(cost));
    let streamed = close(&mut ledger);
    let record = tx(&mut ledger, &hash);
    assert!(record.succeeded(), "{}", record.result);
    assert_eq!(record.delivered_token, Some(rlusd("10")));
    assert_eq!(record.delivered, None);
    assert_eq!(
        ledger.balance(buyer.account()),
        Some(before - FEE_BASE - cost)
    );
    assert_eq!(
        ledger.token_balance(seller.account(), Token::RLUSD),
        Some(value("10"))
    );
    let state = pool(&ledger);
    assert_eq!(
        (state.drops, state.value),
        (POOL_DROPS + cost, value("490"))
    );
    // What it did, as the metadata says: the buyer's XRP and the pool's,
    // the pool's line and the seller's.
    let change = record.change_of(buyer.account()).unwrap();
    assert_eq!(change.before - change.after, FEE_BASE + cost);
    let pool_change = record.change_of(state.account).unwrap();
    assert_eq!(pool_change.after - pool_change.before, cost);
    let line = record
        .line(seller.account(), RLUSD_ISSUER, Token::RLUSD.currency)
        .unwrap();
    assert_eq!(
        line.balance_of(seller.account()),
        Some((TokenValue::ZERO, value("10")))
    );
    let pool_line = record
        .line(state.account, RLUSD_ISSUER, Token::RLUSD.currency)
        .unwrap();
    assert_eq!(
        pool_line.balance_of(state.account),
        Some((value("500"), value("490")))
    );
    // The seller's stream carried it, and its history holds it.
    assert!(
        streamed
            .iter()
            .any(|(connection, r)| *connection == CONNECTION && r.hash == hash)
    );
    let history = request(
        &mut ledger,
        &json!({ "id": 8, "command": "account_tx", "account": seller.account().to_address() }),
    )
    .unwrap();
    let history = api::account_tx_result(&history).unwrap();
    assert_eq!(history.transactions.last().unwrap().hash, hash);
    // The fee stays in the pool: its product grew beyond what no fee
    // would leave.
    let fee_free = amm::swap_in(
        Amount::Xrp(POOL_DROPS),
        Amount::Token(rlusd(POOL_RLUSD)),
        0,
        out,
    )
    .unwrap();
    assert!(drops(fee_free) < cost);
}

#[test]
fn the_token_buys_xrp_across_the_pool() {
    let (mut ledger, wallets) = with_pool(&[1]);
    let holder = &wallets[0];
    ledger.issue(holder.account(), rlusd("100")).unwrap();
    close(&mut ledger);
    // To an account that does not exist yet: the XRP makes it.
    let nobody = wallet(9).account();
    let out = Amount::Xrp(5 * DROPS_PER_XRP);
    // 500 × 5 / (995 × 0.995) RLUSD, up to sixteen digits; and up to
    // what the holder's 100 less it and the pool's 500 more it hold
    // exactly, which the ledger would round instead.
    assert_eq!(token_value(price(&ledger, out)), value("2.525188757859651"));
    let cost = quote(&ledger, holder, out);
    assert_eq!(token_value(cost), value("2.5251887578597"));
    let record = swapped(&mut ledger, holder, nobody, out, cost);
    assert!(record.succeeded(), "{}", record.result);
    assert_eq!(record.delivered, Some(5 * DROPS_PER_XRP));
    assert_eq!(ledger.balance(nobody), Some(5 * DROPS_PER_XRP));
    let held = value("100").checked_sub(token_value(cost)).unwrap();
    assert_eq!(
        ledger.token_balance(holder.account(), Token::RLUSD),
        Some(held)
    );
    let state = pool(&ledger);
    assert_eq!(state.drops, POOL_DROPS - 5 * DROPS_PER_XRP);
    assert_eq!(
        Some(state.value),
        value(POOL_RLUSD).checked_add(token_value(cost))
    );
}

#[test]
fn a_send_max_that_cannot_cover_the_amount_fails_partial() {
    let (mut ledger, wallets) = with_pool(&[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    ledger.issue(a.account(), rlusd("100")).unwrap();
    close(&mut ledger);
    // XRP for RLUSD: a drop short of the price.
    let out = Amount::Token(rlusd("10"));
    let cost = drops(quote(&ledger, a, out));
    let before = (ledger.balance(a.account()).unwrap(), pool(&ledger));
    let short = swapped(&mut ledger, a, b.account(), out, Amount::Xrp(cost - 1));
    assert_eq!(short.result, "tecPATH_PARTIAL");
    assert!(short.lines.is_empty() && short.delivered_token.is_none());
    // It costs its fee, and moves nothing else.
    assert_eq!(ledger.balance(a.account()), Some(before.0 - FEE_BASE));
    assert_eq!(pool(&ledger), before.1);
    assert_eq!(
        ledger.token_balance(b.account(), Token::RLUSD),
        Some(TokenValue::ZERO)
    );
    // RLUSD for XRP: the least value short of the price.
    let out = Amount::Xrp(DROPS_PER_XRP);
    let cost = token_value(quote(&ledger, a, out));
    let less = TokenValue::new(false, cost.mantissa() - 1, cost.exponent()).unwrap();
    let short = swapped(
        &mut ledger,
        a,
        b.account(),
        out,
        Amount::Token(Token::RLUSD.amount(less)),
    );
    assert_eq!(short.result, "tecPATH_PARTIAL");
    assert_eq!(pool(&ledger), before.1);
    // At the price, both go through.
    let exact = swapped(
        &mut ledger,
        a,
        b.account(),
        out,
        Amount::Token(Token::RLUSD.amount(cost)),
    );
    assert!(exact.succeeded(), "{}", exact.result);
}

#[test]
fn swaps_fail_as_the_ledger_fails_them() {
    let (mut ledger, wallets) = with_pool(&[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    let stranger = wallet(3);
    ledger.deposit(stranger.account(), 100 * RESERVE_BASE);
    ledger.issue(a.account(), rlusd("100")).unwrap();
    close(&mut ledger);
    let plenty = Amount::Xrp(50 * RESERVE_BASE);
    // All the pool holds, or more: it cannot deliver it.
    let all = Amount::Token(rlusd(POOL_RLUSD));
    let result = |record: TxRecord| record.result;
    assert_eq!(
        result(swapped(
            &mut ledger,
            a,
            b.account(),
            all,
            Amount::Xrp(MAX_DROPS)
        )),
        "tecPATH_PARTIAL"
    );
    // No line to the receiver.
    let one = Amount::Token(rlusd("1"));
    assert_eq!(
        result(swapped(&mut ledger, a, stranger.account(), one, plenty)),
        "tecPATH_DRY"
    );
    // More XRP than the payer has above its reserve.
    assert_eq!(
        result(swapped(
            &mut ledger,
            &stranger,
            a.account(),
            Amount::Token(rlusd("200")),
            Amount::Xrp(MAX_DROPS)
        )),
        "tecPATH_PARTIAL"
    );
    // No RLUSD to pay with, or a payer the issuer froze.
    let xrp_out = Amount::Xrp(DROPS_PER_XRP);
    let max = Amount::Token(rlusd("10"));
    assert_eq!(
        result(swapped(&mut ledger, b, a.account(), xrp_out, max)),
        "tecPATH_DRY"
    );
    ledger.freeze(a.account(), Token::RLUSD, true).unwrap();
    close(&mut ledger);
    assert_eq!(
        result(swapped(&mut ledger, a, b.account(), xrp_out, max)),
        "tecPATH_DRY"
    );
}

#[test]
fn swaps_go_only_through_a_pool_and_never_into_it() {
    let (mut ledger, wallets) = with_pool(&[1, 2]);
    let (a, b) = (&wallets[0], &wallets[1]);
    ledger.issue(a.account(), rlusd("100")).unwrap();
    close(&mut ledger);
    let result = |record: TxRecord| record.result;
    let plenty = Amount::Xrp(50 * RESERVE_BASE);
    let one = Amount::Token(rlusd("1"));
    let xrp_out = Amount::Xrp(DROPS_PER_XRP);
    let max = Amount::Token(rlusd("10"));
    // Another token than the pool's: no path.
    let other = Token {
        issuer: AccountId([0x42; 20]),
        ..Token::RLUSD
    };
    ledger.add_issuer(other.issuer);
    let other_max = Amount::Token(other.amount(value("10")));
    assert_eq!(
        result(swapped(&mut ledger, a, b.account(), xrp_out, other_max)),
        "tecPATH_DRY"
    );
    assert_eq!(
        result(swapped(&mut ledger, a, b.account(), one, other_max)),
        "tecPATH_DRY"
    );
    // A token for itself, within its SendMax, is a token Payment.
    let same = swapped(&mut ledger, a, b.account(), one, max);
    assert!(same.succeeded(), "{}", same.result);
    // The pool's account takes no Payment.
    let pool_account = pool(&ledger).account;
    assert_eq!(
        result(swapped(&mut ledger, a, pool_account, one, plenty)),
        "tecNO_PERMISSION"
    );
    // To oneself across the pool is a swap; XRP for XRP is no Payment the
    // codec reads.
    let own = swapped(&mut ledger, a, a.account(), one, plenty);
    assert!(own.succeeded(), "{}", own.result);
    let sequence = sequence(&mut ledger, a.account());
    let xrp_for_xrp = Payment {
        account: a.account(),
        destination: b.account(),
        amount: Amount::Xrp(1),
        send_max: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: ledger.ledger() + 10,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing_pub_key: *a.public_key(),
        signature: None,
    }
    .sign(a)
    .blob;
    // The Amount's header (0x61) and its eight bytes, then SendMax (0x69)
    // of XRP after the Fee (0x68 and eight bytes).
    let fee_at = xrp_for_xrp.iter().position(|b| *b == 0x68).unwrap() + 9;
    let mut blob = xrp_for_xrp[..fee_at].to_vec();
    blob.push(0x69);
    blob.extend_from_slice(&(0x4000_0000_0000_0000_u64 | 5).to_be_bytes());
    blob.extend_from_slice(&xrp_for_xrp[fee_at..]);
    assert_eq!(submit(&mut ledger, &blob), "temMALFORMED");
    assert_eq!(ledger.queued(), 0);
}

#[test]
fn a_reset_takes_the_pools() {
    let (mut ledger, _) = with_pool(&[1]);
    let account = pool(&ledger).account;
    ledger.reset();
    assert_eq!(ledger.pool(Token::RLUSD), None);
    assert_eq!(ledger.balance(account), None);
    assert_eq!(ledger.total(), MAX_DROPS);
    assert!(ledger.tokens_conserved());
    assert_eq!(
        amm_info(&mut ledger, Asset::Xrp, rlusd_asset()),
        Err("actNotFound".into())
    );
    // Made again, it has the same account.
    ledger
        .set_pool(Token::RLUSD, POOL_DROPS, value(POOL_RLUSD), POOL_FEE)
        .unwrap();
    close(&mut ledger);
    assert_eq!(pool(&ledger).account, account);
}

/// Whether a pool that held `drops` and `before`, paid `paid_drops` for
/// `before − after` of its token, kept its product with its fee: `(drops +
/// paid × (1 − fee)) × after ≥ drops × before`, both sides counted in
/// [`TRADING_FEE_DENOMINATOR`]ths of a drop, exactly.
fn kept_with_fee(
    drops: u64,
    paid_drops: u64,
    fee: u32,
    before: TokenValue,
    after: TokenValue,
) -> bool {
    let denominator = u64::from(TRADING_FEE_DENOMINATOR);
    let effective = drops * denominator + paid_drops * (denominator - u64::from(fee));
    let token = |value| Amount::Token(Token::RLUSD.amount(value));
    amm::keeps_product(
        [Amount::Xrp(drops * denominator), token(before)],
        [Amount::Xrp(effective), token(after)],
    ) == Some(true)
}

#[derive(Clone, Debug)]
enum Op {
    /// Buys this many millionths of RLUSD with XRP.
    Buy(u64),
    /// Buys this many drops with RLUSD.
    Sell(u64),
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![
            (1..=50_000_000_u64).prop_map(Op::Buy),
            (1..=50_000_000_u64).prop_map(Op::Sell),
        ],
        1..12,
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    // Every swap costs what `amm::swap_in` says, moves exactly that, and
    // leaves the pool a product no lower, with its fee kept in it; the XRP
    // and the tokens add up after every ledger.
    #[test]
    fn swaps_keep_the_pool_and_everything_adds_up(ops in ops()) {
        let (mut ledger, wallets) = with_pool(&[1, 2]);
        let (a, b) = (&wallets[0], &wallets[1]);
        ledger.issue(a.account(), rlusd("1000")).unwrap();
        ledger.deposit(a.account(), 10_000 * DROPS_PER_XRP);
        close(&mut ledger);
        for op in ops {
            let before = pool(&ledger);
            let out = match op {
                Op::Buy(micros) => Amount::Token(Token::RLUSD.amount(TokenValue::from_units(micros, 6).unwrap())),
                Op::Sell(drops) => Amount::Xrp(drops),
            };
            let cost = quote(&ledger, a, out);
            let record = swapped(&mut ledger, a, b.account(), out, cost);
            let after = pool(&ledger);
            if !record.succeeded() {
                // Only a balance the ledger would round, and nothing moved.
                prop_assert_eq!(record.result.as_str(), "tecPRECISION_LOSS");
                prop_assert_eq!(after, before);
                continue;
            }
            prop_assert_eq!(
                amm::keeps_product(
                    [Amount::Xrp(before.drops), Amount::Token(Token::RLUSD.amount(before.value))],
                    [Amount::Xrp(after.drops), Amount::Token(Token::RLUSD.amount(after.value))],
                ),
                Some(true)
            );
            match (out, cost) {
                (Amount::Token(out), Amount::Xrp(paid)) => {
                    prop_assert_eq!(after.drops, before.drops + paid);
                    prop_assert_eq!(Some(after.value), before.value.checked_sub(out.value));
                    prop_assert!(kept_with_fee(before.drops, paid, before.trading_fee, before.value, after.value));
                    // A drop less would not have bought it.
                    let less = amm::swap_out(
                        Amount::Xrp(before.drops),
                        Amount::Token(Token::RLUSD.amount(before.value)),
                        before.trading_fee,
                        Amount::Xrp(paid - 1),
                    ).unwrap();
                    prop_assert!(token_value(less) < out.value);
                }
                (Amount::Xrp(out), Amount::Token(paid)) => {
                    prop_assert_eq!(after.drops, before.drops - out);
                    prop_assert_eq!(Some(after.value), before.value.checked_add(paid.value));
                }
                _ => prop_assert!(false, "a swap pays in the other asset"),
            }
        }
    }
}

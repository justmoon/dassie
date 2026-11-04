//! The order book in the fake ledger
//! (spec/areas/xrpl.md#the-order-book-in-the-fake-ledger): offers of XRP
//! and DSUSD placed, replaced, cancelled and expired, crossed fully and in
//! part at the resting offers' prices, best first and then the earlier,
//! capped by what their owners hold, and counted toward their reserves;
//! read with `book_offers` and `account_offers`. Everything is driven as a
//! client drives it, with transactions signed by the codec and answers read
//! by the API parser; after every ledger the XRP and the tokens still add
//! up.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use dassie_xrpl::amount::{Asset, Currency, IssuedAsset};
use dassie_xrpl::api::{self, AccountOffers, BookOffers, TxRecord};
use dassie_xrpl::codec::{
    Amount, OfferCancelTerms, OfferCreate, OfferTerms, Signing, TF_SET_NO_RIPPLE, TokenAmount,
    TokenValue, Transact, TrustSet, sign_offer, sign_offer_cancel,
};
use dassie_xrpl::fake::{DSUSD_ISSUER, FEE_BASE, FakeLedger, RESERVE_BASE, RESERVE_INC};
use dassie_xrpl::{AccountId, DROPS_PER_XRP, MAX_DROPS, Wallet, hex_upper};
use serde_json::{Value, json};

const CONNECTION: u64 = 1;

fn value(text: &str) -> TokenValue {
    TokenValue::parse(text).unwrap()
}

fn dsusd(text: &str) -> Amount {
    Amount::Token(IssuedAsset::DSUSD.amount(value(text)))
}

const fn xrp(whole: u64) -> Amount {
    Amount::Xrp(whole * DROPS_PER_XRP)
}

fn dsusd_asset() -> Asset {
    IssuedAsset::DSUSD.asset()
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

/// The sequence `account` uses next, with what waits for the next close.
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

/// The validated transaction `hash`: its record, and its metadata as
/// JSON.
fn tx(ledger: &mut FakeLedger, hash: &[u8; 32]) -> (TxRecord, Value) {
    let result = request(
        ledger,
        &json!({ "id": 3, "command": "tx", "transaction": hex_upper(hash) }),
    )
    .unwrap();
    (api::tx_result(&result).unwrap(), result["meta"].clone())
}

/// What a transaction's metadata did to offers: each node's kind, and the
/// offer's owner and sequence.
fn offer_nodes(meta: &Value) -> Vec<(String, String, u64)> {
    let nodes = meta["AffectedNodes"].as_array().unwrap();
    nodes
        .iter()
        .filter_map(|node| {
            let (kind, inner) = node.as_object()?.iter().next()?;
            if inner["LedgerEntryType"] != "Offer" {
                return None;
            }
            let fields = inner.get("NewFields").unwrap_or(&inner["FinalFields"]);
            Some((
                kind.clone(),
                fields["Account"].as_str()?.to_string(),
                fields["Sequence"].as_u64()?,
            ))
        })
        .collect()
}

/// Closes a ledger, and checks what never changes: all the XRP there is,
/// and every token its issuer's.
fn close(ledger: &mut FakeLedger) {
    ledger.close();
    assert_eq!(ledger.total(), MAX_DROPS);
    assert!(ledger.tokens_conserved());
}

/// A line to `issuer`'s `currency` for `wallet`, at the largest limit.
fn trust(ledger: &mut FakeLedger, wallet: &Wallet, issuer: AccountId, currency: Currency) {
    let sequence = sequence(ledger, wallet.account());
    let signed = TrustSet {
        account: wallet.account(),
        limit: TokenAmount {
            value: TokenValue::MAX,
            currency,
            issuer,
        },
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(ledger.ledger() + 10),
        flags: TF_SET_NO_RIPPLE,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(wallet);
    assert_eq!(submit(ledger, &signed.blob), "tesSUCCESS");
}

/// Accounts of `seeds`, each with 100 XRP and a line to DSUSD's issuer,
/// and `minted` DSUSD on it.
fn traders(ledger: &mut FakeLedger, seeds: &[u8], minted: &str) -> Vec<Wallet> {
    let wallets: Vec<Wallet> = seeds.iter().map(|seed| wallet(*seed)).collect();
    for wallet in &wallets {
        ledger.deposit(wallet.account(), 100 * DROPS_PER_XRP);
    }
    close(ledger);
    for wallet in &wallets {
        trust(ledger, wallet, DSUSD_ISSUER, IssuedAsset::DSUSD.currency);
    }
    close(ledger);
    if value(minted) > TokenValue::ZERO {
        for wallet in &wallets {
            ledger
                .issue(wallet.account(), IssuedAsset::DSUSD.amount(value(minted)))
                .unwrap();
        }
        close(ledger);
    }
    wallets
}

/// An offer asking for `pays` and giving `gets`, of the transaction of
/// `sequence`; [`place`] sets its last ledger.
fn terms(sequence: u32, pays: Amount, gets: Amount) -> OfferTerms {
    OfferTerms {
        taker_pays: pays,
        taker_gets: gets,
        offer_sequence: None,
        expiration: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: 0,
        network_id: None,
        memos: Vec::new(),
    }
}

/// Signs and submits `terms` from `wallet`, valid for ten ledgers: what
/// `submit` said, and the hash.
fn place(ledger: &mut FakeLedger, wallet: &Wallet, terms: OfferTerms) -> (String, [u8; 32]) {
    let terms = OfferTerms {
        last_ledger_sequence: ledger.ledger() + 10,
        ..terms
    };
    let signed = sign_offer(wallet, terms).unwrap();
    (submit(ledger, &signed.blob), signed.hash)
}

/// `wallet` offers `gets` for `pays` in the next ledger, which closes:
/// the transaction's record and metadata, and the offer's sequence.
fn offer(
    ledger: &mut FakeLedger,
    wallet: &Wallet,
    pays: Amount,
    gets: Amount,
) -> (TxRecord, Value, u32) {
    let sequence = sequence(ledger, wallet.account());
    let (code, hash) = place(ledger, wallet, terms(sequence, pays, gets));
    assert_eq!(code, "tesSUCCESS");
    close(ledger);
    let (record, meta) = tx(ledger, &hash);
    (record, meta, sequence)
}

fn book(ledger: &mut FakeLedger, gets: Asset, pays: Asset) -> BookOffers {
    let bytes = api::book_offers(4, gets, pays, api::BOOK_PAGE);
    let result = request(ledger, &serde_json::from_slice(&bytes).unwrap()).unwrap();
    api::book_offers_result(&result).unwrap()
}

fn offers_of(ledger: &mut FakeLedger, account: AccountId) -> AccountOffers {
    let bytes = api::account_offers(5, account, None);
    let result = request(ledger, &serde_json::from_slice(&bytes).unwrap()).unwrap();
    api::account_offers_result(&result).unwrap()
}

fn owner_count(ledger: &FakeLedger, account: AccountId) -> u32 {
    ledger.owner_count(account).unwrap()
}

#[test]
fn an_offer_rests_in_its_book_and_counts_toward_the_reserve() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1], "10");
    let maker = &wallets[0];
    let owned = owner_count(&ledger, maker.account());
    // 10 DSUSD for 20 XRP: two million drops a dollar.
    let (record, meta, sequence) = offer(&mut ledger, maker, xrp(20), dsusd("10"));
    assert!(record.succeeded(), "{}", record.result);
    assert_eq!(
        offer_nodes(&meta),
        [(
            "CreatedNode".to_string(),
            maker.account().to_address(),
            u64::from(sequence)
        )]
    );
    assert_eq!(owner_count(&ledger, maker.account()), owned + 1);
    let change = record.change_of(maker.account()).unwrap();
    assert_eq!(change.owner_count, Some(owned + 1));
    let page = offers_of(&mut ledger, maker.account());
    assert!(page.validated);
    let [listed] = page.offers.as_slice() else {
        panic!("one offer");
    };
    assert_eq!(listed.sequence, sequence);
    assert_eq!(
        (listed.taker_pays, listed.taker_gets),
        (xrp(20), dsusd("10"))
    );
    assert_eq!(listed.quality, "2000000");
    // The book of a taker that pays XRP and gets DSUSD.
    let book = book(&mut ledger, dsusd_asset(), Asset::Xrp);
    let [first] = book.offers.as_slice() else {
        panic!("one offer");
    };
    assert_eq!((first.account, first.sequence), (maker.account(), sequence));
    assert_eq!(first.owner_funds, Some(dsusd("10")));
    assert_eq!(first.taker_gets_funded, None);
    // The other way round, there is none.
    assert!(
        self::book(&mut ledger, Asset::Xrp, dsusd_asset())
            .offers
            .is_empty()
    );
}

#[test]
fn an_offer_is_replaced_and_cancelled() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1], "10");
    let maker = &wallets[0];
    let (_, _, first) = offer(&mut ledger, maker, xrp(20), dsusd("10"));
    let owned = owner_count(&ledger, maker.account());
    // The replacement takes the first one's place in one transaction.
    let sequence = sequence(&mut ledger, maker.account());
    let (code, hash) = place(
        &mut ledger,
        maker,
        OfferTerms {
            offer_sequence: Some(first),
            ..terms(sequence, xrp(25), dsusd("10"))
        },
    );
    assert_eq!(code, "tesSUCCESS");
    close(&mut ledger);
    let (record, meta) = tx(&mut ledger, &hash);
    assert!(record.succeeded());
    let address = maker.account().to_address();
    let mut nodes = offer_nodes(&meta);
    nodes.sort();
    assert_eq!(
        nodes,
        [
            (
                "CreatedNode".to_string(),
                address.clone(),
                u64::from(sequence)
            ),
            ("DeletedNode".to_string(), address.clone(), u64::from(first)),
        ]
    );
    assert_eq!(ledger.offer(maker.account(), first), None);
    assert_eq!(
        ledger.offer(maker.account(), sequence).unwrap().taker_pays,
        xrp(25)
    );
    assert_eq!(owner_count(&ledger, maker.account()), owned);
    // Cancelled, it is gone, and so is its reserve.
    let cancel = |ledger: &mut FakeLedger, offer_sequence| {
        let next = self::sequence(ledger, maker.account());
        let terms = OfferCancelTerms {
            offer_sequence,
            fee: FEE_BASE,
            sequence: next,
            last_ledger_sequence: ledger.ledger() + 10,
            network_id: None,
        };
        let signed = sign_offer_cancel(maker, terms).unwrap();
        assert_eq!(submit(ledger, &signed.blob), "tesSUCCESS");
        close(ledger);
        tx(ledger, &signed.hash)
    };
    let (record, meta) = cancel(&mut ledger, sequence);
    assert!(record.succeeded());
    assert_eq!(
        offer_nodes(&meta),
        [("DeletedNode".to_string(), address, u64::from(sequence))]
    );
    assert_eq!(owner_count(&ledger, maker.account()), owned - 1);
    assert!(offers_of(&mut ledger, maker.account()).offers.is_empty());
    // An offer no longer there is cancelled all the same, and nothing
    // changes but the fee.
    let before = ledger.balance(maker.account()).unwrap();
    let (record, meta) = cancel(&mut ledger, sequence);
    assert!(record.succeeded());
    assert!(offer_nodes(&meta).is_empty());
    assert_eq!(ledger.balance(maker.account()), Some(before - FEE_BASE));
}

#[test]
fn an_offer_past_its_expiration_is_gone() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1, 2], "10");
    let (maker, taker) = (&wallets[0], &wallets[1]);
    // It expires once two more ledgers have closed.
    let expiration = u32::try_from(ledger.close_time() + 8).unwrap();
    let sequence = sequence(&mut ledger, maker.account());
    let (code, _) = place(
        &mut ledger,
        maker,
        OfferTerms {
            expiration: Some(expiration),
            ..terms(sequence, xrp(20), dsusd("10"))
        },
    );
    assert_eq!(code, "tesSUCCESS");
    close(&mut ledger);
    let listed = offers_of(&mut ledger, maker.account());
    assert_eq!(listed.offers[0].expiration, Some(expiration));
    close(&mut ledger);
    // Not listed, though it is still there until a crossing meets it.
    assert!(offers_of(&mut ledger, maker.account()).offers.is_empty());
    assert!(
        book(&mut ledger, dsusd_asset(), Asset::Xrp)
            .offers
            .is_empty()
    );
    assert!(ledger.offer(maker.account(), sequence).is_some());
    let owned = owner_count(&ledger, maker.account());
    let (record, meta, taken) = offer(&mut ledger, taker, dsusd("5"), xrp(10));
    assert!(record.succeeded());
    let mut nodes = offer_nodes(&meta);
    nodes.sort();
    assert_eq!(
        nodes,
        [
            (
                "CreatedNode".to_string(),
                taker.account().to_address(),
                u64::from(taken)
            ),
            (
                "DeletedNode".to_string(),
                maker.account().to_address(),
                u64::from(sequence)
            ),
        ]
    );
    // It crossed nothing: the maker's DSUSD and XRP are as they were.
    assert_eq!(
        ledger.token_balance(maker.account(), IssuedAsset::DSUSD),
        Some(value("10"))
    );
    assert_eq!(owner_count(&ledger, maker.account()), owned - 1);
    // An offer that would expire before it applies is refused, though the
    // one it replaces is gone.
    let next = self::sequence(&mut ledger, taker.account());
    let now = u32::try_from(ledger.close_time()).unwrap();
    let (code, hash) = place(
        &mut ledger,
        taker,
        OfferTerms {
            offer_sequence: Some(taken),
            expiration: Some(now),
            ..terms(next, dsusd("5"), xrp(10))
        },
    );
    assert_eq!(code, "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &hash).0.result, "tecEXPIRED");
    assert_eq!(ledger.offer(taker.account(), taken), None);
    assert_eq!(ledger.offer(taker.account(), next), None);
}

#[test]
fn a_taker_crosses_in_part_and_then_in_full_at_the_resting_price() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1, 2, 3], "10");
    let (maker, taker, other) = (&wallets[0], &wallets[1], &wallets[2]);
    let (_, _, resting) = offer(&mut ledger, maker, xrp(20), dsusd("10"));
    let maker_xrp = ledger.balance(maker.account()).unwrap();
    let taker_xrp = ledger.balance(taker.account()).unwrap();
    let owned = owner_count(&ledger, maker.account());
    // 4 DSUSD for 8 XRP: the resting price, all of it crossed.
    let (record, meta, _) = offer(&mut ledger, taker, dsusd("4"), xrp(8));
    assert!(record.succeeded(), "{}", record.result);
    assert_eq!(
        offer_nodes(&meta),
        [(
            "ModifiedNode".to_string(),
            maker.account().to_address(),
            u64::from(resting)
        )]
    );
    let left = ledger.offer(maker.account(), resting).unwrap();
    assert_eq!((left.taker_pays, left.taker_gets), (xrp(12), dsusd("6")));
    assert_eq!(
        ledger.balance(maker.account()),
        Some(maker_xrp + 8 * DROPS_PER_XRP)
    );
    assert_eq!(
        ledger.balance(taker.account()),
        Some(taker_xrp - 8 * DROPS_PER_XRP - FEE_BASE)
    );
    assert_eq!(
        ledger.token_balance(taker.account(), IssuedAsset::DSUSD),
        Some(value("14"))
    );
    // The metadata shows the maker its XRP and its line, as a Payment's
    // does, and its history holds the transaction.
    let change = record.change_of(maker.account()).unwrap();
    assert_eq!(change.after - change.before, 8 * DROPS_PER_XRP);
    let line = record
        .line(maker.account(), DSUSD_ISSUER, IssuedAsset::DSUSD.currency)
        .unwrap();
    assert_eq!(
        line.balance_of(maker.account()),
        Some((value("10"), value("6")))
    );
    let history = request(
        &mut ledger,
        &json!({ "id": 8, "command": "account_tx", "account": maker.account().to_address() }),
    )
    .unwrap();
    let history = api::account_tx_result(&history).unwrap();
    assert_eq!(history.transactions.last().unwrap().hash, record.hash);
    // Another asks for 10 at up to 2.5 XRP each: it gets the 6 left at 2,
    // and the rest rests at its own price, 4 DSUSD for 10 XRP.
    let other_xrp = ledger.balance(other.account()).unwrap();
    let (record, meta, rested) = offer(&mut ledger, other, dsusd("10"), xrp(25));
    assert!(record.succeeded());
    let mut nodes = offer_nodes(&meta);
    nodes.sort();
    assert_eq!(
        nodes,
        [
            (
                "CreatedNode".to_string(),
                other.account().to_address(),
                u64::from(rested)
            ),
            (
                "DeletedNode".to_string(),
                maker.account().to_address(),
                u64::from(resting)
            ),
        ]
    );
    assert_eq!(
        ledger.balance(other.account()),
        Some(other_xrp - 12 * DROPS_PER_XRP - FEE_BASE)
    );
    let rest = ledger.offer(other.account(), rested).unwrap();
    assert_eq!((rest.taker_pays, rest.taker_gets), (dsusd("4"), xrp(10)));
    assert_eq!(ledger.offer(maker.account(), resting), None);
    assert_eq!(owner_count(&ledger, maker.account()), owned - 1);
    assert_eq!(
        ledger.token_balance(maker.account(), IssuedAsset::DSUSD),
        Some(value("0"))
    );
    assert_eq!(
        book(&mut ledger, Asset::Xrp, dsusd_asset()).offers[0].account,
        other.account()
    );
}

#[test]
fn the_best_price_crosses_first_and_then_the_earlier() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1, 2, 3, 4], "10");
    let (dear, early, late, taker) = (&wallets[0], &wallets[1], &wallets[2], &wallets[3]);
    let (_, _, dear_offer) = offer(&mut ledger, dear, xrp(30), dsusd("10"));
    let (_, _, early_offer) = offer(&mut ledger, early, xrp(20), dsusd("10"));
    let (_, _, late_offer) = offer(&mut ledger, late, xrp(20), dsusd("10"));
    let listed: Vec<AccountId> = book(&mut ledger, dsusd_asset(), Asset::Xrp)
        .offers
        .iter()
        .map(|offer| offer.account)
        .collect();
    assert_eq!(listed, [early.account(), late.account(), dear.account()]);
    // 15 DSUSD at up to 3 XRP each: the early offer's 10 and 5 of the
    // late one's, both at 2.
    let taker_xrp = ledger.balance(taker.account()).unwrap();
    let (record, _, _) = offer(&mut ledger, taker, dsusd("15"), xrp(45));
    assert!(record.succeeded());
    assert_eq!(ledger.offer(early.account(), early_offer), None);
    let late_left = ledger.offer(late.account(), late_offer).unwrap();
    assert_eq!(late_left.taker_gets, dsusd("5"));
    assert_eq!(
        ledger.offer(dear.account(), dear_offer).unwrap().taker_gets,
        dsusd("10")
    );
    assert_eq!(
        ledger.balance(taker.account()),
        Some(taker_xrp - 30 * DROPS_PER_XRP - FEE_BASE)
    );
    // An offer that asks too much crosses nothing and rests.
    let (record, meta, sequence) = offer(&mut ledger, taker, dsusd("1"), xrp(1));
    assert!(record.succeeded());
    assert_eq!(
        offer_nodes(&meta),
        [(
            "CreatedNode".to_string(),
            taker.account().to_address(),
            u64::from(sequence)
        )]
    );
}

#[test]
fn what_an_offer_gives_is_capped_by_what_its_owner_holds() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1, 2], "4");
    let (maker, taker) = (&wallets[0], &wallets[1]);
    // Two offers of 10 DSUSD, of an owner that holds 4.
    let (_, _, first) = offer(&mut ledger, maker, xrp(20), dsusd("10"));
    let (_, _, second) = offer(&mut ledger, maker, xrp(30), dsusd("10"));
    let book = book(&mut ledger, dsusd_asset(), Asset::Xrp);
    let [best, next] = book.offers.as_slice() else {
        panic!("two offers");
    };
    assert_eq!(best.owner_funds, Some(dsusd("4")));
    assert_eq!(best.taker_gets_funded, Some(dsusd("4")));
    assert_eq!(best.taker_pays_funded, Some(xrp(8)));
    // The owner's funds are given once, and its next offer is funded with
    // what the first leaves.
    assert_eq!(next.owner_funds, None);
    assert_eq!(next.taker_gets_funded, Some(dsusd("0")));
    // A taker gets the 4, at the price.
    let (record, _, rested) = offer(&mut ledger, taker, dsusd("10"), xrp(20));
    assert!(record.succeeded());
    assert_eq!(
        ledger.token_balance(maker.account(), IssuedAsset::DSUSD),
        Some(value("0"))
    );
    assert_eq!(
        ledger.offer(maker.account(), first).unwrap().taker_gets,
        dsusd("6")
    );
    // What it did not get rests: 6 DSUSD for 12 XRP.
    let rest = ledger.offer(taker.account(), rested).unwrap();
    assert_eq!((rest.taker_pays, rest.taker_gets), (dsusd("6"), xrp(12)));
    // An offer of DSUSD by an account that holds none is refused; the
    // maker's that are now unfunded are taken away when a crossing meets
    // them.
    let next = sequence(&mut ledger, maker.account());
    let (code, hash) = place(&mut ledger, maker, terms(next, xrp(20), dsusd("10")));
    assert_eq!(code, "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &hash).0.result, "tecUNFUNDED_OFFER");
    let buyer = &traders(&mut ledger, &[3], "0")[0];
    let (record, meta, _) = offer(&mut ledger, buyer, dsusd("1"), xrp(3));
    assert!(record.succeeded());
    let mut deleted: Vec<u64> = offer_nodes(&meta)
        .into_iter()
        .filter(|(kind, owner, _)| kind == "DeletedNode" && *owner == maker.account().to_address())
        .map(|(_, _, sequence)| sequence)
        .collect();
    deleted.sort_unstable();
    assert_eq!(deleted, [u64::from(first), u64::from(second)]);
    assert_eq!(ledger.offer(maker.account(), first), None);
}

#[test]
fn an_offer_needs_the_reserve_for_one_more_object() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1], "10");
    let maker = &wallets[0];
    // Down to the reserve of what it owns, and a fee: no room for one more.
    let owned = owner_count(&ledger, maker.account());
    let reserve = RESERVE_BASE + RESERVE_INC * u64::from(owned);
    let balance = ledger.balance(maker.account()).unwrap();
    ledger.take_unrecorded(maker.account(), balance - reserve - FEE_BASE);
    let sequence = sequence(&mut ledger, maker.account());
    let (code, hash) = place(&mut ledger, maker, terms(sequence, xrp(20), dsusd("10")));
    assert_eq!(code, "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &hash).0.result, "tecINSUF_RESERVE_OFFER");
    assert_eq!(ledger.offer(maker.account(), sequence), None);
    assert_eq!(owner_count(&ledger, maker.account()), owned);
}

#[test]
fn an_offer_that_meets_its_owner_s_own_takes_it_away() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1], "10");
    let trader = &wallets[0];
    let (_, _, selling) = offer(&mut ledger, trader, xrp(20), dsusd("10"));
    let owned = owner_count(&ledger, trader.account());
    let before = ledger.balance(trader.account()).unwrap();
    // Buying back at that price crosses nothing: the old one goes, and the
    // new one rests.
    let (record, _, buying) = offer(&mut ledger, trader, dsusd("10"), xrp(20));
    assert!(record.succeeded());
    assert_eq!(ledger.offer(trader.account(), selling), None);
    assert!(ledger.offer(trader.account(), buying).is_some());
    assert_eq!(owner_count(&ledger, trader.account()), owned);
    assert_eq!(ledger.balance(trader.account()), Some(before - FEE_BASE));
    assert_eq!(
        ledger.token_balance(trader.account(), IssuedAsset::DSUSD),
        Some(value("10"))
    );
}

#[test]
fn offers_the_ledger_refuses_are_refused() {
    let mut ledger = FakeLedger::new();
    let wallets = traders(&mut ledger, &[1], "10");
    let maker = &wallets[0];
    // The codec writes what the ledger refuses; the fake refuses it as the
    // ledger does.
    let sequence = sequence(&mut ledger, maker.account());
    let last = ledger.ledger() + 10;
    let unsigned = |pays, gets, flags| OfferCreate {
        account: maker.account(),
        taker_pays: pays,
        taker_gets: gets,
        offer_sequence: None,
        expiration: None,
        fee: FEE_BASE,
        sequence,
        last_ledger_sequence: Some(last),
        flags,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(maker.public_key()),
    };
    for (offer, code) in [
        (unsigned(xrp(0), dsusd("1"), 0), "temBAD_OFFER"),
        (unsigned(dsusd("1"), dsusd("2"), 0), "temREDUNDANT"),
        (unsigned(xrp(1), dsusd("1"), 0x0008_0000), "temINVALID_FLAG"),
    ] {
        let signed = offer.sign(maker);
        assert_eq!(submit(&mut ledger, &signed.blob), code);
    }
    // A token with no line to hold it, before anything is crossed.
    let usd = Currency::parse("USD").unwrap();
    let elsewhere = Amount::Token(TokenAmount {
        value: value("1"),
        currency: usd,
        issuer: DSUSD_ISSUER,
    });
    let (code, hash) = place(&mut ledger, maker, terms(sequence, elsewhere, xrp(1)));
    assert_eq!(code, "tesSUCCESS");
    close(&mut ledger);
    assert_eq!(tx(&mut ledger, &hash).0.result, "tecNO_LINE");
}

#[test]
fn an_issuer_offers_its_own_token_without_bound() {
    let mut ledger = FakeLedger::new();
    let issuer = wallet(20);
    ledger.add_issuer(issuer.account());
    let usd = Currency::parse("USD").unwrap();
    let usd_of = |text: &str| {
        Amount::Token(TokenAmount {
            value: value(text),
            currency: usd,
            issuer: issuer.account(),
        })
    };
    let wallets = traders(&mut ledger, &[1], "0");
    let taker = &wallets[0];
    trust(&mut ledger, taker, issuer.account(), usd);
    close(&mut ledger);
    let (record, _, resting) = offer(&mut ledger, &issuer, xrp(10), usd_of("1000000"));
    assert!(record.succeeded(), "{}", record.result);
    let listed = book(
        &mut ledger,
        Asset::Token {
            currency: usd,
            issuer: issuer.account(),
        },
        Asset::Xrp,
    );
    assert_eq!(listed.offers[0].taker_gets_funded, None);
    // It mints what is taken.
    let (record, _, _) = offer(&mut ledger, taker, usd_of("500000"), xrp(5));
    assert!(record.succeeded());
    assert_eq!(
        ledger.token_balance(
            taker.account(),
            IssuedAsset {
                issuer: issuer.account(),
                currency: usd
            }
        ),
        Some(value("500000"))
    );
    assert_eq!(
        ledger.offer(issuer.account(), resting).unwrap().taker_gets,
        usd_of("500000")
    );
}

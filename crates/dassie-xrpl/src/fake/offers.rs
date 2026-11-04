//! The order book in the fake ledger
//! (spec/areas/xrpl.md#the-order-book-in-the-fake-ledger): offers that an
//! `OfferCreate` places, replaces and crosses and an `OfferCancel` takes
//! away, read with `book_offers` and `account_offers`.
//!
//! An offer rests in the book of the two assets it trades, best quality
//! first ([`crate::offer::quality`]) and the earlier first at one quality,
//! and counts toward its owner's reserve. A new offer crosses the opposite
//! book at the resting offers' prices, best first, as far as its own terms,
//! its owner's funds and theirs go; what is left rests, at the new offer's
//! price, if its owner still holds some of what it gives and has the
//! reserve for one more object. A crossing moves XRP between the two
//! accounts' roots and the token over their lines, as a Payment does, so
//! the metadata shows each account what it did.
//!
//! What a crossing meets at its price that is past its expiration, the
//! taker's own, or whose owner holds nothing of what it gives, it takes
//! away. A move the
//! fake cannot make (a freeze, a line's limit, a balance it would have to
//! round) ends the crossing there. The fake takes no offer flags, crosses
//! no AMM pool, and sends no Payment through the book. Its arithmetic is
//! exact ([`crate::offer`]).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use serde_json::{Value, json};

use super::objects::{self, amount_json};
use super::pools;
use super::tokens::TokenMove;
use super::{Account, FakeLedger, Touched};
use crate::amount::Asset;
use crate::api::account_flags;
use crate::codec::{Amount, Currency, OfferCancel, OfferCreate, TokenAmount, TokenValue};
use crate::hex_upper;
use crate::keys::AccountId;
use crate::offer::{self, quality, quality_text};

/// The most offers one book holds: the offers that ask for one asset and
/// give another.
pub const MAX_BOOK_OFFERS: usize = 256;
/// The most offers the fake holds in all its books.
pub const MAX_OFFERS: usize = 4_096;
/// The most resting offers one `OfferCreate` meets: crossed, or taken away
/// as expired, unfunded or its own.
pub const MAX_CROSSED_OFFERS: usize = 64;
/// The most offers a `book_offers` answer lists, and how many it lists
/// unless asked for fewer: rippled's most.
const MAX_BOOK_PAGE: usize = 100;
/// The fewest, the default and the most offers an `account_offers` page
/// lists, as rippled's.
const ACCOUNT_OFFERS_PAGE: (usize, usize, usize) = (10, 200, 400);

/// The universal flag `tfFullyCanonicalSig`, the only flag an offer of the
/// fake's may carry.
const TF_FULLY_CANONICAL_SIG: u32 = 0x8000_0000;

/// An offer by its owner and the sequence of the transaction that placed
/// it.
pub(super) type OfferKey = (AccountId, u32);

/// An offer resting in the fake's book.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Offer {
    /// What it still asks for.
    pub taker_pays: Amount,
    /// What it still gives for it.
    pub taker_gets: Amount,
    /// When it expires, in seconds since the XRP Ledger's epoch, if it
    /// does.
    pub expiration: Option<u32>,
    /// Its quality as placed, which orders its book and names its
    /// directory; crossing it leaves it as it was, as rippled does.
    pub quality: u64,
    /// When it was placed, counted across all books: the earlier is taken
    /// first at one quality.
    pub placed: u64,
}

/// Nothing of `asset`.
const fn zero(asset: Asset) -> Amount {
    match asset {
        Asset::Xrp => Amount::Xrp(0),
        Asset::Token { currency, issuer } => Amount::Token(TokenAmount {
            value: TokenValue::ZERO,
            currency,
            issuer,
        }),
    }
}

/// `a − b`, of one asset, if it is not below zero and the ledger can hold
/// it exactly.
fn sub(a: Amount, b: Amount) -> Option<Amount> {
    match (a, b) {
        (Amount::Xrp(a), Amount::Xrp(b)) => a.checked_sub(b).map(Amount::Xrp),
        (Amount::Token(a), Amount::Token(b)) if same_token(&a, &b) => {
            let value = a.value.checked_sub(b.value)?;
            (!value.is_negative()).then_some(Amount::Token(TokenAmount { value, ..a }))
        }
        _ => None,
    }
}

/// Whether `a` is less than `b`, two amounts of one asset.
fn less(a: Amount, b: Amount) -> bool {
    match (a, b) {
        (Amount::Xrp(a), Amount::Xrp(b)) => a < b,
        (Amount::Token(a), Amount::Token(b)) => a.value < b.value,
        _ => false,
    }
}

/// The lesser of two amounts of one asset.
fn min(a: Amount, b: Amount) -> Amount {
    if less(b, a) { b } else { a }
}

/// Whether two token amounts are of one token.
fn same_token(a: &TokenAmount, b: &TokenAmount) -> bool {
    (a.currency, a.issuer) == (b.currency, b.issuer)
}

/// One side of a crossing, checked and not yet made: XRP between two
/// roots, or a token's move.
enum Leg {
    Xrp {
        from: AccountId,
        to: AccountId,
        drops: u64,
    },
    Token(TokenMove),
}

/// A take from one resting offer: the taker takes `taken` of what it
/// gives and pays `paid` of what it asks, over `legs`; what the offer then
/// still asks for and gives is `left`, none once either is used up.
struct Fill {
    taken: Amount,
    paid: Amount,
    legs: [Leg; 2],
    left: Option<(Amount, Amount)>,
}

/// What taking from one resting offer comes to.
enum Step {
    /// Its owner holds nothing of what it gives: it is taken away.
    Unfunded,
    /// The crossing ends here.
    Stop,
    /// A take, checked and not yet made.
    Fill(Box<Fill>),
}

/// What an `OfferCreate` alone may be refused for: flags the fake does not
/// model, an amount of nothing, one asset on both sides, an expiration of
/// zero, an offer to replace that the account cannot have placed before,
/// or a price beyond what the ledger names a book directory by.
pub(super) fn preflight_offer_create(tx: &OfferCreate) -> Result<(), &'static str> {
    if tx.flags & !TF_FULLY_CANONICAL_SIG != 0 {
        return Err("temINVALID_FLAG");
    }
    if tx.taker_pays.is_zero() || tx.taker_gets.is_zero() {
        return Err("temBAD_OFFER");
    }
    if tx.taker_pays.asset() == tx.taker_gets.asset() {
        return Err("temREDUNDANT");
    }
    if tx.expiration == Some(0) {
        return Err("temBAD_EXPIRATION");
    }
    if tx
        .offer_sequence
        .is_some_and(|offer| offer == 0 || offer >= tx.sequence)
    {
        return Err("temBAD_SEQUENCE");
    }
    if quality(tx.taker_pays, tx.taker_gets).is_none() {
        return Err("temBAD_OFFER");
    }
    Ok(())
}

/// What an `OfferCancel` alone may be refused for: flags, or an offer the
/// account cannot have placed before.
pub(super) fn preflight_offer_cancel(tx: &OfferCancel) -> Result<(), &'static str> {
    if tx.flags & !TF_FULLY_CANONICAL_SIG != 0 {
        return Err("temINVALID_FLAG");
    }
    if tx.offer_sequence == 0 || tx.offer_sequence >= tx.sequence {
        return Err("temBAD_SEQUENCE");
    }
    Ok(())
}

impl FakeLedger {
    /// `owner`'s offer placed by its transaction of `sequence`, if it still
    /// rests in the book, expired or not.
    #[must_use]
    pub fn offer(&self, owner: AccountId, sequence: u32) -> Option<Offer> {
        self.offers.get(&(owner, sequence)).copied()
    }

    /// How many offers rest in all the books, expired ones included.
    #[must_use]
    pub fn offer_count(&self) -> usize {
        self.offers.len()
    }

    /// Whether `offer` is past its expiration once a ledger closed at
    /// `close_time` is the last: no ledger after it holds it.
    fn expired(offer: &Offer, close_time: u64) -> bool {
        offer
            .expiration
            .is_some_and(|expiration| u64::from(expiration) <= close_time)
    }

    /// The offers that ask for `pays` and give `gets`, best first: by
    /// quality, then by when they were placed.
    fn book(&self, pays: Asset, gets: Asset) -> Vec<(OfferKey, Offer)> {
        let mut book: Vec<(OfferKey, Offer)> = (self.offers.iter())
            .filter(|(_, offer)| {
                offer.taker_pays.asset() == pays && offer.taker_gets.asset() == gets
            })
            .map(|(key, offer)| (*key, *offer))
            .collect();
        book.sort_by_key(|(_, offer)| (offer.quality, offer.placed));
        book
    }

    /// What `owner` holds that it can give of `asset`: its XRP above its
    /// reserve, or its balance of a token on its line unless the issuer
    /// froze the line or all its lines. `None` for an issuer's own token,
    /// which it gives without bound.
    fn holdings(&self, owner: AccountId, asset: Asset) -> Option<Amount> {
        let Asset::Token { currency, issuer } = asset else {
            let held = self.accounts.get(&owner).map_or(0, |account| {
                account.balance.saturating_sub(self.reserve_of(account))
            });
            return Some(Amount::Xrp(held));
        };
        if owner == issuer {
            return None;
        }
        let frozen = self.frozen(owner, issuer, currency);
        let held = (self.line(owner, issuer, currency))
            .and_then(|line| line.view(owner))
            .map_or(TokenValue::ZERO, |view| view.balance);
        let value = if frozen || held.is_negative() {
            TokenValue::ZERO
        } else {
            held
        };
        Some(Amount::Token(TokenAmount {
            value,
            currency,
            issuer,
        }))
    }

    /// What `owner` can give of `of`'s asset now, at most `of`.
    fn funds(&self, owner: AccountId, of: Amount) -> Amount {
        self.holdings(owner, of.asset())
            .map_or(of, |held| min(held, of))
    }

    /// Whether the issuer froze `holder`'s line of `currency`, or all its
    /// lines.
    fn frozen(&self, holder: AccountId, issuer: AccountId, currency: Currency) -> bool {
        let global = (self.accounts.get(&issuer))
            .is_some_and(|account| account.flags & account_flags::GLOBAL_FREEZE != 0);
        let line = (self.line(holder, issuer, currency))
            .and_then(|line| line.view(holder))
            .is_some_and(|view| view.theirs.freeze || view.theirs.deep_freeze);
        global || (holder != issuer && line)
    }

    /// Writes `owner`'s offer, adding one to its owner count if it is new.
    fn write_offer(&mut self, touched: &mut Touched, key: OfferKey, offer: Offer) {
        let before = self.offers.insert(key, offer);
        if before.is_none() {
            self.count_owned(touched, key.0, true);
        }
        touched.offer(key, before, offer);
    }

    /// Takes `key`'s offer away, as it ended, `last`, taking one from its
    /// owner count.
    fn delete_offer(&mut self, touched: &mut Touched, key: OfferKey, last: Offer) {
        let Some(before) = self.offers.remove(&key) else {
            return;
        };
        self.count_owned(touched, key.0, false);
        touched.offer(key, Some(before), last);
    }

    /// One more object `owner` owns, or one fewer.
    fn count_owned(&mut self, touched: &mut Touched, owner: AccountId, more: bool) {
        if let Some(mut account) = self.accounts.get(&owner).copied() {
            account.owner_count = if more {
                account.owner_count.saturating_add(1)
            } else {
                account.owner_count.saturating_sub(1)
            };
            self.put_account(touched, owner, account);
        }
    }

    /// Why `owner` may not place an offer that asks for `pays` and gives
    /// `gets`, as the ledger checks before it applies one: an issuer that
    /// is not there, a freeze, a token it has no line to hold, or nothing
    /// of what it gives.
    fn may_offer(&self, owner: AccountId, pays: Amount, gets: Amount) -> Result<(), &'static str> {
        for token in [pays.token(), gets.token()].into_iter().flatten() {
            if !self.accounts.contains_key(&token.issuer) {
                return Err("tecNO_ISSUER");
            }
            if self.frozen(owner, token.issuer, token.currency) {
                return Err("tecFROZEN");
            }
        }
        // The ledger would make the line; the fake asks for one.
        if let Some(token) = pays.token()
            && token.issuer != owner
            && self.line(owner, token.issuer, token.currency).is_none()
        {
            return Err("tecNO_LINE");
        }
        if self.funds(owner, gets).is_zero() {
            return Err("tecUNFUNDED_OFFER");
        }
        Ok(())
    }

    /// An `OfferCreate`, after its fee: the offer it replaces taken away,
    /// then crossed against the opposite book, and what is left placed.
    /// `prior` is the owner's XRP before the fee, which the reserve for the
    /// offer is checked against, as the ledger checks it.
    pub(super) fn offer_create(
        &mut self,
        touched: &mut Touched,
        tx: &OfferCreate,
        prior: u64,
    ) -> &'static str {
        let owner = tx.account;
        if let Err(code) = self.may_offer(owner, tx.taker_pays, tx.taker_gets) {
            return code;
        }
        // The one it replaces goes first, whatever comes of this one.
        if let Some(sequence) = tx.offer_sequence
            && let Some(old) = self.offer(owner, sequence)
        {
            self.delete_offer(touched, (owner, sequence), old);
        }
        if tx
            .expiration
            .is_some_and(|expiration| u64::from(expiration) <= self.parent_close_time)
        {
            return "tecEXPIRED";
        }
        let (pays, gets) = (tx.taker_pays.asset(), tx.taker_gets.asset());
        if self.offers.len() >= MAX_OFFERS || self.book(pays, gets).len() >= MAX_BOOK_OFFERS {
            return "tecDIR_FULL";
        }
        let (crossed, left_pays) = self.cross(touched, owner, tx.taker_pays, tx.taker_gets);
        // What is left rests at the offer's own price, rounded up.
        let rested = offer::cost(left_pays, tx.taker_gets, tx.taker_pays).ok();
        let (Some(left_gets), Some(account)) = (rested, self.accounts.get(&owner).copied()) else {
            return "tesSUCCESS";
        };
        if left_pays.is_zero() || left_gets.is_zero() || self.funds(owner, left_gets).is_zero() {
            return "tesSUCCESS";
        }
        let reserve = self.reserve.0.saturating_add(
            self.reserve
                .1
                .saturating_mul(u64::from(account.owner_count.saturating_add(1))),
        );
        if prior < reserve {
            return if crossed {
                "tesSUCCESS"
            } else {
                "tecINSUF_RESERVE_OFFER"
            };
        }
        let Some(quality) = quality(left_pays, left_gets) else {
            return "tesSUCCESS";
        };
        self.offers_placed = self.offers_placed.saturating_add(1);
        let placed = Offer {
            taker_pays: left_pays,
            taker_gets: left_gets,
            expiration: tx.expiration,
            quality,
            placed: self.offers_placed,
        };
        self.write_offer(touched, (owner, tx.sequence), placed);
        "tesSUCCESS"
    }

    /// An `OfferCancel`, after its fee: the offer taken away, if it is
    /// still there.
    pub(super) fn offer_cancel(&mut self, touched: &mut Touched, tx: &OfferCancel) -> &'static str {
        let key = (tx.account, tx.offer_sequence);
        if let Some(offer) = self.offers.get(&key).copied() {
            self.delete_offer(touched, key, offer);
        }
        "tesSUCCESS"
    }

    /// Crosses `taker`'s offer, which asks for `wants` and gives `gives`,
    /// against the book that gives what it wants for what it gives, best
    /// first, while their prices are at most its own. Returns whether any
    /// crossed, and what it then still asks for.
    fn cross(
        &mut self,
        touched: &mut Touched,
        taker: AccountId,
        wants: Amount,
        gives: Amount,
    ) -> (bool, Amount) {
        let mut left = (wants, gives);
        let mut crossed = false;
        let book = self.book(gives.asset(), wants.asset());
        let parent = self.parent_close_time;
        for (key, resting) in book.into_iter().take(MAX_CROSSED_OFFERS) {
            if left.0.is_zero() || left.1.is_zero() {
                break;
            }
            let price = offer::crosses(resting.taker_pays, resting.taker_gets, wants, gives);
            if price != Some(true) {
                break;
            }
            if Self::expired(&resting, parent) || key.0 == taker {
                self.delete_offer(touched, key, resting);
                continue;
            }
            match self.step(taker, key.0, &resting, left) {
                Step::Unfunded => self.delete_offer(touched, key, resting),
                Step::Stop => break,
                Step::Fill(fill) => {
                    let Fill {
                        taken,
                        paid,
                        legs,
                        left: rest,
                    } = *fill;
                    let (Some(wants_left), Some(gives_left)) =
                        (sub(left.0, taken), sub(left.1, paid))
                    else {
                        break;
                    };
                    for leg in legs {
                        self.make_leg(touched, leg);
                    }
                    crossed = true;
                    left = (wants_left, gives_left);
                    let after = rest.map_or(
                        Offer {
                            taker_pays: zero(resting.taker_pays.asset()),
                            taker_gets: zero(resting.taker_gets.asset()),
                            ..resting
                        },
                        |(taker_pays, taker_gets)| Offer {
                            taker_pays,
                            taker_gets,
                            ..resting
                        },
                    );
                    if rest.is_some() {
                        self.write_offer(touched, key, after);
                    } else {
                        self.delete_offer(touched, key, after);
                    }
                }
            }
        }
        (crossed, left.0)
    }

    /// What `taker`, which still asks for `left.0` and gives `left.1`,
    /// takes from `maker`'s resting offer at its price: as much as the
    /// taker asks for and the maker holds, or what the taker can pay for.
    fn step(
        &self,
        taker: AccountId,
        maker: AccountId,
        resting: &Offer,
        left: (Amount, Amount),
    ) -> Step {
        let (pays, gets) = (resting.taker_pays, resting.taker_gets);
        let maker_has = self.funds(maker, gets);
        if maker_has.is_zero() {
            return Step::Unfunded;
        }
        let budget = self.funds(taker, left.1);
        if budget.is_zero() {
            return Step::Stop;
        }
        let mut taken = min(left.0, maker_has);
        let Ok(mut paid) = offer::cost(taken, pays, gets) else {
            return Step::Stop;
        };
        if less(budget, paid) {
            let Ok(bought) = offer::bought(budget, pays, gets) else {
                return Step::Stop;
            };
            let Ok(cost) = offer::cost(bought, pays, gets) else {
                return Step::Stop;
            };
            (taken, paid) = (bought, cost);
        }
        if taken.is_zero() {
            return Step::Stop;
        }
        let (Some(pays_left), Some(gets_left)) = (sub(pays, paid), sub(gets, taken)) else {
            return Step::Stop;
        };
        let (Some(to_maker), Some(to_taker)) =
            (self.leg(taker, maker, paid), self.leg(maker, taker, taken))
        else {
            return Step::Stop;
        };
        let rest = (!pays_left.is_zero() && !gets_left.is_zero()).then_some((pays_left, gets_left));
        Step::Fill(Box::new(Fill {
            taken,
            paid,
            legs: [to_maker, to_taker],
            left: rest,
        }))
    }

    /// `amount` from `from` to `to`, checked as a Payment's move is: XRP,
    /// which the caller checked `from` holds, or a token over the lines.
    fn leg(&self, from: AccountId, to: AccountId, amount: Amount) -> Option<Leg> {
        match amount {
            Amount::Xrp(drops) => Some(Leg::Xrp { from, to, drops }),
            Amount::Token(token) => self.plan_token(from, to, token).ok().map(Leg::Token),
        }
    }

    /// Makes a leg on the ledger as it now is.
    fn make_leg(&mut self, touched: &mut Touched, leg: Leg) {
        match leg {
            Leg::Token(planned) => self.move_token(touched, planned),
            Leg::Xrp { from, to, drops } => {
                assert_ne!(from, to, "a crossing is between two accounts");
                let Some(payer) = self.accounts.get(&from).copied() else {
                    return;
                };
                assert!(self.accounts.contains_key(&to), "both accounts are there");
                assert!(
                    payer.balance >= drops,
                    "a crossing pays what its payer holds"
                );
                self.put_account(
                    touched,
                    from,
                    Account {
                        balance: payer.balance - drops,
                        ..payer
                    },
                );
                let payee = self.accounts.get(&to).copied().unwrap_or(payer);
                self.put_account(
                    touched,
                    to,
                    Account {
                        balance: payee.balance.saturating_add(drops),
                        ..payee
                    },
                );
            }
        }
    }

    /// `book_offers`: the offers of the book a taker would pay
    /// `taker_pays` into and get `taker_gets` from, best first, as of the
    /// last validated ledger, with what each owner holds of what it gives.
    pub(super) fn book_offers(&self, request: &Value) -> Result<Value, Value> {
        let invalid = || json!({ "error": "invalidParams" });
        let side = |key: &str| request.get(key).and_then(pools::asset).ok_or_else(invalid);
        let (gets, pays) = (side("taker_gets")?, side("taker_pays")?);
        if gets == pays {
            return Err(json!({ "error": "badMarket" }));
        }
        let limit = request
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
            .map_or(MAX_BOOK_PAGE, |limit| limit.clamp(1, MAX_BOOK_PAGE));
        // What each owner has left to give, offer after offer.
        let mut left: BTreeMap<AccountId, Option<Amount>> = BTreeMap::new();
        let mut offers = Vec::new();
        let book = self.book(pays, gets);
        let listed = (book.iter()).filter(|(_, offer)| !Self::expired(offer, self.close_time));
        for ((owner, sequence), offer) in listed.take(limit) {
            let first = !left.contains_key(owner);
            let held = *left
                .entry(*owner)
                .or_insert_with(|| self.holdings(*owner, gets));
            let key = crate::records::offer_key(*owner, *sequence);
            let object = objects::offer(*owner, *sequence, offer, self.thread(&key));
            let mut entry = objects::to_json(&object.to_bytes()).unwrap_or(Value::Null);
            let Some(fields) = entry.as_object_mut() else {
                continue;
            };
            fields.insert("index".into(), hex_upper(&key).into());
            fields.insert("quality".into(), quality_text(offer.quality).into());
            if first {
                let funds = held.unwrap_or(offer.taker_gets);
                let text = match funds {
                    Amount::Xrp(drops) => alloc::string::ToString::to_string(&drops),
                    Amount::Token(token) => alloc::string::ToString::to_string(&token.value),
                };
                fields.insert("owner_funds".into(), text.into());
            }
            let funded = held.map_or(offer.taker_gets, |held| min(held, offer.taker_gets));
            if funded != offer.taker_gets {
                let pays_funded =
                    offer::bought(funded, offer.taker_gets, offer.taker_pays).unwrap_or(zero(pays));
                fields.insert("taker_gets_funded".into(), amount_json(&funded));
                fields.insert("taker_pays_funded".into(), amount_json(&pays_funded));
            }
            if let Some(held) = held {
                left.insert(*owner, Some(sub(held, funded).unwrap_or(zero(gets))));
            }
            offers.push(entry);
        }
        Ok(json!({
            "ledger_index": self.ledger,
            "offers": offers,
            "validated": true,
        }))
    }

    /// `account_offers`: an account's offers as of the last validated
    /// ledger, in the order of their sequences, a page at a time.
    pub(super) fn account_offers(&self, request: &Value) -> Result<Value, Value> {
        let id = Self::account_of(request)?;
        if !self.accounts.contains_key(&id) {
            return Err(
                json!({ "error": "actNotFound", "ledger_index": self.ledger, "validated": true }),
            );
        }
        let (fewest, default, most) = ACCOUNT_OFFERS_PAGE;
        let limit = request
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
            .map_or(default, |limit| limit.clamp(fewest, most));
        let skip = request
            .get("marker")
            .and_then(Value::as_u64)
            .and_then(|marker| usize::try_from(marker).ok())
            .unwrap_or(0);
        let owned: Vec<(u32, &Offer)> = (self.offers.range((id, 0)..=(id, u32::MAX)))
            .filter(|(_, offer)| !Self::expired(offer, self.close_time))
            .map(|((_, sequence), offer)| (*sequence, offer))
            .collect();
        let page: Vec<Value> = (owned.iter().skip(skip).take(limit))
            .map(|(sequence, offer)| {
                let mut entry = json!({
                    "flags": 0,
                    "seq": sequence,
                    "taker_gets": amount_json(&offer.taker_gets),
                    "taker_pays": amount_json(&offer.taker_pays),
                    "quality": quality_text(offer.quality),
                });
                if let (Some(expiration), Some(fields)) = (offer.expiration, entry.as_object_mut())
                {
                    fields.insert("expiration".into(), expiration.into());
                }
                entry
            })
            .collect();
        let next = skip.saturating_add(page.len());
        let mut result = json!({
            "account": id.to_address(),
            "ledger_index": self.ledger,
            "offers": page,
            "validated": true,
        });
        if next < owned.len()
            && let Some(object) = result.as_object_mut()
        {
            object.insert("marker".into(), next.into());
        }
        Ok(result)
    }
}

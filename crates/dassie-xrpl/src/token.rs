//! The token scheme (spec/areas/xrpl.md#the-token-scheme), sans-IO: the
//! owner's book of a dollar token on the trust line of the node's first XRPL
//! account on the token's network ([`XrplAccount`]) to the token's issuer,
//! which a settlement hub's funding pays from
//! (spec/areas/xrpl.md#owner-books). It peers with no one, and books on a
//! dollar ledger of its own, whose unit is a millionth of a dollar. Each
//! token it books is a [`Token`]: its scheme's ID, its issuer, its
//! currency, its ledger and its network; `xrpl-rlusd` ([`crate::rlusd`]) on
//! mainnet and `xrpl-testnet-dsusd` ([`crate::dsusd`]) on the testnet are
//! this scheme with their own. It pays its fees and its line's reserve in
//! the account's XRP, as gas: on the books of the network's XRP scheme,
//! such as `xrpl-testnet-xrp+xrp`, when that scheme books the same account,
//! and off the books otherwise (spec/decisions/external-assets.md).
//!
//! Before it counts as synced, the account has a line to the issuer with
//! rippling disabled on its side and the largest limit; a line missing or
//! made otherwise is set so with a `TrustSet`. Every validated transaction
//! that touches the line is booked once, in ledger order, from a cursor
//! kept with the books, by what it did to the line:
//!
//! - the owner's payment: posted when it succeeded, voided when it failed;
//! - anything else that raises the line, whatever memo it carries: a
//!   deposit; that lowers it, an issuer's clawback included: a withdrawal.
//!
//! After each, the books hold the line's balance in whole units; what is
//! left below a unit stays on the line and is reconciled through
//! `equity/suspense` once it adds up. Other currencies, other issuers and
//! XRP move nothing here.
//!
//! The issuer can freeze the line, deep-freeze it, freeze its token for
//! every holder, and charge a transfer fee (spec/decisions/issued-assets.md).
//! Our line's flags are read at each sync and with every transaction on it;
//! the issuer's account at every settlement check. While the line is frozen,
//! a global freeze holds or a transfer fee is set, the owner's payments
//! wait.
//!
//! A dollar token converts at par, as any dollar ledger does, with no
//! guard on its price: a token off its peg is not detected
//! (spec/decisions/open-rate-sources.md).

use alloc::string::String;
use alloc::vec::Vec;
use core::marker::PhantomData;

use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
use dassie_settlement::{
    AssetId, Effect, ExternalAsset, LedgerStatus, OwnerPayment, OwnerPaymentState, ReconcileCause,
    SchemeError, SettlementScheme,
};

use crate::account::{News, Ours, XrplAccount};
use crate::amount::IssuedAsset;
use crate::amount::{Currency, TokenAmount, TokenValue};
use crate::api::{AccountInfo, AccountSettings, LineView, TxRecord};
use crate::codec::TF_SET_NO_RIPPLE;
use crate::follower::{Follower, cancel_pending, failure_of, narrow};
use crate::kept::{In, Out, SCHEME_VERSION, TOKEN_SCHEME_VERSION};
use crate::keys::AccountId;
use crate::network::Network;
use crate::scheme::account_of;

/// A dollar token the scheme books: what tells one token scheme from
/// another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// The scheme's ID, which is also the slug of its ledger.
    pub scheme: Name,
    /// Its issuer on its network.
    pub issuer: AccountId,
    /// Its currency code.
    pub currency: Currency,
    /// The ledger the scheme books on: a dollar ledger of its own, `USD` at
    /// par (spec/decisions/issued-assets.md).
    pub ledger: LedgerId,
    /// The network it is issued on, whose account the scheme books.
    pub network: &'static Network,
}

impl Token {
    /// The line the scheme settles on.
    #[must_use]
    pub const fn line(&self) -> IssuedAsset {
        IssuedAsset {
            issuer: self.issuer,
            currency: self.currency,
        }
    }

    /// `value` of the token.
    #[must_use]
    pub const fn amount(&self, value: TokenValue) -> TokenAmount {
        TokenAmount {
            value,
            currency: self.currency,
            issuer: self.issuer,
        }
    }
}

/// A token a [`TokenScheme`] books, as a type, so each token's scheme
/// is a type of its own ([`crate::RlusdScheme`], [`crate::DsusdScheme`]).
pub trait IssuedToken: 'static {
    /// The token.
    const TOKEN: Token;
}

/// The ledger's unit, a millionth of a dollar, in internal units: the
/// books count dollars at scale 9 (spec/areas/ledger.md#ledgers-and-amounts).
/// A token's sixteen digits keep a millionth exact for any balance a node
/// could hold.
pub const UNITS_PER_MICRO: u64 = 1_000;
/// The decimals of a millionth.
const DECIMALS: u8 = 6;
/// The most whole units the books hold for the line.
const MAX_MICROS: u64 = MAX_TOTAL / UNITS_PER_MICRO;
/// The longest a failed `TrustSet` waits before the next, in ledgers: about
/// a quarter of an hour on the testnet. Each failure costs a fee, so the
/// wait doubles with each.
const MAX_TRUST_WAIT_LEDGERS: u32 = 256;

/// What the issuer's account says that the scheme heeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Issuer {
    /// It froze its token for every holder.
    global_freeze: bool,
    /// It charges a fee on transfers between holders.
    transfer_fee: bool,
}

impl From<AccountSettings> for Issuer {
    fn from(settings: AccountSettings) -> Self {
        Self {
            global_freeze: settings.global_freeze(),
            transfer_fee: settings.charges_transfer_fee(),
        }
    }
}

/// The scheme that books token `T`.
pub struct TokenScheme<T: IssuedToken> {
    /// What it keeps of the account, as every scheme on it does.
    f: Follower,
    // Learnt from the account, as it told the scheme.
    /// Our line, as the ledger last said: `None` until it said, then the
    /// line from our side, or `None` if there is none.
    line: Option<Option<LineView>>,
    /// The issuer's account, as last read: `None` until read, then what
    /// it says, or `None` if it does not exist.
    issuer: Option<Option<Issuer>>,
    /// Told that the account is synced, since it was last written off.
    account_synced: bool,
    /// Our `TrustSet` on its way.
    trust_set: Option<[u8; 32]>,
    /// The validated ledger from which a `TrustSet` may be signed again,
    /// and how long the next failure makes it wait.
    trust_at: u32,
    trust_wait: u32,
    /// The token, as a type only: the scheme holds none of it.
    token: PhantomData<fn() -> T>,
}

impl<T: IssuedToken> TokenScheme<T> {
    /// The scheme that books the line of `account`, with what the books
    /// last checkpointed and the state they kept, of this version
    /// (spec/areas/xrpl.md#state-and-restarts).
    ///
    /// # Errors
    ///
    /// If the account is on another network than the token, or the state is
    /// invalid: a state the scheme never wrote.
    pub fn new(
        account: &XrplAccount,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let token = T::TOKEN;
        assert_eq!(token.ledger.asset().code, "USD", "a dollar token");
        assert_eq!(token.ledger.realm(), token.network.realm, "its realm");
        if account.network() != token.network {
            return Err(SchemeError::InvalidConfig);
        }
        let mut scheme = Self {
            f: Follower::new(account, checkpoint, MAX_MICROS),
            line: None,
            issuer: None,
            account_synced: false,
            trust_set: None,
            trust_at: 0,
            trust_wait: 1,
            token: PhantomData,
        };
        if !state.is_empty() {
            scheme.restore(state).ok_or(SchemeError::InvalidData)?;
        }
        Ok(scheme)
    }

    /// Whether the line is as the scheme needs it: there, with rippling
    /// disabled on our side and the largest limit.
    fn line_ok(&self) -> bool {
        matches!(
            self.line,
            Some(Some(view)) if view.ours.no_ripple && view.limit == TokenValue::MAX
        )
    }

    /// Whether the issuer froze our line, so that it pays only the issuer.
    fn frozen(&self) -> bool {
        matches!(self.line, Some(Some(view)) if view.theirs.freeze || view.theirs.deep_freeze)
    }

    /// Whether the scheme may pay out of the line now, as far as the line
    /// and the issuer go: synced, the line not frozen, the issuer read, no
    /// global freeze and no transfer fee, which a Payment without `SendMax`
    /// cannot pay (spec/roadmap.md#not-built-yet).
    fn can_pay(&self) -> bool {
        self.synced()
            && !self.frozen()
            && matches!(
                self.issuer,
                Some(Some(Issuer {
                    global_freeze: false,
                    transfer_fee: false
                }))
            )
    }

    /// Up to date with the account, with the line as it needs it.
    fn synced(&self) -> bool {
        self.account_synced && self.line_ok()
    }

    /// What a line holds: its balance in whole units, none below zero. A
    /// balance below zero books nothing before it is converted: only a
    /// balance above zero too large for the books saturates.
    fn bookable(line: Option<&LineView>) -> u64 {
        let micros = line
            .filter(|view| !view.balance.is_negative())
            .map_or(Some(0), |view| view.balance.floor_units(DECIMALS))
            .map_or(MAX_MICROS, |micros| {
                u64::try_from(micros.max(0)).map_or(MAX_MICROS, |micros| micros.min(MAX_MICROS))
            });
        micros * UNITS_PER_MICRO
    }

    /// Takes one piece of news from the account.
    fn tell(&mut self, account: &mut XrplAccount, news: &News, effects: &mut Vec<Effect>) {
        let id = self.id();
        match news {
            News::Line { key, line } if *key == T::TOKEN.line() => {
                self.line = Some(line.and_then(|line| line.view(self.f.account)));
            }
            News::Line { .. } => {}
            News::Synced(info) => self.synced_at(account, info, effects),
            News::Transaction { record, ours } => {
                let mine = ours.filter(|ours| ours.scheme == id);
                self.transaction(account, record, mine, effects);
            }
            News::Cancelled { scheme, hash, .. } if *scheme == id => {
                // It moved nothing, not even a fee.
                if self.f.is_owner_payment(hash) {
                    self.f.owner_done(OwnerPaymentState::Cancelled, effects);
                } else if self.trust_set == Some(*hash) {
                    self.trust_failed();
                }
            }
            News::Cancelled { .. } => {}
            News::WrittenOff { .. } => self.written_off(effects),
            News::LedgerClosed { .. } => self.correct_line(account),
            // What became of it is in the line the sync matched.
            News::Unread { hash, .. } if self.f.is_owner_payment(hash) => {
                self.f.owner_done(OwnerPaymentState::Unknown, effects);
            }
            News::Unread { .. } | News::Found { .. } => {}
            News::Issuer { issuer, settings } if *issuer == T::TOKEN.issuer => {
                self.issuer = Some(settings.map(Issuer::from));
                self.correct_line(account);
            }
            News::Issuer { .. } => {}
        }
    }

    /// Up to date as of `info`, with the line as of the same ledger, which
    /// the account told just before: the books are made to match it, and a
    /// line that is not as the scheme needs it is set so.
    fn synced_at(
        &mut self,
        account: &mut XrplAccount,
        info: &AccountInfo,
        effects: &mut Vec<Effect>,
    ) {
        self.account_synced = true;
        let at = (info.ledger_index, u32::MAX);
        if let Some(cursor) = self.f.cursor {
            self.match_line(effects);
            self.f.cursor = Some(cursor.max(at));
        }
        account.read_issuer(T::TOKEN.issuer);
        self.check_line(account, at, effects);
    }

    /// Once the account is synced and the line is as the scheme needs it,
    /// the first time since it last started from nothing: what the line
    /// holds is the owner's deposit, and the cursor set at `at`. A line not
    /// as it needs it is set so.
    fn check_line(&mut self, account: &mut XrplAccount, at: (u32, u32), effects: &mut Vec<Effect>) {
        if !self.line_ok() {
            self.correct_line(account);
            return;
        }
        if self.f.cursor.is_some() || !self.account_synced {
            return;
        }
        let target = Self::bookable(self.line.flatten().as_ref());
        if self.f.booked == 0 && target > 0 {
            effects.push(Effect::Deposit { amount: target });
            self.f.booked = target;
        }
        self.match_line(effects);
        self.f.cursor = Some(at);
    }

    /// Signs a `TrustSet` that sets the line as the scheme needs it, unless
    /// one is on its way (signed before a restart, too), the last failed
    /// too recently, or the issuer is not known to exist, to which a
    /// `TrustSet` would only fail.
    fn correct_line(&mut self, account: &mut XrplAccount) {
        let issuer_exists = matches!(self.issuer, Some(Some(_)));
        if self.line_ok()
            || !self.account_synced
            || self.trust_set.is_some()
            || account.signs_trust_set(self.id())
            || self.f.validated < self.trust_at
            || !issuer_exists
        {
            return;
        }
        let limit = T::TOKEN.amount(TokenValue::MAX);
        // Not now (no XRP for it, as gas): tried again as ledgers close.
        if let Ok(hash) = account.sign_trust_set(self.id(), limit, TF_SET_NO_RIPPLE) {
            self.trust_set = Some(hash);
        }
    }

    /// Our `TrustSet` failed, or can never apply: the next waits, twice as
    /// long as the last.
    fn trust_failed(&mut self) {
        self.trust_set = None;
        self.trust_at = self.f.validated.saturating_add(self.trust_wait);
        self.trust_wait = self
            .trust_wait
            .saturating_mul(2)
            .min(MAX_TRUST_WAIT_LEDGERS);
    }

    /// The account is gone, and the line with it: what the books held is a
    /// loss through suspense, and the scheme starts from nothing.
    fn written_off(&mut self, effects: &mut Vec<Effect>) {
        self.f.written_off(effects);
        self.line = None;
        self.issuer = None;
        self.account_synced = false;
        self.trust_set = None;
        self.trust_at = 0;
        self.trust_wait = 1;
    }

    /// Makes the books match the line in whole units.
    fn match_line(&mut self, effects: &mut Vec<Effect>) {
        let Some(line) = self.line else {
            return;
        };
        let target = Self::bookable(line.as_ref());
        self.f
            .reconcile_to(target, ReconcileCause::Unexplained, effects);
    }

    /// Books one validated transaction, once, in ledger order, by what it
    /// did to the line. The owner's payment is booked whenever the ledger
    /// shows it, even behind the cursor. Before the first sync, it only
    /// says what the line is.
    fn transaction(
        &mut self,
        account: &mut XrplAccount,
        record: &TxRecord,
        mine: Option<Ours>,
        effects: &mut Vec<Effect>,
    ) {
        let at = (record.ledger_index, record.index);
        if let Some(ours) = mine
            && !ours.settles
            && self.trust_set == Some(record.hash)
        {
            if record.succeeded() {
                self.trust_set = None;
                self.trust_wait = 1;
            } else {
                self.trust_failed();
            }
        }
        let change = record.line(self.f.account, T::TOKEN.issuer, T::TOKEN.currency);
        let after = change.map(|change| change.after().and_then(|line| line.view(self.f.account)));
        let payment = mine.filter(|ours| ours.settles);
        match self.f.cursor {
            None => {
                if let Some(after) = after {
                    self.line = Some(after);
                }
                self.check_line(account, at, effects);
                return;
            }
            Some(cursor) if at <= cursor => {
                if let Some(booked) = payment.and_then(|ours| self.ours(record, &ours, effects)) {
                    self.f.hold(booked, effects);
                    self.match_line(effects);
                }
                return;
            }
            Some(_) => {}
        }
        self.f.cursor = Some(at);
        // What the transaction did to the line is what its metadata says,
        // before and after, never what the scheme last saw: after a
        // restart it saw nothing, and the line may have moved since.
        let before =
            change.map(|change| change.before().and_then(|line| line.view(self.f.account)));
        if let Some(after) = after {
            self.line = Some(after);
        }
        let mut booked = i128::from(self.f.booked);
        if let Some(owner) = payment.and_then(|ours| self.ours(record, &ours, effects)) {
            booked = owner;
        } else if let (Some(before), Some(after)) = (before, after) {
            let moved = i128::from(Self::bookable(after.as_ref()))
                - i128::from(Self::bookable(before.as_ref()));
            let amount = narrow(moved.abs());
            if moved > 0 {
                effects.push(Effect::Deposit { amount });
                booked += moved;
            } else if moved < 0 {
                effects.push(Effect::Withdrawal { amount });
                booked += moved;
            }
        }
        self.f.hold(booked, effects);
        self.match_line(effects);
        // A line set otherwise since, such as rippling enabled outside the
        // node, is set again.
        self.check_line(account, at, effects);
    }

    /// Books one of our Payments the ledger validated: the owner's payment
    /// on its way, or the owner's last one read again. Its fee is gas,
    /// which this ledger does not hold. Returns what the books then hold,
    /// or `None` for any other, which is booked as any transaction is, by
    /// what it did to the line.
    fn ours(&mut self, record: &TxRecord, ours: &Ours, effects: &mut Vec<Effect>) -> Option<i128> {
        if self.f.is_owner_payment(&record.hash) {
            Some(self.owner_paid(record, effects))
        } else if self.f.was_owner_payment(&record.hash) {
            Some(self.owner_read_again(record, ours, effects))
        } else {
            None
        }
    }

    /// Books the owner's payment the ledger validated: the hold on the
    /// owner's funds posted when it succeeded, voided when it failed. Its
    /// fee is gas, which this ledger does not hold. Returns what the books
    /// then hold.
    fn owner_paid(&mut self, record: &TxRecord, effects: &mut Vec<Effect>) -> i128 {
        let booked = i128::from(self.f.booked);
        let Some((_, micros)) = self.f.owner_payment else {
            return booked;
        };
        if record.succeeded() {
            self.f.owner_done(OwnerPaymentState::Applied, effects);
            booked - i128::from(micros * UNITS_PER_MICRO)
        } else {
            self.f
                .owner_done(OwnerPaymentState::Failed(failure_of(record)), effects);
            booked
        }
    }

    /// The owner's last payment, read again. One whose
    /// outcome the scheme had given up on as unknown, its hold voided, is
    /// booked now as the owner's withdrawal of what it paid when it
    /// succeeded, and what the line booked through suspense meanwhile comes
    /// back out; one already booked books nothing again. Returns what the
    /// books then hold.
    fn owner_read_again(
        &mut self,
        record: &TxRecord,
        ours: &Ours,
        effects: &mut Vec<Effect>,
    ) -> i128 {
        let booked = i128::from(self.f.booked);
        let Some((hash, OwnerPaymentState::Unknown)) = self.f.owner_outcome else {
            return booked;
        };
        if record.succeeded() {
            let amount = paid(ours);
            effects.push(Effect::Withdrawal { amount });
            self.f.owner_outcome = Some((hash, OwnerPaymentState::Applied));
            booked - i128::from(amount)
        } else {
            self.f.owner_outcome = Some((hash, OwnerPaymentState::Failed(failure_of(record))));
            booked
        }
    }
}

/// What a Payment of ours paid, as we signed it, in internal units: a
/// Payment of ours is never partial, so it delivered all of it.
fn paid(ours: &Ours) -> u64 {
    let micros = ours
        .token
        .and_then(|value| value.floor_units(DECIMALS))
        .and_then(|micros| u64::try_from(micros).ok())
        .unwrap_or(0);
    assert!(micros <= MAX_MICROS, "we sign at most what the books hold");
    micros * UNITS_PER_MICRO
}

/// What is kept with the books: what every scheme on the account keeps.
impl<T: IssuedToken> TokenScheme<T> {
    /// Reads back what [`SettlementScheme::state`] wrote: what every scheme
    /// on the account keeps, and every byte of it. It reads the version
    /// before its own beside it, written while a dollar token had a peg
    /// guard: the same, then the guard's part, which it reads whole and
    /// drops (spec/areas/xrpl.md#state-and-restarts).
    fn restore(&mut self, state: &[u8]) -> Option<()> {
        let (mut input, version) = In::state_of(state, &[SCHEME_VERSION, TOKEN_SCHEME_VERSION])?;
        self.f.read(&mut input)?;
        if version == SCHEME_VERSION {
            skip_guard(&mut input)?;
        }
        input.finish()
    }
}

/// Reads the part of a token scheme's state of [`SCHEME_VERSION`] that its
/// peg guard kept, and drops it: where it stood (a code up to 2), since
/// when it was crossing, when a reading last counted, and the last price
/// that did with its ledger. `None` for a part no guard wrote.
fn skip_guard(input: &mut In<'_>) -> Option<()> {
    (input.u8()? <= 2).then_some(())?;
    input.option(In::u64)?;
    input.option(In::u64)?;
    input.option(|input| Some((input.u64()?, input.u32()?)))?;
    Some(())
}

impl<T: IssuedToken> SettlementScheme for TokenScheme<T> {
    fn id(&self) -> Name {
        T::TOKEN.scheme
    }

    fn ledger(&self) -> LedgerId {
        T::TOKEN.ledger
    }

    fn asset(&self) -> Option<AssetId> {
        Some(T::TOKEN.network.first())
    }

    fn issuer(&self) -> Option<String> {
        Some(T::TOKEN.issuer.to_address())
    }

    fn balance(&self) -> u64 {
        self.f.booked
    }

    /// Synced once the account is and the line is as the scheme needs it.
    /// A freeze keeps it synced: it still sees its ledger.
    fn status(&self) -> LedgerStatus {
        if self.synced() {
            LedgerStatus::Synced
        } else {
            LedgerStatus::Syncing
        }
    }

    /// The cursor is set by the first sync with the line as the scheme
    /// needs it, after what the line held was booked as the owner's; it is
    /// dropped only with the account, when a testnet reset starts it from
    /// nothing.
    fn has_synced(&self) -> bool {
        self.f.cursor.is_some()
    }

    /// An owner book: it peers with no one (spec/areas/xrpl.md#owner-books).
    fn peers(&self) -> bool {
        false
    }

    /// The issuer's account is read at each check: a global freeze and a
    /// transfer fee change only its account, which our account's stream
    /// never carries.
    fn on_check(&mut self, _now_ms: u64, asset: &mut dyn ExternalAsset) {
        if let Some(account) = account_of(asset) {
            account.read_issuer(T::TOKEN.issuer);
        }
    }

    fn unit(&self) -> u64 {
        UNITS_PER_MICRO
    }

    fn state(&self) -> Vec<u8> {
        let mut out = Out::state(TOKEN_SCHEME_VERSION);
        self.f.write(&mut out);
        out.0
    }

    fn state_version(&self) -> u64 {
        self.f.fingerprint(0)
    }

    fn on_asset(&mut self, _now_ms: u64, asset: &mut dyn ExternalAsset, effects: &mut Vec<Effect>) {
        let account = account_of(asset);
        assert!(account.is_some(), "the scheme books an XRPL account");
        let Some(account) = account else {
            return;
        };
        assert_eq!(
            account.account(),
            self.f.account,
            "the scheme's own account"
        );
        // The line first: a scheme new on a synced account makes it sync
        // again, and the line is read with it.
        account.follow_line(T::TOKEN.line());
        let news = account.news().to_vec();
        if self.f.catch_up(account, &news) {
            self.written_off(effects);
        }
        for item in &news {
            self.tell(account, item, effects);
        }
        self.f.caught_up(account, self.id(), effects);
    }

    fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        let _ = asset;
        cancel_pending(pending, effects);
    }

    /// A `settlement` message on it is a bare notice and books nothing:
    /// whatever a Payment moved on the ledger is the owner's deposit.
    fn books_messages(&self) -> bool {
        false
    }

    /// Acknowledged, booking nothing: a message on an owner book books
    /// nothing, and is acknowledged so the channel, which carries one
    /// settlement at a time, moves on (spec/areas/xrpl.md#owner-books).
    fn handle_settlement(
        &mut self,
        _peer: Name,
        _amount: u64,
        _data: &[u8],
        _peer_state: &[u8],
        _effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        Ok(())
    }

    fn deposit(&mut self, _amount: u64, _effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    /// A Payment of whole millionths of the token, without paths and never
    /// partial, with the tag and the memo if given
    /// ([`owner_memos`](crate::scheme::owner_memos)),
    /// executed at once, as the XRP scheme pays one
    /// (spec/areas/xrpl.md#deposits-and-withdrawals). `Unavailable` while
    /// the line or the issuer would stop it, or the account cannot sign it
    /// now. Its fee is gas, on the books of the network's
    /// XRP scheme if that runs on the account, so nothing on this ledger
    /// is held for it.
    fn pay_owner(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        payment: &OwnerPayment,
    ) -> Result<Name, SchemeError> {
        let (destination, memos) = self.f.owner_request(payment)?;
        let micros = payment.amount / UNITS_PER_MICRO;
        let whole = payment.amount.is_multiple_of(UNITS_PER_MICRO);
        if micros == 0 || micros > MAX_MICROS || !whole {
            return Err(SchemeError::InvalidData);
        }
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        if !self.can_pay() {
            return Err(SchemeError::Unavailable);
        }
        let value = TokenValue::from_units(micros, DECIMALS).ok_or(SchemeError::InvalidData)?;
        let token = T::TOKEN.amount(value);
        let hash =
            account.sign_token_payment_to(self.id(), (destination, payment.tag), token, memos)?;
        Ok(self.f.owner_signed(account, self.id(), hash, micros))
    }

    fn owner_payment(&self, id: Name) -> OwnerPaymentState {
        self.f.owner_state(id)
    }

    fn check_invariants(&self) {
        self.f.check_invariants();
        assert!((1..=MAX_TRUST_WAIT_LEDGERS).contains(&self.trust_wait));
        assert_eq!(self.f.booked % UNITS_PER_MICRO, 0, "whole units");
        // The books hold the line in whole units, once the scheme booked
        // from it and knows it.
        if self.account_synced
            && self.f.cursor.is_some()
            && let Some(line) = self.line
        {
            assert_eq!(
                self.f.booked,
                Self::bookable(line.as_ref()),
                "the books match the line"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // A line below zero books nothing, however far below: one beyond what
    // the books can convert is still a debt, never the most they hold.
    #[test]
    fn a_line_far_below_zero_books_nothing() {
        let mut line = LineView {
            counterparty: crate::dsusd::ISSUER,
            balance: TokenValue::parse("-1").unwrap(),
            limit: TokenValue::MAX,
            counterparty_limit: TokenValue::MAX,
            ours: crate::api::LineFlags::default(),
            theirs: crate::api::LineFlags::default(),
        };
        assert_eq!(crate::DsusdScheme::bookable(Some(&line)), 0);
        line.balance = TokenValue::parse("-1e40").unwrap();
        assert!(line.balance.floor_units(DECIMALS).is_none());
        assert_eq!(
            crate::DsusdScheme::bookable(Some(&line)),
            0,
            "negative holdings are never a positive asset"
        );
    }
}

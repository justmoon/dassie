//! The token scheme (spec/areas/xrpl.md#the-token-scheme), sans-IO: a
//! settlement scheme that settles in a dollar token on the trust line of
//! the node's first XRPL account on the token's network ([`XrplAccount`])
//! to the token's issuer, and books on a dollar ledger of its own, whose
//! unit is a millionth of a dollar. Each token it settles in is a
//! [`Token`]: its scheme's ID, its memo type, its issuer, its currency, its
//! ledger and its network; `xrpl-rlusd` ([`crate::rlusd`]) on mainnet and
//! `xrpl-testnet-dsusd` ([`crate::dsusd`]) on the testnet are this scheme
//! with their own. It pays its fees and its line's reserve in the account's
//! XRP, as gas: on the books of the network's XRP scheme, such as
//! `xrpl-testnet-xrp+xrp`, when that scheme settles from the same account, and
//! off the books otherwise (spec/decisions/external-assets.md).
//!
//! Before it counts as synced, the account has a line to the issuer with
//! rippling disabled on its side and the largest limit; a line missing or
//! made otherwise is set so with a `TrustSet`. Every validated transaction
//! that touches the line is booked once, in ledger order, from a cursor
//! kept with the books, by what it did to the line:
//!
//! - one of our settlements: posted when it succeeded, voided when it
//!   failed;
//! - a Payment from a peer's known address, with our memo naming the peer,
//!   that delivered the token of our issuer: a settlement from it, by what
//!   it delivered, in whole units;
//! - one of at least one largest packet with our memo naming a node not our
//!   peer at that address: an entry payment, held off the books as the XRP
//!   scheme holds one ([`crate::entry`]);
//! - anything else that raises the line: a deposit; that lowers it, an
//!   issuer's clawback included: a withdrawal.
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
//! a global freeze holds or a transfer fee is set, the scheme settles
//! nothing; while the line is deep-frozen, a global freeze holds or a
//! transfer fee is set, peers cannot pay us either
//! ([`can_receive`](SettlementScheme::can_receive)).
//!
//! A token its pool prices has a peg guard ([`crate::peg`]): the account
//! reads the pool for it once a minute, and the node hands it its XRP/USD
//! rate. While the guard pauses the scheme, off its peg or without a
//! reading that counts, the scheme settles nothing, peers cannot pay us,
//! and the connector converts nothing to or from its ledger
//! ([`converts`](SettlementScheme::converts)); what the line shows is still
//! booked (spec/decisions/issued-assets.md).

use alloc::string::String;
use alloc::vec::Vec;
use core::marker::PhantomData;

use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
use dassie_settlement::{
    AssetId, DollarRate, Effect, EntryPayment, ExternalAsset, LedgerStatus, OwnerPayment,
    OwnerPaymentState, PegView, PreparedSettlement, ReconcileCause, SchemeError, SettlementScheme,
};
use rand_core::CryptoRng;

use crate::account::{News, Ours, XrplAccount};
use crate::amount::IssuedAsset;
use crate::amount::{Currency, TokenAmount, TokenValue};
use crate::api::{AccountInfo, AccountSettings, LineView, TxRecord};
use crate::codec::{Memo, TF_SET_NO_RIPPLE};
use crate::follower::{Follower, check_claim, failure_of, hash_name, narrow};
use crate::from_hex;
use crate::kept::{In, Out};
use crate::keys::AccountId;
use crate::network::Network;
use crate::peg::{Guard, Peg, Timings};
use crate::scheme::account_of;

/// A dollar token the scheme settles in: what tells one token scheme from
/// another. Each is a contract between peers that run its scheme, so none
/// changes but with the peer protocol's version
/// (spec/areas/xrpl.md#the-payment-it-signs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// The scheme's ID, which is also the slug of its ledger.
    pub scheme: Name,
    /// The memo that marks a settlement in it: its own, so a Payment is
    /// never read as a settlement of another XRPL scheme.
    pub memo_type: &'static [u8],
    /// Its issuer on its network.
    pub issuer: AccountId,
    /// Its currency code.
    pub currency: Currency,
    /// The ledger the scheme books on: a dollar ledger of its own, `USD` at
    /// par (spec/decisions/issued-assets.md).
    pub ledger: LedgerId,
    /// The network it is issued on, whose account the scheme settles from.
    pub network: &'static Network,
    /// What prices it, for its peg guard: its pool of XRP on its network.
    /// `None` for a token no pool prices at a dollar, which no scheme may
    /// settle in (spec/decisions/issued-assets.md).
    pub peg: Option<Peg>,
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

/// A token a [`TokenScheme`] settles in, as a type, so each token's scheme
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

/// The scheme that settles in token `T`.
pub struct TokenScheme<T: IssuedToken> {
    /// What it keeps of the account, as every scheme on it does.
    f: Follower,
    /// Its peg guard, if its token has one; kept with the books.
    guard: Option<Guard>,
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
    /// The scheme that settles from `account`, for node `node`, with what
    /// the books last checkpointed and the state they kept.
    ///
    /// # Errors
    ///
    /// If the account is on another network than the token, or the state is
    /// invalid, or looks up a transaction the account has in flight or
    /// remembers as never sent: a state the two never wrote.
    pub fn new(
        account: &XrplAccount,
        node: Name,
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
            f: Follower::new(account, node, checkpoint, MAX_MICROS),
            guard: token.peg.map(Guard::new),
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
        scheme.f.fits(account)?;
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

    /// Whether no peer can pay us on our line: it is deep-frozen, the
    /// issuer froze its token for every holder, or it charges a transfer
    /// fee, which no node of the scheme pays, as a Payment without `SendMax` cannot
    /// (spec/areas/settlement.md#when-peers-cannot-pay-us).
    fn cannot_receive(&self) -> bool {
        matches!(self.line, Some(Some(view)) if view.theirs.deep_freeze)
            || matches!(self.issuer, Some(Some(issuer)) if issuer.global_freeze || issuer.transfer_fee)
    }

    /// Runs its peg guard, if it has one, on `timings`: the node's, which
    /// are dev timers' in dev and the end-to-end tests
    /// ([`Timings::DEV`]). What the guard keeps does not change.
    pub fn set_peg_timings(&mut self, timings: Timings) {
        self.guard = self.guard.take().map(|guard| guard.timed(timings));
    }

    /// Whether the peg guard pauses the scheme now.
    fn paused(&self) -> bool {
        self.guard
            .as_ref()
            .is_some_and(|guard| guard.paused().is_some())
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

    /// Whether the scheme may settle now: it may pay, and the peg guard
    /// does not pause it.
    fn can_settle(&self) -> bool {
        self.can_pay() && !self.paused()
    }

    /// Up to date with the account, with the line as it needs it.
    fn synced(&self) -> bool {
        self.account_synced && self.line_ok()
    }

    /// Takes entry payments of at least `minimum`, in internal units: one
    /// largest packet, as the XRP scheme does. Until it is told, it takes
    /// none.
    pub fn take_entries_from(&mut self, minimum: u64) {
        self.f.entries.take_from(minimum);
    }

    /// Whether the node peers on the scheme, as its config says
    /// ([`crate::scheme::SchemeConfig::peers`]): one that does not is an
    /// owner book (spec/areas/xrpl.md#owner-books). It peers until told.
    pub fn set_peers(&mut self, peers: bool) {
        self.f.peering = peers;
    }

    /// What it holds of entry payments, in internal units: on the line,
    /// and never on the books.
    #[must_use]
    pub fn entries_held(&self) -> u64 {
        self.f.entries.total()
    }

    /// What the books should hold for a line: [`Self::bookable`], less
    /// the entry payments held.
    fn target(&self, line: Option<&LineView>) -> u64 {
        Self::bookable(line).saturating_sub(self.f.entries.total())
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

    /// Takes one piece of news from the account, at `now_ms`.
    fn tell(
        &mut self,
        now_ms: u64,
        account: &mut XrplAccount,
        news: &News,
        effects: &mut Vec<Effect>,
    ) {
        let id = self.id();
        match news {
            News::Line { key, line } if *key == T::TOKEN.line() => {
                self.line = Some(line.and_then(|line| line.view(self.f.account)));
            }
            News::Line { .. } => {}
            News::Synced(info) => {
                // A new connection: the pool is read at the next check.
                if let Some(guard) = &mut self.guard {
                    guard.ask_again();
                }
                self.synced_at(account, info, effects);
            }
            News::Transaction { record, ours } => {
                let mine = ours.filter(|ours| ours.scheme == id);
                self.transaction(account, record, mine, effects);
            }
            News::Cancelled {
                scheme,
                hash,
                settles,
            } if *scheme == id => {
                // It moved nothing, not even a fee.
                if self.f.is_owner_payment(hash) {
                    self.f.owner_done(OwnerPaymentState::Cancelled, effects);
                } else if *settles {
                    effects.push(Effect::Cancel {
                        settlement: hash_name(hash),
                    });
                } else if self.trust_set == Some(*hash) {
                    self.trust_failed();
                }
            }
            News::Cancelled { .. } => {}
            News::WrittenOff { restarted } => self.written_off(*restarted, effects),
            News::LedgerClosed { .. } => {
                self.correct_line(account);
                self.f.ask_recover(account, self.account_synced);
            }
            // What became of it is in the line the sync matched.
            News::Unread { hash, .. } if self.f.is_owner_payment(hash) => {
                self.f.owner_done(OwnerPaymentState::Unknown, effects);
            }
            News::Unread { hash, ours } if ours.scheme == id => self.f.unread(*hash),
            News::Unread { .. } => {}
            News::Found { hash, record } => self.recovered(*hash, record, effects),
            News::Issuer { issuer, settings } if *issuer == T::TOKEN.issuer => {
                self.issuer = Some(settings.map(Issuer::from));
                self.correct_line(account);
            }
            News::Issuer { .. } => {}
            News::Pool { key, pool } if *key == T::TOKEN.line() => {
                if let Some(guard) = &mut self.guard {
                    guard.reading(now_ms, pool.as_ref());
                }
            }
            News::Pool { .. } => {}
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
        self.f.ask_recover(account, self.account_synced);
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
    fn written_off(&mut self, restarted: bool, effects: &mut Vec<Effect>) {
        self.f.written_off(restarted, effects);
        self.line = None;
        self.issuer = None;
        self.account_synced = false;
        self.trust_set = None;
        self.trust_at = 0;
        self.trust_wait = 1;
    }

    /// Makes the books match the line in whole units, less the entry
    /// payments held.
    fn match_line(&mut self, effects: &mut Vec<Effect>) {
        let Some(line) = self.line else {
            return;
        };
        let target = self.target(line.as_ref());
        self.f
            .reconcile_to(target, ReconcileCause::Unexplained, effects);
    }

    /// Books one validated transaction, once, in ledger order, by what it
    /// did to the line. One of ours is settled whenever the ledger shows
    /// it, even behind the cursor. Before the first sync, it only says what
    /// the line is.
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
        match self.f.cursor {
            None => {
                if let Some(after) = after {
                    self.line = Some(after);
                }
                self.check_line(account, at, effects);
                return;
            }
            Some(cursor) if at <= cursor => {
                if let Some(ours) = mine.filter(|ours| ours.settles) {
                    let booked = self.ours(record, &ours, effects);
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
        if let Some(ours) = mine {
            if ours.settles {
                booked = self.ours(record, &ours, effects);
            }
        } else if let (Some(before), Some(after)) = (before, after) {
            if let Some((peer, amount)) = self.settled_by(record) {
                effects.push(Effect::IncomingSettlement {
                    peer,
                    amount,
                    id: Some(hash_name(&record.hash)),
                });
                booked += i128::from(amount);
            } else if self.held_entry(record) {
                // Off the books until its node peers, or its hold is over.
            } else {
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
        }
        self.f.hold(booked, effects);
        self.match_line(effects);
        // A line set otherwise since, such as rippling enabled outside the
        // node, is set again.
        self.check_line(account, at, effects);
    }

    /// Settles one of our settlements the ledger validated, which paid
    /// `paid` in internal units: posted or voided. Its fee is gas, which
    /// this ledger does not hold. Returns what the books then hold.
    fn resolve(&mut self, record: &TxRecord, paid: u64, effects: &mut Vec<Effect>) -> i128 {
        self.f.resolved(&record.hash);
        let settlement = hash_name(&record.hash);
        if record.succeeded() {
            effects.push(Effect::Finalize { settlement });
            i128::from(self.f.booked) - i128::from(paid)
        } else {
            effects.push(Effect::Cancel { settlement });
            i128::from(self.f.booked)
        }
    }

    /// Books one of our Payments the ledger validated: the owner's payment
    /// on its way, the owner's last one read again, or a settlement.
    /// Returns what the books then hold.
    fn ours(&mut self, record: &TxRecord, ours: &Ours, effects: &mut Vec<Effect>) -> i128 {
        if self.f.is_owner_payment(&record.hash) {
            self.owner_paid(record, effects)
        } else if self.f.was_owner_payment(&record.hash) {
            self.owner_read_again(record, ours, effects)
        } else {
            self.resolve(record, paid(ours), effects)
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

    /// The owner's last payment, read again: never a settlement. One whose
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

    /// The outcome of a settlement of ours that a restart left pending,
    /// posted or voided now (spec/areas/xrpl.md#state-and-restarts). `hash`
    /// is what was looked up. One the scheme read before the restart, it
    /// booked then, with what it did to the line. One it never read is
    /// booked now as one of ours read behind the cursor is, by what the
    /// books hold pending for it, which is what we signed, never by what a
    /// server says it delivered: what the line booked through suspense at
    /// the sync comes back out.
    fn recovered(&mut self, hash: [u8; 32], record: &TxRecord, effects: &mut Vec<Effect>) {
        let Some(recovering) = self.f.found(&hash) else {
            return;
        };
        let settlement = hash_name(&hash);
        if let (true, Some(amount)) = (recovering.unread, recovering.amount) {
            let booked = self.resolve(record, amount, effects);
            self.f.hold(booked, effects);
            self.match_line(effects);
            return;
        }
        effects.push(if record.succeeded() {
            Effect::Finalize { settlement }
        } else {
            Effect::Cancel { settlement }
        });
    }

    /// The peer a transaction settles from, and how much in internal units:
    /// a successful Payment to us that delivered the token of our issuer, with
    /// our memo naming a peer whose known address sent it. Less than a unit
    /// is no settlement.
    fn settled_by(&self, record: &TxRecord) -> Option<(Name, u64)> {
        let (peer, amount) = self.named_by(record)?;
        self.f.sent_by_peer(record, peer).then_some((peer, amount))
    }

    /// The node a successful Payment to us that delivered the token of our
    /// issuer names in our memo, other than ours, and how much it delivered
    /// in internal units: whoever sent it. Less than a unit names no one.
    fn named_by(&self, record: &TxRecord) -> Option<(Name, u64)> {
        let delivered = record.delivered_token?;
        if delivered.currency != T::TOKEN.currency || delivered.issuer != T::TOKEN.issuer {
            return None;
        }
        let node = self.f.named_by(record, T::TOKEN.memo_type)?;
        let micros = u64::try_from(delivered.value.floor_units(DECIMALS)?).ok()?;
        let held = self.f.booked.saturating_add(self.f.entries.total()) / UNITS_PER_MICRO;
        let room = MAX_MICROS.saturating_sub(held);
        let amount = micros.min(room) * UNITS_PER_MICRO;
        (amount > 0).then_some((node, amount))
    }

    /// Holds `record` as an entry payment, as the XRP scheme does
    /// (spec/decisions/pay-to-enter.md). Returns whether it does.
    fn held_entry(&mut self, record: &TxRecord) -> bool {
        let Some((node, amount)) = self.named_by(record) else {
            return false;
        };
        self.f.hold_entry(record, node, amount)
    }
}

/// What a settlement of ours paid, as we signed it, in internal units: a
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

/// What is kept with the books: what every scheme on the account keeps,
/// and the peg guard.
impl<T: IssuedToken> TokenScheme<T> {
    /// Reads back what [`SettlementScheme::state`] wrote: what every
    /// scheme on the account keeps, then the peg guard, for a token that
    /// has one, and every byte of it.
    fn restore(&mut self, state: &[u8]) -> Option<()> {
        let mut input = In::state(state)?;
        self.f.read(&mut input)?;
        if let Some(peg) = T::TOKEN.peg {
            self.guard = Some(Guard::read(peg, &mut input)?);
        }
        input.finish()
    }
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

    /// Version 2: its memo type is its scheme's ID
    /// (spec/decisions/scheme-names.md), so a node of version 1, which
    /// wrote another, is refused as a peer on it rather than read wrong.
    fn supported_versions(&self) -> &'static [u16] {
        &[2]
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
    /// nothing. Until then no peer is taken, so none pays into a line that
    /// is missing (spec/decisions/synced-before-peering.md).
    fn has_synced(&self) -> bool {
        self.f.cursor.is_some()
    }

    fn peers(&self) -> bool {
        self.f.peering
    }

    /// Not while the issuer stops what peers pay us, nor while the peg
    /// guard pauses the scheme: a peer asked to settle in a token off its
    /// peg would pay us at par.
    fn can_receive(&self) -> bool {
        !self.cannot_receive() && !self.paused()
    }

    fn converts(&self) -> bool {
        !self.paused()
    }

    fn peg(&self) -> Option<PegView> {
        self.guard.as_ref().map(Guard::view)
    }

    fn rate_currencies(&self) -> &'static [&'static str] {
        if self.guard.is_some() { &["XRP"] } else { &[] }
    }

    fn on_rate(&mut self, code: &str, rate: Option<DollarRate>) {
        if let (Some(guard), "XRP") = (&mut self.guard, code) {
            guard.rate(rate);
        }
    }

    /// The issuer's account is read at each check: a global freeze and a
    /// transfer fee change only its account, which our account's stream
    /// never carries. The pool is read once a minute, for the peg guard, or
    /// once a second on dev timers.
    fn on_check(&mut self, now_ms: u64, asset: &mut dyn ExternalAsset) {
        let read_pool = self.guard.as_mut().is_some_and(|guard| {
            guard.tick(now_ms);
            guard.ask(now_ms)
        });
        if let Some(account) = account_of(asset) {
            account.read_issuer(T::TOKEN.issuer);
            if read_pool {
                account.read_pool(T::TOKEN.line());
            }
        }
    }

    fn unit(&self) -> u64 {
        UNITS_PER_MICRO
    }

    fn state(&self) -> Vec<u8> {
        let mut out = Out::state();
        self.f.write(&mut out);
        if let Some(guard) = &self.guard {
            guard.write(&mut out);
        }
        out.0
    }

    fn state_version(&self) -> u64 {
        self.f
            .fingerprint(self.guard.as_ref().map_or(0, Guard::fingerprint))
    }

    fn on_asset(&mut self, now_ms: u64, asset: &mut dyn ExternalAsset, effects: &mut Vec<Effect>) {
        if let Some(guard) = &mut self.guard {
            guard.tick(now_ms);
        }
        let account = account_of(asset);
        assert!(
            account.is_some(),
            "the scheme settles through an XRPL account"
        );
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
            self.written_off(true, effects);
        }
        for item in &news {
            self.tell(now_ms, account, item, effects);
        }
        if self.f.expire_entries(effects) {
            self.match_line(effects);
        }
        self.f.caught_up(account, self.id(), effects);
    }

    fn peering_info(&self, out: &mut Vec<u8>) {
        self.f.peering_info(out);
    }

    fn create_peering_request(
        &mut self,
        _peer: Name,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        self.f.create_peering_request(peering_info, out)
    }

    fn accept_peering_request(
        &mut self,
        _peer: Name,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError> {
        self.f.accept_peering_request(data, response)
    }

    fn finalize_peering_request(
        &mut self,
        _peer: Name,
        peering_info: &[u8],
        _data: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        self.f.finalize_peering_request(peering_info)
    }

    fn on_peered(&mut self, peer: Name, state: &[u8]) {
        self.f.on_peered(peer, state);
    }

    fn on_closed(&mut self, peer: Name) {
        self.f.on_closed(peer);
    }

    /// As in the XRP scheme: the address the request names is where `peer`
    /// settles from.
    fn entry(&self, peer: Name, data: &[u8]) -> EntryPayment {
        self.f.entry(peer, data)
    }

    /// As in the XRP scheme: what it held from `peer`, from the address it
    /// peered with, is its settlements.
    fn take_entry(&mut self, peer: Name, effects: &mut Vec<Effect>) {
        self.f.take_entry(peer, effects);
        self.match_line(effects);
    }

    /// Unavailable unless synced, the line not frozen, the issuer read with
    /// no global freeze and no transfer fee, the peg guard not pausing the
    /// scheme, and the account signs the Payment now
    /// ([`XrplAccount::sign_token_payment`]), with its fee in gas. It
    /// settles whole millionths, which a token's value holds exactly.
    fn prepare_settlement(
        &mut self,
        _rng: &mut dyn CryptoRng,
        asset: Option<&mut dyn ExternalAsset>,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        let destination = self.f.destination(peer, peer_state)?;
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        if !self.can_settle() {
            return Err(SchemeError::Unavailable);
        }
        let micros = amount / UNITS_PER_MICRO;
        let value = TokenValue::from_units(micros, DECIMALS).ok_or(SchemeError::Unavailable)?;
        let memo = Memo {
            memo_type: T::TOKEN.memo_type.to_vec(),
            memo_data: self.f.node.as_str().as_bytes().to_vec(),
        };
        let token = T::TOKEN.amount(value);
        let hash = account.sign_token_payment(self.id(), destination, token, alloc::vec![memo])?;
        Ok(PreparedSettlement {
            id: hash_name(&hash),
            amount: micros * UNITS_PER_MICRO,
            message: hash.to_vec(),
        })
    }

    fn settlement_message(&self, settlement: Name) -> Vec<u8> {
        from_hex(settlement.as_str()).unwrap_or_default()
    }

    fn execute_settlement(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        settlement: Name,
        effects: &mut Vec<Effect>,
    ) {
        Follower::execute(asset, self.id(), settlement, effects);
    }

    fn discard_settlement(&mut self, asset: Option<&mut dyn ExternalAsset>, settlement: Name) {
        Follower::discard(asset, self.id(), settlement);
    }

    fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        let id = self.id();
        self.f.reconcile(asset, id, pending, effects);
    }

    fn handle_settlement(
        &mut self,
        _peer: Name,
        _amount: u64,
        data: &[u8],
        _peer_state: &[u8],
        _effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        // The claim books nothing: the line says what arrived, once.
        check_claim(data)
    }

    fn handle_message(&mut self, _peer: Name, _message: &[u8], _effects: &mut Vec<Effect>) {}

    fn deposit(&mut self, _amount: u64, _effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    /// A Payment of whole millionths of the token, without paths and never
    /// partial, with the tag and one memo of [`OWNER_MEMO_TYPE`] if given,
    /// executed at once, as the XRP scheme pays one
    /// (spec/areas/xrpl.md#deposits-and-withdrawals). `Unavailable` while
    /// the line or the issuer would stop a settlement (the peg guard aside:
    /// a token off its peg is still the owner's to move), or the account
    /// cannot sign it now. Its fee is gas, on the books of the network's
    /// XRP scheme if that runs on the account, so nothing on this ledger
    /// is held for it.
    ///
    /// [`OWNER_MEMO_TYPE`]: crate::scheme::OWNER_MEMO_TYPE
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
        assert_eq!(
            self.guard.is_some(),
            T::TOKEN.peg.is_some(),
            "a guard for a token a pool prices"
        );
        if let Some(guard) = &self.guard {
            guard.check_invariants();
        }
        assert_eq!(self.f.booked % UNITS_PER_MICRO, 0, "whole units");
        // The books hold the line in whole units, once the scheme booked
        // from it and knows it.
        if self.account_synced
            && self.f.cursor.is_some()
            && let Some(line) = self.line
        {
            assert_eq!(
                self.f.booked,
                self.target(line.as_ref()),
                "the books match the line"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::account::FIRST;
    use crate::keys::Wallet;

    // spec/areas/xrpl.md#the-token-scheme: a Payment whose Amount names the
    // payee as its issuer, any issuer the payee trusts, may report what it
    // delivered with the payee's account as the issuer. That is no
    // settlement: the payee credits less than arrived, never more, and
    // books what the line rose by as the owner's deposit.
    #[test]
    fn a_payment_naming_us_as_its_issuer_is_no_settlement() {
        use dassie_settlement::SettlementScheme;

        let id = dassie_ledger::name!(crate::dsusd::SCHEME_ID);
        let account = XrplAccount::new(FIRST, &[id], "", &[1; 32], &[]).unwrap();
        let node = dassie_ledger::name!("node");
        let mut scheme = crate::DsusdScheme::new(&account, node, None, &[]).unwrap();
        let peer = dassie_ledger::name!("peer");
        let theirs = Wallet::from_entropy(&[2; 32]).account();
        let address = theirs.to_address();
        let mut state = vec![u8::try_from(address.len()).unwrap()];
        state.extend_from_slice(address.as_bytes());
        scheme.on_peered(peer, &state);
        let delivered = |issuer| TokenAmount {
            value: TokenValue::parse("4").unwrap(),
            currency: crate::dsusd::CURRENCY,
            issuer,
        };
        let mut record = TxRecord {
            hash: [3; 32],
            ledger_index: 10,
            index: 0,
            validated: true,
            account: theirs,
            is_payment: true,
            destination: Some(account.account()),
            destination_tag: None,
            sequence: 1,
            fee: 10,
            result: "tesSUCCESS".into(),
            delivered: None,
            delivered_token: Some(delivered(crate::dsusd::ISSUER)),
            memo: Some((crate::dsusd::MEMO_TYPE.to_vec(), b"peer".to_vec())),
            changes: Vec::new(),
            lines: Vec::new(),
        };
        assert_eq!(
            scheme.settled_by(&record),
            Some((peer, 4_000_000 * UNITS_PER_MICRO))
        );
        record.delivered_token = Some(delivered(account.account()));
        assert_eq!(scheme.settled_by(&record), None);
    }

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

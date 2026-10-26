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
//! `xrpl-testnet+xrp`, when that scheme settles from the same account, and
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

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::marker::PhantomData;

use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
use dassie_settlement::{
    AssetId, DollarRate, Effect, EntryPayment, ExternalAsset, LedgerStatus, PegView,
    PreparedSettlement, ReconcileCause, SchemeError, SettlementScheme,
};
use rand_core::CryptoRng;
use serde_json::{Value, json};

use crate::account::{LineKey, News, Ours, Resumed, XrplAccount};
use crate::amount::{Currency, TokenAmount, TokenValue};
use crate::api::{AccountInfo, AccountSettings, LineView, TxRecord};
use crate::codec::{Memo, TF_SET_NO_RIPPLE};
use crate::entry::Entries;
use crate::keys::AccountId;
use crate::network::Network;
use crate::peg::{Guard, Peg, Timings};
use crate::scheme::{
    MAX_PEERS, MAX_RECOVER_WAIT_LEDGERS, MAX_RECOVERING, Recovering, account_of, address_bytes,
    hash_name, missed_write_off, name_hash, narrow, parse_address, recover, restore_unread,
    restore_write_offs, unread_value,
};
use crate::{from_hex, hex_upper};

/// A dollar token the scheme settles in: what tells one token scheme from
/// another. Each is a contract between peers that run its scheme, so none
/// changes but with the peer protocol's version
/// (spec/areas/xrpl.md#the-payment-it-signs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// The scheme's ID, which is also the slug of its ledger.
    pub scheme_id: &'static str,
    /// The scheme's ID as a name.
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
    pub const fn line(&self) -> LineKey {
        LineKey {
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
    /// The account it settles from.
    account: AccountId,
    /// Our node's ID, as our memos carry it.
    node: Name,
    /// What the books hold on the line, in internal units.
    booked: u64,
    // Kept with the books.
    cursor: Option<(u32, u32)>,
    /// Peers' addresses, with when each was learnt: past [`MAX_PEERS`], the
    /// longest known goes.
    peers: BTreeMap<Name, (AccountId, u64)>,
    /// The peer table's revision, counted up on each address learnt and
    /// each forgotten: the order addresses were learnt in, and part of
    /// [`fingerprint`](Self::fingerprint), so that the node keeps every
    /// change to the table. Not kept: a restore learns the addresses
    /// anew, and it counts up from there.
    peers_revision: u64,
    recovering: Vec<Recovering>,
    /// How many times the account was written off, as the scheme last saw
    /// it ([`XrplAccount::write_offs`]).
    write_offs: u64,
    /// What nodes not yet peered paid us to enter, off the books
    /// (spec/decisions/pay-to-enter.md).
    entries: Entries,
    // Learnt from the account, as it told the scheme.
    /// Our line, as the ledger last said: `None` until it said, then the
    /// line from our side, or `None` if there is none.
    line: Option<Option<LineView>>,
    /// The issuer's account, as last read: `None` until read, then what
    /// it says, or `None` if it does not exist.
    issuer: Option<Option<Issuer>>,
    /// Told that the account is synced, since it was last written off.
    account_synced: bool,
    validated: u32,
    /// Our `TrustSet` on its way.
    trust_set: Option<[u8; 32]>,
    /// The validated ledger from which a `TrustSet` may be signed again,
    /// and how long the next failure makes it wait.
    trust_at: u32,
    trust_wait: u32,
    /// Its peg guard, if its token has one; kept with the books.
    guard: Option<Guard>,
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
        assert_eq!(token.scheme.as_str(), token.scheme_id, "the token's name");
        assert_eq!(token.ledger.asset().code, "USD", "a dollar token");
        assert_eq!(token.ledger.realm(), token.network.realm, "its realm");
        if account.network() != token.network {
            return Err(SchemeError::InvalidConfig);
        }
        let mut scheme = Self {
            account: account.account(),
            node,
            booked: checkpoint.unwrap_or(0),
            cursor: None,
            peers: BTreeMap::new(),
            peers_revision: 0,
            recovering: Vec::new(),
            write_offs: 0,
            entries: Entries::default(),
            line: None,
            issuer: None,
            account_synced: false,
            validated: 0,
            trust_set: None,
            trust_at: 0,
            trust_wait: 1,
            guard: token.peg.map(Guard::new),
            token: PhantomData,
        };
        if !state.is_empty() {
            scheme.restore(state).ok_or(SchemeError::InvalidData)?;
        }
        if scheme
            .recovering
            .iter()
            .any(|r| account.holds_transaction(&r.hash))
        {
            return Err(SchemeError::InvalidData);
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

    /// Whether the scheme may settle now, as far as the line, the issuer
    /// and the peg go: synced, the line not frozen, the issuer read, no
    /// global freeze and no transfer fee, which a Payment without `SendMax`
    /// cannot pay (spec/roadmap.md#not-built-yet), and the guard not
    /// pausing it.
    fn can_settle(&self) -> bool {
        self.synced()
            && !self.frozen()
            && !self.paused()
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

    /// Takes entry payments of at least `minimum`, in internal units: one
    /// largest packet, as the XRP scheme does. Until it is told, it takes
    /// none.
    pub fn take_entries_from(&mut self, minimum: u64) {
        self.entries.take_from(minimum);
    }

    /// What it holds of entry payments, in internal units: on the line,
    /// and never on the books.
    #[must_use]
    pub fn entries_held(&self) -> u64 {
        self.entries.total()
    }

    /// What the books should hold for a line: [`Self::bookable`], less
    /// the entry payments held.
    fn target(&self, line: Option<&LineView>) -> u64 {
        Self::bookable(line).saturating_sub(self.entries.total())
    }

    /// What a line holds: its balance in whole units, none below zero.
    fn bookable(line: Option<&LineView>) -> u64 {
        let micros = line
            .map_or(Some(0), |view| view.balance.floor_units(DECIMALS))
            .map_or(MAX_MICROS, |micros| {
                u64::try_from(micros.max(0)).map_or(MAX_MICROS, |micros| micros.min(MAX_MICROS))
            });
        micros * UNITS_PER_MICRO
    }

    /// Whether the books hold anything of the line.
    const fn holds_anything(&self) -> bool {
        self.cursor.is_some() || self.booked > 0
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
                self.line = Some(line.and_then(|line| line.view(self.account)));
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
                if *settles {
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
                self.ask_recover(account);
            }
            News::Unread { hash, ours } if ours.scheme == id => self.unread(*hash),
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
        if let Some(cursor) = self.cursor {
            self.match_line(effects);
            self.cursor = Some(cursor.max(at));
        }
        account.read_issuer(T::TOKEN.issuer);
        self.check_line(account, at, effects);
        self.ask_recover(account);
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
        if self.cursor.is_some() || !self.account_synced {
            return;
        }
        let target = Self::bookable(self.line.flatten().as_ref());
        if self.booked == 0 && target > 0 {
            effects.push(Effect::Deposit { amount: target });
            self.booked = target;
        }
        self.match_line(effects);
        self.cursor = Some(at);
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
            || self.validated < self.trust_at
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
        self.trust_at = self.validated.saturating_add(self.trust_wait);
        self.trust_wait = self
            .trust_wait
            .saturating_mul(2)
            .min(MAX_TRUST_WAIT_LEDGERS);
    }

    /// The account is gone, and the line with it: what the books held is a
    /// loss through suspense, and the scheme starts from nothing.
    fn written_off(&mut self, restarted: bool, effects: &mut Vec<Effect>) {
        if restarted {
            for recovering in &mut self.recovering {
                recovering.ask_at = 0;
            }
        }
        if self.booked > 0 {
            effects.push(Effect::Reconcile {
                amount: self.booked,
                gain: false,
                cause: ReconcileCause::Unexplained,
            });
            self.booked = 0;
        }
        // What they held went with the line.
        self.entries.clear();
        self.cursor = None;
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
        if target != self.booked {
            effects.push(Effect::Reconcile {
                amount: target.abs_diff(self.booked),
                gain: target > self.booked,
                cause: ReconcileCause::Unexplained,
            });
            self.booked = target;
        }
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
        let change = record.line(self.account, T::TOKEN.issuer, T::TOKEN.currency);
        let after = change.map(|change| change.after().and_then(|line| line.view(self.account)));
        match self.cursor {
            None => {
                if let Some(after) = after {
                    self.line = Some(after);
                }
                self.check_line(account, at, effects);
                return;
            }
            Some(cursor) if at <= cursor => {
                if let Some(ours) = mine.filter(|ours| ours.settles) {
                    let booked = self.resolve(record, paid(&ours), effects);
                    self.hold(booked, effects);
                    self.match_line(effects);
                }
                return;
            }
            Some(_) => {}
        }
        self.cursor = Some(at);
        // What the transaction did to the line is what its metadata says,
        // before and after, never what the scheme last saw: after a
        // restart it saw nothing, and the line may have moved since.
        let before = change.map(|change| change.before().and_then(|line| line.view(self.account)));
        if let Some(after) = after {
            self.line = Some(after);
        }
        let mut booked = i128::from(self.booked);
        if let Some(ours) = mine {
            if ours.settles {
                booked = self.resolve(record, paid(&ours), effects);
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
        self.hold(booked, effects);
        self.match_line(effects);
        // A line set otherwise since, such as rippling enabled outside the
        // node, is set again.
        self.check_line(account, at, effects);
    }

    /// Makes the books hold `booked`. Below zero, they hold nothing, and
    /// what they are short comes from suspense.
    fn hold(&mut self, booked: i128, effects: &mut Vec<Effect>) {
        self.booked = narrow(booked.clamp(0, i128::from(MAX_TOTAL)));
        if booked < 0 {
            effects.push(Effect::Reconcile {
                amount: narrow(-booked),
                gain: true,
                cause: ReconcileCause::Unexplained,
            });
        }
    }

    /// Settles one of our settlements the ledger validated, which paid
    /// `paid` in internal units: posted or voided. Its fee is gas, which
    /// this ledger does not hold. Returns what the books then hold.
    fn resolve(&mut self, record: &TxRecord, paid: u64, effects: &mut Vec<Effect>) -> i128 {
        self.recovering.retain(|r| r.hash != record.hash);
        let settlement = hash_name(&record.hash);
        if record.succeeded() {
            effects.push(Effect::Finalize { settlement });
            i128::from(self.booked) - i128::from(paid)
        } else {
            effects.push(Effect::Cancel { settlement });
            i128::from(self.booked)
        }
    }

    /// A settlement of ours that validated while the scheme was not on the
    /// account, and that it did not read again: booked as one never read
    /// once found, as the XRP scheme does.
    fn unread(&mut self, hash: [u8; 32]) {
        if let Some(recovering) = self.recovering.iter_mut().find(|r| r.hash == hash) {
            recovering.unread = true;
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
        let Some(position) = self.recovering.iter().position(|r| r.hash == hash) else {
            return;
        };
        let recovering = self.recovering.remove(position);
        let settlement = hash_name(&hash);
        if let (true, Some(amount)) = (recovering.unread, recovering.amount) {
            let booked = self.resolve(record, amount, effects);
            self.hold(booked, effects);
            self.match_line(effects);
            return;
        }
        effects.push(if record.succeeded() {
            Effect::Finalize { settlement }
        } else {
            Effect::Cancel { settlement }
        });
    }

    /// Asks for the outcome of a restart's settlements, as the XRP scheme
    /// does: each again only once its wait is over, twice as long each time,
    /// and only once the account synced, which tells first what the scheme
    /// did not read again.
    fn ask_recover(&mut self, account: &mut XrplAccount) {
        if !self.account_synced {
            return;
        }
        let validated = self.validated;
        for recovering in &mut self.recovering {
            if !account.can_look_up() {
                return;
            }
            if validated >= recovering.ask_at && !account.is_looking_up(&recovering.hash) {
                recovering.ask_at = validated.saturating_add(recovering.wait);
                recovering.wait = recovering
                    .wait
                    .saturating_mul(2)
                    .min(MAX_RECOVER_WAIT_LEDGERS);
                account.look_up(recovering.hash);
            }
        }
    }

    /// The peer a transaction settles from, and how much in internal units:
    /// a successful Payment to us that delivered the token of our issuer, with
    /// our memo naming a peer whose known address sent it. Less than a unit
    /// is no settlement.
    fn settled_by(&self, record: &TxRecord) -> Option<(Name, u64)> {
        let (peer, amount) = self.named_by(record)?;
        (self.peers.get(&peer).map(|(address, _)| *address) == Some(record.account))
            .then_some((peer, amount))
    }

    /// The node a successful Payment to us that delivered the token of our
    /// issuer names in our memo, other than ours, and how much it delivered
    /// in internal units: whoever sent it. Less than a unit names no one.
    fn named_by(&self, record: &TxRecord) -> Option<(Name, u64)> {
        if !(record.succeeded() && record.is_payment && record.destination == Some(self.account)) {
            return None;
        }
        let delivered = record.delivered_token?;
        if delivered.currency != T::TOKEN.currency || delivered.issuer != T::TOKEN.issuer {
            return None;
        }
        let (memo_type, memo_data) = record.memo.as_ref()?;
        if memo_type.as_slice() != T::TOKEN.memo_type {
            return None;
        }
        let node = Name::new(core::str::from_utf8(memo_data).ok()?).ok()?;
        if node == self.node {
            return None;
        }
        let micros = u64::try_from(delivered.value.floor_units(DECIMALS)?).ok()?;
        let held = self.booked.saturating_add(self.entries.total()) / UNITS_PER_MICRO;
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
        let read = self.validated.max(record.ledger_index);
        self.entries
            .hold(node, record.account, amount, record.hash, read)
    }

    /// Books the entry payments whose hold is over as the owner's
    /// deposits.
    fn expire_entries(&mut self, effects: &mut Vec<Effect>) {
        let expired = self.entries.expire(self.validated);
        if expired.is_empty() {
            return;
        }
        for entry in expired {
            effects.push(Effect::Deposit {
                amount: entry.amount,
            });
            self.booked = self.booked.saturating_add(entry.amount);
        }
        // What fees or a reserve took meanwhile came out of what was held:
        // the books match what is left, the shortfall through suspense.
        self.match_line(effects);
    }

    /// Keeps a peer's address; past [`MAX_PEERS`], the one learnt longest
    /// ago goes, as in the XRP scheme.
    fn learn(&mut self, peer: Name, address: AccountId) {
        if !self.peers.contains_key(&peer)
            && self.peers.len() >= MAX_PEERS
            && let Some(oldest) = self
                .peers
                .iter()
                .min_by_key(|(_, (_, learnt))| *learnt)
                .map(|(name, _)| *name)
        {
            self.peers.remove(&oldest);
        }
        self.peers_revision = self.peers_revision.wrapping_add(1);
        self.peers.insert(peer, (address, self.peers_revision));
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

/// What is kept with the books: the same as the XRP scheme keeps.
impl<T: IssuedToken> TokenScheme<T> {
    fn fingerprint(&self) -> u64 {
        let mut mix: u64 = 0xcbf2_9ce4_8422_2325;
        let mut add = |value: u64| {
            mix ^= value;
            mix = mix.wrapping_mul(0x0100_0000_01b3);
        };
        let (ledger, index) = self.cursor.unwrap_or((u32::MAX, u32::MAX));
        add(u64::from(ledger) << 32 | u64::from(index));
        add(self.peers_revision);
        for Recovering { hash, unread, .. } in &self.recovering {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]) | u64::from(*unread) << 16);
        }
        add(self.recovering.len() as u64);
        add(self.write_offs);
        add(self.guard.as_ref().map_or(0, Guard::fingerprint));
        self.entries.fingerprint(&mut add);
        mix
    }

    fn state_value(&self) -> Value {
        let peers: serde_json::Map<String, Value> = self
            .peers
            .iter()
            .map(|(peer, (address, _))| (peer.as_str().to_string(), address.to_address().into()))
            .collect();
        let recovering: Vec<Value> = self
            .recovering
            .iter()
            .map(|r| hex_upper(&r.hash).into())
            .collect();
        let mut state = json!({
            "cursor": self.cursor.map(|(ledger, index)| json!([ledger, index])),
            "peers": peers,
            "recovering": recovering,
            "write_offs": self.write_offs,
            "unread": unread_value(&self.recovering),
            "entries": self.entries.value(),
        });
        if let (Some(guard), Some(object)) = (&self.guard, state.as_object_mut()) {
            object.insert("peg".into(), guard.state());
        }
        state
    }

    fn restore(&mut self, state: &[u8]) -> Option<()> {
        let value: Value = serde_json::from_slice(state).ok()?;
        let object = value.as_object()?;
        if ["sequence", "in_flight", "newest"]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return None;
        }
        let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
        self.cursor = match value.get("cursor")? {
            Value::Null => None,
            cursor => Some((number(cursor.get(0)?)?, number(cursor.get(1)?)?)),
        };
        self.write_offs = restore_write_offs(&value)?;
        // A state from before the guard has none: it starts anew.
        if let (Some(peg), Some(kept)) = (T::TOKEN.peg, value.get("peg")) {
            self.guard = Some(Guard::restore(peg, kept)?);
        }
        let peers = value.get("peers")?.as_object()?;
        let recovering = value.get("recovering")?.as_array()?;
        if peers.len() > MAX_PEERS || recovering.len() > MAX_RECOVERING {
            return None;
        }
        for (peer, address) in peers {
            self.learn(Name::new(peer).ok()?, AccountId::parse(address.as_str()?)?);
        }
        for hash in recovering {
            let hash: [u8; 32] = from_hex(hash.as_str()?)?.try_into().ok()?;
            if self.recovering.iter().any(|r| r.hash == hash) {
                return None;
            }
            self.recovering.push(Recovering::new(hash));
        }
        self.entries.restore(&value)?;
        // Entry payments are read past the first sync only.
        if self.cursor.is_none() && self.entries.total() > 0 {
            return None;
        }
        restore_unread(&value, &mut self.recovering)
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

    fn supported_versions(&self) -> &'static [u16] {
        &[1]
    }

    fn balance(&self) -> u64 {
        self.booked
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
        self.cursor.is_some()
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
        serde_json::to_vec(&self.state_value()).unwrap_or_default()
    }

    fn state_version(&self) -> u64 {
        self.fingerprint()
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
        assert_eq!(account.account(), self.account, "the scheme's own account");
        // The line first: a scheme new on a synced account makes it sync
        // again, and the line is read with it.
        account.follow_line(T::TOKEN.line());
        self.validated = account.validated();
        let news = account.news().to_vec();
        if missed_write_off(self.write_offs, account, &news) {
            self.written_off(true, effects);
        }
        self.write_offs = account.write_offs();
        for item in &news {
            self.tell(now_ms, account, item, effects);
        }
        if self.cursor.is_some() {
            self.expire_entries(effects);
        }
        account.follow(self.id(), self.cursor, self.holds_anything());
    }

    fn peering_info(&self, out: &mut Vec<u8>) {
        address_bytes(self.account, out);
    }

    fn create_peering_request(
        &mut self,
        _peer: Name,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        parse_address(peering_info)?;
        address_bytes(self.account, out);
        Ok(())
    }

    fn accept_peering_request(
        &mut self,
        _peer: Name,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError> {
        parse_address(data)?;
        response.clear();
        Ok(data.to_vec())
    }

    fn finalize_peering_request(
        &mut self,
        _peer: Name,
        peering_info: &[u8],
        _data: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        parse_address(peering_info)?;
        Ok(peering_info.to_vec())
    }

    fn on_peered(&mut self, peer: Name, state: &[u8]) {
        let address = parse_address(state);
        assert!(address.is_ok(), "a peer state the scheme did not return");
        if let Ok(address) = address {
            self.learn(peer, address);
        }
    }

    /// As in the XRP scheme: a Payment from its address after the close is
    /// the owner's deposit (spec/decisions/closing-channels.md), once held
    /// as an entry payment if it is one.
    fn on_closed(&mut self, peer: Name) {
        // A new revision, or the node would not keep that the address
        // went, and a restart would bring it back.
        if self.peers.remove(&peer).is_some() {
            self.peers_revision = self.peers_revision.wrapping_add(1);
        }
    }

    /// As in the XRP scheme: the address the request names is where `peer`
    /// settles from.
    fn entry(&self, peer: Name, data: &[u8]) -> EntryPayment {
        self.entries.entry(peer, parse_address(data).ok())
    }

    /// As in the XRP scheme: what it held from `peer`, from the address it
    /// peered with, is its settlements.
    fn take_entry(&mut self, peer: Name, effects: &mut Vec<Effect>) {
        let Some((address, _)) = self.peers.get(&peer).copied() else {
            return;
        };
        for entry in self.entries.take(peer, address) {
            effects.push(Effect::IncomingSettlement {
                peer,
                amount: entry.amount,
                id: Some(hash_name(&entry.hash)),
            });
            self.booked = self.booked.saturating_add(entry.amount);
        }
        // The peer is credited what it paid; what the account no longer
        // holds of it, spent on fees or held back by a reserve meanwhile,
        // is a shortfall the books match through suspense.
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
        let destination = parse_address(peer_state)?;
        if self.peers.get(&peer).map(|(address, _)| *address) != Some(destination) {
            self.learn(peer, destination);
        }
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        if !self.can_settle() {
            return Err(SchemeError::Unavailable);
        }
        let micros = amount / UNITS_PER_MICRO;
        let value = TokenValue::from_units(micros, DECIMALS).ok_or(SchemeError::Unavailable)?;
        let memo = Memo {
            memo_type: T::TOKEN.memo_type.to_vec(),
            memo_data: self.node.as_str().as_bytes().to_vec(),
        };
        let token = T::TOKEN.amount(value);
        let hash = account.sign_token_payment(self.id(), destination, token, alloc::vec![memo])?;
        Ok(PreparedSettlement {
            id: hash_name(&hash),
            amount: micros * UNITS_PER_MICRO,
            message: hash.to_vec(),
            peer_state: None,
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
        let account = asset.and_then(account_of);
        assert!(
            account.is_some(),
            "the scheme settles through an XRPL account"
        );
        if let (Some(account), Some(hash)) = (account, name_hash(settlement))
            && account.execute(self.id(), hash)
        {
            return;
        }
        effects.push(Effect::Cancel { settlement });
    }

    fn discard_settlement(&mut self, asset: Option<&mut dyn ExternalAsset>, settlement: Name) {
        if let (Some(account), Some(hash)) = (asset.and_then(account_of), name_hash(settlement)) {
            account.discard(self.id(), hash);
        }
    }

    fn reconcile(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        let account = asset.and_then(account_of);
        assert!(
            account.is_some(),
            "the scheme settles through an XRPL account"
        );
        let Some(account) = account else {
            return;
        };
        for (id, amount) in pending {
            let Some(hash) = name_hash(*id) else {
                continue;
            };
            match account.resume(self.id(), hash) {
                Resumed::InFlight => {}
                Resumed::NeverSent => effects.push(Effect::Cancel { settlement: *id }),
                Resumed::Unknown => recover(&mut self.recovering, hash, *amount),
            }
        }
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
        if data.len() == 32 {
            Ok(())
        } else {
            Err(SchemeError::InvalidData)
        }
    }

    fn handle_message(&mut self, _peer: Name, _message: &[u8], _effects: &mut Vec<Effect>) {}

    fn deposit(&mut self, _amount: u64, _effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    fn check_invariants(&self) {
        assert!(self.peers.len() <= MAX_PEERS);
        self.entries.check_invariants();
        assert!(
            self.cursor.is_some() || self.entries.total() == 0,
            "entry payments are read past the first sync"
        );
        assert!(self.recovering.len() <= MAX_RECOVERING);
        let mut seen = BTreeSet::new();
        assert!(
            self.recovering.iter().all(|r| seen.insert(r.hash)),
            "each settlement looked up once"
        );
        for recovering in &self.recovering {
            assert!((1..=MAX_RECOVER_WAIT_LEDGERS).contains(&recovering.wait));
        }
        assert!((1..=MAX_TRUST_WAIT_LEDGERS).contains(&self.trust_wait));
        assert!(self.booked <= MAX_TOTAL, "the books hold at most a total");
        assert_eq!(
            self.guard.is_some(),
            T::TOKEN.peg.is_some(),
            "a guard for a token a pool prices"
        );
        if let Some(guard) = &self.guard {
            guard.check_invariants();
        }
        assert_eq!(self.booked % UNITS_PER_MICRO, 0, "whole units");
        // The books hold the line in whole units, once the scheme booked
        // from it and knows it.
        if self.account_synced
            && self.cursor.is_some()
            && let Some(line) = self.line
        {
            assert_eq!(
                self.booked,
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
}

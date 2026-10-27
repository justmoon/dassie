//! The XRP scheme (spec/areas/xrpl.md), sans-IO: `xrpl-testnet-xrp` on the
//! testnet ([`XrplScheme`]), `xrpl-xrp` on mainnet ([`XrplMainnetScheme`]).
//!
//! It is the owner's book of the XRP of the node's first XRPL account on its
//! network ([`XrplAccount`]), which connects, signs and follows the ledger
//! for it, and which a settlement hub's funding pays from
//! (spec/areas/xrpl.md#owner-books). It peers with no one. Each network's
//! XRP it books is an [`Xrp`]: its scheme's ID, its ledger and its network.
//! Every validated transaction the account tells it of is booked once, in
//! ledger order, from a cursor kept with the books:
//!
//! - the owner's payment: posted when it succeeded, voided when it failed,
//!   its fee booked either way;
//! - a transaction another scheme on the account signed, such as an RLUSD
//!   owner's payment or a `TrustSet`: its fee, booked here as a ledger fee,
//!   since this is the ledger whose scheme books the account's XRP
//!   (spec/decisions/external-assets.md);
//! - anything else that adds XRP, whatever memo it carries: a deposit; that
//!   takes it: a withdrawal.
//!
//! After each, what the books hold must be the account's balance less its
//! reserve; any difference (a reserve change, a balance below the reserve)
//! is reconciled through `equity/suspense`, the part a change of the
//! reserve explains as the reserve's. The reserve itself is never booked,
//! so it is never spent. The owner's payments are whole drops, failures are
//! checked, and an account created by a payment is a deposit like any
//! other.
//!
//! It books only its account's network, which the account checks before it
//! reads anything (spec/decisions/real-money.md). Mainnet's books on a
//! ledger of the live realm, which runs only in a node of that realm
//! (spec/decisions/real-money.md).

use alloc::vec::Vec;
use core::any::Any;
use core::marker::PhantomData;

use dassie_ledger::{LedgerId, Name};
use dassie_settlement::{
    AssetId, Effect, ExternalAsset, LedgerStatus, OwnerPayment, OwnerPaymentState,
    PreparedSettlement, ReconcileCause, SchemeError, SettlementScheme,
};
use rand_core::CryptoRng;
use serde_json::Value;

use crate::UNITS_PER_DROP;
use crate::account::{MAX_ACCOUNT_DROPS, News, Ours, XrplAccount};
use crate::api::{AccountInfo, TxRecord};
use crate::codec::Memo;
use crate::follower::{Follower, cancel_pending, failure_of, hash_name, narrow};
use crate::kept::{In, Out, SCHEME_VERSION};
use crate::keys::AccountId;
use crate::network::{self, Network};

/// The testnet scheme's ID: the ledger, its network and its asset
/// (spec/decisions/scheme-names.md).
pub const SCHEME_ID: &str = "xrpl-testnet-xrp";

/// A network's XRP, as the scheme books it: what tells one XRP scheme from
/// another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Xrp {
    /// The scheme's ID, which is also the slug of its ledger.
    pub scheme_id: &'static str,
    /// The scheme's ID as a name: its network's XRP scheme, whose books
    /// hold the account's XRP.
    pub scheme: Name,
    /// The ledger the scheme books on, of its network's realm.
    pub ledger: LedgerId,
    /// The network whose XRP it is.
    pub network: &'static Network,
}

/// XRP on the testnet, as `xrpl-testnet-xrp` books it.
pub const TESTNET_XRP: Xrp = Xrp {
    scheme_id: SCHEME_ID,
    scheme: dassie_ledger::name!(SCHEME_ID),
    ledger: LedgerId::XrplTestnetXrp,
    network: &network::TESTNET,
};

/// A network's XRP an [`XrpScheme`] books, as a type, so each network's XRP
/// scheme is a type of its own ([`XrplScheme`]).
pub trait NativeXrp: 'static {
    /// The XRP.
    const XRP: Xrp;
}

/// XRP on the testnet, as a type.
#[derive(Clone, Copy, Debug)]
pub enum TestnetXrp {}

impl NativeXrp for TestnetXrp {
    const XRP: Xrp = TESTNET_XRP;
}

/// The `xrpl-testnet-xrp` scheme.
pub type XrplScheme = XrpScheme<TestnetXrp>;

/// The mainnet scheme's ID: the ledger and its asset, mainnet being the
/// network a name leaves out (spec/decisions/scheme-names.md).
pub const MAINNET_SCHEME_ID: &str = "xrpl-xrp";

/// XRP on mainnet, as `xrpl-xrp` books it: real money.
pub const MAINNET_XRP: Xrp = Xrp {
    scheme_id: MAINNET_SCHEME_ID,
    scheme: dassie_ledger::name!(MAINNET_SCHEME_ID),
    ledger: LedgerId::XrplXrp,
    network: &network::MAINNET,
};

/// XRP on mainnet, as a type.
#[derive(Clone, Copy, Debug)]
pub enum MainnetXrp {}

impl NativeXrp for MainnetXrp {
    const XRP: Xrp = MAINNET_XRP;
}

/// The `xrpl-xrp` scheme, on mainnet.
pub type XrplMainnetScheme = XrpScheme<MainnetXrp>;

/// The memos of an owner's payment ([`SettlementScheme::pay_owner`]): its
/// one memo, if any, of the type its reader expects, such as the key of
/// the account a deposit opens in a settlement hub.
///
/// # Errors
///
/// `InvalidData` for a memo beyond
/// [`MAX_OWNER_MEMO_BYTES`](dassie_settlement::MAX_OWNER_MEMO_BYTES).
pub(crate) fn owner_memos(payment: &OwnerPayment) -> Result<Vec<Memo>, SchemeError> {
    let Some(memo) = &payment.memo else {
        return Ok(Vec::new());
    };
    if !memo.fits() {
        return Err(SchemeError::InvalidData);
    }
    Ok(alloc::vec![Memo {
        memo_type: memo.memo_type.clone(),
        memo_data: memo.data.clone(),
    }])
}

/// What an XRPL scheme's config says: nothing that changes it
/// (spec/areas/xrpl.md#owner-books). It may say `"peers"`, a boolean,
/// which a node read when the XRPL schemes peered: it is read still, so a
/// config that says it loads, and it has no effect, since every XRPL
/// scheme is an owner book. The account it books has a config of its own
/// ([`crate::account::Config`]), set apart from any scheme's
/// (spec/areas/settlement.md#external-assets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemeConfig;

impl SchemeConfig {
    /// The config in `text`: empty, or a JSON object with at most
    /// `peers`, a boolean, which it reads and ignores.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidConfig`] if it is no JSON object, is longer
    /// than a config may be, `peers` is no boolean, or it names anything
    /// else, such as the account's server, which is the account's config.
    pub fn parse(text: &str) -> Result<Self, SchemeError> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Self);
        }
        if text.len() > crate::account::MAX_CONFIG {
            return Err(SchemeError::InvalidConfig);
        }
        let value: Value = serde_json::from_str(text).map_err(|_| SchemeError::InvalidConfig)?;
        let Value::Object(mut object) = value else {
            return Err(SchemeError::InvalidConfig);
        };
        match object.remove("peers") {
            None | Some(Value::Bool(_)) => {}
            Some(_) => return Err(SchemeError::InvalidConfig),
        }
        if !object.is_empty() {
            return Err(SchemeError::InvalidConfig);
        }
        Ok(Self)
    }

    /// Whether `text`, a config the scheme refuses, names what is the
    /// account's (its server, its faucet or its validator list
    /// publishers): the owner sets that as the account's config.
    #[must_use]
    pub fn names_the_account(text: &str) -> bool {
        serde_json::from_str::<Value>(text).is_ok_and(|value| {
            ["server", "faucet", "publishers"]
                .iter()
                .any(|key| value.get(key).is_some())
        })
    }
}

/// The account `asset` is, if it is one.
pub(crate) fn account_of(asset: &mut dyn ExternalAsset) -> Option<&mut XrplAccount> {
    let any: &mut dyn Any = asset;
    any.downcast_mut()
}

/// The scheme that books the XRP of `N`'s network.
pub struct XrpScheme<N: NativeXrp> {
    /// What it keeps of the account, as every scheme on it does.
    f: Follower,
    // Learnt from the account, as it told the scheme.
    /// The account's balance after the last transaction booked, in drops.
    balance: Option<u64>,
    owner_count: u32,
    /// The reserve, base and per owned object, in drops.
    reserve: (u64, u64),
    /// The reserve the books were last matched to, in drops: at the next
    /// match, what a change from it makes is the reserve's. Unknown until
    /// the first match after a start or a write-off, which explains
    /// nothing.
    matched_reserve: Option<u64>,
    /// Told that the account is synced, since it was last written off.
    synced: bool,
    /// The network's XRP, as a type only.
    xrp: PhantomData<fn() -> N>,
}

impl<N: NativeXrp> XrpScheme<N> {
    /// The scheme that books `account`, with what the books last
    /// checkpointed and the state they kept: of this version, or of the
    /// one the scheme wrote while it peered
    /// (spec/areas/xrpl.md#state-and-restarts).
    ///
    /// # Errors
    ///
    /// If the account is on another network, or the state is invalid: a
    /// state the scheme never wrote.
    pub fn new(
        account: &XrplAccount,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let xrp = N::XRP;
        assert_eq!(xrp.scheme.as_str(), xrp.scheme_id, "the scheme's name");
        assert_eq!(xrp.scheme, xrp.network.xrp_scheme, "its network's");
        assert_eq!(xrp.ledger.asset().code, "XRP", "a ledger of XRP");
        assert_eq!(xrp.ledger.realm(), xrp.network.realm, "its realm");
        if account.network() != xrp.network {
            return Err(SchemeError::InvalidConfig);
        }
        let mut scheme = Self {
            f: Follower::new(account, checkpoint, MAX_ACCOUNT_DROPS),
            balance: None,
            owner_count: 0,
            reserve: (0, 0),
            matched_reserve: None,
            synced: false,
            xrp: PhantomData,
        };
        if !state.is_empty() {
            let (mut input, version) = In::versioned(state).ok_or(SchemeError::InvalidData)?;
            scheme
                .f
                .read(&mut input, version)
                .ok_or(SchemeError::InvalidData)?;
            input.finish().ok_or(SchemeError::InvalidData)?;
        }
        Ok(scheme)
    }

    /// The account it books.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.f.account
    }

    /// The reserve the account holds back, in drops.
    fn reserve_drops(&self, owner_count: u32) -> u64 {
        let (base, increment) = self.reserve;
        base.saturating_add(increment.saturating_mul(u64::from(owner_count)))
    }

    /// What the books should hold for a balance: what is above the reserve,
    /// in internal units.
    fn bookable(&self, balance: u64, owner_count: u32) -> u64 {
        above(balance, self.reserve_drops(owner_count))
    }

    /// Takes one piece of news from the account.
    fn tell(&mut self, news: &News, effects: &mut Vec<Effect>) {
        let id = self.id();
        match news {
            News::Synced(info) => self.synced(info, effects),
            News::Transaction { record, ours } => {
                let mine = ours.filter(|ours| ours.scheme == id);
                let other = ours.filter(|ours| ours.scheme != id);
                self.transaction(record, mine, other, effects);
            }
            // The account cancels only what can never apply: proven absent
            // past its last ledger, its number taken by another, or written
            // off. It moved nothing, not even a fee.
            News::Cancelled { scheme, hash, .. }
                if *scheme == id && self.f.is_owner_payment(hash) =>
            {
                self.f.owner_done(OwnerPaymentState::Cancelled, effects);
            }
            News::Cancelled { .. } => {}
            News::WrittenOff { .. } => self.written_off(effects),
            News::LedgerClosed { reserve_changed } => {
                if *reserve_changed && self.synced {
                    self.match_balance(effects);
                }
            }
            // What became of it is in the balance the sync matched.
            News::Unread { hash, .. } if self.f.is_owner_payment(hash) => {
                self.f.owner_done(OwnerPaymentState::Unknown, effects);
            }
            News::Unread { .. }
            | News::Found { .. }
            | News::Line { .. }
            | News::Issuer { .. }
            | News::Pool { .. } => {}
        }
    }

    /// Up to date as of `info`: the books are made to match the balance.
    /// The first sync books what the account holds above the reserve as
    /// the owner's deposit.
    fn synced(&mut self, info: &AccountInfo, effects: &mut Vec<Effect>) {
        let first = self.f.cursor.is_none() && self.f.booked == 0;
        self.balance = Some(info.balance);
        self.owner_count = info.owner_count;
        let target = self.bookable(info.balance, info.owner_count);
        if first && target > 0 {
            effects.push(Effect::Deposit { amount: target });
            self.f.booked = target;
        }
        self.match_balance(effects);
        let at = (info.ledger_index, u32::MAX);
        self.f.cursor = Some(self.f.cursor.map_or(at, |cursor| cursor.max(at)));
        self.synced = true;
    }

    /// The account is gone: what the books held is a loss through suspense,
    /// and the scheme starts from nothing.
    fn written_off(&mut self, effects: &mut Vec<Effect>) {
        self.f.written_off(effects);
        self.balance = None;
        self.matched_reserve = None;
        self.synced = false;
    }

    /// Makes the books match the balance less the reserve. What the books
    /// would hold had the reserve not changed since the last match, and
    /// they do not, is unexplained; the rest, from there to the balance
    /// less the reserve now, is the reserve's change, which explains no
    /// more than the books hold above zero.
    fn match_balance(&mut self, effects: &mut Vec<Effect>) {
        let Some(balance) = self.balance else {
            return;
        };
        let reserve = self.reserve_drops(self.owner_count);
        let target = above(balance, reserve);
        let unchanged = self
            .matched_reserve
            .map_or(target, |matched| above(balance, matched));
        self.f
            .reconcile_to(unchanged, ReconcileCause::Unexplained, effects);
        self.f
            .reconcile_to(target, ReconcileCause::Reserve, effects);
        self.matched_reserve = Some(reserve);
    }

    /// Books one validated transaction, once, in ledger order. The owner's
    /// payment is booked whenever the ledger shows it, even behind the
    /// cursor: what the balance booked through suspense meanwhile comes
    /// back out. So is the fee of one another scheme on the account signed
    /// (`other`), once the scheme has synced, unless the account tells it
    /// again for the scheme that signed it: this scheme was told it, and
    /// booked it, when the ledger validated it. Before its first sync, a
    /// scheme books no other: the sync books what the account holds then
    /// as the owner's, which a scheme that joins an account other schemes
    /// read from earlier is told after their history. One of its own that
    /// is no owner's payment, a settlement signed before the XRPL schemes
    /// stopped peering, is booked as anyone's.
    fn transaction(
        &mut self,
        record: &TxRecord,
        mine: Option<Ours>,
        other: Option<Ours>,
        effects: &mut Vec<Effect>,
    ) {
        let at = (record.ledger_index, record.index);
        if self.f.cursor.is_none_or(|cursor| at <= cursor) {
            if let Some(booked) = mine.and_then(|ours| self.ours(record, ours, effects)) {
                self.f.hold(booked, effects);
                self.match_balance(effects);
            } else if other.is_some_and(|other| !other.again) && self.f.cursor.is_some() {
                let booked = self.gas(record, effects);
                self.f.hold(booked, effects);
                self.match_balance(effects);
            }
            return;
        }
        self.f.cursor = Some(at);
        let change = record.change_of(self.f.account);
        if let Some(change) = change {
            self.balance = Some(change.after);
            if let Some(count) = change.owner_count {
                self.owner_count = count;
            }
        }
        let mut booked = i128::from(self.f.booked);
        if let Some(owner) = mine.and_then(|ours| self.ours(record, ours, effects)) {
            booked = owner;
        } else if other.is_some() {
            // Its fee is ours to book; anything else it did to the XRP is
            // a deposit or a withdrawal, as from anyone.
            booked = self.gas(record, effects);
            if let Some(change) = change {
                let moved =
                    i128::from(change.after) - i128::from(change.before) + i128::from(record.fee);
                let amount = units(narrow(moved.abs()));
                if moved > 0 {
                    effects.push(Effect::Deposit { amount });
                    booked += i128::from(amount);
                } else if moved < 0 {
                    effects.push(Effect::Withdrawal { amount });
                    booked -= i128::from(amount);
                }
            }
        } else if let Some(change) = change {
            if change.after > change.before {
                let amount = units(change.after - change.before);
                effects.push(Effect::Deposit { amount });
                booked += i128::from(amount);
            } else if change.after < change.before {
                let amount = units(change.before - change.after);
                effects.push(Effect::Withdrawal { amount });
                booked -= i128::from(amount);
            }
        }
        self.f.hold(booked, effects);
        self.match_balance(effects);
    }

    /// Books one of our transactions the ledger validated, if it is the
    /// owner's payment on its way or the owner's last one read again.
    /// Returns what the books then hold; `None` for any other.
    fn ours(&mut self, record: &TxRecord, ours: Ours, effects: &mut Vec<Effect>) -> Option<i128> {
        if self.f.is_owner_payment(&record.hash) {
            Some(self.owner_paid(record, effects))
        } else if self.f.was_owner_payment(&record.hash) {
            Some(self.owner_read_again(record, ours.drops, effects))
        } else {
            None
        }
    }

    /// The owner's last payment, read again. One whose outcome the scheme
    /// had given up on as unknown, its hold voided, is booked now as the
    /// owner's withdrawal of the `drops` it paid when it succeeded, with
    /// its fee either way, and what the balance booked through suspense
    /// meanwhile comes back out; one already booked books nothing again.
    /// Returns what the books then hold.
    fn owner_read_again(
        &mut self,
        record: &TxRecord,
        drops: u64,
        effects: &mut Vec<Effect>,
    ) -> i128 {
        let mut booked = i128::from(self.f.booked);
        let Some((hash, OwnerPaymentState::Unknown)) = self.f.owner_outcome else {
            return booked;
        };
        if record.succeeded() {
            let amount = units(drops);
            effects.push(Effect::Withdrawal { amount });
            booked -= i128::from(amount);
            self.f.owner_outcome = Some((hash, OwnerPaymentState::Applied));
        } else {
            self.f.owner_outcome = Some((hash, OwnerPaymentState::Failed(failure_of(record))));
        }
        let fee = units(record.fee);
        effects.push(Effect::Fee {
            amount: fee,
            id: hash_name(&record.hash),
        });
        booked - i128::from(fee)
    }

    /// Books the owner's payment the ledger validated: the hold on the
    /// owner's funds posted when it succeeded, voided when it failed, and
    /// its fee either way. Returns what the books then hold.
    fn owner_paid(&mut self, record: &TxRecord, effects: &mut Vec<Effect>) -> i128 {
        let mut booked = i128::from(self.f.booked);
        let Some((_, drops)) = self.f.owner_payment else {
            return booked;
        };
        if record.succeeded() {
            booked -= i128::from(units(drops));
            self.f.owner_done(OwnerPaymentState::Applied, effects);
        } else {
            self.f
                .owner_done(OwnerPaymentState::Failed(failure_of(record)), effects);
        }
        let fee = units(record.fee);
        effects.push(Effect::Fee {
            amount: fee,
            id: hash_name(&record.hash),
        });
        booked - i128::from(fee)
    }

    /// Books the fee of a transaction another scheme on the account signed:
    /// the XRP it spent as gas, a ledger fee here. Returns what the books
    /// then hold.
    fn gas(&self, record: &TxRecord, effects: &mut Vec<Effect>) -> i128 {
        let fee = units(record.fee);
        effects.push(Effect::Fee {
            amount: fee,
            id: hash_name(&record.hash),
        });
        i128::from(self.f.booked) - i128::from(fee)
    }
}

/// What the books should hold for a balance above a reserve of `reserve`
/// drops, in internal units.
fn above(balance: u64, reserve: u64) -> u64 {
    units(balance.saturating_sub(reserve))
}

/// Drops in internal units. Only amounts checked against
/// [`MAX_ACCOUNT_DROPS`] come here.
pub(crate) fn units(drops: u64) -> u64 {
    assert!(drops <= MAX_ACCOUNT_DROPS, "drops past what the books hold");
    drops * UNITS_PER_DROP
}

impl<N: NativeXrp> SettlementScheme for XrpScheme<N> {
    fn id(&self) -> Name {
        N::XRP.scheme
    }

    fn ledger(&self) -> LedgerId {
        N::XRP.ledger
    }

    fn asset(&self) -> Option<AssetId> {
        Some(N::XRP.network.first())
    }

    fn balance(&self) -> u64 {
        self.f.booked
    }

    fn status(&self) -> LedgerStatus {
        if self.synced {
            LedgerStatus::Synced
        } else {
            LedgerStatus::Syncing
        }
    }

    /// The cursor is set by the first sync, after what the account held
    /// was booked as the owner's, and kept with the state; it is dropped
    /// only with the account, when a testnet reset starts it from nothing.
    fn has_synced(&self) -> bool {
        self.f.cursor.is_some()
    }

    /// An owner book: it peers with no one (spec/areas/xrpl.md#owner-books).
    fn peers(&self) -> bool {
        false
    }

    fn unit(&self) -> u64 {
        UNITS_PER_DROP
    }

    fn reserve(&self) -> u64 {
        self.reserve_drops(self.owner_count)
            .saturating_mul(UNITS_PER_DROP)
    }

    fn state(&self) -> Vec<u8> {
        let mut out = Out::state(SCHEME_VERSION);
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
        let news = account.news().to_vec();
        if self.f.catch_up(account, &news) {
            self.written_off(effects);
        }
        self.f.book_entries(&news, effects);
        // The reserve the books are matched to is the one the account had
        // when it last told something: a reserve it read meanwhile, before
        // it is synced again, is matched when it is.
        if !news.is_empty() {
            self.reserve = account.reserve_params();
        }
        for item in &news {
            self.tell(item, effects);
        }
        self.f.caught_up(account, self.id(), effects);
    }

    /// None: it peers with no one.
    fn peering_info(&self, out: &mut Vec<u8>) {
        out.clear();
    }

    fn create_peering_request(
        &mut self,
        _peer: Name,
        _peering_info: &[u8],
        _out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    fn accept_peering_request(
        &mut self,
        _peer: Name,
        _data: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        Err(SchemeError::Unsupported)
    }

    fn finalize_peering_request(
        &mut self,
        _peer: Name,
        _peering_info: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        Err(SchemeError::Unsupported)
    }

    fn prepare_settlement(
        &mut self,
        _rng: &mut dyn CryptoRng,
        _asset: Option<&mut dyn ExternalAsset>,
        _peer: Name,
        _amount: u64,
        _peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        Err(SchemeError::Unsupported)
    }

    /// It prepares none, so none is executed: one the node asks for anyway
    /// can never apply.
    fn execute_settlement(
        &mut self,
        _asset: Option<&mut dyn ExternalAsset>,
        settlement: Name,
        effects: &mut Vec<Effect>,
    ) {
        effects.push(Effect::Cancel { settlement });
    }

    fn reconcile(
        &mut self,
        _asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        cancel_pending(pending, effects);
    }

    fn handle_settlement(
        &mut self,
        _peer: Name,
        _amount: u64,
        _data: &[u8],
        _peer_state: &[u8],
        _effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    fn deposit(&mut self, _amount: u64, _effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    /// A Payment of whole drops of XRP, never partial, with the tag and
    /// the memo if given ([`owner_memos`]), executed at once: its
    /// submission goes out after the batch that keeps it with the account's
    /// and the scheme's state (spec/areas/xrpl.md#deposits-and-withdrawals).
    fn pay_owner(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        payment: &OwnerPayment,
    ) -> Result<Name, SchemeError> {
        let (destination, memos) = self.f.owner_request(payment)?;
        let drops = payment.amount / UNITS_PER_DROP;
        if drops == 0 || drops > MAX_ACCOUNT_DROPS {
            return Err(SchemeError::InvalidData);
        }
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        let hash = account.sign_payment_to(self.id(), (destination, payment.tag), drops, memos)?;
        Ok(self.f.owner_signed(account, self.id(), hash, drops))
    }

    fn owner_payment(&self, id: Name) -> OwnerPaymentState {
        self.f.owner_state(id)
    }

    fn check_invariants(&self) {
        self.f.check_invariants();
        if self.synced {
            assert!(self.f.cursor.is_some(), "a synced scheme has a cursor");
            // The books hold what is above the reserve, once the scheme
            // knows the balance.
            if let Some(balance) = self.balance {
                assert_eq!(
                    self.f.booked,
                    self.bookable(balance, self.owner_count),
                    "the books match the balance"
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::api::BalanceChange;

    /// The scheme of `N`, on its network's first account, synced at ledger
    /// 5 with the books holding 20 XRP.
    fn synced<N: NativeXrp>() -> (XrplAccount, XrpScheme<N>) {
        let network = N::XRP.network;
        let account =
            XrplAccount::new(network.first(), &[network.xrp_scheme], "", &[7; 32], &[]).unwrap();
        let mut scheme = XrpScheme::<N>::new(&account, Some(units(20_000_000)), &[]).unwrap();
        scheme.f.cursor = Some((5, 0));
        scheme.balance = Some(20_000_000);
        scheme.synced = true;
        scheme.check_invariants();
        (account, scheme)
    }

    /// A successful Payment of a million drops from `from` to `to`, with one
    /// memo of `memo_type` naming `node`.
    fn payment(from: AccountId, to: AccountId, memo_type: &[u8], node: &str) -> TxRecord {
        TxRecord {
            hash: [1; 32],
            ledger_index: 10,
            index: 0,
            validated: true,
            account: from,
            is_payment: true,
            destination: Some(to),
            destination_tag: None,
            sequence: 1,
            fee: 10,
            result: "tesSUCCESS".into(),
            delivered: Some(1_000_000),
            delivered_token: None,
            memo: Some((memo_type.to_vec(), node.as_bytes().to_vec())),
            changes: alloc::vec![BalanceChange {
                account: to,
                before: 20_000_000,
                after: 21_000_000,
                owner_count: None,
            }],
            lines: Vec::new(),
        }
    }

    // spec/areas/xrpl.md#following-the-account: a Payment that carries the
    // memo a settlement carried while the XRPL schemes peered, naming a
    // node, is the owner's deposit like any other, on either network.
    #[test]
    fn a_payment_with_a_settlement_memo_is_a_deposit() {
        fn deposit<N: NativeXrp>(memo_type: &[u8]) {
            let (account, mut scheme) = synced::<N>();
            let record = payment(AccountId([1; 20]), account.account(), memo_type, "node1");
            let mut effects = Vec::new();
            scheme.transaction(&record, None, None, &mut effects);
            scheme.check_invariants();
            assert_eq!(
                effects,
                [Effect::Deposit {
                    amount: units(1_000_000)
                }]
            );
            assert_eq!(scheme.balance(), units(21_000_000));
        }
        let testnet = b"https://dassie.land/settlement/xrpl-testnet-xrp/v1";
        let mainnet = b"https://dassie.land/settlement/xrpl-xrp/v1";
        for memo_type in [&testnet[..], &mainnet[..]] {
            deposit::<TestnetXrp>(memo_type);
            deposit::<MainnetXrp>(memo_type);
        }
    }

    // spec/areas/xrpl.md#owner-books: a scheme's config may say `peers`,
    // which is read and changes nothing; what is the account's, its server,
    // faucet and publishers, is the account's own config, and refused here.
    #[test]
    fn a_config_may_say_only_whether_the_scheme_peers() {
        for accepted in ["", "  ", r#"{"peers": false}"#, r#"{"peers": true}"#, "{}"] {
            assert_eq!(SchemeConfig::parse(accepted), Ok(SchemeConfig), "{accepted}");
        }
        for refused in [
            r#"{"peers": 0}"#,
            r#"{"peers": "no"}"#,
            r#"{"server": "wss://a", "peers": false}"#,
            r#"{"faucet": null}"#,
            "[]",
            "peers",
            "1",
        ] {
            assert_eq!(
                SchemeConfig::parse(refused),
                Err(SchemeError::InvalidConfig),
                "{refused}"
            );
        }
        assert!(SchemeConfig::names_the_account(r#"{"server": "wss://a"}"#));
        assert!(SchemeConfig::names_the_account(r#"{"faucet": null}"#));
        assert!(SchemeConfig::names_the_account(r#"{"publishers": []}"#));
        assert!(!SchemeConfig::names_the_account(r#"{"other": 1}"#));
        let long = alloc::format!(r#"{{"peers":{}}}"#, " ".repeat(crate::account::MAX_CONFIG));
        assert_eq!(SchemeConfig::parse(&long), Err(SchemeError::InvalidConfig));
    }

    // spec/areas/xrpl.md#owner-books: every XRP scheme is an owner book: it
    // says it peers with no one, gives no peering info, refuses every step
    // of a peering and every settlement as unsupported, and cancels any
    // settlement a restart left pending.
    #[test]
    fn an_xrp_scheme_peers_with_no_one() {
        fn refuses<N: NativeXrp>() {
            let (_, mut scheme) = synced::<N>();
            assert!(!scheme.peers());
            let peer = Name::new("node1").unwrap();
            let mut info = alloc::vec![1];
            scheme.peering_info(&mut info);
            assert!(info.is_empty());
            let address = [1; 20];
            let mut out = Vec::new();
            assert_eq!(
                scheme.create_peering_request(peer, &address, &mut out),
                Err(SchemeError::Unsupported)
            );
            assert_eq!(
                scheme.accept_peering_request(peer, &address),
                Err(SchemeError::Unsupported)
            );
            assert_eq!(
                scheme.finalize_peering_request(peer, &address),
                Err(SchemeError::Unsupported)
            );
            let mut rng = dassie_kit::sim::SeededRng::new(1);
            assert_eq!(
                scheme
                    .prepare_settlement(&mut rng, None, peer, units(1), &address)
                    .err(),
                Some(SchemeError::Unsupported)
            );
            let mut effects = Vec::new();
            assert_eq!(
                scheme.handle_settlement(peer, units(1), &[], &address, &mut effects),
                Err(SchemeError::Unsupported)
            );
            let settlement = hash_name(&[2; 32]);
            scheme.reconcile(None, &[(settlement, units(1))], &mut effects);
            assert_eq!(effects, [Effect::Cancel { settlement }]);
            scheme.check_invariants();
        }
        refuses::<TestnetXrp>();
        refuses::<MainnetXrp>();
    }
}

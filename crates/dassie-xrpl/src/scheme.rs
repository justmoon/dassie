//! The XRP scheme (spec/areas/xrpl.md), sans-IO: `xrpl-testnet` on the
//! testnet ([`XrplScheme`]), `xrpl` on mainnet ([`XrplMainnetScheme`]).
//!
//! It settles in the XRP of the node's first XRPL account on its network
//! ([`XrplAccount`]), which connects, signs and follows the ledger for it.
//! Each network's XRP it settles in is an [`Xrp`]: its scheme's ID, its
//! memo type, its ledger and its network.
//! Every validated transaction the account tells it of is booked once, in
//! ledger order, from a cursor kept with the books:
//!
//! - one of our settlements: posted when it succeeded, voided when it
//!   failed, its fee booked either way;
//! - a Payment from a peer's known address, with our memo naming the peer:
//!   a settlement from it, by the XRP it delivered;
//! - a Payment of at least one largest packet with our memo naming a node
//!   that is not our peer at that address: an entry payment, held off the
//!   books until the node peers from that address, which books it as its
//!   settlement, or for a while, after which it is the owner's deposit
//!   ([`crate::entry`], spec/decisions/pay-to-enter.md);
//! - a transaction another scheme on the account signed, such as an RLUSD
//!   settlement or a `TrustSet`: its fee, booked here as a ledger fee, since
//!   this is the ledger whose scheme settles in the account's XRP
//!   (spec/decisions/external-assets.md);
//! - anything else that adds XRP: a deposit; that takes it: a withdrawal.
//!
//! After each, what the books hold must be the account's balance less its
//! reserve; any difference (a reserve change, a balance below the reserve)
//! is reconciled through `equity/suspense`, the part a change of the
//! reserve explains as the reserve's. The reserve itself is never booked,
//! so it is never spent (ACC-19). Settlements are whole drops
//! (ACC-21), failures are checked (ACC-22), and an account created by a
//! payment is a deposit like any other (ACC-20).
//!
//! It settles only on its account's network, which the account checks
//! before it reads anything (spec/decisions/real-money.md). Each network's
//! scheme has a memo type of its own, so a Payment of one is never read as
//! a settlement of the other (spec/decisions/mainnet-schemes.md): the
//! original's mainnet module, never registered, wrote the testnet's, and
//! would have taken every settlement for a deposit (ACC-13). Mainnet's
//! books on a ledger of the live realm, which runs only in a node of that
//! realm, still refused (spec/decisions/real-money.md).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::any::Any;
use core::marker::PhantomData;

use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
use dassie_settlement::{
    AssetId, Effect, EntryPayment, ExternalAsset, LedgerStatus, MAX_OWNER_MEMO_BYTES, OwnerPayment,
    OwnerPaymentState, PaymentFailure, PreparedSettlement, ReconcileCause, SchemeError,
    SettlementScheme,
};
use rand_core::CryptoRng;
use serde_json::{Value, json};

use crate::account::{MAX_ACCOUNT_DROPS, News, Ours, Resumed, XrplAccount};
use crate::api::{AccountInfo, TxRecord};
use crate::codec::Memo;
use crate::entry::Entries;
use crate::keys::AccountId;
use crate::network::{self, Network};
use crate::{UNITS_PER_DROP, from_hex, hex_upper};

/// The testnet scheme's ID.
pub const SCHEME_ID: &str = "xrpl-testnet";
/// The memo that marks a settlement on the testnet, as the original wrote
/// it (spec/areas/xrpl.md#the-payment-it-signs).
pub const MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrp-testnet/v1";

/// A network's XRP, as the scheme settles in it: what tells one XRP scheme
/// from another. Each is a contract between peers that run its scheme, so
/// none changes but with the peer protocol's version
/// (spec/areas/xrpl.md#the-payment-it-signs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Xrp {
    /// The scheme's ID, which is also the slug of its ledger.
    pub scheme_id: &'static str,
    /// The scheme's ID as a name: its network's XRP scheme, whose books
    /// hold the account's XRP.
    pub scheme: Name,
    /// The memo that marks a settlement in it: its own, so a Payment is
    /// never read as a settlement of another XRPL scheme.
    pub memo_type: &'static [u8],
    /// The ledger the scheme books on, of its network's realm.
    pub ledger: LedgerId,
    /// The network whose XRP it is.
    pub network: &'static Network,
}

/// XRP on the testnet, as `xrpl-testnet` settles in it.
pub const TESTNET_XRP: Xrp = Xrp {
    scheme_id: SCHEME_ID,
    scheme: dassie_ledger::name!(SCHEME_ID),
    memo_type: MEMO_TYPE,
    ledger: LedgerId::XrplTestnetXrp,
    network: &network::TESTNET,
};

/// A network's XRP an [`XrpScheme`] settles in, as a type, so each
/// network's XRP scheme is a type of its own ([`XrplScheme`]).
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

/// The `xrpl-testnet` scheme.
pub type XrplScheme = XrpScheme<TestnetXrp>;

/// The mainnet scheme's ID.
pub const MAINNET_SCHEME_ID: &str = "xrpl";
/// The memo that marks a settlement on mainnet: its own, never the
/// testnet's, which the original wrote on both (ACC-13). A contract between
/// peers, as the testnet's is (spec/areas/xrpl.md#the-payment-it-signs).
pub const MAINNET_MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrp/v1";

/// XRP on mainnet, as `xrpl` settles in it: real money.
pub const MAINNET_XRP: Xrp = Xrp {
    scheme_id: MAINNET_SCHEME_ID,
    scheme: dassie_ledger::name!(MAINNET_SCHEME_ID),
    memo_type: MAINNET_MEMO_TYPE,
    ledger: LedgerId::XrplXrp,
    network: &network::MAINNET,
};

/// XRP on mainnet, as a type.
#[derive(Clone, Copy, Debug)]
pub enum MainnetXrp {}

impl NativeXrp for MainnetXrp {
    const XRP: Xrp = MAINNET_XRP;
}

/// The `xrpl` scheme, on mainnet.
pub type XrplMainnetScheme = XrpScheme<MainnetXrp>;

/// The memo type of the owner's payments out of the account
/// ([`SettlementScheme::pay_owner`]), such as a deposit into a settlement
/// hub, whose memo names the account to open there. Never a settlement's.
pub const OWNER_MEMO_TYPE: &[u8] = b"https://dassie.land/owner-payment/v1";

/// Most peers whose addresses are kept: at least the node's settlement
/// channels open, with core nodes and with downlinks, so every node it
/// holds a channel with keeps its own; an address goes when its channel
/// closes. A table keyed by account, rather than this state,
/// is to hold them (spec/roadmap.md#periphery).
pub const MAX_PEERS: usize = 1536;
/// Most settlements of ours whose outcome is looked for after a restart.
pub(crate) const MAX_RECOVERING: usize = 64;
/// The longest a settlement's outcome goes unasked after a restart, in
/// ledgers: about a quarter of an hour on the testnet. One that no history
/// holds is never found, and is asked less often the longer it is not.
pub(crate) const MAX_RECOVER_WAIT_LEDGERS: u32 = 256;

/// The account `asset` is, if it is one.
pub(crate) fn account_of(asset: &mut dyn ExternalAsset) -> Option<&mut XrplAccount> {
    let any: &mut dyn Any = asset;
    any.downcast_mut()
}

/// Whether a scheme that last saw `write_offs` missed a write-off of
/// `account` while it was not on it (stopped, or removed): the account
/// counts another, and is not telling one now, which the scheme writes off
/// in order. What it held went with the account, and its cursor is on a
/// chain gone, which would hold the account back
/// (spec/areas/xrpl.md#following-the-account).
pub(crate) fn missed_write_off(write_offs: u64, account: &XrplAccount, news: &[News]) -> bool {
    let telling = news
        .iter()
        .any(|news| matches!(news, News::WrittenOff { .. }));
    write_offs != account.write_offs() && !telling
}

/// A settlement of ours a restart left pending, though the account no
/// longer has it in flight: its outcome is looked up by its hash. Only the
/// hash, and whether it is unread, are kept with the books; a restart asks
/// at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Recovering {
    pub(crate) hash: [u8; 32],
    /// What the books hold pending for it, in internal units: what we
    /// signed, as the node hands it at each reconcile. A settlement never
    /// read is booked by it, never by what a server says it delivered.
    pub(crate) amount: Option<u64>,
    /// Whether the scheme never read it: it validated while the scheme was
    /// not on the account, and the history no longer reached back to it
    /// once it was ([`News::Unread`]). What it did is in the balance the
    /// scheme synced to, booked through suspense; otherwise, the scheme
    /// read it before a restart, and booked it then.
    pub(crate) unread: bool,
    /// The validated ledger from which it may be asked again.
    pub(crate) ask_at: u32,
    /// How many ledgers the next lookup waits for the one after it: it
    /// doubles with each, up to [`MAX_RECOVER_WAIT_LEDGERS`].
    pub(crate) wait: u32,
}

impl Recovering {
    pub(crate) const fn new(hash: [u8; 32]) -> Self {
        Self {
            hash,
            amount: None,
            unread: false,
            ask_at: 0,
            wait: 1,
        }
    }
}

/// The scheme that settles in the XRP of `N`'s network.
pub struct XrpScheme<N: NativeXrp> {
    /// The account it settles from.
    account: AccountId,
    /// Our node's ID, as our memos carry it.
    node: Name,
    /// What the books hold on the account, in internal units.
    booked: u64,
    // Kept with the books.
    cursor: Option<(u32, u32)>,
    /// Peers' addresses, with when each was learnt: past [`MAX_PEERS`], the
    /// longest known goes.
    peers: BTreeMap<Name, (AccountId, u64)>,
    next_learnt: u64,
    recovering: Vec<Recovering>,
    /// How many times the account was written off, as the scheme last saw
    /// it ([`XrplAccount::write_offs`]).
    write_offs: u64,
    /// The owner's payment on its way: its hash, and the drops it pays.
    owner_payment: Option<([u8; 32], u64)>,
    /// What nodes not yet peered paid us to enter, off the books.
    entries: Entries,
    // Learnt from the account, as it told the scheme.
    /// The owner's last payment that is no longer on its way, and what
    /// became of it, kept with the books so a restart still answers for it.
    owner_outcome: Option<([u8; 32], OwnerPaymentState)>,
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
    validated: u32,
    /// Told that the account is synced, since it was last written off.
    synced: bool,
    /// The network's XRP, as a type only.
    xrp: PhantomData<fn() -> N>,
}

impl<N: NativeXrp> XrpScheme<N> {
    /// The scheme that settles from `account`, for node `node`, with what
    /// the books last checkpointed and the state they kept.
    ///
    /// # Errors
    ///
    /// If the account is on another network, or the state is invalid, or
    /// looks up a transaction the account has in flight or remembers as
    /// never sent: a state the two never wrote.
    pub fn new(
        account: &XrplAccount,
        node: Name,
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
            account: account.account(),
            node,
            booked: checkpoint.unwrap_or(0),
            cursor: None,
            peers: BTreeMap::new(),
            next_learnt: 0,
            recovering: Vec::new(),
            write_offs: 0,
            owner_payment: None,
            owner_outcome: None,
            entries: Entries::default(),
            balance: None,
            owner_count: 0,
            reserve: (0, 0),
            matched_reserve: None,
            validated: 0,
            synced: false,
            xrp: PhantomData,
        };
        if !state.is_empty() {
            scheme.restore(state).ok_or(SchemeError::InvalidData)?;
        }
        // A settlement is looked up only when the account has it neither in
        // flight nor as never sent, and it never has it so again.
        if scheme
            .recovering
            .iter()
            .any(|r| account.holds_transaction(&r.hash))
        {
            return Err(SchemeError::InvalidData);
        }
        Ok(scheme)
    }

    /// The account it settles from.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.account
    }

    /// Takes entry payments of at least `minimum`, in internal units: one
    /// largest packet (spec/decisions/pay-to-enter.md). Until it is told,
    /// it takes none.
    pub fn take_entries_from(&mut self, minimum: u64) {
        self.entries.take_from(minimum);
    }

    /// What it holds of entry payments, in internal units: in the account,
    /// and never on the books.
    #[must_use]
    pub fn entries_held(&self) -> u64 {
        self.entries.total()
    }

    /// The reserve the account holds back, in drops.
    fn reserve_drops(&self, owner_count: u32) -> u64 {
        let (base, increment) = self.reserve;
        base.saturating_add(increment.saturating_mul(u64::from(owner_count)))
    }

    /// What the books should hold for a balance: what is above the reserve,
    /// in internal units, less the entry payments held.
    fn bookable(&self, balance: u64, owner_count: u32) -> u64 {
        self.above(balance, self.reserve_drops(owner_count))
    }

    /// What the books should hold for a balance above a reserve of
    /// `reserve` drops, in internal units: the entry payments held are not
    /// theirs until their nodes peer or their hold is over.
    fn above(&self, balance: u64, reserve: u64) -> u64 {
        units(balance.saturating_sub(reserve)).saturating_sub(self.entries.total())
    }

    /// Whether the books hold anything of the account.
    const fn holds_anything(&self) -> bool {
        self.cursor.is_some() || self.booked > 0
    }

    /// Takes one piece of news from the account.
    fn tell(&mut self, account: &mut XrplAccount, news: &News, effects: &mut Vec<Effect>) {
        let id = self.id();
        match news {
            News::Synced(info) => self.synced(account, info, effects),
            News::Transaction { record, ours } => {
                let mine = ours.filter(|ours| ours.scheme == id);
                let other = ours.filter(|ours| ours.scheme != id);
                self.transaction(record, mine, other, effects);
            }
            News::Cancelled { scheme, hash, .. } if *scheme == id => {
                // The account cancels only what can never apply: proven
                // absent past its last ledger, its number taken by another,
                // or written off. It moved nothing, not even a fee.
                if self.is_owner_payment(hash) {
                    self.owner_done(OwnerPaymentState::Cancelled, effects);
                } else {
                    effects.push(Effect::Cancel {
                        settlement: hash_name(hash),
                    });
                }
            }
            News::Cancelled { .. } => {}
            News::WrittenOff { restarted } => self.written_off(*restarted, effects),
            News::LedgerClosed { reserve_changed } => {
                if *reserve_changed && self.synced {
                    self.match_balance(effects);
                }
                self.ask_recover(account);
            }
            // What became of it is in the balance the sync matched.
            News::Unread { hash, .. } if self.is_owner_payment(hash) => {
                self.owner_done(OwnerPaymentState::Unknown, effects);
            }
            News::Unread { hash, ours } if ours.scheme == id => self.unread(*hash),
            News::Unread { .. } => {}
            News::Found { hash, record } => self.recovered(*hash, record, effects),
            News::Line { .. } | News::Issuer { .. } | News::Pool { .. } => {}
        }
    }

    /// Up to date as of `info`: the books are made to match the balance.
    /// The first sync books what the account holds above the reserve as
    /// the owner's deposit.
    fn synced(&mut self, account: &mut XrplAccount, info: &AccountInfo, effects: &mut Vec<Effect>) {
        let first = self.cursor.is_none() && self.booked == 0;
        self.balance = Some(info.balance);
        self.owner_count = info.owner_count;
        let target = self.bookable(info.balance, info.owner_count);
        if first && target > 0 {
            effects.push(Effect::Deposit { amount: target });
            self.booked = target;
        }
        self.match_balance(effects);
        self.cursor = Some(self.cursor.map_or((info.ledger_index, u32::MAX), |cursor| {
            cursor.max((info.ledger_index, u32::MAX))
        }));
        self.synced = true;
        self.ask_recover(account);
    }

    /// The account is gone: what the books held is a loss through suspense,
    /// and the scheme starts from nothing.
    fn written_off(&mut self, restarted: bool, effects: &mut Vec<Effect>) {
        // What is looked for is asked again on the new chain.
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
        // What they held went with the account.
        self.entries.clear();
        self.cursor = None;
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
        let target = self.above(balance, reserve);
        let unchanged = self
            .matched_reserve
            .map_or(target, |matched| self.above(balance, matched));
        self.reconcile_to(unchanged, ReconcileCause::Unexplained, effects);
        self.reconcile_to(target, ReconcileCause::Reserve, effects);
        self.matched_reserve = Some(reserve);
    }

    /// Makes the books hold `target`, through suspense, for `cause`.
    fn reconcile_to(&mut self, target: u64, cause: ReconcileCause, effects: &mut Vec<Effect>) {
        if target != self.booked {
            effects.push(Effect::Reconcile {
                amount: target.abs_diff(self.booked),
                gain: target > self.booked,
                cause,
            });
            self.booked = target;
        }
    }

    /// Books one validated transaction, once, in ledger order. One of ours
    /// is settled whenever the ledger shows it, even behind the cursor:
    /// what the balance booked through suspense meanwhile comes back out.
    /// So is the fee of one another scheme on the account signed (`other`),
    /// once the scheme has synced, unless the account tells it again for
    /// the scheme that signed it: this scheme was told it, and booked it,
    /// when the ledger validated it. Before its first sync, a scheme books
    /// no other: the sync books what the account holds then as the
    /// owner's, which a scheme that joins an account other schemes read
    /// from earlier is told after their history.
    fn transaction(
        &mut self,
        record: &TxRecord,
        mine: Option<Ours>,
        other: Option<Ours>,
        effects: &mut Vec<Effect>,
    ) {
        let at = (record.ledger_index, record.index);
        if self.cursor.is_none_or(|cursor| at <= cursor) {
            if let Some(ours) = mine {
                let booked = self.ours(record, ours, effects);
                self.hold(booked, effects);
                self.match_balance(effects);
            } else if other.is_some_and(|other| !other.again) && self.cursor.is_some() {
                let booked = self.gas(record, effects);
                self.hold(booked, effects);
                self.match_balance(effects);
            }
            return;
        }
        self.cursor = Some(at);
        let change = record.change_of(self.account);
        if let Some(change) = change {
            self.balance = Some(change.after);
            if let Some(count) = change.owner_count {
                self.owner_count = count;
            }
        }
        let mut booked = i128::from(self.booked);
        if let Some(ours) = mine {
            booked = self.ours(record, ours, effects);
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
            if let Some(peer) = self.settled_by(record) {
                let amount = units(record.delivered.unwrap_or(0));
                effects.push(Effect::IncomingSettlement {
                    peer,
                    amount,
                    id: Some(hash_name(&record.hash)),
                });
                booked += i128::from(amount);
            } else if self.held_entry(record) {
                // Off the books until its node peers, or its hold is over.
            } else if change.after > change.before {
                let amount = units(change.after - change.before);
                effects.push(Effect::Deposit { amount });
                booked += i128::from(amount);
            } else if change.after < change.before {
                let amount = units(change.before - change.after);
                effects.push(Effect::Withdrawal { amount });
                booked -= i128::from(amount);
            }
        }
        self.hold(booked, effects);
        self.match_balance(effects);
    }

    /// Makes the books hold `booked`. Below zero, they hold nothing, and
    /// what they are short comes from suspense, so that a reconcile after
    /// starts from what the books hold.
    fn hold(&mut self, booked: i128, effects: &mut Vec<Effect>) {
        self.booked = narrow(booked.max(0));
        if booked < 0 {
            effects.push(Effect::Reconcile {
                amount: narrow(-booked),
                gain: true,
                cause: ReconcileCause::Unexplained,
            });
        }
    }

    /// Settles one of our transactions the ledger validated, which paid
    /// `paid` in internal units: posted or voided, and its fee booked either
    /// way, and no longer looked up. Returns what the books then hold.
    fn resolve(&mut self, record: &TxRecord, paid: u64, effects: &mut Vec<Effect>) -> i128 {
        self.recovering.retain(|r| r.hash != record.hash);
        let mut booked = i128::from(self.booked);
        let settlement = hash_name(&record.hash);
        if record.succeeded() {
            effects.push(Effect::Finalize { settlement });
            booked -= i128::from(paid);
        } else {
            effects.push(Effect::Cancel { settlement });
        }
        effects.push(Effect::Fee {
            amount: units(record.fee),
            id: settlement,
        });
        booked - i128::from(units(record.fee))
    }

    /// Whether `hash` is the owner's payment on its way.
    fn is_owner_payment(&self, hash: &[u8; 32]) -> bool {
        self.owner_payment.is_some_and(|(owner, _)| owner == *hash)
    }

    /// The owner's payment on its way is no longer: what became of it,
    /// which posts the node's hold on the owner's funds if it applied, and
    /// voids it otherwise. One whose outcome the scheme never read is in
    /// the balance it synced to.
    fn owner_done(&mut self, outcome: OwnerPaymentState, effects: &mut Vec<Effect>) {
        if let Some((hash, _)) = self.owner_payment.take() {
            self.owner_outcome = Some((hash, outcome));
            effects.push(Effect::OwnerPaid {
                id: hash_name(&hash),
                applied: outcome == OwnerPaymentState::Applied,
            });
        }
    }

    /// Books one of our transactions the ledger validated: the owner's
    /// payment on its way, the owner's last one read again, or a
    /// settlement. Returns what the books then hold.
    fn ours(&mut self, record: &TxRecord, ours: Ours, effects: &mut Vec<Effect>) -> i128 {
        if self.is_owner_payment(&record.hash) {
            self.owner_paid(record, effects)
        } else if self.was_owner_payment(&record.hash) {
            self.owner_read_again(record, ours.drops, effects)
        } else {
            self.resolve(record, units(ours.drops), effects)
        }
    }

    /// Whether `hash` is the owner's last payment no longer on its way.
    fn was_owner_payment(&self, hash: &[u8; 32]) -> bool {
        self.owner_outcome.is_some_and(|(owner, _)| owner == *hash)
    }

    /// The owner's last payment, read again: never a settlement. One whose
    /// outcome the scheme had given up on as unknown, its hold voided, is
    /// booked now as the owner's withdrawal of the `drops` it paid when it
    /// succeeded, with its fee either way, and what the balance booked
    /// through suspense meanwhile comes back out; one already booked books
    /// nothing again. Returns what the books then hold.
    fn owner_read_again(
        &mut self,
        record: &TxRecord,
        drops: u64,
        effects: &mut Vec<Effect>,
    ) -> i128 {
        let mut booked = i128::from(self.booked);
        let Some((hash, OwnerPaymentState::Unknown)) = self.owner_outcome else {
            return booked;
        };
        if record.succeeded() {
            let amount = units(drops);
            effects.push(Effect::Withdrawal { amount });
            booked -= i128::from(amount);
            self.owner_outcome = Some((hash, OwnerPaymentState::Applied));
        } else {
            self.owner_outcome = Some((hash, OwnerPaymentState::Failed(failure_of(record))));
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
        let mut booked = i128::from(self.booked);
        let Some((_, drops)) = self.owner_payment else {
            return booked;
        };
        if record.succeeded() {
            booked -= i128::from(units(drops));
            self.owner_done(OwnerPaymentState::Applied, effects);
        } else {
            self.owner_done(OwnerPaymentState::Failed(failure_of(record)), effects);
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
        i128::from(self.booked) - i128::from(fee)
    }

    /// A settlement of ours that validated while the scheme was not on the
    /// account, and that it did not read again: it is booked as one never
    /// read once found. One not looked up (past [`MAX_RECOVERING`]) stays
    /// pending, as any is.
    fn unread(&mut self, hash: [u8; 32]) {
        if let Some(recovering) = self.recovering.iter_mut().find(|r| r.hash == hash) {
            recovering.unread = true;
        }
    }

    /// The outcome of a settlement of ours that a restart left pending,
    /// posted or voided now (spec/areas/xrpl.md#state-and-restarts). `hash`
    /// is what was looked up. One the scheme read before the restart, it
    /// booked then, with its fee and what it did to the balance. One it
    /// never read is booked now as one of ours read behind the cursor is:
    /// by what the books hold pending for it, which is what we signed, with
    /// its fee, and what the balance booked through suspense at the sync
    /// comes back out. The account checked the fee before it told it.
    fn recovered(&mut self, hash: [u8; 32], record: &TxRecord, effects: &mut Vec<Effect>) {
        let Some(position) = self.recovering.iter().position(|r| r.hash == hash) else {
            return;
        };
        let recovering = self.recovering.remove(position);
        let settlement = hash_name(&hash);
        if let (true, Some(amount)) = (recovering.unread, recovering.amount) {
            let booked = self.resolve(record, amount, effects);
            self.hold(booked, effects);
            self.match_balance(effects);
            return;
        }
        effects.push(if record.succeeded() {
            Effect::Finalize { settlement }
        } else {
            Effect::Cancel { settlement }
        });
    }

    /// Asks for the outcome of a restart's settlements not asked for yet,
    /// as many at a time as the account looks up. Each is asked again only
    /// once its wait is over, and waits twice as long after each lookup,
    /// so one never found is not asked at every ledger.
    /// Only once synced: what the scheme did not read again, the account
    /// tells with the sync, before any lookup can be answered.
    fn ask_recover(&mut self, account: &mut XrplAccount) {
        if !self.synced {
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

    /// The peer a transaction settles from: a successful Payment of XRP to
    /// us with our memo, naming a peer whose known address sent it.
    fn settled_by(&self, record: &TxRecord) -> Option<Name> {
        let peer = self.named_by(record)?;
        (self.peers.get(&peer).map(|(address, _)| *address) == Some(record.account)).then_some(peer)
    }

    /// The node a successful Payment of XRP to us names in our memo, other
    /// than ours: whoever sent it.
    fn named_by(&self, record: &TxRecord) -> Option<Name> {
        if !(record.succeeded() && record.is_payment && record.destination == Some(self.account)) {
            return None;
        }
        record.delivered?;
        let (memo_type, memo_data) = record.memo.as_ref()?;
        if memo_type.as_slice() != N::XRP.memo_type {
            return None;
        }
        let node = Name::new(core::str::from_utf8(memo_data).ok()?).ok()?;
        (node != self.node).then_some(node)
    }

    /// Holds `record` as an entry payment, if it is one the table takes: a
    /// Payment with our memo naming a node not our peer at the address it
    /// came from, of at least one largest packet
    /// (spec/decisions/pay-to-enter.md). Returns whether it does.
    fn held_entry(&mut self, record: &TxRecord) -> bool {
        let Some(node) = self.named_by(record) else {
            return false;
        };
        let amount = units(record.delivered.unwrap_or(0));
        // Its hold starts when the scheme reads it, as after a restart,
        // so its node has the whole of it to ask.
        let read = self.validated.max(record.ledger_index);
        self.entries
            .hold(node, record.account, amount, record.hash, read)
    }

    /// Books the entry payments whose hold is over as the owner's
    /// deposits.
    fn expire_entries(&mut self, effects: &mut Vec<Effect>) {
        for entry in self.entries.expire(self.validated) {
            effects.push(Effect::Deposit {
                amount: entry.amount,
            });
            self.booked = self.booked.saturating_add(entry.amount);
        }
    }

    /// Keeps a peer's address; past [`MAX_PEERS`], the one learnt longest
    /// ago goes. Only peerings made are learnt, each with a settlement
    /// channel, whose address goes when the channel closes, and a node has
    /// at most [`MAX_PEERS`] channels open: no peer's address goes for
    /// another's.
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
        self.next_learnt = self.next_learnt.wrapping_add(1);
        self.peers.insert(peer, (address, self.next_learnt));
    }
}

/// What is kept with the books.
impl<N: NativeXrp> XrpScheme<N> {
    /// A number that changes whenever what [`state`](SettlementScheme::state)
    /// keeps does, cheaply: the node writes the state only then.
    fn fingerprint(&self) -> u64 {
        let mut mix: u64 = 0xcbf2_9ce4_8422_2325;
        let mut add = |value: u64| {
            mix ^= value;
            mix = mix.wrapping_mul(0x0100_0000_01b3);
        };
        let (ledger, index) = self.cursor.unwrap_or((u32::MAX, u32::MAX));
        add(u64::from(ledger) << 32 | u64::from(index));
        add(self.next_learnt);
        for Recovering { hash, unread, .. } in &self.recovering {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]) | u64::from(*unread) << 16);
        }
        add(self.recovering.len() as u64);
        add(self.write_offs);
        if let Some((hash, drops)) = self.owner_payment {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]));
            add(drops);
        }
        if let Some((hash, outcome)) = self.owner_outcome {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]));
            add(outcome_code(outcome));
        }
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
        json!({
            "cursor": self.cursor.map(|(ledger, index)| json!([ledger, index])),
            "peers": peers,
            "recovering": recovering,
            "write_offs": self.write_offs,
            "unread": unread_value(&self.recovering),
            "owner_payment": self
                .owner_payment
                .map(|(hash, drops)| json!([hex_upper(&hash), drops])),
            "owner_outcome": self
                .owner_outcome
                .map(|(hash, outcome)| json!([hex_upper(&hash), outcome_code(outcome)])),
            "entries": self.entries.value(),
        })
    }

    fn restore(&mut self, state: &[u8]) -> Option<()> {
        let value: Value = serde_json::from_slice(state).ok()?;
        let object = value.as_object()?;
        // What the account keeps is not the scheme's: a state that still
        // holds it was never split from the account's, and is refused
        // rather than losing what was in flight.
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
        // More than the scheme keeps is a state it did not write: refused,
        // rather than orphaning a settlement by dropping it.
        let peers = value.get("peers")?.as_object()?;
        let recovering = value
            .get("recovering")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        if peers.len() > MAX_PEERS || recovering.len() > MAX_RECOVERING {
            return None;
        }
        for (peer, address) in peers {
            self.learn(Name::new(peer).ok()?, AccountId::parse(address.as_str()?)?);
        }
        for hash in recovering {
            let hash: [u8; 32] = from_hex(hash.as_str()?)?.try_into().ok()?;
            // Each settlement once.
            if self.recovering.iter().any(|r| r.hash == hash) {
                return None;
            }
            self.recovering.push(Recovering::new(hash));
        }
        restore_unread(&value, &mut self.recovering)?;
        self.owner_payment = restore_owner_payment(&value)?;
        self.owner_outcome = restore_owner_outcome(&value)?;
        self.entries.restore(&value)?;
        // Entry payments are read past the first sync only.
        if self.cursor.is_none() && self.entries.total() > 0 {
            return None;
        }
        Some(())
    }
}

/// The XRP Ledger's result of a payment to an account that does not exist,
/// of less than the reserve that would create it.
const TOO_LITTLE_RESULT: &str = "tecNO_DST_INSUF_XRP";

/// Why the owner's payment `record`, which failed, failed.
fn failure_of(record: &TxRecord) -> PaymentFailure {
    if record.result == TOO_LITTLE_RESULT {
        PaymentFailure::TooLittle
    } else {
        PaymentFailure::Other
    }
}

/// How a state writes what became of an owner's payment. A failure written
/// before failures were told apart reads as one of another kind.
const fn outcome_code(outcome: OwnerPaymentState) -> u64 {
    match outcome {
        OwnerPaymentState::Unknown | OwnerPaymentState::InFlight => 0,
        OwnerPaymentState::Applied => 1,
        OwnerPaymentState::Failed(PaymentFailure::Other) => 2,
        OwnerPaymentState::Failed(PaymentFailure::TooLittle) => 3,
        OwnerPaymentState::Cancelled => 4,
    }
}

/// The owner's last payment no longer on its way, and what became of it.
fn restore_owner_outcome(state: &Value) -> Option<Option<([u8; 32], OwnerPaymentState)>> {
    match state.get("owner_outcome") {
        None | Some(Value::Null) => Some(None),
        Some(outcome) => {
            let hash = from_hex(outcome.get(0)?.as_str()?)?.try_into().ok()?;
            let outcome = match outcome.get(1)?.as_u64()? {
                0 => OwnerPaymentState::Unknown,
                1 => OwnerPaymentState::Applied,
                2 => OwnerPaymentState::Failed(PaymentFailure::Other),
                3 => OwnerPaymentState::Failed(PaymentFailure::TooLittle),
                4 => OwnerPaymentState::Cancelled,
                _ => return None,
            };
            Some(Some((hash, outcome)))
        }
    }
}

/// The owner's payment on its way, as a scheme's state keeps it: written
/// only since the owner pays out of the account.
fn restore_owner_payment(state: &Value) -> Option<Option<([u8; 32], u64)>> {
    match state.get("owner_payment") {
        None | Some(Value::Null) => Some(None),
        Some(payment) => {
            let hash = from_hex(payment.get(0)?.as_str()?)?.try_into().ok()?;
            let drops = payment
                .get(1)?
                .as_u64()
                .filter(|drops| *drops <= MAX_ACCOUNT_DROPS)?;
            Some(Some((hash, drops)))
        }
    }
}

/// Looks up settlement `hash`, pending in the books for `amount`, unless it
/// is already, or [`MAX_RECOVERING`] are: one restored from the state is
/// told its amount.
pub(crate) fn recover(recovering: &mut Vec<Recovering>, hash: [u8; 32], amount: u64) {
    assert!(
        amount <= MAX_TOTAL,
        "the books hold at most a total pending"
    );
    if let Some(known) = recovering.iter_mut().find(|r| r.hash == hash) {
        known.amount = Some(amount);
    } else if recovering.len() < MAX_RECOVERING {
        let mut new = Recovering::new(hash);
        new.amount = Some(amount);
        recovering.push(new);
    }
}

/// Which of the settlements looked up the scheme never read, as its state
/// keeps them.
pub(crate) fn unread_value(recovering: &[Recovering]) -> Value {
    recovering
        .iter()
        .filter(|r| r.unread)
        .map(|r| Value::from(hex_upper(&r.hash)))
        .collect()
}

/// Marks the settlements a state keeps as unread: each one looked up, and
/// once. A state written before the scheme kept them has none.
pub(crate) fn restore_unread(state: &Value, recovering: &mut [Recovering]) -> Option<()> {
    let unread = match state.get("unread") {
        None => return Some(()),
        Some(unread) => unread.as_array()?,
    };
    for hash in unread {
        let hash: [u8; 32] = from_hex(hash.as_str()?)?.try_into().ok()?;
        let recovering = recovering.iter_mut().find(|r| r.hash == hash)?;
        if recovering.unread {
            return None;
        }
        recovering.unread = true;
    }
    Some(())
}

/// The count of write-offs a scheme's state keeps: written only since the
/// account counts them, before which none was missed.
pub(crate) fn restore_write_offs(state: &Value) -> Option<u64> {
    match state.get("write_offs") {
        None => Some(0),
        Some(count) => count.as_u64(),
    }
}

/// Drops in internal units. Only amounts checked against
/// [`MAX_ACCOUNT_DROPS`] come here.
pub(crate) fn units(drops: u64) -> u64 {
    assert!(drops <= MAX_ACCOUNT_DROPS, "drops past what the books hold");
    drops * UNITS_PER_DROP
}

/// What the books hold, or a difference from it. Every amount booked is
/// within [`MAX_TOTAL`], and so is what the books held before it.
pub(crate) fn narrow(booked: i128) -> u64 {
    let narrow = u64::try_from(booked);
    assert!(narrow.is_ok(), "booked past what a u64 holds");
    narrow.unwrap_or_default()
}

/// A transaction hash as a settlement ID: uppercase hex, 64 characters.
pub(crate) fn hash_name(hash: &[u8; 32]) -> Name {
    Name::hex(hash, true)
}

/// A settlement ID as the transaction hash it is, if it is one.
pub(crate) fn name_hash(settlement: Name) -> Option<[u8; 32]> {
    from_hex(settlement.as_str())?.try_into().ok()
}

/// Peering info: the address, as the original's OER wrote it (IA5String,
/// 25 to 35 characters, after a length byte).
pub(crate) fn address_bytes(account: AccountId, out: &mut Vec<u8>) {
    let address = account.to_address();
    out.clear();
    out.push(u8::try_from(address.len()).unwrap_or(0));
    out.extend_from_slice(address.as_bytes());
}

pub(crate) fn parse_address(bytes: &[u8]) -> Result<AccountId, SchemeError> {
    let (&length, rest) = bytes.split_first().ok_or(SchemeError::InvalidData)?;
    if usize::from(length) != rest.len() {
        return Err(SchemeError::InvalidData);
    }
    let text = core::str::from_utf8(rest).map_err(|_| SchemeError::InvalidData)?;
    AccountId::parse(text).ok_or(SchemeError::InvalidData)
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

    fn supported_versions(&self) -> &'static [u16] {
        &[1]
    }

    fn balance(&self) -> u64 {
        self.booked
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
        self.cursor.is_some()
    }

    fn unit(&self) -> u64 {
        UNITS_PER_DROP
    }

    fn reserve(&self) -> u64 {
        self.reserve_drops(self.owner_count)
            .saturating_mul(UNITS_PER_DROP)
    }

    fn state(&self) -> Vec<u8> {
        serde_json::to_vec(&self.state_value()).unwrap_or_default()
    }

    fn state_version(&self) -> u64 {
        self.fingerprint()
    }

    fn on_asset(&mut self, _now_ms: u64, asset: &mut dyn ExternalAsset, effects: &mut Vec<Effect>) {
        let account = account_of(asset);
        assert!(
            account.is_some(),
            "the scheme settles through an XRPL account"
        );
        let Some(account) = account else {
            return;
        };
        assert_eq!(account.account(), self.account, "the scheme's own account");
        self.validated = account.validated();
        let news = account.news().to_vec();
        if missed_write_off(self.write_offs, account, &news) {
            self.written_off(true, effects);
        }
        self.write_offs = account.write_offs();
        // The reserve the books are matched to is the one the account had
        // when it last told something: a reserve it read meanwhile, before
        // it is synced again, is matched when it is.
        if !news.is_empty() {
            self.reserve = account.reserve_params();
        }
        for item in &news {
            self.tell(account, item, effects);
        }
        if self.cursor.is_some() {
            self.expire_entries(effects);
        }
        // One the account keeps no more, and told nothing of, was lost
        // with its state: what it did is in the balance. One it keeps for
        // the scheme, validated while the scheme was away, it tells again.
        if let Some((hash, _)) = self.owner_payment
            && !account.keeps_transaction(&hash)
        {
            self.owner_done(OwnerPaymentState::Unknown, effects);
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
        // The responder's address, checked now and learnt once the peering
        // is made (`on_peered`); the original ignored it.
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
        // The state is the peer's address, as accept or finalize returned
        // it: learnt only now, so attempts never take a peer's place.
        let address = parse_address(state);
        assert!(address.is_ok(), "a peer state the scheme did not return");
        if let Ok(address) = address {
            self.learn(peer, address);
        }
    }

    /// A Payment from its address is no settlement any more: it was
    /// followed past every one the peer made before the close, and is the
    /// owner's deposit if one comes (spec/decisions/closing-channels.md),
    /// once held as an entry payment if it is one.
    fn on_closed(&mut self, peer: Name) {
        self.peers.remove(&peer);
    }

    /// The address the request names is where `peer` settles from.
    fn entry(&self, peer: Name, data: &[u8]) -> EntryPayment {
        self.entries.entry(peer, parse_address(data).ok())
    }

    /// Books what it held from `peer`, from the address it peered with, as
    /// its settlements: the books hold it from now on.
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
    }

    /// Unavailable unless the account can sign it now
    /// ([`XrplAccount::sign_payment`]). It settles whole drops.
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
        let drops = amount / UNITS_PER_DROP;
        let memo = Memo {
            memo_type: N::XRP.memo_type.to_vec(),
            memo_data: self.node.as_str().as_bytes().to_vec(),
        };
        let hash = account.sign_payment(self.id(), destination, drops, alloc::vec![memo])?;
        Ok(PreparedSettlement {
            id: hash_name(&hash),
            amount: drops * UNITS_PER_DROP,
            // The original's settlement proof: the hash, 32 bytes.
            message: hash.to_vec(),
            peer_state: None,
        })
    }

    fn settlement_message(&self, settlement: Name) -> Vec<u8> {
        // The ID is the hash, which is the message.
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
        // Signed and then lost, or never sent: it can never apply.
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
        // Every settlement the node holds pending is ours. One in flight is
        // submitted again; one never sent is cancelled; one settled before
        // the node could post it (its posting waited for an earlier
        // announcement) is looked for by its hash, and posted or voided by
        // what the ledger says.
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
        // The claim books nothing: the ledger says what arrived, once
        // (spec/areas/xrpl.md#settlement-messages). It must at least be a
        // transaction hash.
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

    /// A Payment of whole drops of XRP, never partial, with the tag and
    /// one memo of [`OWNER_MEMO_TYPE`] if given, executed at once: its
    /// submission goes out after the batch that keeps it with the account's
    /// and the scheme's state (spec/areas/xrpl.md#deposits-and-withdrawals).
    fn pay_owner(
        &mut self,
        asset: Option<&mut dyn ExternalAsset>,
        payment: &OwnerPayment,
    ) -> Result<Name, SchemeError> {
        if self.owner_payment.is_some() {
            return Err(SchemeError::Unavailable);
        }
        let destination = AccountId::parse(&payment.destination).ok_or(SchemeError::InvalidData)?;
        let drops = payment.amount / UNITS_PER_DROP;
        let memo_too_long = payment
            .memo
            .as_ref()
            .is_some_and(|memo| memo.len() > MAX_OWNER_MEMO_BYTES);
        if destination == self.account || drops == 0 || drops > MAX_ACCOUNT_DROPS || memo_too_long {
            return Err(SchemeError::InvalidData);
        }
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        let memos = payment
            .memo
            .iter()
            .map(|data| Memo {
                memo_type: OWNER_MEMO_TYPE.to_vec(),
                memo_data: data.clone(),
            })
            .collect();
        let hash = account.sign_payment_to(self.id(), (destination, payment.tag), drops, memos)?;
        let executed = account.execute(self.id(), hash);
        assert!(executed, "a payment just signed is in flight");
        self.owner_payment = Some((hash, drops));
        Ok(hash_name(&hash))
    }

    fn owner_payment(&self, id: Name) -> OwnerPaymentState {
        let Some(hash) = name_hash(id) else {
            return OwnerPaymentState::Unknown;
        };
        if self.is_owner_payment(&hash) {
            return OwnerPaymentState::InFlight;
        }
        match self.owner_outcome {
            Some((done, outcome)) if done == hash => outcome,
            _ => OwnerPaymentState::Unknown,
        }
    }

    fn check_invariants(&self) {
        assert!(self.peers.len() <= MAX_PEERS);
        self.entries.check_invariants();
        assert!(
            self.cursor.is_some() || self.entries.total() == 0,
            "entry payments are read past the first sync"
        );
        assert!(
            self.owner_outcome
                .is_none_or(|(_, outcome)| outcome != OwnerPaymentState::InFlight),
            "the last owner's payment done is no longer on its way"
        );
        assert!(self.recovering.len() <= MAX_RECOVERING);
        let mut seen = BTreeSet::new();
        assert!(
            self.recovering.iter().all(|r| seen.insert(r.hash)),
            "each settlement looked up once"
        );
        for recovering in &self.recovering {
            assert!((1..=MAX_RECOVER_WAIT_LEDGERS).contains(&recovering.wait));
            assert!(recovering.amount.is_none_or(|amount| amount <= MAX_TOTAL));
            assert!(
                recovering.ask_at <= self.validated.saturating_add(MAX_RECOVER_WAIT_LEDGERS),
                "a lookup waits at most its longest wait"
            );
        }
        assert!(self.booked <= MAX_TOTAL, "the books hold at most a total");
        if self.synced {
            assert!(self.cursor.is_some(), "a synced scheme has a cursor");
            // The books hold what is above the reserve, once the scheme
            // knows the balance.
            if let Some(balance) = self.balance {
                assert_eq!(
                    self.booked,
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

    /// A successful Payment of XRP from `from` to `to`, with one memo of
    /// `memo_type` naming `peer`.
    fn payment(from: AccountId, to: AccountId, memo_type: &[u8], peer: Name) -> TxRecord {
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
            memo: Some((memo_type.to_vec(), peer.as_str().as_bytes().to_vec())),
            changes: Vec::new(),
            lines: Vec::new(),
        }
    }

    /// Whether the scheme of `N` takes a Payment from its peer's address,
    /// naming the peer, with `memo_type` for the peer's settlement.
    fn settles<N: NativeXrp>(memo_type: &[u8]) -> bool {
        let network = N::XRP.network;
        let account =
            XrplAccount::new(network.first(), &[network.xrp_scheme], "", &[7; 32], &[]).unwrap();
        let node = Name::new("node0").unwrap();
        let mut scheme = XrpScheme::<N>::new(&account, node, None, &[]).unwrap();
        let (peer, address) = (Name::new("node1").unwrap(), AccountId([1; 20]));
        scheme.learn(peer, address);
        scheme.check_invariants();
        let record = payment(address, account.account(), memo_type, peer);
        scheme.settled_by(&record) == Some(peer)
    }

    // spec/decisions/mainnet-schemes.md: each network's XRP scheme reads
    // only its own memo type, so a Payment with the other network's, from
    // a peer's address and naming the peer, is never its settlement but the
    // owner's deposit; the original wrote the testnet's on both (ACC-13).
    #[test]
    fn a_settlement_of_the_other_network_is_a_deposit() {
        assert!(settles::<TestnetXrp>(MEMO_TYPE));
        assert!(!settles::<TestnetXrp>(MAINNET_MEMO_TYPE));
        assert!(settles::<MainnetXrp>(MAINNET_MEMO_TYPE));
        assert!(!settles::<MainnetXrp>(MEMO_TYPE));
        assert!(!settles::<MainnetXrp>(OWNER_MEMO_TYPE));
    }
}

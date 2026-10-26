//! The XRP scheme (spec/areas/xrpl.md), sans-IO: `xrpl-testnet-xrp` on the
//! testnet ([`XrplScheme`]), `xrpl-xrp` on mainnet ([`XrplMainnetScheme`]).
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
//! so it is never spent. Settlements are whole drops,
//! failures are checked, and an account created by a
//! payment is a deposit like any other.
//!
//! It settles only on its account's network, which the account checks
//! before it reads anything (spec/decisions/real-money.md). Each network's
//! scheme has a memo type of its own, so a Payment of one is never read as
//! a settlement of the other (spec/decisions/mainnet-schemes.md): the
//! original's mainnet module, never registered, wrote the testnet's, and
//! would have taken every settlement for a deposit. Mainnet's
//! books on a ledger of the live realm, which runs only in a node of that
//! realm, still refused (spec/decisions/real-money.md).

use alloc::vec::Vec;
use core::any::Any;
use core::marker::PhantomData;

use dassie_ledger::{LedgerId, Name};
use dassie_settlement::{
    AssetId, Effect, EntryPayment, ExternalAsset, LedgerStatus, OwnerPayment, OwnerPaymentState,
    PreparedSettlement, ReconcileCause, SchemeError, SettlementScheme,
};
use rand_core::CryptoRng;
use serde_json::Value;

use crate::account::{MAX_ACCOUNT_DROPS, News, Ours, XrplAccount};
use crate::api::{AccountInfo, TxRecord};
use crate::codec::Memo;
use crate::follower::{Follower, check_claim, failure_of, hash_name, narrow};
use crate::kept::{In, Out};
use crate::keys::AccountId;
use crate::network::{self, Network};
use crate::{UNITS_PER_DROP, from_hex};

/// The testnet scheme's ID: the ledger, its network and its asset
/// (spec/decisions/scheme-names.md).
pub const SCHEME_ID: &str = "xrpl-testnet-xrp";
/// The memo that marks a settlement on the testnet: the scheme's ID, in
/// the URL every XRPL scheme's memo type is
/// (spec/areas/xrpl.md#the-payment-it-signs).
pub const MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrpl-testnet-xrp/v1";

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

/// XRP on the testnet, as `xrpl-testnet-xrp` settles in it.
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

/// The `xrpl-testnet-xrp` scheme.
pub type XrplScheme = XrpScheme<TestnetXrp>;

/// The mainnet scheme's ID: the ledger and its asset, mainnet being the
/// network a name leaves out (spec/decisions/scheme-names.md).
pub const MAINNET_SCHEME_ID: &str = "xrpl-xrp";
/// The memo that marks a settlement on mainnet: its own, never the
/// testnet's, which the original wrote on both. A contract between
/// peers, as the testnet's is (spec/areas/xrpl.md#the-payment-it-signs).
pub const MAINNET_MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrpl-xrp/v1";

/// XRP on mainnet, as `xrpl-xrp` settles in it: real money.
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

/// The `xrpl-xrp` scheme, on mainnet.
pub type XrplMainnetScheme = XrpScheme<MainnetXrp>;

/// The memo type of the owner's payments out of the account
/// ([`SettlementScheme::pay_owner`]), such as a deposit into a settlement
/// hub, whose memo names the account to open there. Never a settlement's.
pub const OWNER_MEMO_TYPE: &[u8] = b"https://dassie.land/owner-payment/v1";

/// What an XRPL scheme's config says: whether the node peers on the
/// scheme (spec/areas/xrpl.md#owner-books). The account it settles
/// through has a config of its own ([`crate::account::Config`]), set apart
/// from any scheme's (spec/areas/settlement.md#external-assets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemeConfig {
    /// Whether the node peers on the scheme: `"peers": false` makes it an
    /// owner book, which peers with nobody
    /// ([`SettlementScheme::peers`]). It peers unless told.
    pub peers: bool,
}

impl SchemeConfig {
    /// The config in `text`: empty, or a JSON object with at most
    /// `peers`, a boolean.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidConfig`] if it is no JSON object, is longer
    /// than a config may be, `peers` is no boolean, or it names anything
    /// else, such as the account's server, which is the account's config.
    pub fn parse(text: &str) -> Result<Self, SchemeError> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Self { peers: true });
        }
        if text.len() > crate::account::MAX_CONFIG {
            return Err(SchemeError::InvalidConfig);
        }
        let value: Value = serde_json::from_str(text).map_err(|_| SchemeError::InvalidConfig)?;
        let Value::Object(mut object) = value else {
            return Err(SchemeError::InvalidConfig);
        };
        let peers = match object.remove("peers") {
            None => true,
            Some(Value::Bool(peers)) => peers,
            Some(_) => return Err(SchemeError::InvalidConfig),
        };
        if !object.is_empty() {
            return Err(SchemeError::InvalidConfig);
        }
        Ok(Self { peers })
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

pub use crate::follower::MAX_PEERS;

/// The account `asset` is, if it is one.
pub(crate) fn account_of(asset: &mut dyn ExternalAsset) -> Option<&mut XrplAccount> {
    let any: &mut dyn Any = asset;
    any.downcast_mut()
}

/// The scheme that settles in the XRP of `N`'s network.
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
            f: Follower::new(account, node, checkpoint, MAX_ACCOUNT_DROPS),
            balance: None,
            owner_count: 0,
            reserve: (0, 0),
            matched_reserve: None,
            synced: false,
            xrp: PhantomData,
        };
        if !state.is_empty() {
            let mut input = In::state(state).ok_or(SchemeError::InvalidData)?;
            scheme.f.read(&mut input).ok_or(SchemeError::InvalidData)?;
            input.finish().ok_or(SchemeError::InvalidData)?;
        }
        scheme.f.fits(account)?;
        Ok(scheme)
    }

    /// The account it settles from.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.f.account
    }

    /// Takes entry payments of at least `minimum`, in internal units: one
    /// largest packet (spec/decisions/pay-to-enter.md). Until it is told,
    /// it takes none.
    pub fn take_entries_from(&mut self, minimum: u64) {
        self.f.entries.take_from(minimum);
    }

    /// Whether the node peers on the scheme, as its config says
    /// ([`SchemeConfig::peers`]): one that does not is an owner book
    /// (spec/areas/xrpl.md#owner-books). It peers until told.
    pub fn set_peers(&mut self, peers: bool) {
        self.f.peering = peers;
    }

    /// What it holds of entry payments, in internal units: in the account,
    /// and never on the books.
    #[must_use]
    pub fn entries_held(&self) -> u64 {
        self.f.entries.total()
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
        units(balance.saturating_sub(reserve)).saturating_sub(self.f.entries.total())
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
                if self.f.is_owner_payment(hash) {
                    self.f.owner_done(OwnerPaymentState::Cancelled, effects);
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
                self.f.ask_recover(account, self.synced);
            }
            // What became of it is in the balance the sync matched.
            News::Unread { hash, .. } if self.f.is_owner_payment(hash) => {
                self.f.owner_done(OwnerPaymentState::Unknown, effects);
            }
            News::Unread { hash, ours } if ours.scheme == id => self.f.unread(*hash),
            News::Unread { .. } => {}
            News::Found { hash, record } => self.recovered(*hash, record, effects),
            News::Line { .. } | News::Issuer { .. } | News::Pool { .. } => {}
        }
    }

    /// Up to date as of `info`: the books are made to match the balance.
    /// The first sync books what the account holds above the reserve as
    /// the owner's deposit.
    fn synced(&mut self, account: &mut XrplAccount, info: &AccountInfo, effects: &mut Vec<Effect>) {
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
        self.f.ask_recover(account, self.synced);
    }

    /// The account is gone: what the books held is a loss through suspense,
    /// and the scheme starts from nothing.
    fn written_off(&mut self, restarted: bool, effects: &mut Vec<Effect>) {
        self.f.written_off(restarted, effects);
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
        self.f
            .reconcile_to(unchanged, ReconcileCause::Unexplained, effects);
        self.f
            .reconcile_to(target, ReconcileCause::Reserve, effects);
        self.matched_reserve = Some(reserve);
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
        if self.f.cursor.is_none_or(|cursor| at <= cursor) {
            if let Some(ours) = mine {
                let booked = self.ours(record, ours, effects);
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
        self.f.hold(booked, effects);
        self.match_balance(effects);
    }

    /// Settles one of our transactions the ledger validated, which paid
    /// `paid` in internal units: posted or voided, and its fee booked either
    /// way, and no longer looked up. Returns what the books then hold.
    fn resolve(&mut self, record: &TxRecord, paid: u64, effects: &mut Vec<Effect>) -> i128 {
        self.f.resolved(&record.hash);
        let mut booked = i128::from(self.f.booked);
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

    /// Books one of our transactions the ledger validated: the owner's
    /// payment on its way, the owner's last one read again, or a
    /// settlement. Returns what the books then hold.
    fn ours(&mut self, record: &TxRecord, ours: Ours, effects: &mut Vec<Effect>) -> i128 {
        if self.f.is_owner_payment(&record.hash) {
            self.owner_paid(record, effects)
        } else if self.f.was_owner_payment(&record.hash) {
            self.owner_read_again(record, ours.drops, effects)
        } else {
            self.resolve(record, units(ours.drops), effects)
        }
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

    /// The outcome of a settlement of ours that a restart left pending,
    /// posted or voided now (spec/areas/xrpl.md#state-and-restarts). `hash`
    /// is what was looked up. One the scheme read before the restart, it
    /// booked then, with its fee and what it did to the balance. One it
    /// never read is booked now as one of ours read behind the cursor is:
    /// by what the books hold pending for it, which is what we signed, with
    /// its fee, and what the balance booked through suspense at the sync
    /// comes back out. The account checked the fee before it told it.
    fn recovered(&mut self, hash: [u8; 32], record: &TxRecord, effects: &mut Vec<Effect>) {
        let Some(recovering) = self.f.found(&hash) else {
            return;
        };
        let settlement = hash_name(&hash);
        if let (true, Some(amount)) = (recovering.unread, recovering.amount) {
            let booked = self.resolve(record, amount, effects);
            self.f.hold(booked, effects);
            self.match_balance(effects);
            return;
        }
        effects.push(if record.succeeded() {
            Effect::Finalize { settlement }
        } else {
            Effect::Cancel { settlement }
        });
    }

    /// The peer a transaction settles from: a successful Payment of XRP to
    /// us with our memo, naming a peer whose known address sent it.
    fn settled_by(&self, record: &TxRecord) -> Option<Name> {
        let peer = self.named_by(record)?;
        self.f.sent_by_peer(record, peer).then_some(peer)
    }

    /// The node a successful Payment of XRP to us names in our memo, other
    /// than ours: whoever sent it.
    fn named_by(&self, record: &TxRecord) -> Option<Name> {
        record.delivered?;
        self.f.named_by(record, N::XRP.memo_type)
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
        self.f.hold_entry(record, node, amount)
    }
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

    fn supported_versions(&self) -> &'static [u16] {
        &[1]
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

    fn peers(&self) -> bool {
        self.f.peering
    }

    fn unit(&self) -> u64 {
        UNITS_PER_DROP
    }

    fn reserve(&self) -> u64 {
        self.reserve_drops(self.owner_count)
            .saturating_mul(UNITS_PER_DROP)
    }

    fn state(&self) -> Vec<u8> {
        let mut out = Out::state();
        self.f.write(&mut out);
        out.0
    }

    fn state_version(&self) -> u64 {
        self.f.fingerprint(0)
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
        assert_eq!(
            account.account(),
            self.f.account,
            "the scheme's own account"
        );
        let news = account.news().to_vec();
        if self.f.catch_up(account, &news) {
            self.written_off(true, effects);
        }
        // The reserve the books are matched to is the one the account had
        // when it last told something: a reserve it read meanwhile, before
        // it is synced again, is matched when it is.
        if !news.is_empty() {
            self.reserve = account.reserve_params();
        }
        for item in &news {
            self.tell(account, item, effects);
        }
        if self.f.expire_entries(effects) {
            self.match_balance(effects);
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

    /// The address the request names is where `peer` settles from.
    fn entry(&self, peer: Name, data: &[u8]) -> EntryPayment {
        self.f.entry(peer, data)
    }

    /// Books what it held from `peer`, from the address it peered with, as
    /// its settlements: the books hold it from now on.
    fn take_entry(&mut self, peer: Name, effects: &mut Vec<Effect>) {
        self.f.take_entry(peer, effects);
        self.match_balance(effects);
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
        let destination = self.f.destination(peer, peer_state)?;
        let account = asset.and_then(account_of).ok_or(SchemeError::Unavailable)?;
        let drops = amount / UNITS_PER_DROP;
        let memo = Memo {
            memo_type: N::XRP.memo_type.to_vec(),
            memo_data: self.f.node.as_str().as_bytes().to_vec(),
        };
        let hash = account.sign_payment(self.id(), destination, drops, alloc::vec![memo])?;
        Ok(PreparedSettlement {
            id: hash_name(&hash),
            amount: drops * UNITS_PER_DROP,
            // The original's settlement proof: the hash, 32 bytes.
            message: hash.to_vec(),
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
        check_claim(data)
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
        scheme.f.learn(peer, address);
        scheme.check_invariants();
        let record = payment(address, account.account(), memo_type, peer);
        scheme.settled_by(&record) == Some(peer)
    }

    // spec/decisions/mainnet-schemes.md: each network's XRP scheme reads
    // only its own memo type, so a Payment with the other network's, from
    // a peer's address and naming the peer, is never its settlement but the
    // owner's deposit; the original wrote the testnet's on both.
    #[test]
    fn a_settlement_of_the_other_network_is_a_deposit() {
        assert!(settles::<TestnetXrp>(MEMO_TYPE));
        assert!(!settles::<TestnetXrp>(MAINNET_MEMO_TYPE));
        assert!(settles::<MainnetXrp>(MAINNET_MEMO_TYPE));
        assert!(!settles::<MainnetXrp>(MEMO_TYPE));
        assert!(!settles::<MainnetXrp>(OWNER_MEMO_TYPE));
    }

    // spec/areas/xrpl.md#owner-books: a scheme's config says only whether
    // it peers; what is the account's, its server, faucet and publishers,
    // is the account's own config, and refused here.
    #[test]
    fn a_config_says_only_whether_the_scheme_peers() {
        let parse = |text: &str| SchemeConfig::parse(text).unwrap();
        assert!(parse("").peers);
        assert!(parse("  ").peers);
        assert!(!parse(r#"{"peers": false}"#).peers);
        assert!(parse(r#"{"peers": true}"#).peers);
        assert!(parse("{}").peers);
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

    // spec/areas/settlement.md#owner-books: an owner book peers with nobody:
    // it says so, and refuses every step of a peering as unsupported.
    #[test]
    fn an_owner_book_refuses_to_peer() {
        let network = TestnetXrp::XRP.network;
        let account =
            XrplAccount::new(network.first(), &[network.xrp_scheme], "", &[7; 32], &[]).unwrap();
        let mut scheme = XrplScheme::new(&account, Name::new("node0").unwrap(), None, &[]).unwrap();
        assert!(scheme.peers());
        scheme.set_peers(false);
        assert!(!scheme.peers());
        let peer = Name::new("node1").unwrap();
        let mut info = Vec::new();
        crate::follower::address_bytes(AccountId([1; 20]), &mut info);
        let mut out = Vec::new();
        assert_eq!(
            scheme.create_peering_request(peer, &info, &mut out),
            Err(SchemeError::Unsupported)
        );
        assert_eq!(
            scheme.accept_peering_request(peer, &info, &mut out),
            Err(SchemeError::Unsupported)
        );
        assert_eq!(
            scheme.finalize_peering_request(peer, &info, &[]),
            Err(SchemeError::Unsupported)
        );
        scheme.check_invariants();
    }
}

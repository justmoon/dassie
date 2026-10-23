//! The `xrpl-testnet` settlement scheme (spec/areas/xrpl.md), sans-IO.
//!
//! It settles in the XRP of the node's first XRPL testnet account
//! ([`XrplAccount`]), which connects, signs and follows the ledger for it.
//! Every validated transaction the account tells it of is booked once, in
//! ledger order, from a cursor kept with the books:
//!
//! - one of our settlements: posted when it succeeded, voided when it
//!   failed, its fee booked either way;
//! - a Payment from a peer's known address, with our memo naming the peer:
//!   a settlement from it, by the XRP it delivered;
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
//! It settles only on the testnet, which the account checks before it
//! reads anything (spec/decisions/real-money.md). The original's mainnet
//! module, never registered, would have taken every settlement for a
//! deposit (ACC-13); a mainnet scheme waits for real money, with a memo
//! type of its own.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::any::Any;

use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
use dassie_settlement::{
    AssetId, Effect, ExternalAsset, LedgerStatus, PreparedSettlement, ReconcileCause, SchemeError,
    SettlementScheme,
};
use rand_core::CryptoRng;
use serde_json::{Value, json};

use crate::account::{self, MAX_ACCOUNT_DROPS, News, Ours, Resumed, XrplAccount};
use crate::api::{AccountInfo, TxRecord};
use crate::codec::Memo;
use crate::keys::AccountId;
use crate::{UNITS_PER_DROP, from_hex, hex_upper};

/// The scheme's ID.
pub const SCHEME_ID: &str = "xrpl-testnet";
/// The memo that marks a settlement, as the original wrote it
/// (spec/areas/xrpl.md#the-payment-it-signs).
pub const MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrp-testnet/v1";

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

/// The `xrpl-testnet` scheme.
pub struct XrplScheme {
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
    validated: u32,
    /// Told that the account is synced, since it was last written off.
    synced: bool,
}

impl XrplScheme {
    /// The scheme that settles from `account`, for node `node`, with what
    /// the books last checkpointed and the state they kept.
    ///
    /// # Errors
    ///
    /// If the state is invalid, or looks up a transaction the account has
    /// in flight or remembers as never sent: a state the two never wrote.
    pub fn new(
        account: &XrplAccount,
        node: Name,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let mut scheme = Self {
            account: account.account(),
            node,
            booked: checkpoint.unwrap_or(0),
            cursor: None,
            peers: BTreeMap::new(),
            next_learnt: 0,
            recovering: Vec::new(),
            write_offs: 0,
            balance: None,
            owner_count: 0,
            reserve: (0, 0),
            matched_reserve: None,
            validated: 0,
            synced: false,
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

    /// The reserve the account holds back, in drops.
    fn reserve_drops(&self, owner_count: u32) -> u64 {
        let (base, increment) = self.reserve;
        base.saturating_add(increment.saturating_mul(u64::from(owner_count)))
    }

    /// What the books should hold for a balance: what is above the reserve,
    /// in internal units.
    fn bookable(&self, balance: u64, owner_count: u32) -> u64 {
        units(balance.saturating_sub(self.reserve_drops(owner_count)))
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
                effects.push(Effect::Cancel {
                    settlement: hash_name(hash),
                });
            }
            News::Cancelled { .. } => {}
            News::WrittenOff { restarted } => self.written_off(*restarted, effects),
            News::LedgerClosed { reserve_changed } => {
                if *reserve_changed && self.synced {
                    self.match_balance(effects);
                }
                self.ask_recover(account);
            }
            News::Unread { hash, ours } if ours.scheme == id => self.unread(*hash),
            News::Unread { .. } => {}
            News::Found { hash, record } => self.recovered(*hash, record, effects),
            News::Line { .. } | News::Issuer { .. } => {}
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
        let target = units(balance.saturating_sub(reserve));
        let unchanged = self
            .matched_reserve
            .map_or(target, |matched| units(balance.saturating_sub(matched)));
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
                let booked = self.resolve(record, units(ours.drops), effects);
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
            booked = self.resolve(record, units(ours.drops), effects);
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
        if !(record.succeeded() && record.is_payment && record.destination == Some(self.account)) {
            return None;
        }
        record.delivered?;
        let (memo_type, memo_data) = record.memo.as_ref()?;
        if memo_type.as_slice() != MEMO_TYPE {
            return None;
        }
        let peer = Name::new(core::str::from_utf8(memo_data).ok()?).ok()?;
        (self.peers.get(&peer).map(|(address, _)| *address) == Some(record.account)).then_some(peer)
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
impl XrplScheme {
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
        restore_unread(&value, &mut self.recovering)
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

impl SettlementScheme for XrplScheme {
    fn id(&self) -> Name {
        dassie_ledger::name!(SCHEME_ID)
    }

    fn ledger(&self) -> LedgerId {
        LedgerId::XrplTestnetXrp
    }

    fn asset(&self) -> Option<AssetId> {
        Some(account::FIRST)
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
    /// owner's deposit if one comes (spec/decisions/closing-channels.md).
    fn on_closed(&mut self, peer: Name) {
        self.peers.remove(&peer);
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
            memo_type: MEMO_TYPE.to_vec(),
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

    fn check_invariants(&self) {
        assert!(self.peers.len() <= MAX_PEERS);
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

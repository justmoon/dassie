//! A model of the XRP Ledger for simulations and tests: accounts with
//! balances, sequences and reserves, Payments of XRP checked as the ledger
//! checks them (signature, sequence, fee, last ledger, funds), ledgers that
//! close on command with real result codes and close times, the metadata,
//! streams and history the node reads, resets, and a faucet. It is the
//! testnet ([`FakeLedger::new`]) or mainnet ([`FakeLedger::mainnet`]),
//! whose network ID it reports, which has no faucet and never resets
//! (spec/decisions/mainnet-schemes.md).
//!
//! Tokens on trust lines ([`tokens`]): lines as `RippleState` entries with
//! their owner reserve, `TrustSet`, token Payments that ripple through the
//! issuer with the ledger's failures, and issuers that mint, freeze, set a
//! transfer fee and claw back. The fake is the only source of dollar tokens
//! on demand: [`FakeLedger::new`] makes DSUSD's testnet issuer, and
//! [`FakeLedger::mainnet`] RLUSD's mainnet issuer, each at its address.
//!
//! AMM pools ([`pools`]): a constant-product pool of XRP and a token with a
//! trading fee, `amm_info`, and Payments with `SendMax` that cross it.
//!
//! Keys (`fake/signers.rs`): an account signs with its master key unless it is
//! disabled, its regular key, or its signer list's signers together, each
//! alone, as of when a transaction applies; destination tags, which an
//! account may require; and `account_objects`.
//!
//! It answers the subset of the rippled WebSocket API in [`crate::api`],
//! at API version 1, as rippled does. It checks what it is sent with the
//! node's own codec, so the codec is pinned by the xrpl.js vectors, not by
//! this.
//!
//! Proofs (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation):
//! each closed ledger has a real header, a transaction tree of every
//! transaction it applied, the fake's own included, with its binary
//! metadata, and a state tree of every object in rippled's binary form,
//! with the skip lists and the fee settings (`fake/chain.rs`,
//! `fake/objects.rs`, `fake/meta.rs`). Validators sign each ledger and a
//! publisher lists them (`fake/validators.rs`); the fake serves
//! `validations`, `tx_proof`, `state_proof` and `manifest` (`fake/proofs.rs`),
//! and any connection can be made to lie ([`lies`]).

mod chain;
pub mod lies;
mod meta;
mod objects;
pub mod pools;
mod proofs;
pub mod shamap;
mod signers;
pub mod tokens;
mod validators;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::amount::IssuedAsset;
use crate::api::{SignerList, account_flags};
use crate::codec::{
    Amount, Currency, Payment, Signing, Transact, Transaction, hash, multi_signed_fee,
};
use crate::keys::AccountId;
use crate::network::{MAINNET, Network, TESTNET};
use crate::{DROPS_PER_XRP, from_hex, hex_upper};

pub use chain::{MAX_KEPT_LEDGERS, MAX_STATE_LEDGERS, MAX_VALIDATED_LEDGERS};

/// How a `tx` answer comes for a transaction a server has not saved with
/// its ledger yet ([`FakeLedger::answer_unsaved`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsaved {
    /// From the server's cache, without its metadata, not validated.
    WithoutMeta,
    /// `txnNotFound`: not in its cache, and not saved.
    NotFound,
}
pub use lies::Lie;
pub use pools::{POOL_ACCOUNT_FLAGS, PoolState, pool_account};
pub use proofs::MAX_STATE_PROOF_KEYS;
pub use tokens::{ExactSum, ripple_state};
pub use validators::{
    DEFAULT_SEED, KeyedSignatures, LIST_SITE, MAINNET_SEED, VALIDATORS, publisher_key,
    with_publishers,
};

use chain::Chain;
use meta::{Applied, Touched};
use objects::{Thread, token_json};
use pools::{Pool, PoolSet};
use tokens::{Clawback, IssuerSet, LineKey};
use validators::Validators;

/// The reference fee, in drops.
pub const FEE_BASE: u64 = 10;
/// The base reserve, in drops: 1 XRP.
pub const RESERVE_BASE: u64 = 1_000_000;
/// The reserve per owned object, in drops.
pub const RESERVE_INC: u64 = 200_000;
/// What the faucet gives, in drops: 1000 XRP.
pub const FAUCET_DROPS: u64 = 1_000_000_000;
/// The first ledger: in the millions, as the testnet's are, so a lookup
/// from ledger 1 spans more than a server searches.
pub const FIRST_LEDGER: u32 = 5_000_000;
/// When the first ledger closed, in seconds since the XRP Ledger's epoch.
pub const FIRST_CLOSE_TIME: u64 = 800_000_000;
/// The same, by a node's clock: milliseconds since 1970. A test whose clock
/// starts here reads the fake's validations as fresh.
pub const FIRST_CLOSE_MS: u64 = (FIRST_CLOSE_TIME + crate::unl::RIPPLE_EPOCH_UNIX_S) * 1_000;
/// The seconds between one ledger's close and the next's.
pub const CLOSE_INTERVAL_S: u64 = 4;
/// The ledger a reset starts the chain again from: far below where it was,
/// as a new genesis is.
pub const RESTART_LEDGER: u32 = 2_000;
/// The most ledgers a `tx` lookup may span, as rippled's `kMaxRange`.
const MAX_TX_RANGE_LEDGERS: u64 = 1_000;
/// The ledgers an issuer's or genesis's own transaction may wait.
const OWN_TX_LEDGERS: u32 = 20;

/// RLUSD's issuer on the XRPL mainnet: the fake ledger's mainnet issues
/// RLUSD at the same address, so the scheme runs on it unchanged.
pub const RLUSD_ISSUER: AccountId = crate::rlusd::ISSUER;
/// RLUSD's currency code: "RLUSD" in 40 hex digits.
pub const RLUSD_CURRENCY: Currency = crate::rlusd::CURRENCY;
/// DSUSD's issuer on the XRPL testnet: the fake ledger issues its test
/// DSUSD at the same address, so the scheme runs on it unchanged.
pub const DSUSD_ISSUER: AccountId = crate::dsusd::ISSUER;
/// DSUSD's currency code: "DSUSD" in 40 hex digits.
pub const DSUSD_CURRENCY: Currency = crate::dsusd::CURRENCY;
/// What an issuer is given from genesis, in drops, to pay its fees.
pub const ISSUER_DROPS: u64 = 1_000 * DROPS_PER_XRP;
/// An issuer's flags, as RLUSD's: lines ripple through it,
/// it may claw back, it requires no authorisation and keeps the right to
/// freeze.
pub const ISSUER_FLAGS: u32 =
    account_flags::DEFAULT_RIPPLE | account_flags::ALLOW_TRUST_LINE_CLAWBACK;

/// A connection to the fake ledger.
pub type ConnectionId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Account {
    balance: u64,
    sequence: u32,
    owner_count: u32,
    /// Its flags ([`account_flags`]).
    flags: u32,
    /// Its transfer rate in billionths; 0 when it charges nothing.
    transfer_rate: u32,
    /// Its regular key, as the account of the key.
    regular_key: Option<AccountId>,
}

impl Account {
    const fn new(balance: u64, sequence: u32) -> Self {
        Self {
            balance,
            sequence,
            owner_count: 0,
            flags: 0,
            transfer_rate: 0,
            regular_key: None,
        }
    }
}

/// A transaction waiting for the next close.
#[derive(Clone, Debug)]
enum Tx {
    /// Of a kind the codec reads: submitted signed, or the fake's own,
    /// such as a deposit or an issuer's mint, signed by none
    /// ([`unsigned`]).
    Ledger(Box<Transaction>),
    /// An issuer's own: a global freeze or a transfer rate.
    IssuerSet(IssuerSet),
    /// An issuer's own.
    Clawback(Clawback),
    /// An issuer's own: makes its token's pool, or sets it.
    PoolSet(PoolSet),
}

impl Tx {
    fn account(&self) -> AccountId {
        match self {
            Self::Ledger(tx) => tx.sender(),
            Self::IssuerSet(tx) => tx.account,
            Self::Clawback(tx) => tx.account,
            Self::PoolSet(tx) => tx.account,
        }
    }

    fn sequence(&self) -> u32 {
        match self {
            Self::Ledger(tx) => Transact::sequence(tx.as_ref()),
            Self::IssuerSet(tx) => tx.sequence,
            Self::Clawback(tx) => tx.sequence,
            Self::PoolSet(tx) => tx.sequence,
        }
    }

    fn last_ledger_sequence(&self) -> Option<u32> {
        match self {
            Self::Ledger(tx) => Transact::last_ledger_sequence(tx.as_ref()),
            Self::IssuerSet(tx) => Some(tx.last_ledger_sequence),
            Self::Clawback(tx) => Some(tx.last_ledger_sequence),
            Self::PoolSet(tx) => Some(tx.last_ledger_sequence),
        }
    }

    fn fee(&self) -> u64 {
        match self {
            Self::Ledger(tx) => Transact::fee(tx.as_ref()),
            Self::IssuerSet(_) | Self::Clawback(_) | Self::PoolSet(_) => FEE_BASE,
        }
    }

    /// Who signed it, if it was signed and submitted.
    fn signing(&self) -> Option<&Signing> {
        match self {
            Self::Ledger(tx) => Some(tx.signing()).filter(|signing| signing.is_signed()),
            Self::IssuerSet(_) | Self::Clawback(_) | Self::PoolSet(_) => None,
        }
    }

    /// Its blob, if it was signed and submitted.
    fn signed_blob(&self) -> Option<Vec<u8>> {
        match self {
            Self::Ledger(tx) => self.signing().map(|_| tx.blob()),
            Self::IssuerSet(_) | Self::Clawback(_) | Self::PoolSet(_) => None,
        }
    }
}

/// Who signs the fake's own transactions: none, as it acts without keys.
const fn unsigned() -> Signing {
    Signing::multi()
}

/// What applying a transaction came to.
struct Outcome {
    code: &'static str,
    /// What a successful Payment delivered.
    delivered: Option<Amount>,
}

impl Outcome {
    const fn code(code: &'static str) -> Self {
        Self {
            code,
            delivered: None,
        }
    }
}

/// The fake ledger. A clone is a server that stops where the ledger was:
/// one behind the others, once they go on.
#[derive(Clone)]
pub struct FakeLedger {
    accounts: BTreeMap<AccountId, Account>,
    /// Trust lines, by their low and high account and currency.
    lines: BTreeMap<LineKey, crate::api::TrustLine>,
    /// The accounts that issue tokens here, which the fake acts as.
    issuers: BTreeSet<AccountId>,
    /// What each issuer's holders hold of each of its currencies, by
    /// what it minted less what came back to it, was clawed back or wiped.
    supply: BTreeMap<IssuedAsset, ExactSum>,
    /// The pools of XRP and a token, by the token.
    pools: BTreeMap<IssuedAsset, Pool>,
    /// Signer lists, by their owner.
    signer_lists: BTreeMap<AccountId, SignerList>,
    /// The last validated ledger.
    ledger: u32,
    /// When it closed, in seconds since the XRP Ledger's epoch.
    close_time: u64,
    /// The first ledger the history holds: before it, `account_tx` and
    /// `tx` know nothing.
    history_from: u32,
    /// The first ledger `account_tx` reaches, at or after `history_from`.
    account_tx_from: u32,
    history: Vec<Applied>,
    queued: Vec<Tx>,
    queued_hashes: BTreeSet<[u8; 32]>,
    accounts_of: BTreeMap<ConnectionId, BTreeSet<AccountId>>,
    ledger_stream: BTreeSet<ConnectionId>,
    /// The genesis account, which pays the faucet's XRP.
    genesis: AccountId,
    /// What the fees burned, in drops.
    burned: u64,
    /// What resets wiped, in drops.
    wiped: u64,
    /// The reserve, which [`set_reserve`](Self::set_reserve) changes.
    reserve: (u64, u64),
    /// The network it is: whether it has a faucet, and whether it resets.
    network: &'static Network,
    /// The network ID `server_state` reports: its network's, unless
    /// [`set_network_id`](Self::set_network_id) changes it.
    network_id: Option<u32>,
    /// Where each transaction is in the history, by its hash.
    history_index: BTreeMap<[u8; 32], usize>,
    /// The transaction that last changed each object, by the object's key.
    threads: BTreeMap<[u8; 32], Thread>,
    /// The ledgers closed, with their trees.
    chain: Chain,
    /// The validators and their publisher.
    validators: Validators,
    /// Who lies how.
    lies: lies::Lies,
    /// The last transactions submitted that never applied, each with its
    /// hash, newest last: what a lying server says applied.
    never_applied: Vec<([u8; 32], Vec<u8>)>,
    /// How many more `tx` answers in binary come as rippled's do between
    /// validating a ledger and saving it, and how.
    unsaved_answers: (u32, Unsaved),
    /// How many times `account_info` asked about each account, whether or
    /// not it exists: a test waits on it to know a client looked again.
    reads: BTreeMap<AccountId, u64>,
}

/// The most transactions remembered as never applied.
const MAX_NEVER_APPLIED: usize = 64;

/// The most accounts whose reads are counted: a dev environment's fake runs
/// for days, asked about any account.
const MAX_COUNTED_READS: usize = 1_024;

/// When the fake's first list of validators expires: as late as the
/// ledger's clock counts. A test that lets a list expire publishes another.
const FIRST_LIST_EXPIRATION: u32 = u32::MAX;

impl Default for FakeLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeLedger {
    /// The testnet: a ledger with only the genesis account and DSUSD's
    /// issuer, with validators from [`DEFAULT_SEED`]. Test RLUSD is not on
    /// it: no scheme settles in it (spec/decisions/issued-assets.md).
    #[must_use]
    pub fn new() -> Self {
        Self::with_seed(DEFAULT_SEED)
    }

    /// The testnet as [`new`](Self::new) makes it, with validators and
    /// their publisher from `seed`: a ledger of another seed shares no key
    /// with it.
    #[must_use]
    pub fn with_seed(seed: [u8; 32]) -> Self {
        Self::on(&TESTNET, seed, DSUSD_ISSUER)
    }

    /// Mainnet: a ledger with only the genesis account and RLUSD's issuer,
    /// which reports mainnet's network ID, has no faucet and never resets.
    /// The testnet's DSUSD is not on it. Its validators come from
    /// [`MAINNET_SEED`], so its UNL shares no key with the testnet's, as
    /// live.
    #[must_use]
    pub fn mainnet() -> Self {
        Self::on(&MAINNET, MAINNET_SEED, RLUSD_ISSUER)
    }

    /// A ledger of `network` with only the genesis account and `issuer`,
    /// with validators and their publisher from `seed`, its first ledger
    /// sealed.
    fn on(network: &'static Network, seed: [u8; 32], issuer: AccountId) -> Self {
        let genesis = AccountId([0xAB; 20]);
        let mut accounts = BTreeMap::new();
        accounts.insert(genesis, Account::new(crate::MAX_DROPS, 1));
        let mut ledger = Self {
            accounts,
            lines: BTreeMap::new(),
            issuers: BTreeSet::new(),
            supply: BTreeMap::new(),
            pools: BTreeMap::new(),
            signer_lists: BTreeMap::new(),
            ledger: FIRST_LEDGER,
            close_time: FIRST_CLOSE_TIME,
            history_from: FIRST_LEDGER,
            account_tx_from: FIRST_LEDGER,
            history: Vec::new(),
            queued: Vec::new(),
            queued_hashes: BTreeSet::new(),
            accounts_of: BTreeMap::new(),
            ledger_stream: BTreeSet::new(),
            genesis,
            burned: 0,
            wiped: 0,
            reserve: (RESERVE_BASE, RESERVE_INC),
            network,
            network_id: Some(network.id),
            history_index: BTreeMap::new(),
            threads: BTreeMap::new(),
            chain: Chain::new(),
            validators: Validators::new(seed, FIRST_LIST_EXPIRATION),
            lies: lies::Lies::default(),
            unsaved_answers: (0, Unsaved::WithoutMeta),
            never_applied: Vec::new(),
            reads: BTreeMap::new(),
        };
        ledger.add_issuer(issuer);
        ledger.seal_first();
        ledger
    }

    /// The network it is.
    #[must_use]
    pub const fn network(&self) -> &'static Network {
        self.network
    }

    /// Makes `issuer` an account the fake acts as, with [`ISSUER_FLAGS`]
    /// and [`ISSUER_DROPS`] from genesis, as of the current ledger. It
    /// stays through resets, as an issuer that came back at its address.
    pub fn add_issuer(&mut self, issuer: AccountId) {
        assert_ne!(issuer, self.genesis, "genesis issues nothing");
        if self.issuers.insert(issuer) && !self.accounts.contains_key(&issuer) {
            let genesis = self
                .accounts
                .entry(self.genesis)
                .or_insert(Account::new(0, 1));
            assert!(genesis.balance >= ISSUER_DROPS, "genesis funds issuers");
            genesis.balance -= ISSUER_DROPS;
            let mut account = Account::new(ISSUER_DROPS, self.ledger);
            account.flags = ISSUER_FLAGS;
            self.accounts.insert(issuer, account);
        }
    }

    /// The last validated ledger.
    #[must_use]
    pub const fn ledger(&self) -> u32 {
        self.ledger
    }

    /// How many transactions wait for the next close, which applies them.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    /// An account's balance in drops, if it exists.
    #[must_use]
    pub fn balance(&self, account: AccountId) -> Option<u64> {
        self.accounts.get(&account).map(|a| a.balance)
    }

    /// The sequence an account uses next, if it exists.
    #[must_use]
    pub fn sequence(&self, account: AccountId) -> Option<u32> {
        self.accounts.get(&account).map(|a| a.sequence)
    }

    /// The objects an account owns, such as trust lines, if it exists.
    #[must_use]
    pub fn owner_count(&self, account: AccountId) -> Option<u32> {
        self.accounts.get(&account).map(|a| a.owner_count)
    }

    /// An account's flags ([`account_flags`]), if it exists.
    #[must_use]
    pub fn account_flags(&self, account: AccountId) -> Option<u32> {
        self.accounts.get(&account).map(|a| a.flags)
    }

    /// All XRP there is, with what fees burned and resets wiped: it never
    /// changes.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.accounts
            .values()
            .map(|a| a.balance)
            .sum::<u64>()
            .saturating_add(self.burned)
            .saturating_add(self.wiped)
    }

    /// What fees burned, in drops.
    #[must_use]
    pub const fn burned(&self) -> u64 {
        self.burned
    }

    /// Forgets the history before the current ledger, as a server that
    /// keeps little does.
    pub fn forget_history(&mut self) {
        self.history_from = self.ledger.saturating_add(1);
        self.account_tx_from = self.history_from;
        self.history.clear();
        self.history_index.clear();
        self.chain.forget_before(self.history_from);
    }

    /// Cuts `account_tx` at the current ledger, while `tx` still finds
    /// what came before, as a server whose account index is behind does.
    pub fn shorten_account_history(&mut self) {
        self.account_tx_from = self.ledger.saturating_add(1);
    }

    /// Changes the reserve, as the network may vote to.
    pub fn set_reserve(&mut self, base: u64, increment: u64) {
        self.reserve = (base, increment);
    }

    /// Makes the next `answers` answers of `tx` in binary come as an honest
    /// rippled's do for a moment after it validated or acquired a ledger
    /// and before it saved it, as `how` says.
    pub fn answer_unsaved(&mut self, answers: u32, how: Unsaved) {
        self.unsaved_answers = (answers, how);
    }

    /// Changes the network ID the server reports, as a server of another
    /// network would; `None` leaves it out.
    pub fn set_network_id(&mut self, id: Option<u32>) {
        self.network_id = id;
    }

    /// Sets when the last ledger closed, in seconds since the XRP Ledger's
    /// epoch, as for a chain that started then: one started later closes
    /// later than every ledger of one before it, as after a reset.
    pub fn set_close_time(&mut self, close_time: u64) {
        self.close_time = close_time;
        self.reseal();
    }

    /// Starts the chain again, as a testnet reset does: every account but
    /// genesis and the issuers, every trust line, signer list and pool, and
    /// all history wiped, from a ledger below the last, which closes later.
    /// Only the testnet resets.
    pub fn reset(&mut self) {
        assert!(self.network.resets(), "only the testnet resets");
        self.ledger = RESTART_LEDGER.min(self.ledger.saturating_sub(1));
        self.close_time = self.close_time.saturating_add(CLOSE_INTERVAL_S);
        self.history_from = self.ledger;
        self.account_tx_from = self.ledger;
        self.history.clear();
        self.history_index.clear();
        self.never_applied.clear();
        self.threads.clear();
        let (genesis, issuers) = (self.genesis, &self.issuers);
        let kept = |id: &AccountId| *id == genesis || issuers.contains(id);
        let wiped: u64 = self
            .accounts
            .iter()
            .filter(|(id, _)| !kept(id))
            .map(|(_, account)| account.balance)
            .sum();
        self.wiped = self.wiped.saturating_add(wiped);
        self.accounts.retain(|id, _| kept(id));
        // Their lines are gone, and with them what their holders held.
        for account in self.accounts.values_mut() {
            account.owner_count = 0;
        }
        self.lines.clear();
        self.supply.clear();
        self.pools.clear();
        self.signer_lists.clear();
        self.queued.clear();
        self.queued_hashes.clear();
        self.seal_first();
    }

    /// Removes `account` and wipes its XRP, while the chain goes on: an
    /// account absent as of the ledgers its node reads next, which no
    /// transaction in its history explains.
    pub fn remove_account(&mut self, account: AccountId) {
        if let Some(removed) = self.accounts.remove(&account) {
            self.wiped = self.wiped.saturating_add(removed.balance);
        }
        self.signer_lists.remove(&account);
    }

    /// Moves `drops` from genesis to `to`, which exists, as of the next
    /// ledger and in no transaction: a balance changed behind its
    /// account's back, which no real ledger does, so that a test shows the
    /// books notice what nothing explains. No XRP is made or lost.
    pub fn move_unrecorded(&mut self, to: AccountId, drops: u64) {
        assert_ne!(to, self.genesis, "genesis pays");
        assert!(self.accounts.contains_key(&to), "to an account that exists");
        assert!(
            self.balance(self.genesis)
                .is_some_and(|balance| balance >= drops),
            "genesis has the drops"
        );
        if let Some(genesis) = self.accounts.get_mut(&self.genesis) {
            genesis.balance -= drops;
        }
        if let Some(account) = self.accounts.get_mut(&to) {
            account.balance = account.balance.saturating_add(drops);
        }
    }

    /// Moves `drops` of `from`'s XRP to genesis, as of the next ledger and
    /// in no transaction: what the fees of a long life would have burnt,
    /// so that a test brings an account low, such as a token hub's escrow
    /// whose XRP pays its fees, without sending each transaction. No XRP
    /// is made or lost.
    pub fn take_unrecorded(&mut self, from: AccountId, drops: u64) {
        assert_ne!(from, self.genesis, "to genesis");
        assert!(
            self.balance(from).is_some_and(|balance| balance >= drops),
            "an account that holds the drops"
        );
        if let Some(account) = self.accounts.get_mut(&from) {
            account.balance -= drops;
        }
        if let Some(genesis) = self.accounts.get_mut(&self.genesis) {
            genesis.balance = genesis.balance.saturating_add(drops);
        }
    }

    /// The connections subscribed to `account`'s transactions.
    #[must_use]
    pub fn subscribers(&self, account: AccountId) -> BTreeSet<ConnectionId> {
        (self.accounts_of.iter())
            .filter(|(_, accounts)| accounts.contains(&account))
            .map(|(connection, _)| *connection)
            .collect()
    }

    /// A connection went away.
    pub fn disconnect(&mut self, connection: ConnectionId) {
        self.accounts_of.remove(&connection);
        self.ledger_stream.remove(&connection);
        self.forget_liar(connection);
    }

    /// Pays `drops` from genesis to `to`, in the next ledger: a deposit from
    /// outside.
    pub fn deposit(&mut self, to: AccountId, drops: u64) {
        self.deposit_tagged(to, drops, None);
    }

    /// Pays `drops` from genesis to `to` with destination tag `tag`, in the
    /// next ledger: a deposit from outside, as to an account that requires
    /// a tag.
    pub fn deposit_tagged(&mut self, to: AccountId, drops: u64, tag: Option<u32>) {
        let sequence = self.next_sequence(self.genesis);
        self.queued
            .push(Tx::Ledger(Box::new(Transaction::Payment(Payment {
                account: self.genesis,
                destination: to,
                destination_tag: tag,
                amount: Amount::Xrp(drops),
                send_max: None,
                fee: FEE_BASE,
                sequence,
                last_ledger_sequence: Some(self.ledger.saturating_add(OWN_TX_LEDGERS)),
                flags: 0,
                network_id: None,
                memos: Vec::new(),
                signing: unsigned(),
            }))));
    }

    /// Counts a read of the account `request` names, of the first
    /// [`MAX_COUNTED_READS`] accounts asked about.
    fn count_read(&mut self, request: &Value) {
        let Ok(account) = Self::account_of(request) else {
            return;
        };
        if self.reads.len() < MAX_COUNTED_READS || self.reads.contains_key(&account) {
            let reads = self.reads.entry(account).or_default();
            *reads = reads.saturating_add(1);
        }
    }

    /// How many times `account_info` asked about `account`.
    #[must_use]
    pub fn reads_of(&self, account: AccountId) -> u64 {
        self.reads.get(&account).copied().unwrap_or(0)
    }

    /// Sets the sequence `account` uses next, as for an account that has
    /// sent all but the last few transactions it ever can.
    pub fn set_sequence(&mut self, account: AccountId, sequence: u32) {
        if let Some(account) = self.accounts.get_mut(&account) {
            account.sequence = sequence;
        }
    }

    /// Sets `account`'s flags ([`account_flags`]), as an `AccountSet` of
    /// its owner's would.
    pub fn set_account_flags(&mut self, account: AccountId, flags: u32) {
        if let Some(account) = self.accounts.get_mut(&account) {
            account.flags = flags;
        }
    }

    /// The faucet: funds `to` in the next ledger. Only the testnet has
    /// one; on mainnet, the owner deposits ([`deposit`](Self::deposit)).
    pub fn fund(&mut self, to: AccountId) {
        assert!(self.network.faucet.is_some(), "mainnet has no faucet");
        self.deposit(to, FAUCET_DROPS);
    }

    /// Answers a request from `connection`. What it returns is for that
    /// connection.
    pub fn request(&mut self, connection: ConnectionId, bytes: &[u8]) -> Vec<u8> {
        let Ok(request) = serde_json::from_slice::<Value>(bytes) else {
            return Vec::new();
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let command = request.get("command").and_then(Value::as_str).unwrap_or("");
        let outcome = match command {
            "server_state" => Ok(self.server_state(connection)),
            "subscribe" => self.subscribe(connection, &request),
            "account_info" => {
                self.count_read(&request);
                self.account_info(&request)
            }
            "account_tx" => self.account_tx(connection, &request),
            "ledger_entry" => self.ledger_entry(&request),
            "account_objects" => self.account_objects(&request),
            "amm_info" => self.amm_info(&request),
            "tx" => self.tx(connection, &request),
            "submit" => self.submit(&request),
            "validations" => self.validations(connection, &request),
            "tx_proof" => self.tx_proof(connection, &request),
            "state_proof" => self.state_proof(connection, &request),
            "manifest" => self.manifest(&request),
            _ => Err(json!({ "error": "unknownCmd" })),
        };
        let response = match outcome {
            Ok(result) => {
                json!({ "type": "response", "id": id, "status": "success", "result": result })
            }
            Err(mut error) => {
                if let Some(object) = error.as_object_mut() {
                    object.insert("type".into(), "response".into());
                    object.insert("id".into(), id);
                    object.insert("status".into(), "error".into());
                }
                error
            }
        };
        serde_json::to_vec(&response).unwrap_or_default()
    }

    /// Closes a ledger [`CLOSE_INTERVAL_S`] after the last: applies what
    /// was submitted, and returns what the streams carry, by connection.
    pub fn close(&mut self) -> Vec<(ConnectionId, Vec<u8>)> {
        self.close_at(self.close_time.saturating_add(CLOSE_INTERVAL_S))
    }

    /// Closes a ledger at `close_time`, in seconds since the XRP Ledger's
    /// epoch, or a second after the last if that is later: close times
    /// grow along a chain, as rippled makes each ledger's close at least a
    /// second after its parent's.
    pub fn close_at(&mut self, close_time: u64) -> Vec<(ConnectionId, Vec<u8>)> {
        self.ledger = self.ledger.saturating_add(1);
        self.close_time = close_time.max(self.close_time.saturating_add(1));
        let mut out = Vec::new();
        let queued = core::mem::take(&mut self.queued);
        self.queued_hashes.clear();
        // Only what applies takes an index in the ledger.
        let mut index = 0_u32;
        for tx in queued {
            let Some(applied) = self.apply(&tx, index) else {
                self.never_applied(&tx);
                continue;
            };
            index = index.saturating_add(1);
            let hearing: Vec<ConnectionId> = (self.accounts_of.iter())
                .filter(|(_, accounts)| applied.accounts.iter().any(|a| accounts.contains(a)))
                .map(|(connection, _)| *connection)
                .collect();
            for connection in hearing {
                if let Some(message) = self.streamed(connection, &applied.message) {
                    out.push((connection, message));
                }
            }
            self.history_index.insert(applied.hash, self.history.len());
            self.history.push(applied);
        }
        self.seal_next();
        let closed = json!({
            "type": "ledgerClosed",
            "ledger_index": self.ledger,
            "ledger_time": self.close_time,
            "fee_base": FEE_BASE,
            "reserve_base": self.reserve.0,
            "reserve_inc": self.reserve.1,
            "validated_ledgers": alloc::format!("{}-{}", self.history_from, self.ledger),
            "ledger_hash": self.ledger_hash(self.ledger).map(|hash| hex_upper(&hash)),
        });
        let closed = serde_json::to_vec(&closed).unwrap_or_default();
        for connection in &self.ledger_stream {
            out.push((*connection, closed.clone()));
        }
        out
    }

    /// Remembers `tx`, submitted and dropped as a ledger closed, as one
    /// that never applied.
    fn never_applied(&mut self, tx: &Tx) {
        let Some(blob) = tx.signed_blob() else {
            return;
        };
        if self.never_applied.len() == MAX_NEVER_APPLIED {
            self.never_applied.remove(0);
        }
        self.never_applied.push((hash(&blob), blob));
    }

    fn next_sequence(&self, account: AccountId) -> u32 {
        let base = self.accounts.get(&account).map_or(1, |a| a.sequence);
        let queued = self
            .queued
            .iter()
            .filter(|tx| tx.account() == account)
            .count();
        base.saturating_add(u32::try_from(queued).unwrap_or(u32::MAX))
    }

    fn reserve_of(&self, account: &Account) -> u64 {
        self.reserve.0.saturating_add(
            self.reserve
                .1
                .saturating_mul(u64::from(account.owner_count)),
        )
    }

    fn honest_server_state(&self) -> Value {
        let mut state = json!({
            "validated_ledger": {
                "seq": self.ledger,
                "close_time": self.close_time,
                "base_fee": FEE_BASE,
                "reserve_base": self.reserve.0,
                "reserve_inc": self.reserve.1,
            },
            "load_factor": 256,
            "load_base": 256,
            "complete_ledgers": alloc::format!("{}-{}", self.history_from, self.ledger),
        });
        if let (Some(id), Some(object)) = (self.network_id, state.as_object_mut()) {
            object.insert("network_id".into(), id.into());
        }
        json!({ "state": state })
    }

    fn subscribe(&mut self, connection: ConnectionId, request: &Value) -> Result<Value, Value> {
        if let Some(accounts) = request.get("accounts").and_then(Value::as_array) {
            for account in accounts {
                let account = account
                    .as_str()
                    .and_then(AccountId::parse)
                    .ok_or_else(|| json!({ "error": "actMalformed" }))?;
                self.accounts_of
                    .entry(connection)
                    .or_default()
                    .insert(account);
            }
        }
        let streams = request.get("streams").and_then(Value::as_array);
        if streams.is_some_and(|streams| streams.iter().any(|s| s.as_str() == Some("ledger"))) {
            self.ledger_stream.insert(connection);
        }
        Ok(json!({
            "ledger_index": self.ledger,
            "fee_base": FEE_BASE,
            "reserve_base": self.reserve.0,
            "reserve_inc": self.reserve.1,
        }))
    }

    fn account_of(request: &Value) -> Result<AccountId, Value> {
        request
            .get("account")
            .and_then(Value::as_str)
            .and_then(AccountId::parse)
            .ok_or_else(|| json!({ "error": "actMalformed" }))
    }

    fn account_info(&self, request: &Value) -> Result<Value, Value> {
        let id = Self::account_of(request)?;
        // As rippled, an absence names the ledger it is as of.
        let account = self.accounts.get(&id).ok_or_else(
            || json!({ "error": "actNotFound", "ledger_index": self.ledger, "validated": true }),
        )?;
        Ok(json!({
            "account_data": meta::account_fields(id, account),
            "ledger_index": self.ledger,
            "validated": true,
        }))
    }

    /// `ledger_entry` for a trust line: `ripple_state` by its two accounts
    /// and currency, as of the last validated ledger.
    fn ledger_entry(&self, request: &Value) -> Result<Value, Value> {
        let malformed = || json!({ "error": "malformedRequest" });
        let state = request.get("ripple_state").ok_or_else(malformed)?;
        let accounts = state
            .get("accounts")
            .and_then(Value::as_array)
            .filter(|accounts| accounts.len() == 2)
            .ok_or_else(malformed)?;
        let account = |index: usize| {
            accounts
                .get(index)
                .and_then(Value::as_str)
                .and_then(AccountId::parse)
                .ok_or_else(malformed)
        };
        let (a, b) = (account(0)?, account(1)?);
        let currency = state
            .get("currency")
            .and_then(Value::as_str)
            .and_then(Currency::parse)
            .ok_or_else(malformed)?;
        if a == b {
            return Err(malformed());
        }
        let key = tokens::line_key(a, b, currency);
        let line = self.lines.get(&key).ok_or_else(
            || json!({ "error": "entryNotFound", "ledger_index": self.ledger, "validated": true }),
        )?;
        let mut node = meta::line_fields(line);
        if let Some(object) = node.as_object_mut() {
            object.insert("LedgerEntryType".into(), "RippleState".into());
        }
        Ok(json!({
            "index": hex_upper(&tokens::line_index(&key)),
            "ledger_index": self.ledger,
            "node": node,
            "validated": true,
        }))
    }

    fn honest_account_tx(&self, request: &Value) -> Result<Value, Value> {
        let id = Self::account_of(request)?;
        let number = |key: &str| {
            request
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        // As rippled at API version 1: a range beyond the history is cut
        // to it, and the answer says so.
        let min = number("ledger_index_min")
            .unwrap_or(self.account_tx_from)
            .max(self.account_tx_from);
        let max = number("ledger_index_max")
            .unwrap_or(self.ledger)
            .min(self.ledger);
        let limit = usize::try_from(request.get("limit").and_then(Value::as_u64).unwrap_or(200))
            .unwrap_or(200)
            .clamp(1, 400);
        let skip = request
            .get("marker")
            .and_then(Value::as_u64)
            .and_then(|m| usize::try_from(m).ok())
            .unwrap_or(0);
        let matching: Vec<&Applied> = self
            .history
            .iter()
            .filter(|a| a.ledger >= min && a.ledger <= max && a.accounts.contains(&id))
            .collect();
        let binary = proofs::wants_binary(request);
        let page: Vec<Value> = matching
            .iter()
            .skip(skip)
            .take(limit)
            .map(|a| {
                if binary {
                    return proofs::binary_entry(&a.blob, &a.meta, a.ledger);
                }
                let mut tx = a.message.get("transaction").cloned().unwrap_or(Value::Null);
                if let Some(object) = tx.as_object_mut() {
                    object.insert("ledger_index".into(), a.ledger.into());
                }
                json!({ "tx": tx, "meta": a.message.get("meta"), "validated": true })
            })
            .collect();
        let next = skip.saturating_add(page.len());
        let mut result = json!({
            "account": id.to_address(),
            "ledger_index_min": min,
            "ledger_index_max": max,
            "transactions": page,
        });
        if next < matching.len()
            && let Some(object) = result.as_object_mut()
        {
            object.insert("marker".into(), next.into());
        }
        Ok(result)
    }

    fn honest_tx(&self, request: &Value) -> Result<Value, Value> {
        let hash: [u8; 32] = request
            .get("transaction")
            .and_then(Value::as_str)
            .and_then(from_hex)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| json!({ "error": "notImpl" }))?;
        // As rippled, a range is checked before anything is looked up: not
        // backwards, and at most `MAX_TX_RANGE_LEDGERS`.
        if let (Some(min), Some(max)) = (
            request.get("min_ledger").and_then(Value::as_u64),
            request.get("max_ledger").and_then(Value::as_u64),
        ) {
            if max < min {
                return Err(json!({ "error": "invalidLgrRange" }));
            }
            if max - min > MAX_TX_RANGE_LEDGERS {
                return Err(json!({ "error": "excessiveLgrRange" }));
            }
        }
        let found = (self.history_index.get(&hash)).and_then(|at| self.history.get(*at));
        if let Some(applied) = found
            && proofs::wants_binary(request)
        {
            return Ok(proofs::binary_tx(
                &applied.blob,
                &applied.meta,
                applied.ledger,
            ));
        }
        if let Some(applied) = found {
            let mut result = applied
                .message
                .get("transaction")
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(object) = result.as_object_mut() {
                object.insert(
                    "meta".into(),
                    applied.message.get("meta").cloned().unwrap_or(Value::Null),
                );
                object.insert("ledger_index".into(), applied.ledger.into());
                object.insert("validated".into(), true.into());
            }
            return Ok(result);
        }
        let number = |key: &str| request.get(key).and_then(Value::as_u64);
        let searched_all = match (number("min_ledger"), number("max_ledger")) {
            (Some(min), Some(max)) => {
                min >= u64::from(self.history_from) && max <= u64::from(self.ledger)
            }
            _ => false,
        };
        Err(json!({ "error": "txnNotFound", "searched_all": searched_all }))
    }

    fn submit(&mut self, request: &Value) -> Result<Value, Value> {
        let blob = request
            .get("tx_blob")
            .and_then(Value::as_str)
            .and_then(from_hex)
            .ok_or_else(|| json!({ "error": "invalidTransaction" }))?;
        let result = |code: &str| Ok(json!({ "engine_result": code, "tx_blob": hex_upper(&blob) }));
        // What the codec cannot read, the node would never send: a partial
        // Payment, paths, `SendMax` of XRP for XRP, or any other
        // transaction.
        let Ok(tx) = Transaction::decode(&blob) else {
            return result("temMALFORMED");
        };
        let tx_hash = hash(&blob);
        if self.queued_hashes.contains(&tx_hash) || self.history_index.contains_key(&tx_hash) {
            return result("tefALREADY");
        }
        // Signatures are checked once: they cannot change. Whether the keys
        // may sign for the sender is checked again as it applies.
        if !tx.signatures_valid() {
            return result("temBAD_SIGNATURE");
        }
        let tx = Tx::Ledger(Box::new(tx));
        if let Err(code) = preflight(&tx).and_then(|()| self.preclaim(&tx)) {
            return result(code);
        }
        self.queued_hashes.insert(tx_hash);
        self.queued.push(tx);
        result("tesSUCCESS")
    }

    /// What the ledger checks of a transaction against the accounts, in
    /// rippled's order: the sender, the fee (one more base fee for each
    /// signer), the last ledger, the sequence, and who signed.
    fn preclaim(&self, tx: &Tx) -> Result<(), &'static str> {
        if !self.accounts.contains_key(&tx.account()) {
            return Err("terNO_ACCOUNT");
        }
        let signers = tx.signing().map_or(0, Signing::signers);
        if tx.fee() < multi_signed_fee(FEE_BASE, signers) {
            return Err("telINSUF_FEE_P");
        }
        if tx
            .last_ledger_sequence()
            .is_some_and(|last| last <= self.ledger)
        {
            return Err("tefMAX_LEDGER");
        }
        let next = self.next_sequence(tx.account());
        if tx.sequence() < next {
            return Err("tefPAST_SEQ");
        }
        if tx.sequence() > next {
            return Err("terPRE_SEQ");
        }
        match tx.signing() {
            Some(signing) => self.authorized(tx.account(), signing),
            None => Ok(()),
        }
    }

    /// Applies a queued transaction, as the ledger closes: its fee is
    /// burned and its sequence used whatever its result, and nothing else
    /// changes unless it succeeds. One past its last ledger, out of
    /// sequence, or whose keys may no longer sign for its sender (`tef`)
    /// applies nothing, costs nothing and uses no sequence.
    fn apply(&mut self, tx: &Tx, index: u32) -> Option<Applied> {
        let sender = *self.accounts.get(&tx.account())?;
        let expired = tx
            .last_ledger_sequence()
            .is_some_and(|last| last < self.ledger);
        if expired || tx.sequence() != sender.sequence {
            return None;
        }
        if let Some(signing) = tx.signing()
            && self.authorized(tx.account(), signing).is_err()
        {
            return None;
        }
        let fee = tx.fee().min(sender.balance);
        self.burned = self.burned.saturating_add(fee);
        let mut touched = Touched::default();
        let after_fee = Account {
            balance: sender.balance - fee,
            sequence: sender.sequence.saturating_add(1),
            ..sender
        };
        self.put_account(&mut touched, tx.account(), after_fee);
        let outcome = match tx {
            Tx::Ledger(tx) => self.apply_ledger(&mut touched, tx, sender.balance),
            Tx::IssuerSet(set) => Outcome::code(self.issuer_set(&mut touched, set)),
            Tx::Clawback(clawback) => Outcome::code(self.claw(&mut touched, clawback)),
            Tx::PoolSet(set) => Outcome::code(self.apply_pool_set(&mut touched, set)),
        };
        Some(self.applied(tx, index, &outcome, &touched))
    }

    /// A transaction of a kind the codec reads, after its fee; `prior` is
    /// the sender's XRP before it, which reserves are checked against.
    fn apply_ledger(&mut self, touched: &mut Touched, tx: &Transaction, prior: u64) -> Outcome {
        match tx {
            Transaction::Payment(payment) => self.pay(touched, payment),
            Transaction::TrustSet(set) => Outcome::code(self.trust_set(touched, set, prior)),
            Transaction::AccountSet(set) => Outcome::code(self.account_set(touched, set)),
            Transaction::SetRegularKey(set) => Outcome::code(self.set_regular_key(touched, set)),
            Transaction::SignerListSet(set) => {
                Outcome::code(self.signer_list_set(touched, set, prior))
            }
        }
    }

    /// A Payment, after its fee: of what it names, or across a pool when
    /// its `SendMax` is in the other asset of one.
    fn pay(&mut self, touched: &mut Touched, payment: &Payment) -> Outcome {
        if self.is_pool_account(payment.destination) {
            return Outcome::code("tecNO_PERMISSION");
        }
        let requires_tag = self
            .accounts
            .get(&payment.destination)
            .is_some_and(|account| account.flags & account_flags::REQUIRE_DEST_TAG != 0);
        if requires_tag && payment.destination_tag.is_none() {
            return Outcome::code("tecDST_TAG_NEEDED");
        }
        let (from, to) = (payment.account, payment.destination);
        match (payment.amount, payment.send_max) {
            (Amount::Xrp(drops), None) => self.pay_xrp(touched, payment, drops),
            (Amount::Token(amount), None) => self.pay_token(touched, from, to, amount),
            (Amount::Token(amount), Some(Amount::Token(max)))
                if (max.issuer, max.currency) == (amount.issuer, amount.currency) =>
            {
                if max.value < amount.value {
                    Outcome::code("tecPATH_PARTIAL")
                } else {
                    self.pay_token(touched, from, to, amount)
                }
            }
            (Amount::Token(amount), Some(Amount::Xrp(max))) => {
                self.swap_xrp_for_token(touched, payment, amount, max)
            }
            (Amount::Xrp(drops), Some(Amount::Token(max))) => {
                self.swap_token_for_xrp(touched, payment, drops, max)
            }
            // No pool holds two tokens, and no path goes through XRP.
            (Amount::Token(_), Some(Amount::Token(_))) => Outcome::code("tecPATH_DRY"),
            // The codec reads no such Payment.
            (Amount::Xrp(_), Some(Amount::Xrp(_))) => Outcome::code("temBAD_SEND_XRP_MAX"),
        }
    }

    /// A Payment of XRP, after its fee.
    fn pay_xrp(&mut self, touched: &mut Touched, payment: &Payment, amount: u64) -> Outcome {
        let Some(sender) = self.accounts.get(&payment.account).copied() else {
            return Outcome::code("tefINTERNAL");
        };
        let destination = self.accounts.get(&payment.destination).copied();
        let code = if destination.is_none() && amount < self.reserve.0 {
            "tecNO_DST_INSUF_XRP"
        } else if sender.balance < amount.saturating_add(self.reserve_of(&sender))
            && payment.account != self.genesis
        {
            "tecUNFUNDED_PAYMENT"
        } else {
            "tesSUCCESS"
        };
        if code != "tesSUCCESS" {
            return Outcome::code(code);
        }
        let sender_after = Account {
            balance: sender.balance - amount,
            ..sender
        };
        self.put_account(touched, payment.account, sender_after);
        let after = destination.map_or_else(
            || Account::new(amount, self.ledger),
            |d| Account {
                balance: d.balance.saturating_add(amount),
                ..d
            },
        );
        self.put_account(touched, payment.destination, after);
        Outcome {
            code,
            delivered: Some(Amount::Xrp(amount)),
        }
    }

    /// Writes an account, keeping what it was before the transaction.
    fn put_account(&mut self, touched: &mut Touched, id: AccountId, account: Account) {
        touched.account(id, self.accounts.get(&id).copied());
        self.accounts.insert(id, account);
    }

    /// The fake's own transaction, unsigned: its hash is made from its
    /// ledger and index.
    fn queue_own(&mut self, tx: Tx) {
        self.queued.push(tx);
    }

    /// The sequence and last ledger of `issuer`'s next own transaction, if
    /// it is one of the fake's issuers.
    fn own_slot(&self, issuer: AccountId) -> Result<(u32, u32), &'static str> {
        if !self.issuers.contains(&issuer) || !self.accounts.contains_key(&issuer) {
            return Err("terNO_ACCOUNT");
        }
        Ok((
            self.next_sequence(issuer),
            self.ledger.saturating_add(OWN_TX_LEDGERS),
        ))
    }
}

/// Universal transaction flags: `tfFullyCanonicalSig`.
const TF_FULLY_CANONICAL_SIG: u32 = 0x8000_0000;
/// The Payment flags the fake knows: `tfNoRippleDirect` and
/// `tfLimitQuality`, which change nothing without paths. The codec refuses
/// `tfPartialPayment`.
const PAYMENT_FLAGS: u32 = 0x0001_0000 | 0x0004_0000 | TF_FULLY_CANONICAL_SIG;

/// What the ledger checks before anything else: the transaction alone.
fn preflight(tx: &Tx) -> Result<(), &'static str> {
    let Tx::Ledger(tx) = tx else {
        return Ok(());
    };
    match tx.as_ref() {
        Transaction::Payment(payment) => {
            if payment.flags & !PAYMENT_FLAGS != 0 {
                return Err("temINVALID_FLAG");
            }
            if payment.amount.is_zero() || payment.send_max.is_some_and(|max| max.is_zero()) {
                return Err("temBAD_AMOUNT");
            }
            // To oneself only across assets: a swap.
            let spent = payment.send_max.unwrap_or(payment.amount).asset();
            if payment.account == payment.destination && spent == payment.amount.asset() {
                return Err("temREDUNDANT");
            }
            Ok(())
        }
        Transaction::TrustSet(trust_set) => tokens::preflight_trust_set(trust_set),
        Transaction::AccountSet(set) => signers::preflight_account_set(set),
        Transaction::SetRegularKey(set) => signers::preflight_set_regular_key(set),
        Transaction::SignerListSet(set) => signers::preflight_signer_list_set(set),
    }
}

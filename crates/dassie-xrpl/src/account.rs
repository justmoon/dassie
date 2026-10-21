//! The node's account on the XRP Ledger testnet, as an external asset
//! (spec/areas/xrpl.md#the-account), sans-IO.
//!
//! The account owns what the schemes that settle through it would
//! otherwise race for: its keys, the connection to its ledger, the next
//! sequence and the transactions in flight, and what it holds as the ledger
//! says. It says what to send ([`AssetEffect::Send`]) and reads what comes
//! back; what it learns for its schemes it keeps as [`News`], which the
//! node hands every scheme on it before the account forgets it:
//!
//! - every validated transaction that touches the account, in ledger
//!   order, with the scheme that signed it if it was one of ours in flight;
//! - that it is up to date, as of a validated ledger;
//! - that a transaction of a scheme can never apply, or that the account
//!   and everything it held are gone (a testnet reset);
//! - each ledger that closes, and each transaction a scheme looked up.
//!
//! Each scheme keeps its own cursor into the transactions, and tells the
//! account where it is ([`XrplAccount::follow`]): on each connection the
//! account reads its history from the earliest cursor of its schemes.
//!
//! It runs only on the testnet: a server that reports another network, or
//! none, stops it before the account is read, so mainnet XRP is never
//! booked, written off or signed for (spec/decisions/real-money.md). It
//! acts only on what a server says as of a validated ledger at or past the
//! newest it has seen; a ledger below the newest that closed later is a new
//! chain, which only a testnet reset makes, and only on a network
//! [`resets`] allows (spec/decisions/ledger-resets.md).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use dassie_ledger::{MAX_TOTAL, Name};
use dassie_settlement::{
    AssetEffect, AssetId, AssetKind, ExternalAsset, Holding, LedgerInput, LedgerStatus, SchemeError,
};
use serde_json::{Value, json};

use crate::api::{self, AccountInfo, ApiError, LedgerClosed, Message, TxRecord};
use crate::codec::{Amount, Memo, Payment};
use crate::keys::{AccountId, Wallet};
use crate::{UNITS_PER_DROP, from_hex, hex_upper};

/// The node's first XRPL testnet account: the one whose keys are at
/// `node/settlement-module/xrpl-testnet/seed-testnet`
/// (spec/areas/identity.md#derivation-chain).
pub const FIRST: AssetId = AssetId {
    kind: AssetKind::XrplTestnetAccount,
    index: 0,
};
/// The testnet's public server.
pub const DEFAULT_SERVER: &str = "wss://s.altnet.rippletest.net:51233";
/// The testnet's faucet.
pub const DEFAULT_FAUCET: &str = "https://faucet.altnet.rippletest.net/accounts";

/// Most of our transactions on their way at once, of all the schemes on
/// the account.
pub const MAX_IN_FLIGHT: usize = 8;
/// The most fee the account pays, in drops: above it, it signs nothing.
pub const MAX_FEE_DROPS: u64 = 1_000;
/// Most schemes that settle through one account.
pub const MAX_SCHEMES: usize = 4;
/// Most transactions looked up for the schemes at once; the rest are
/// asked as answers come in, a ledger at a time.
pub const MAX_LOOKUPS: usize = 8;
/// How many ledgers a transaction may take to get in.
const LEDGER_WINDOW: u32 = 20;
/// How long a request may go unanswered before the connection is made
/// anew.
const REQUEST_TIMEOUT_MS: u64 = 20_000;
/// How often the faucet is asked, while the account does not exist.
const FUND_RETRY_MS: u64 = 60_000;
/// Most requests waiting for an answer: a submission and a lookup for
/// each transaction in flight, the schemes' lookups, and the four that
/// keep the account in sync.
const MAX_PENDING: usize = 2 * MAX_IN_FLIGHT + MAX_LOOKUPS + 4;
/// Most transactions held while catching up.
const MAX_BUFFERED: usize = 256;
/// Most transactions remembered as never sent, until their schemes have
/// reconciled them. Past it, the oldest is forgotten, and its scheme
/// looks it up by its hash, which finds nothing and leaves it pending.
const MAX_NEVER_SENT: usize = 2 * MAX_IN_FLIGHT;
/// Most news kept between two tellings: one message's worth, at most a page
/// of history, what was held while catching up, and what was in flight.
// PAGE is a few hundred: no truncation.
#[allow(clippy::cast_possible_truncation)]
const MAX_NEWS: usize = api::PAGE as usize + MAX_BUFFERED + MAX_IN_FLIGHT + 4;
/// Most effects between two takings: one message's worth.
const MAX_EFFECTS: usize = MAX_PENDING + MAX_IN_FLIGHT + 4;
/// Most bytes of config.
const MAX_CONFIG: usize = 1024;
/// How long the connection may say nothing, no ledger closing, before it
/// is made anew: a ledger closes every few seconds.
const LEDGER_SILENCE_MS: u64 = 30_000;
/// The most drops the books can hold for the account: [`MAX_TOTAL`] in
/// units. A server that reports more for it is no ledger to follow.
pub(crate) const MAX_ACCOUNT_DROPS: u64 = MAX_TOTAL / UNITS_PER_DROP;
/// Why the account stops when a server reports more than that.
const TOO_MUCH_XRP: &str = "the server reported more XRP for the account than the books can hold";
/// The network ID the testnet's servers report: mainnet's is 0, and
/// devnet's 2.
const TESTNET_NETWORK_ID: u32 = 1;
/// Why the account stops when its server reports another network, or none.
const WRONG_NETWORK: &str = "the server is not on the XRP Ledger testnet; remove the scheme and add it with a testnet server";
/// Why the account stops when a network that is never reset starts again
/// below a ledger it has seen.
const CHAIN_WENT_BACK: &str =
    "the server's ledger went back on a network that is never reset; nothing was written off";
/// Why the account stops when it is gone from a network that is never
/// reset.
pub(crate) const ACCOUNT_GONE: &str =
    "the account is gone from a network that is never reset; nothing was written off";

/// Whether the chain of network `network_id` may start again from a new
/// genesis, so that a ledger below one the account has seen, closed later,
/// is a reset, and an account gone with it is written off. Only the
/// testnet: on any other network, mainnet included, a ledger below one
/// seen is never taken, and an absent account never written off, whatever
/// else lets the account run there (spec/decisions/real-money.md,
/// spec/decisions/ledger-resets.md).
#[must_use]
pub const fn resets(network_id: u32) -> bool {
    network_id == TESTNET_NETWORK_ID
}

/// A validated ledger: its index, and when it closed, in seconds since the
/// XRP Ledger's epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Validated {
    index: u32,
    close_time: u64,
}

/// Where a server's validated ledger stands against the newest the account
/// has seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chain {
    /// At or past it: the chain the account follows.
    Current,
    /// Below it, closed before it: a server behind the others, whose
    /// answers are old.
    Behind,
    /// Below it, closed after it: a new chain, which a lagging server
    /// cannot make. A reset, on a network that [`resets`].
    Restarted,
    /// The same, on a network that never resets: refused.
    WentBack,
}

/// Where `ledger`, of a server on network `network_id`, stands against
/// `newest`, the newest the account has seen. Close times only grow along
/// a chain, so a lower ledger that closed later is on another.
fn chain(newest: Option<Validated>, network_id: u32, ledger: Validated) -> Chain {
    match newest {
        Some(newest) if ledger.index < newest.index => {
            if ledger.close_time <= newest.close_time {
                Chain::Behind
            } else if resets(network_id) {
                Chain::Restarted
            } else {
                Chain::WentBack
            }
        }
        _ => Chain::Current,
    }
}

/// Where the account connects, and where it asks for test XRP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// A WebSocket URL, `wss://` (or `ws://`, which the node allows only
    /// where it allows private addresses). A server that reports another
    /// network than the testnet, or none, stops the account.
    pub server: String,
    /// The faucet, an HTTPS URL; `None` to never ask.
    pub faucet: Option<String>,
}

impl Config {
    /// The config in `text`: empty for the testnet's defaults, or JSON with
    /// `server` and `faucet` (a URL or `null`), each optional.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidConfig`] if it is not that.
    pub fn parse(text: &str) -> Result<Self, SchemeError> {
        let mut config = Self {
            server: DEFAULT_SERVER.to_string(),
            faucet: Some(DEFAULT_FAUCET.to_string()),
        };
        let text = text.trim();
        if text.is_empty() {
            return Ok(config);
        }
        if text.len() > MAX_CONFIG {
            return Err(SchemeError::InvalidConfig);
        }
        let value: Value = serde_json::from_str(text).map_err(|_| SchemeError::InvalidConfig)?;
        let object = value.as_object().ok_or(SchemeError::InvalidConfig)?;
        for (key, field) in object {
            match (key.as_str(), field) {
                ("server", Value::String(server))
                    if server.starts_with("wss://") || server.starts_with("ws://") =>
                {
                    config.server.clone_from(server);
                }
                ("faucet", Value::String(faucet)) if faucet.starts_with("https://") => {
                    config.faucet = Some(faucet.clone());
                }
                ("faucet", Value::Null) => config.faucet = None,
                _ => return Err(SchemeError::InvalidConfig),
            }
        }
        Ok(config)
    }
}

/// A transaction of ours, as a scheme on the account signed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ours {
    /// The scheme that signed it.
    pub scheme: Name,
    /// The XRP it pays, in drops.
    pub drops: u64,
}

/// What the account learnt for the schemes on it, in the order it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum News {
    /// Up to date as of this account state: every transaction before it
    /// was told.
    Synced(AccountInfo),
    /// A validated transaction that touches the account, as the stream, the
    /// account's history or a lookup gave it: each scheme books it once,
    /// from its own cursor.
    Transaction {
        /// The transaction.
        record: TxRecord,
        /// The scheme that signed it, if it was in flight: it is no longer.
        ours: Option<Ours>,
    },
    /// A transaction of `scheme` can never apply: its settlement is
    /// cancelled.
    Cancelled {
        /// The scheme that signed it.
        scheme: Name,
        /// The transaction.
        hash: [u8; 32],
    },
    /// The account is gone, and with it what the books held of it: a
    /// testnet reset, or the account absent. `restarted` when the chain
    /// started again.
    WrittenOff {
        /// Whether the chain started again.
        restarted: bool,
    },
    /// A ledger closed while the account was synced.
    LedgerClosed {
        /// Whether the reserve changed with it.
        reserve_changed: bool,
    },
    /// A transaction a scheme looked up, validated.
    Found(TxRecord),
}

/// What became of a settlement a scheme still holds pending after a
/// restart ([`XrplAccount::resume`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resumed {
    /// In flight: submitted again once synced.
    InFlight,
    /// Signed and never sent, and dropped past its last ledger: it can
    /// never apply.
    NeverSent,
    /// Not known to the account: only the ledger can say.
    Unknown,
}

/// One of our transactions on its way, kept with the books.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InFlight {
    scheme: Name,
    hash: [u8; 32],
    blob: Vec<u8>,
    drops: u64,
    fee: u64,
    sequence: u32,
    last_ledger: u32,
    submitted_at: u32,
    /// Whether the node executed it: its pending transfer is on disk, so
    /// it may be submitted. One the node never executed is never sent.
    executed: bool,
    /// To submit when the connection allows.
    needs_submit: bool,
    /// Past its last ledger and looked for.
    looked_up: bool,
}

/// A transaction signed and never sent, dropped past its last ledger: kept
/// until its scheme has reconciled it, so that it is cancelled rather than
/// looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NeverSent {
    scheme: Name,
    hash: [u8; 32],
}

/// Where a scheme on the account is: its cursor, and whether its books
/// hold anything of the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Follower {
    cursor: Option<(u32, u32)>,
    holds: bool,
}

/// A request waiting for its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Subscribe,
    ServerState,
    AccountInfo,
    AccountTx,
    Tx([u8; 32]),
    Submit([u8; 32]),
    /// A transaction a scheme looks up, wherever the server's history has
    /// it.
    LookUp([u8; 32]),
}

/// Where the account stands with its ledger.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// No connection.
    Disconnected,
    /// Connected; waiting for the server's state, which must be the
    /// testnet's, then the subscription, then the account's state, as of
    /// a ledger the stream covers.
    Starting {
        subscribed: bool,
        state: bool,
        info: Option<AccountInfo>,
    },
    /// Reading the history from ledger `from`, the earliest cursor of its
    /// schemes, up to `info`'s ledger.
    CatchingUp { info: AccountInfo, from: u32 },
    /// The account does not exist: waiting for funds.
    Unfunded,
    /// Up to date.
    Synced,
}

/// An XRPL testnet account.
pub struct XrplAccount {
    id: AssetId,
    config: Config,
    wallet: Wallet,
    // Kept with the books.
    sequence: u32,
    in_flight: Vec<InFlight>,
    never_sent: Vec<NeverSent>,
    /// The newest validated ledger seen, by its server's state or the
    /// ledger stream. Kept with the books whenever the rest is: any ledger
    /// of the chain tells a server behind from a chain started again.
    newest: Option<Validated>,
    // Learnt from the ledger.
    phase: Phase,
    /// The network the connection's server is on, once checked.
    network_id: Option<u32>,
    next_id: u64,
    pending: BTreeMap<u64, (Request, u64)>,
    validated: u32,
    fee_base: u64,
    load: (u64, u64),
    reserve_base: u64,
    reserve_inc: u64,
    owner_count: u32,
    balance: Option<u64>,
    /// The last transaction the holding was read from: one at or before it
    /// is older than the balance.
    at: Option<(u32, u32)>,
    buffer: Vec<TxRecord>,
    fund_at_ms: u64,
    funding: bool,
    /// The account's state was asked for and the answer dropped as older
    /// than what the account had seen: asked again as the next ledger
    /// closes.
    ask_account: bool,
    failed: Option<&'static str>,
    now_ms: u64,
    /// When the connection last said anything.
    heard_ms: u64,
    followers: BTreeMap<Name, Follower>,
    news: Vec<News>,
    effects: Vec<AssetEffect>,
}

impl XrplAccount {
    /// The account `id`, with the wallet of `entropy`, connecting as
    /// `config` says, from the state the books kept.
    ///
    /// # Errors
    ///
    /// If the config or the state is invalid.
    pub fn new(
        id: AssetId,
        config: &str,
        entropy: &[u8; 32],
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let mut account = Self {
            id,
            config: Config::parse(config)?,
            wallet: Wallet::from_entropy(entropy),
            sequence: 0,
            in_flight: Vec::new(),
            never_sent: Vec::new(),
            newest: None,
            phase: Phase::Disconnected,
            network_id: None,
            next_id: 1,
            pending: BTreeMap::new(),
            validated: 0,
            fee_base: 10,
            load: (1, 1),
            reserve_base: 0,
            reserve_inc: 0,
            owner_count: 0,
            balance: None,
            at: None,
            buffer: Vec::new(),
            fund_at_ms: 0,
            funding: false,
            ask_account: false,
            failed: None,
            now_ms: 0,
            heard_ms: 0,
            followers: BTreeMap::new(),
            news: Vec::new(),
            effects: Vec::new(),
        };
        if !state.is_empty() {
            account.restore(state).ok_or(SchemeError::InvalidData)?;
        }
        Ok(account)
    }

    /// Its address on the ledger.
    #[must_use]
    pub fn account(&self) -> AccountId {
        self.wallet.account()
    }

    /// Its config.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// What it learnt since the schemes on it were last told.
    #[must_use]
    pub fn news(&self) -> &[News] {
        &self.news
    }

    /// Whether it is up to date with its ledger, and running.
    #[must_use]
    pub fn is_synced(&self) -> bool {
        self.phase == Phase::Synced && self.failed.is_none()
    }

    /// The newest validated ledger it knows of.
    #[must_use]
    pub const fn validated(&self) -> u32 {
        self.validated
    }

    /// The reserve, base and per owned object, in drops.
    #[must_use]
    pub const fn reserve_params(&self) -> (u64, u64) {
        (self.reserve_base, self.reserve_inc)
    }

    /// `scheme` settles through the account, and is at `cursor`; `holds`
    /// whether its books hold anything of it. The account reads its history
    /// from the earliest cursor of its schemes, and an account gone from a
    /// network that never resets stops it if any of them held anything.
    ///
    /// # Panics
    ///
    /// Past [`MAX_SCHEMES`]: the node makes no more.
    pub fn follow(&mut self, scheme: Name, cursor: Option<(u32, u32)>, holds: bool) {
        assert!(
            self.followers.contains_key(&scheme) || self.followers.len() < MAX_SCHEMES,
            "more schemes on an account than there may be"
        );
        self.followers.insert(scheme, Follower { cursor, holds });
    }

    /// The reserve the account holds back now, in drops.
    fn reserve_drops(&self, owner_count: u32) -> u64 {
        self.reserve_base
            .saturating_add(self.reserve_inc.saturating_mul(u64::from(owner_count)))
    }

    /// The fee a transaction pays now, in drops.
    fn fee(&self) -> u64 {
        let (factor, base) = self.load;
        let fee = u128::from(self.fee_base) * u128::from(factor) / u128::from(base.max(1));
        u64::try_from(fee).unwrap_or(u64::MAX).max(10)
    }

    fn tell(&mut self, news: News) {
        assert!(
            self.news.len() < MAX_NEWS,
            "more news than one message makes"
        );
        self.news.push(news);
    }

    fn effect(&mut self, effect: AssetEffect) {
        assert!(
            self.effects.len() < MAX_EFFECTS,
            "more effects than one message makes"
        );
        self.effects.push(effect);
    }
}

/// What the schemes on the account ask of it.
impl XrplAccount {
    /// Signs a Payment of `drops` to `destination` for `scheme`, with the
    /// next sequence, and keeps it in flight, not yet executed. Returns its
    /// hash.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] unless the account is synced and knows
    /// its sequence, has room for another transaction in flight, the amount
    /// is at least a drop, the network fee is at most [`MAX_FEE_DROPS`],
    /// and the amount and the fee fit above the reserve after what is in
    /// flight.
    pub fn sign_payment(
        &mut self,
        scheme: Name,
        destination: AccountId,
        drops: u64,
        memos: Vec<Memo>,
    ) -> Result<[u8; 32], SchemeError> {
        let fee = self.fee();
        let in_flight: u64 = self
            .in_flight
            .iter()
            .map(|t| t.drops.saturating_add(t.fee))
            .sum();
        let spare = self
            .balance
            .map(|balance| balance.saturating_sub(self.reserve_drops(self.owner_count)))
            .unwrap_or(0)
            .saturating_sub(in_flight);
        let ready = self.is_synced()
            && self.sequence < u32::MAX
            && self.in_flight.len() < MAX_IN_FLIGHT
            && self.sequence > 0
            && drops > 0
            && fee <= MAX_FEE_DROPS
            && drops.saturating_add(fee) <= spare;
        if !ready {
            return Err(SchemeError::Unavailable);
        }
        let payment = Payment {
            account: self.account(),
            destination,
            amount: Amount::Xrp(drops),
            fee,
            sequence: self.sequence,
            last_ledger_sequence: self.validated.saturating_add(LEDGER_WINDOW),
            flags: 0,
            // Only the testnet is signed for, and a transaction names its
            // network only above 1024.
            network_id: None,
            memos,
            signing_pub_key: *self.wallet.public_key(),
            signature: None,
        };
        let signed = payment.sign(&self.wallet);
        self.sequence = self.sequence.saturating_add(1);
        self.in_flight.push(InFlight {
            scheme,
            hash: signed.hash,
            blob: signed.blob,
            drops,
            fee,
            sequence: signed.tx.sequence,
            last_ledger: signed.tx.last_ledger_sequence,
            submitted_at: self.validated,
            executed: false,
            needs_submit: false,
            looked_up: false,
        });
        Ok(signed.hash)
    }

    /// The node executed `scheme`'s transaction `hash`: it is submitted,
    /// now or once synced. Returns whether it was in flight; one that is
    /// not was signed and then lost, or dropped as never sent, and can
    /// never apply.
    pub fn execute(&mut self, scheme: Name, hash: [u8; 32]) -> bool {
        let synced = self.phase == Phase::Synced;
        let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) else {
            self.never_sent.retain(|n| n.hash != hash);
            return false;
        };
        assert_eq!(transaction.scheme, scheme, "a scheme executes its own");
        transaction.executed = true;
        if synced {
            self.submit(hash);
        } else {
            transaction.needs_submit = true;
        }
        true
    }

    /// Forgets `scheme`'s transaction `hash`, signed and never executed:
    /// the node could not book it. Its sequence is free again, if it was
    /// the last one given.
    pub fn discard(&mut self, scheme: Name, hash: [u8; 32]) {
        let Some(position) = self
            .in_flight
            .iter()
            .position(|t| t.hash == hash && t.scheme == scheme && !t.executed)
        else {
            return;
        };
        let transaction = self.in_flight.remove(position);
        if transaction.sequence.saturating_add(1) == self.sequence {
            self.sequence = transaction.sequence;
        }
    }

    /// `scheme` still holds the settlement of transaction `hash` pending
    /// after a restart: one in flight is submitted again once synced; one
    /// dropped as never sent can never apply, and is forgotten now.
    pub fn resume(&mut self, scheme: Name, hash: [u8; 32]) -> Resumed {
        if let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
            assert_eq!(transaction.scheme, scheme, "a scheme resumes its own");
            transaction.executed = true;
            transaction.needs_submit = true;
            return Resumed::InFlight;
        }
        if let Some(position) = self
            .never_sent
            .iter()
            .position(|n| n.hash == hash && n.scheme == scheme)
        {
            self.never_sent.remove(position);
            return Resumed::NeverSent;
        }
        Resumed::Unknown
    }

    /// Whether a lookup may be asked now: synced, with room for one.
    #[must_use]
    pub fn can_look_up(&self) -> bool {
        let asking = self
            .pending
            .values()
            .filter(|(request, _)| matches!(request, Request::LookUp(_)))
            .count();
        self.phase == Phase::Synced && asking < MAX_LOOKUPS
    }

    /// Whether transaction `hash` is being looked up.
    #[must_use]
    pub fn is_looking_up(&self, hash: &[u8; 32]) -> bool {
        self.pending
            .values()
            .any(|(request, _)| *request == Request::LookUp(*hash))
    }

    /// Looks transaction `hash` up in all the server's history: what is
    /// found validated comes as [`News::Found`]. When it was submitted is
    /// not kept, and a server searches a range of at most 1000 ledgers.
    pub fn look_up(&mut self, hash: [u8; 32]) {
        if self.can_look_up() && !self.is_looking_up(&hash) {
            self.send(Request::LookUp(hash));
        }
    }
}

/// The connection.
impl XrplAccount {
    fn send(&mut self, request: Request) {
        // Without a connection, nothing is sent: what is needed is asked
        // once it is made.
        if self.phase == Phase::Disconnected {
            return;
        }
        if self.pending.len() >= MAX_PENDING {
            // The connection is not keeping up: make it anew.
            self.reconnect();
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let account = self.account();
        let message = match request {
            Request::Subscribe => api::subscribe(id, account),
            Request::ServerState => api::server_state(id),
            Request::AccountInfo => api::account_info(id, account),
            Request::AccountTx => {
                let Phase::CatchingUp { info, from } = &self.phase else {
                    return;
                };
                api::account_tx(id, account, *from, info.ledger_index, None)
            }
            Request::Tx(hash) => {
                let Some(transaction) = self.in_flight.iter().find(|t| t.hash == hash) else {
                    return;
                };
                api::tx(id, &hash, transaction.submitted_at, transaction.last_ledger)
            }
            Request::Submit(hash) => {
                let Some(transaction) = self.in_flight.iter().find(|t| t.hash == hash) else {
                    return;
                };
                api::submit(id, &transaction.blob)
            }
            Request::LookUp(hash) => api::tx_anywhere(id, &hash),
        };
        self.pending.insert(id, (request, self.now_ms));
        self.effect(AssetEffect::Send { message });
    }

    fn reconnect(&mut self) {
        self.disconnected();
        self.effect(AssetEffect::Reconnect);
    }

    fn disconnected(&mut self) {
        self.phase = Phase::Disconnected;
        self.network_id = None;
        self.pending.clear();
        self.buffer.clear();
        // A faucet's answer may come for an older connection, and be
        // dropped: it is asked again in time.
        self.funding = false;
        self.ask_account = false;
        for transaction in &mut self.in_flight {
            transaction.needs_submit = transaction.executed;
            transaction.looked_up = false;
        }
    }

    fn connected(&mut self) {
        self.disconnected();
        self.heard_ms = self.now_ms;
        self.phase = Phase::Starting {
            subscribed: false,
            state: false,
            info: None,
        };
        // The network is checked first, so nothing is read or booked from
        // another; then the account is subscribed, and read once the
        // subscription is on, so nothing falls between what it says and
        // what the stream says next.
        self.send(Request::ServerState);
    }

    fn message(&mut self, bytes: &[u8]) {
        match api::parse(bytes) {
            Message::Response { id, outcome } => {
                if let Some((request, _)) = self.pending.remove(&id) {
                    self.response(request, outcome);
                }
            }
            Message::Transaction(record) => {
                if self.phase == Phase::Synced {
                    self.transaction(&record);
                } else if !matches!(self.phase, Phase::Disconnected) {
                    if self.buffer.len() >= MAX_BUFFERED {
                        self.reconnect();
                    } else {
                        self.buffer.push(*record);
                    }
                }
            }
            Message::LedgerClosed(closed) => self.ledger_closed(closed),
            Message::Ignored => {}
        }
    }

    fn response(&mut self, request: Request, outcome: Result<Value, ApiError>) {
        match request {
            Request::Subscribe => {
                if outcome.is_err() {
                    self.reconnect();
                } else if let Phase::Starting { subscribed, .. } = &mut self.phase {
                    *subscribed = true;
                    self.send(Request::AccountInfo);
                }
            }
            Request::ServerState => self.server_state(outcome),
            Request::AccountInfo => self.account_info(outcome),
            Request::AccountTx => self.account_tx(outcome),
            Request::Tx(hash) => self.looked_up(hash, outcome),
            Request::Submit(hash) => self.submitted(hash, outcome.ok().as_ref()),
            Request::LookUp(_) => self.found(outcome),
        }
    }

    /// The server's state, the first answer on a connection: a server off
    /// the testnet, or that does not say, stops the account before anything
    /// is read from it. On the testnet, its ledger says whether it is
    /// behind, or on a chain started again; then the account is
    /// subscribed.
    fn server_state(&mut self, outcome: Result<Value, ApiError>) {
        let Some(state) = outcome.ok().as_ref().and_then(api::server_state_result) else {
            self.reconnect();
            return;
        };
        let Some(network_id) = state.network_id.filter(|&id| id == TESTNET_NETWORK_ID) else {
            self.failed = Some(WRONG_NETWORK);
            self.disconnected();
            return;
        };
        self.network_id = Some(network_id);
        let ledger = Validated {
            index: state.validated_ledger,
            close_time: state.close_time,
        };
        match chain(self.newest, network_id, ledger) {
            Chain::Current => self.newest = Some(ledger),
            // What it says of the account is older than what the account
            // saw: its answers wait for it to catch up.
            Chain::Behind => {}
            Chain::Restarted => self.restarted(ledger),
            Chain::WentBack => {
                self.failed = Some(CHAIN_WENT_BACK);
                self.disconnected();
                return;
            }
        }
        self.validated = self.validated.max(state.validated_ledger);
        self.fee_base = state.base_fee;
        self.load = (state.load_factor, state.load_base);
        self.reserve_base = state.reserve_base;
        self.reserve_inc = state.reserve_inc;
        if let Phase::Starting { state, .. } = &mut self.phase {
            *state = true;
            self.send(Request::Subscribe);
        }
    }

    fn account_info(&mut self, outcome: Result<Value, ApiError>) {
        match outcome {
            Ok(result) => {
                let Some(info) = api::account_info_result(&result) else {
                    self.reconnect();
                    return;
                };
                if info.balance > MAX_ACCOUNT_DROPS {
                    self.failed = Some(TOO_MUCH_XRP);
                    return;
                }
                // A server behind what the account has seen says nothing
                // new: asked again when the next ledger closes.
                if info.ledger_index < self.seen() {
                    self.ask_account = true;
                    return;
                }
                let next = self
                    .in_flight
                    .iter()
                    .map(|t| t.sequence.saturating_add(1))
                    .max()
                    .unwrap_or(0);
                self.sequence = info.sequence.max(next);
                match &mut self.phase {
                    Phase::Starting { info: slot, .. } => *slot = Some(info),
                    Phase::Unfunded => {
                        self.phase = Phase::Starting {
                            subscribed: true,
                            state: true,
                            info: Some(info),
                        };
                    }
                    _ => {}
                }
                self.start();
            }
            // Absent only as of a validated ledger the account has not seen
            // past: a server behind says so of a ledger before the account
            // was made, while a settlement of ours may have applied since.
            // Otherwise it is asked again when the next ledger closes.
            Err(error) if error.error == "actNotFound" => {
                if error.validated
                    && error
                        .ledger_index
                        .is_some_and(|ledger| ledger >= self.seen())
                {
                    self.absent();
                } else {
                    self.ask_account = true;
                }
            }
            Err(_) => self.reconnect(),
        }
    }

    /// The newest validated ledger the account has seen, or a scheme on it
    /// booked from: what a server says as of one before it is old.
    fn seen(&self) -> u32 {
        let newest = self.newest.map_or(0, |newest| newest.index);
        let booked = self
            .followers
            .values()
            .filter_map(|follower| follower.cursor)
            .chain(self.at)
            .map(|(ledger, _)| ledger)
            .max()
            .unwrap_or(0);
        newest.max(booked)
    }

    /// Whether anything of the account is held: by the books of a scheme
    /// on it, or in flight. An absent account writes it off.
    fn holds_anything(&self) -> bool {
        !self.in_flight.is_empty()
            || self
                .followers
                .values()
                .any(|follower| follower.holds || follower.cursor.is_some())
    }

    /// The account does not exist, as of a validated ledger the account
    /// has not seen past. On a network that [`resets`], what the books
    /// held is gone with it, and the faucet is asked; on any other, an
    /// account that held anything does not just go, and the account stops.
    pub(crate) fn absent(&mut self) {
        if self.holds_anything() && !self.network_id.is_some_and(resets) {
            self.failed = Some(ACCOUNT_GONE);
            self.disconnected();
            return;
        }
        self.write_off(false);
        self.phase = Phase::Unfunded;
        self.fund();
    }

    /// The chain started again below the newest ledger seen (a testnet
    /// reset): it follows the new one, and what the books held is gone.
    fn restarted(&mut self, ledger: Validated) {
        assert!(
            self.network_id.is_some_and(resets),
            "only a network that resets starts again"
        );
        self.newest = Some(ledger);
        self.validated = ledger.index;
        self.write_off(true);
    }

    /// What the books hold of the account is gone (a testnet reset): what
    /// was in flight can never apply now, since an account made again
    /// starts its sequence anew. Its schemes write off what they held.
    fn write_off(&mut self, restarted: bool) {
        assert!(
            !self.holds_anything() || self.network_id.is_some_and(resets),
            "only a network that resets writes the account off"
        );
        for transaction in core::mem::take(&mut self.in_flight) {
            if transaction.executed {
                self.tell(News::Cancelled {
                    scheme: transaction.scheme,
                    hash: transaction.hash,
                });
            } else {
                self.never_sent(transaction.scheme, transaction.hash);
            }
        }
        self.tell(News::WrittenOff { restarted });
        for follower in self.followers.values_mut() {
            *follower = Follower {
                cursor: None,
                holds: false,
            };
        }
        self.at = None;
        self.balance = None;
    }

    /// Remembers `scheme`'s transaction `hash` as never sent; past
    /// [`MAX_NEVER_SENT`], the oldest is forgotten.
    fn never_sent(&mut self, scheme: Name, hash: [u8; 32]) {
        if self.never_sent.len() >= MAX_NEVER_SENT {
            self.never_sent.remove(0);
        }
        self.never_sent.push(NeverSent { scheme, hash });
    }

    fn fund(&mut self) {
        let Some(url) = self.config.faucet.clone() else {
            return;
        };
        if self.funding || self.fund_at_ms > self.now_ms {
            return;
        }
        self.funding = true;
        self.fund_at_ms = self.now_ms.saturating_add(FUND_RETRY_MS);
        let body = serde_json::to_vec(&json!({ "destination": self.account().to_address() }))
            .unwrap_or_default();
        self.effect(AssetEffect::Fund { url, body });
    }

    /// Once the server's state and the account's are known: reads what
    /// happened while away, from the earliest cursor of its schemes.
    fn start(&mut self) {
        let Phase::Starting {
            state: true,
            info: Some(info),
            ..
        } = self.phase.clone()
        else {
            return;
        };
        let from = self
            .followers
            .values()
            .filter_map(|follower| follower.cursor)
            .min();
        match from {
            Some(cursor) if cursor < (info.ledger_index, u32::MAX) => {
                self.at = self.at.max(Some(cursor));
                self.phase = Phase::CatchingUp {
                    info,
                    from: cursor.0,
                };
                self.send(Request::AccountTx);
            }
            _ => self.synced(info),
        }
    }

    fn account_tx(&mut self, outcome: Result<Value, ApiError>) {
        let Phase::CatchingUp { info, from } = self.phase.clone() else {
            return;
        };
        let page = match outcome {
            Ok(result) => api::account_tx_result(&result),
            // The server's history does not reach back: the balance says
            // what changed, without who it was.
            Err(error) if error.error.starts_with("lgr") => {
                self.synced(info);
                return;
            }
            // Busy, or anything else: from the start, on a new connection.
            Err(_) => None,
        };
        let Some(page) = page else {
            self.reconnect();
            return;
        };
        for record in &page.transactions {
            self.transaction(record);
        }
        match page.marker {
            // The next page asks for the same range, as the marker is for.
            Some(marker) => {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                let message =
                    api::account_tx(id, self.account(), from, info.ledger_index, Some(&marker));
                self.pending.insert(id, (Request::AccountTx, self.now_ms));
                self.effect(AssetEffect::Send { message });
            }
            None => self.synced(info),
        }
    }

    /// Up to date as of `info`: the schemes are told, then what came
    /// meanwhile, and what waits is submitted.
    fn synced(&mut self, info: AccountInfo) {
        self.balance = Some(info.balance);
        self.owner_count = info.owner_count;
        self.at = Some(self.at.map_or((info.ledger_index, u32::MAX), |at| {
            at.max((info.ledger_index, u32::MAX))
        }));
        self.validated = self.validated.max(info.ledger_index);
        self.phase = Phase::Synced;
        self.tell(News::Synced(info));
        for record in core::mem::take(&mut self.buffer) {
            self.transaction(&record);
        }
        let waiting: Vec<[u8; 32]> = self
            .in_flight
            .iter()
            .filter(|t| t.needs_submit)
            .map(|t| t.hash)
            .collect();
        for hash in waiting {
            self.submit(hash);
        }
        self.expire();
    }

    fn ledger_closed(&mut self, closed: LedgerClosed) {
        let ledger = Validated {
            index: closed.ledger_index,
            close_time: closed.close_time,
        };
        match self.network_id.map(|id| chain(self.newest, id, ledger)) {
            Some(Chain::Current) => self.newest = Some(ledger),
            // The chain started again under the connection: made anew, so
            // that the server's state says so before anything is read.
            Some(Chain::Restarted | Chain::WentBack) => {
                self.reconnect();
                return;
            }
            Some(Chain::Behind) | None => {}
        }
        self.validated = self.validated.max(closed.ledger_index);
        self.fee_base = closed.fee_base;
        let reserve_changed =
            (closed.reserve_base, closed.reserve_inc) != (self.reserve_base, self.reserve_inc);
        self.reserve_base = closed.reserve_base;
        self.reserve_inc = closed.reserve_inc;
        let asking = self
            .pending
            .values()
            .any(|(r, _)| *r == Request::AccountInfo);
        match self.phase {
            Phase::Synced => {
                self.tell(News::LedgerClosed { reserve_changed });
                // An answer older than what the account had seen was
                // dropped, such as the sequence read again after a
                // settlement was given up: asked again.
                if core::mem::take(&mut self.ask_account) && !asking {
                    self.send(Request::AccountInfo);
                }
                self.expire();
            }
            Phase::Unfunded
            | Phase::Starting {
                subscribed: true,
                info: None,
                ..
            } if !asking => {
                self.send(Request::AccountInfo);
            }
            _ => {}
        }
    }

    /// Looks for our transactions past their last ledger. One the node
    /// never executed was never sent: past its last ledger it is dropped,
    /// remembered as never sent, and the sequence read again.
    fn expire(&mut self) {
        let validated = self.validated;
        let (keep, dropped): (Vec<InFlight>, Vec<InFlight>) = core::mem::take(&mut self.in_flight)
            .into_iter()
            .partition(|t| t.executed || validated <= t.last_ledger);
        self.in_flight = keep;
        if !dropped.is_empty() {
            for transaction in dropped {
                self.never_sent(transaction.scheme, transaction.hash);
            }
            self.send(Request::AccountInfo);
        }
        let expired: Vec<[u8; 32]> = self
            .in_flight
            .iter_mut()
            .filter(|t| validated > t.last_ledger && !t.looked_up)
            .map(|t| {
                t.looked_up = true;
                t.hash
            })
            .collect();
        for hash in expired {
            self.send(Request::Tx(hash));
        }
    }

    fn submit(&mut self, hash: [u8; 32]) {
        if let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
            transaction.needs_submit = false;
            self.send(Request::Submit(hash));
        }
    }

    /// What `submit` said. Only a malformed transaction (`tem…`) is known
    /// never to apply: anything else may still, or may have already, after
    /// a restart. It is settled by the stream, or by looking for it once
    /// past its last ledger.
    fn submitted(&mut self, hash: [u8; 32], result: Option<&Value>) {
        let Some(code) = result.and_then(api::submit_result) else {
            return;
        };
        if code.starts_with("tem") {
            self.give_up(hash);
        } else if (code.starts_with("ter") || code.starts_with("tel"))
            && let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash)
        {
            transaction.needs_submit = true;
        }
    }

    /// The answer to looking for a transaction past its last ledger.
    fn looked_up(&mut self, hash: [u8; 32], outcome: Result<Value, ApiError>) {
        match outcome {
            Ok(result) => match api::tx_result(&result).filter(|r| r.validated) {
                Some(record) if self.phase == Phase::Synced => self.transaction(&record),
                // Not validated yet, or unreadable: ask again later.
                _ => self.look_again(hash),
            },
            Err(error) if error.error == "txnNotFound" && error.searched_all == Some(true) => {
                self.give_up(hash);
            }
            // Not every ledger could be searched: ask again at the next.
            Err(_) => self.look_again(hash),
        }
    }

    fn look_again(&mut self, hash: [u8; 32]) {
        if let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
            transaction.looked_up = false;
        }
    }

    /// Our transaction never applied: its scheme cancels its settlement,
    /// and the account's sequence is read again.
    fn give_up(&mut self, hash: [u8; 32]) {
        let Some(position) = self.in_flight.iter().position(|t| t.hash == hash) else {
            return;
        };
        let transaction = self.in_flight.remove(position);
        self.tell(News::Cancelled {
            scheme: transaction.scheme,
            hash: transaction.hash,
        });
        self.send(Request::AccountInfo);
    }

    /// A scheme's lookup, answered: only a transaction found validated is
    /// news. Not found, or not validated yet, it waits, to be asked again.
    fn found(&mut self, outcome: Result<Value, ApiError>) {
        let Ok(result) = outcome else {
            return;
        };
        if let Some(record) = api::tx_result(&result).filter(|r| r.validated) {
            self.tell(News::Found(record));
        }
    }

    /// A validated transaction that touches the account: it moves the
    /// holding if it is newer than what the account read, settles one of
    /// ours in flight whatever its place, and is told to every scheme.
    fn transaction(&mut self, record: &TxRecord) {
        if !record.validated {
            return;
        }
        if !self.bookable_record(record) {
            self.failed = Some(TOO_MUCH_XRP);
            return;
        }
        let at = (record.ledger_index, record.index);
        let account = self.account();
        if self.at.is_none_or(|seen| at > seen) {
            self.at = Some(at);
            if record.account == account {
                self.sequence = self.sequence.max(record.sequence.saturating_add(1));
            }
            if let Some(change) = record.change_of(account) {
                self.balance = Some(change.after);
                if let Some(count) = change.owner_count {
                    self.owner_count = count;
                }
            }
        }
        let ours = self
            .in_flight
            .iter()
            .position(|t| t.hash == record.hash)
            .map(|position| {
                let transaction = self.in_flight.remove(position);
                Ours {
                    scheme: transaction.scheme,
                    drops: transaction.drops,
                }
            });
        self.tell(News::Transaction {
            record: record.clone(),
            ours,
        });
    }

    /// Whether the amounts `record` names for the account fit the books.
    fn bookable_record(&self, record: &TxRecord) -> bool {
        let change = record.change_of(self.account());
        [
            Some(record.fee),
            record.delivered,
            change.map(|change| change.before),
            change.map(|change| change.after),
        ]
        .into_iter()
        .flatten()
        .all(|drops| drops <= MAX_ACCOUNT_DROPS)
    }
}

/// What is kept with the books.
impl XrplAccount {
    /// A number that changes whenever what [`state`](ExternalAsset::state)
    /// keeps does, cheaply: the node writes the state only then. Except
    /// the newest ledger seen, which changes as each ledger closes, and is
    /// written with the rest: an older ledger of the same chain tells a
    /// reset as well.
    fn fingerprint(&self) -> u64 {
        let mut mix: u64 = 0xcbf2_9ce4_8422_2325;
        let mut add = |value: u64| {
            mix ^= value;
            mix = mix.wrapping_mul(0x0100_0000_01b3);
        };
        let head = |hash: &[u8; 32]| {
            let mut head = [0u8; 8];
            head.copy_from_slice(hash.get(..8).unwrap_or(&[0; 8]));
            u64::from_be_bytes(head)
        };
        add(u64::from(self.sequence));
        for transaction in &self.in_flight {
            add(head(&transaction.hash) ^ u64::from(transaction.executed));
        }
        add(self.in_flight.len() as u64);
        for never_sent in &self.never_sent {
            add(head(&never_sent.hash));
        }
        add(self.never_sent.len() as u64);
        mix
    }

    fn state_value(&self) -> Value {
        let in_flight: Vec<Value> = self
            .in_flight
            .iter()
            .map(|t| {
                json!({
                    "scheme": t.scheme.as_str(),
                    "blob": hex_upper(&t.blob),
                    "drops": t.drops,
                    "fee": t.fee,
                    "sequence": t.sequence,
                    "last_ledger": t.last_ledger,
                    "submitted_at": t.submitted_at,
                    "executed": t.executed,
                })
            })
            .collect();
        let never_sent: Vec<Value> = self
            .never_sent
            .iter()
            .map(|n| json!({ "scheme": n.scheme.as_str(), "hash": hex_upper(&n.hash) }))
            .collect();
        json!({
            "sequence": self.sequence,
            "in_flight": in_flight,
            "never_sent": never_sent,
            "newest": self.newest.map(|newest| json!([newest.index, newest.close_time])),
        })
    }

    fn restore(&mut self, state: &[u8]) -> Option<()> {
        let value: Value = serde_json::from_slice(state).ok()?;
        let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
        self.sequence = number(value.get("sequence")?)?;
        self.newest = match value.get("newest") {
            None | Some(Value::Null) => None,
            Some(newest) => Some(Validated {
                index: number(newest.get(0)?)?,
                close_time: newest.get(1)?.as_u64()?,
            }),
        };
        // More than the account keeps is a state it did not write: refused,
        // rather than orphaning a settlement by dropping it.
        let in_flight = value.get("in_flight")?.as_array()?;
        let never_sent = value.get("never_sent")?.as_array()?;
        if in_flight.len() > MAX_IN_FLIGHT || never_sent.len() > MAX_NEVER_SENT {
            return None;
        }
        for entry in in_flight {
            self.in_flight.push(restore_in_flight(entry)?);
        }
        for entry in never_sent {
            self.never_sent.push(NeverSent {
                scheme: Name::new(entry.get("scheme")?.as_str()?).ok()?,
                hash: from_hex(entry.get("hash")?.as_str()?)?.try_into().ok()?,
            });
        }
        // Each transaction once, in flight or never sent, and every one in
        // flight below the next sequence: what the account writes.
        let mut seen = BTreeSet::new();
        let distinct = (self.in_flight.iter().map(|t| t.hash))
            .chain(self.never_sent.iter().map(|n| n.hash))
            .all(|hash| seen.insert(hash));
        let ordered = self.in_flight.iter().all(|t| t.sequence < self.sequence);
        (distinct && ordered).then_some(())
    }
}

/// One transaction in flight, as the account wrote it.
fn restore_in_flight(entry: &Value) -> Option<InFlight> {
    let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
    let blob = from_hex(entry.get("blob")?.as_str()?)?;
    let executed = entry
        .get("executed")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    Some(InFlight {
        scheme: Name::new(entry.get("scheme")?.as_str()?).ok()?,
        hash: crate::codec::hash(&blob),
        blob,
        drops: entry
            .get("drops")?
            .as_u64()
            .filter(|&drops| drops <= MAX_ACCOUNT_DROPS)?,
        fee: entry
            .get("fee")?
            .as_u64()
            .filter(|&fee| fee <= MAX_ACCOUNT_DROPS)?,
        sequence: number(entry.get("sequence")?)?,
        last_ledger: number(entry.get("last_ledger")?)?,
        submitted_at: number(entry.get("submitted_at")?)?,
        executed,
        needs_submit: executed,
        looked_up: false,
    })
}

/// What [`ExternalAsset::check_invariants`] asserts, in parts.
impl XrplAccount {
    fn check_in_flight(&self) {
        let mut seen = BTreeSet::new();
        let distinct = (self.in_flight.iter().map(|t| t.hash))
            .chain(self.never_sent.iter().map(|n| n.hash))
            .all(|hash| seen.insert(hash));
        assert!(distinct, "each transaction once, in flight or never sent");
        for transaction in &self.in_flight {
            assert!(
                transaction.sequence < self.sequence,
                "in flight below the next sequence"
            );
            assert!(
                !transaction.needs_submit || transaction.executed,
                "only what the node executed is submitted"
            );
            assert!(
                !transaction.looked_up || transaction.executed,
                "only what the node executed is looked up"
            );
            assert!(transaction.drops <= MAX_ACCOUNT_DROPS && transaction.fee <= MAX_ACCOUNT_DROPS);
        }
    }

    fn check_phase(&self) {
        match self.phase {
            Phase::Disconnected => {
                assert!(self.pending.is_empty(), "nothing asked while disconnected");
                assert!(self.network_id.is_none(), "a network is a connection's");
                assert!(!self.ask_account, "asked again on a connection");
                assert!(self.buffer.is_empty());
                assert!(!self.funding);
            }
            Phase::Synced => {
                assert!(self.buffer.is_empty(), "what was held is told once synced");
                assert!(self.at.is_some(), "a synced account read its holding");
            }
            Phase::Starting { .. } | Phase::CatchingUp { .. } | Phase::Unfunded => {}
        }
    }
}

impl ExternalAsset for XrplAccount {
    fn id(&self) -> AssetId {
        self.id
    }

    fn address(&self) -> Option<String> {
        Some(self.account().to_address())
    }

    fn holdings(&self) -> Vec<Holding> {
        self.balance
            .map(|balance| Holding {
                code: "XRP".to_string(),
                issuer: None,
                amount: u128::from(balance),
                scale: 6,
                reserve: u128::from(self.reserve_drops(self.owner_count)),
            })
            .into_iter()
            .collect()
    }

    fn status(&self) -> LedgerStatus {
        if let Some(why) = self.failed {
            return LedgerStatus::Failed(why);
        }
        match self.phase {
            Phase::Disconnected => LedgerStatus::Connecting,
            Phase::Starting { .. } | Phase::CatchingUp { .. } => LedgerStatus::Syncing,
            Phase::Unfunded => LedgerStatus::Unfunded,
            Phase::Synced => LedgerStatus::Synced,
        }
    }

    fn connection(&self) -> Option<&str> {
        Some(&self.config.server)
    }

    fn on_ledger(&mut self, now_ms: u64, input: LedgerInput<'_>) {
        self.now_ms = self.now_ms.max(now_ms);
        if self.failed.is_some() {
            return;
        }
        match input {
            LedgerInput::Connected => self.connected(),
            LedgerInput::Disconnected => self.disconnected(),
            LedgerInput::Message(bytes) => {
                self.heard_ms = self.now_ms;
                self.message(bytes);
            }
            LedgerInput::Funded(_) => {
                self.funding = false;
                if self.phase == Phase::Unfunded {
                    self.send(Request::AccountInfo);
                }
            }
        }
    }

    fn deadline(&self) -> Option<u64> {
        if self.failed.is_some() {
            return None;
        }
        let request = self
            .pending
            .values()
            .map(|(_, sent)| sent.saturating_add(REQUEST_TIMEOUT_MS))
            .min();
        let fund = (self.phase == Phase::Unfunded && !self.funding && self.config.faucet.is_some())
            .then_some(self.fund_at_ms);
        let silence = (self.phase != Phase::Disconnected)
            .then_some(self.heard_ms.saturating_add(LEDGER_SILENCE_MS));
        [request, fund, silence].into_iter().flatten().min()
    }

    fn on_timer(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
        if self.failed.is_some() {
            return;
        }
        let late = self
            .pending
            .values()
            .any(|(_, sent)| sent.saturating_add(REQUEST_TIMEOUT_MS) <= now_ms);
        // A connection that went quiet may be half open: made anew.
        let silent = self.phase != Phase::Disconnected
            && self.heard_ms.saturating_add(LEDGER_SILENCE_MS) <= now_ms;
        if late || silent {
            self.reconnect();
            return;
        }
        if self.phase == Phase::Unfunded {
            self.fund();
        }
    }

    fn has_faucet(&self) -> bool {
        self.config.faucet.is_some()
    }

    /// The owner's top-up: the same request as for an account that does
    /// not exist, at the same pace.
    fn ask_faucet(&mut self) -> Result<(), SchemeError> {
        if self.config.faucet.is_none() {
            return Err(SchemeError::Unsupported);
        }
        // Its answer is taken on a connection; while there is none, the
        // faucet waits.
        if self.phase == Phase::Disconnected || self.funding || self.fund_at_ms > self.now_ms {
            return Err(SchemeError::Unavailable);
        }
        self.fund();
        Ok(())
    }

    fn told(&mut self) {
        self.news.clear();
    }

    fn take_effects(&mut self, out: &mut Vec<AssetEffect>) {
        out.append(&mut self.effects);
    }

    fn detach(&mut self, scheme: Name) {
        self.followers.remove(&scheme);
    }

    fn state(&self) -> Vec<u8> {
        serde_json::to_vec(&self.state_value()).unwrap_or_default()
    }

    fn state_version(&self) -> u64 {
        self.fingerprint()
    }

    fn check_invariants(&self) {
        assert!(self.in_flight.len() <= MAX_IN_FLIGHT);
        assert!(self.never_sent.len() <= MAX_NEVER_SENT);
        assert!(self.pending.len() <= MAX_PENDING);
        assert!(self.buffer.len() <= MAX_BUFFERED);
        assert!(self.followers.len() <= MAX_SCHEMES);
        assert!(self.news.len() <= MAX_NEWS);
        assert!(self.effects.len() <= MAX_EFFECTS);
        assert!(self.next_id >= 1, "request IDs start at 1");
        self.check_in_flight();
        self.check_phase();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const NEWEST: Validated = Validated {
        index: 5_000_000,
        close_time: 800_000_000,
    };

    // spec/decisions/ledger-resets.md: a ledger below the newest seen that
    // closed later is a reset only on the testnet. On mainnet, devnet or any
    // other network it is never taken, whatever lets the account run there;
    // one that closed earlier is a server behind, anywhere.
    #[test]
    fn only_the_testnet_starts_again() {
        let lower_later = Validated {
            index: 2_000,
            close_time: 800_000_004,
        };
        let lower_earlier = Validated {
            index: 4_999_990,
            close_time: 799_999_960,
        };
        let higher = Validated {
            index: 5_000_001,
            close_time: 800_000_004,
        };
        assert_eq!(chain(Some(NEWEST), 1, lower_later), Chain::Restarted);
        for network in [0, 2, 21_338, u32::MAX] {
            assert!(!resets(network));
            assert_eq!(chain(Some(NEWEST), network, lower_later), Chain::WentBack);
        }
        for network in [0, 1, 2, 21_338] {
            assert_eq!(chain(Some(NEWEST), network, lower_earlier), Chain::Behind);
            assert_eq!(chain(Some(NEWEST), network, NEWEST), Chain::Current);
            assert_eq!(chain(Some(NEWEST), network, higher), Chain::Current);
            assert_eq!(chain(None, network, lower_later), Chain::Current);
        }
    }

    // spec/areas/xrpl.md#the-account: schemes on one account take its
    // sequences in order, never the same one; and what a scheme signed and
    // never sent is remembered for that scheme alone, until it reconciles
    // it.
    #[test]
    fn schemes_on_one_account_never_share_a_sequence() {
        let (a, b) = (dassie_ledger::name!("a"), dassie_ledger::name!("b"));
        let mut account = XrplAccount::new(FIRST, "", &[1; 32], &[]).unwrap();
        account.phase = Phase::Synced;
        account.at = Some((10, u32::MAX));
        account.sequence = 5;
        account.balance = Some(1_000_000_000);
        account.validated = 10;
        let to = Wallet::from_entropy(&[2; 32]).account();
        let first = account.sign_payment(a, to, 1_000, Vec::new()).unwrap();
        let second = account.sign_payment(b, to, 1_000, Vec::new()).unwrap();
        let third = account.sign_payment(a, to, 1_000, Vec::new()).unwrap();
        let sequences: Vec<u32> = account.in_flight.iter().map(|t| t.sequence).collect();
        assert_eq!(sequences, [5, 6, 7]);
        assert!(account.execute(b, second));
        // Past their last ledger, the two never executed are dropped.
        account.validated = 11 + LEDGER_WINDOW;
        account.expire();
        assert_eq!(account.resume(b, second), Resumed::InFlight);
        assert_eq!(account.resume(b, first), Resumed::Unknown, "another's");
        assert_eq!(account.resume(a, first), Resumed::NeverSent);
        assert_eq!(account.resume(a, first), Resumed::Unknown, "reconciled");
        assert!(!account.execute(a, third), "never sent, never executed");
        assert_eq!(account.resume(a, third), Resumed::Unknown, "executed");
        account.check_invariants();
    }

    // spec/decisions/ledger-resets.md: on a network that is never reset, an
    // account that held anything and is gone stops the account: nothing is
    // cancelled or written off, and the faucet is not asked.
    #[test]
    fn an_account_gone_from_a_network_never_reset_stops_the_scheme() {
        use dassie_settlement::{Effect, SettlementScheme};

        for (network, stops) in [(0, true), (2, true), (1, false)] {
            let node = dassie_ledger::name!("node");
            let mut account = XrplAccount::new(FIRST, "", &[1; 32], &[]).unwrap();
            let mut scheme =
                crate::XrplScheme::new(account.account(), node, Some(5_000), &[]).unwrap();
            account.phase = Phase::Synced;
            account.at = Some((NEWEST.index, u32::MAX));
            account.follow(scheme.id(), Some((NEWEST.index, u32::MAX)), true);
            account.network_id = Some(network);
            account.absent();
            let mut effects = Vec::new();
            scheme.on_asset(0, &mut account, &mut effects);
            account.told();
            if stops {
                assert_eq!(account.status(), LedgerStatus::Failed(ACCOUNT_GONE));
                assert!(effects.is_empty(), "{effects:?}");
                assert_eq!(scheme.balance(), 5_000);
            } else {
                assert_eq!(account.status(), LedgerStatus::Unfunded);
                assert!(effects.contains(&Effect::Reconcile {
                    amount: 5_000,
                    gain: false
                }));
            }
            account.check_invariants();
            scheme.check_invariants();
        }
    }
}

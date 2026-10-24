//! The node's account on an XRP Ledger network, as an external asset
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
//! - each ledger that closes, and each transaction a scheme looked up;
//! - the trust lines its schemes follow, read at each sync as of the same
//!   ledger as the account, and an issuer's account a scheme asked for.
//!
//! Each scheme keeps its own cursor into the transactions, and tells the
//! account where it is ([`XrplAccount::follow`]): on each connection the
//! account walks its history back to the earliest cursor of its schemes. What
//! a scheme not on the account (stopped, or removed) misses of its own
//! transactions, the account keeps for it until it is back: one that can
//! never apply as never sent, one the ledger validated as kept, so that the
//! scheme books it as its own when it reads it again, or, when the history
//! no longer reaches back to it, is told it did not, and books it as its
//! own once it has looked it up. So is what it was told of its own in the
//! input whose bookings the books refused, which stopped it.
//!
//! It runs only on its own network ([`Network`], which its kind names): a
//! server that reports another network, or none, stops it before the
//! account is read, so another network's XRP is never booked, written off
//! or signed for (spec/decisions/real-money.md). It acts only on what a
//! ledger a quorum of its own UNL validated proves: its anchor, from its
//! publishers' lists ([`anchors`]), which alone moves what it takes as
//! validated. An anchor that the kept one's chain does not hold is a new
//! chain, which only a testnet reset makes, and only on a network
//! [`resets`] allows (spec/decisions/ledger-resets.md). What it holds, its
//! lines, their issuers, the reserve and a pool are proven at the anchor;
//! every transaction it tells by its branch in a proven ledger, found by
//! the history the proven objects name; an absence only when proven
//! ([`history`], spec/designs/xrpl-validation.md#4-what-is-proven-and-how).
//! What a server says otherwise is a hint.

mod anchors;
mod history;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use dassie_ledger::{MAX_TOTAL, Name};
use dassie_settlement::{
    AssetEffect, AssetId, Detach, ExternalAsset, Freeze, Holding, LedgerInput, LedgerStatus,
    SchemeError,
};
use serde_json::{Value, json};

use crate::amount::{Asset, Currency, TokenAmount, TokenValue};
use crate::anchor::{KeptAnchor, ProvenLists};
use crate::api::{
    self, AccountInfo, AccountSettings, ApiError, LedgerClosed, Message, TrustLine, TxRecord,
};
use crate::codec::{Amount, Memo, Payment, Signing, Transact, TrustSet};
use crate::keys::{AccountId, PublicKey, Wallet};
use crate::network::{Network, TESTNET};
use crate::peg::PoolReading;
use crate::records::Decoded;
use crate::unl::{Secp256k1Signatures, Unl, ValidationSignatures};
use crate::validator_list::{MAX_PUBLISHERS, UnlConfig};
use crate::{UNITS_PER_DROP, from_hex, hex_upper};

pub use anchors::{
    Doubt, FETCH_TIMEOUT_MS, LIST_REFRESH_MS, LIST_RETRY_MS, MAX_MANIFEST_ASKS, STALL_MS,
};
use anchors::{Fetching, Round};
pub use history::{HISTORY_BOUND, MAX_PROOFS, MAX_PROVING, MAX_WALKED};
use history::{Proof, Walk};

/// The scale the account reports a token it holds at: fifteen decimals,
/// the most a token's sixteen digits leave below a unit of it.
pub const TOKEN_SCALE: u8 = 15;

/// The node's first XRPL testnet account: the one whose keys are at
/// `node/settlement-module/xrpl-testnet/seed-testnet`
/// (spec/areas/identity.md#derivation-chain).
pub const FIRST: AssetId = TESTNET.first();
/// The testnet's proof server, which the project runs: a rippled that also
/// serves the proof methods (spec/designs/xrpl-validation.md#3-the-proof-api)
/// beside every method of a stock server.
pub const DEFAULT_SERVER: &str = TESTNET.server;
/// The testnet's faucet.
pub const DEFAULT_FAUCET: &str = crate::network::TESTNET_FAUCET;

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
/// Most transactions the hints name, to prove before the history needs
/// them; the rest the history finds.
const MAX_HINTS: usize = 64;
/// Most skip lists kept to prove the ledgers of transactions, and asked
/// for at once.
const MAX_LISTS: usize = 16;
const MAX_LIST_ASKS: usize = 2;
/// How many ledgers a transaction may take to get in.
const LEDGER_WINDOW: u32 = 20;
/// How long a request may go unanswered before the connection is made
/// anew.
const REQUEST_TIMEOUT_MS: u64 = 20_000;
/// How often the faucet is asked, while the account does not exist.
const FUND_RETRY_MS: u64 = 60_000;
/// Most requests waiting for an answer: a submission for each transaction
/// in flight, the proofs of transactions and the skip lists that prove
/// their ledgers, the server's state, the subscription, a read of what the
/// schemes asked for and a page of history, and an anchoring round's proof
/// or its manifests.
const MAX_PENDING: usize = MAX_IN_FLIGHT + MAX_PROVING + MAX_LIST_ASKS + 4 + 1 + MAX_MANIFEST_ASKS;
/// Most transactions remembered as never sent, until their schemes have
/// reconciled them. Past it, the oldest is forgotten, and its scheme
/// looks it up by its hash, which finds nothing and leaves it pending.
const MAX_NEVER_SENT: usize = 2 * MAX_IN_FLIGHT;
/// Most transactions kept for schemes that stopped, or were removed, after
/// the ledger validated them: each scheme's are what it had in flight, and
/// it signs nothing more until it has read them again.
const MAX_PARKED: usize = MAX_IN_FLIGHT * MAX_SCHEMES;
/// Most news kept between two tellings: one message's worth, at most the
/// proven transactions kept and the one just proven, each of ours in
/// flight found or cancelled, what was kept for schemes and not read
/// again, the schemes' lookups, and for each scheme its line, its issuer
/// and its pool.
const MAX_NEWS: usize =
    MAX_PROOFS + 1 + 2 * MAX_IN_FLIGHT + MAX_PARKED + MAX_LOOKUPS + 4 + 3 * MAX_SCHEMES;
/// Most effects between two takings: one message's worth, and a fetch of
/// each publisher's list.
const MAX_EFFECTS: usize = MAX_PENDING + MAX_IN_FLIGHT + 4 + MAX_PUBLISHERS;
/// Most bytes of config: a server, a faucet and the most publishers, each
/// with its key and its sites.
const MAX_CONFIG: usize = 4096;
/// How long the connection may say nothing, no ledger closing, before it
/// is made anew: a ledger closes every few seconds.
const LEDGER_SILENCE_MS: u64 = 30_000;
/// The most drops the books can hold for the account: [`MAX_TOTAL`] in
/// units. A server that reports more for it is no ledger to follow.
pub(crate) const MAX_ACCOUNT_DROPS: u64 = MAX_TOTAL / UNITS_PER_DROP;
/// Why the account stops when a server reports more than that.
const TOO_MUCH_XRP: &str = "the server reported more XRP for the account than the books can hold";
/// The network ID the testnet's servers report: mainnet's is 0, and
/// devnet's 2. Nothing signs for a server that reports another
/// (spec/decisions/real-money.md).
pub const TESTNET_NETWORK_ID: u32 = TESTNET.id;
/// Why the account stops when its validators validate a ledger below its
/// anchor that closed later, on a network that is never reset.
const CHAIN_WENT_BACK: &str =
    "the validators went back on a network that is never reset; nothing was written off";
/// Why the account stops when its validators validate a later ledger off
/// its anchor's chain, on a network that is never reset.
const CHAIN_FORKED: &str = "the validators validated another chain on a network that is never reset; nothing was written off";
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

/// The newest validated ledger a state written before anchors kept: its
/// index, and when it closed, in seconds since the XRP Ledger's epoch. A
/// hint, without a hash, which the first anchor replaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Validated {
    index: u32,
    close_time: u64,
}

/// A trust line the account follows for a scheme on it: of its token's
/// issuer, in its currency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineKey {
    /// The token's issuer.
    pub issuer: AccountId,
    /// The token's currency.
    pub currency: Currency,
}

impl LineKey {
    /// The token, as an asset.
    #[must_use]
    pub const fn asset(&self) -> Asset {
        Asset::Token {
            currency: self.currency,
            issuer: self.issuer,
        }
    }
}

/// Where the account connects, and where it asks for test XRP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// A WebSocket URL, `wss://` (or `ws://`, which the node allows only
    /// where it allows private addresses). A server that reports another
    /// network than the account's, or none, stops the account.
    pub server: String,
    /// The faucet, an HTTPS URL; `None` to never ask.
    pub faucet: Option<String>,
    /// The validator list publishers it trusts, and the floor on its UNL:
    /// its network's, or the owner's, which replace them entirely
    /// (spec/designs/xrpl-validation.md#10-configuration-and-state).
    pub unl: UnlConfig,
}

impl Config {
    /// The config of a testnet account in `text`
    /// ([`parse_on`](Self::parse_on)).
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidConfig`] if it is not one.
    pub fn parse(text: &str) -> Result<Self, SchemeError> {
        Self::parse_on(text, &TESTNET)
    }

    /// The config of an account on `network` in `text`: empty for the
    /// network's defaults, or JSON with `server` and `faucet` (a URL or
    /// `null`), each optional, and the owner's validator list `publishers`
    /// with perhaps a `floor`, as [`UnlConfig::with_override`] reads them.
    /// A network without a faucet takes none but `null`
    /// (spec/decisions/mainnet-schemes.md); a floor comes only with
    /// publishers.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidConfig`] if it is not that.
    pub fn parse_on(text: &str, network: &Network) -> Result<Self, SchemeError> {
        let mut config = Self {
            server: network.server.to_string(),
            faucet: network.faucet.map(ToString::to_string),
            unl: UnlConfig::for_network(network.id).ok_or(SchemeError::InvalidConfig)?,
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
        let mut unl = serde_json::Map::new();
        for (key, field) in object {
            match (key.as_str(), field) {
                ("publishers" | "floor", _) => {
                    unl.insert(key.clone(), field.clone());
                }
                ("server", Value::String(server))
                    if server.starts_with("wss://") || server.starts_with("ws://") =>
                {
                    config.server.clone_from(server);
                }
                ("faucet", Value::String(faucet))
                    if faucet.starts_with("https://") && network.faucet.is_some() =>
                {
                    config.faucet = Some(faucet.clone());
                }
                ("faucet", Value::Null) => config.faucet = None,
                _ => return Err(SchemeError::InvalidConfig),
            }
        }
        if !unl.is_empty() {
            config.unl = (config.unl)
                .with_override(&Value::Object(unl))
                .ok_or(SchemeError::InvalidConfig)?;
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
    /// The token it pays, for a Payment of a token.
    pub token: Option<TokenValue>,
    /// Whether it settles with a peer: a Payment. A `TrustSet` does not.
    pub settles: bool,
    /// Whether it was kept for its scheme while the scheme was not on the
    /// account, and is told again for it alone: the schemes that were on
    /// the account were told it when the ledger validated it.
    pub again: bool,
}

/// What the account learnt for the schemes on it, in the order it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum News {
    /// Up to date as of this account state: every transaction before it
    /// was told.
    Synced(AccountInfo),
    /// A validated transaction that touches the account, proven in a
    /// ledger of the anchor's chain, as the history the proven state names
    /// found it: each scheme books it once, from its own cursor.
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
        /// Whether it was a settlement, rather than a `TrustSet`.
        settles: bool,
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
    /// A transaction of a scheme's, kept for it while it was not on the
    /// account, that it did not read again once back: the history the
    /// account walked had a gap there, which the server no longer has. What
    /// it did is in the balance the scheme synced to; the scheme looks it up
    /// by its hash, and books it as one it never read, by the amount it
    /// settled, once proven.
    Unread {
        /// The transaction.
        hash: [u8; 32],
        /// What it was, as the scheme signed it.
        ours: Ours,
    },
    /// A transaction a scheme looked up, proven in a ledger of the anchor's
    /// chain.
    Found {
        /// The hash the scheme looked up.
        hash: [u8; 32],
        /// What the ledger holds under it.
        record: TxRecord,
    },
    /// A trust line a scheme follows, as proven at the anchor of the
    /// [`Synced`](Self::Synced) that comes right after it: `None` if the
    /// account has none.
    Line {
        /// Which line.
        key: LineKey,
        /// The line.
        line: Option<TrustLine>,
    },
    /// An issuer's account a scheme asked for, or of a line a scheme
    /// follows, as proven at the anchor: `None` if it does not exist.
    Issuer {
        /// The issuer.
        issuer: AccountId,
        /// What its account says about its tokens.
        settings: Option<AccountSettings>,
    },
    /// The AMM pool of XRP and a token a scheme asked for
    /// ([`XrplAccount::read_pool`]), both its balances as proven at one
    /// anchor: `None` if there is none.
    Pool {
        /// The token, as its issuer and currency.
        key: LineKey,
        /// The pool.
        pool: Option<PoolReading>,
    },
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
    /// The token a Payment of a token pays.
    token: Option<TokenValue>,
    /// Whether it is a `TrustSet`, which settles nothing.
    trust_set: bool,
    /// The reserve it may take, in drops: a `TrustSet` that makes the
    /// account's side of a line hold one.
    reserve: u64,
    sequence: u32,
    last_ledger: u32,
    submitted_at: u32,
    /// Whether the node executed it: its pending transfer is on disk, so
    /// it may be submitted. One the node never executed is never sent.
    executed: bool,
    /// To submit when the connection allows.
    needs_submit: bool,
    /// Past its last ledger, with a sequence the account used, and its
    /// proof asked for on this connection.
    looked_up: bool,
}

/// A transaction signed and never sent, dropped past its last ledger: kept
/// until its scheme has reconciled it, so that it is cancelled rather than
/// looked for. So is one that can never apply while its scheme is not on
/// the account to be told so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NeverSent {
    scheme: Name,
    hash: [u8; 32],
}

/// A transaction of a scheme that was not on the account when the ledger
/// validated it, because it stopped or was removed: kept until the scheme
/// is back and the account has synced, so that the scheme books it as its
/// own when it reads it again, never as a withdrawal. One it did not read
/// again it is told of ([`News::Unread`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Parked {
    scheme: Name,
    hash: [u8; 32],
    drops: u64,
    /// The token a Payment of a token paid.
    token: Option<TokenValue>,
    /// Whether it is a `TrustSet`.
    trust_set: bool,
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
    Submit([u8; 32]),
    /// A transaction's proof: in the ledger of this index, which a proven
    /// object or transaction names, or wherever the server finds it.
    TxProof([u8; 32], Option<u32>),
    /// A skip list, by its key, at the ledger of a hash: to prove the
    /// ledger of a transaction.
    Ledger([u8; 32], [u8; 32]),
    /// An anchoring round's state proof at the server's validated ledger,
    /// of everything the account follows.
    Anchor,
    /// A state proof at the anchor, of this hash, of what the schemes asked
    /// for: issuers' accounts and pools.
    Read([u8; 32]),
    /// The account's history from one ledger to another, a page of it: a
    /// hint, which names transactions to prove sooner.
    AccountTx(u32, u32),
    /// The validations of the ledger of this hash.
    Validations([u8; 32]),
    /// The newest manifest of a validator, by its master key.
    Manifest(PublicKey),
    /// A skip list, by its key, at the ledger of a hash: to tell a later
    /// anchor's ancestry.
    SkipList([u8; 32], [u8; 32]),
}

/// Where the account stands with its ledger.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// No connection.
    Disconnected,
    /// Connected; waiting for the server's state, which must be of the
    /// account's network, then the subscription, then an anchor, which
    /// proves the account's state.
    Starting { subscribed: bool, state: bool },
    /// Proving the history from the earliest cursor of its schemes up to
    /// `info`'s ledger, the anchor.
    CatchingUp { info: AccountInfo },
    /// The account does not exist: waiting for funds.
    Unfunded,
    /// Up to date.
    Synced,
}

/// An XRPL account, on the network its kind names.
pub struct XrplAccount {
    id: AssetId,
    network: &'static Network,
    /// The schemes that may settle through it.
    schemes: Vec<Name>,
    config: Config,
    wallet: Wallet,
    // Kept with the books.
    sequence: u32,
    in_flight: Vec<InFlight>,
    never_sent: Vec<NeverSent>,
    parked: Vec<Parked>,
    /// How many times the account was written off: a scheme that kept
    /// another count missed a write-off while it was not on the account.
    write_offs: u64,
    /// The newest anchor: a ledger a quorum of its UNL validated. Kept
    /// with the books whenever the rest is: any anchor of the chain tells
    /// a server behind from a chain started again.
    anchor: Option<KeptAnchor>,
    /// The newest ledger a state written before anchors kept: a hint the
    /// first anchor replaces.
    hint: Option<Validated>,
    /// The UNL, from its publishers' lists, with the validators'
    /// manifests: kept with the books.
    unl: Unl,
    /// Counted up whenever what the UNL keeps changes, so that it is
    /// written.
    unl_changes: u64,
    /// How validation signatures are checked: for real, but in a
    /// simulation.
    signatures: Box<dyn ValidationSignatures + Send>,
    // Learnt from the ledger.
    phase: Phase,
    /// Each publisher's list, as it is fetched, in the config's order.
    fetches: Vec<Fetching>,
    /// The anchoring round in flight.
    round: Option<Round>,
    /// Until when the account waits after a server refused a proof, before
    /// it connects anew; 0 while it does not.
    stalled_until_ms: u64,
    /// Why it waits, while it does.
    waiting: Option<&'static str>,
    /// The lies it caught, by kind.
    doubts: BTreeMap<Doubt, u64>,
    /// Where in the UNL's validators that did not count the next round's
    /// questions for newer manifests start.
    manifests_from: usize,
    /// Whether the reserve changed, by the hints, since the schemes were
    /// last told of a closed ledger.
    reserve_changed: bool,
    /// The walk of the history back from the anchor, while it goes on.
    walk: Option<Walk>,
    /// Transactions proven, by their hashes, until they are told.
    proofs: BTreeMap<[u8; 32], Proof>,
    /// Skip lists proven to prove the ledgers of transactions.
    lists: ProvenLists,
    /// Transactions the hints named, to prove.
    hints: BTreeSet<[u8; 32]>,
    /// Transactions the schemes look up.
    lookups: BTreeSet<[u8; 32]>,
    /// Our transactions whose proof the server did not find, on this
    /// connection.
    tried: BTreeSet<[u8; 32]>,
    /// The account's sequence at the anchor, as proven.
    proven_sequence: u32,
    /// The issuers' accounts the schemes asked for: read at the anchor, or
    /// told with the next.
    issuers_asked: BTreeSet<AccountId>,
    /// The keys of a read at the anchor of what the schemes asked for, on
    /// its way.
    reading: Option<Vec<[u8; 32]>>,
    /// The pools the schemes asked for, each with the account its `AMM`
    /// object last named: told with the next anchor that proves it.
    pools: BTreeMap<LineKey, history::PoolWatch>,
    /// How often the history had a gap beyond the history bound, which the
    /// server no longer had.
    gaps: u64,
    /// How often one of our transactions could be proven neither applied
    /// nor absent.
    unproven: u64,
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
    /// The last transaction told, or the anchor the holding was proven at:
    /// one at or before it is older than the balance.
    at: Option<(u32, u32)>,
    fund_at_ms: u64,
    funding: bool,
    failed: Option<&'static str>,
    now_ms: u64,
    /// When the connection last said anything.
    heard_ms: u64,
    followers: BTreeMap<Name, Follower>,
    /// The schemes the books stopped, until the node restarts: what they
    /// settle in is still on their books.
    stopped: BTreeSet<Name>,
    /// The trust lines its schemes follow, as last proven: `None` until
    /// they were, then the line, or `None` if there is none.
    lines: BTreeMap<LineKey, Option<Option<TrustLine>>>,
    /// The account of each issuer of a line its schemes follow, as last
    /// proven: shown with the line, never kept.
    issuers: BTreeMap<AccountId, AccountSettings>,
    /// What the owner may spend on each scheme's ledger, by the books.
    funds: BTreeMap<Name, u64>,
    news: Vec<News>,
    effects: Vec<AssetEffect>,
}

impl XrplAccount {
    /// The account `id`, on the network its kind names, through which
    /// `schemes` may settle, with the wallet of `entropy`, connecting as
    /// `config` says, from the state the books kept.
    ///
    /// # Errors
    ///
    /// If `id` is of no XRPL network, or the config or the state is
    /// invalid: a state that names a scheme not among `schemes` is one the
    /// account did not write.
    ///
    /// # Panics
    ///
    /// With more than [`MAX_SCHEMES`] schemes.
    pub fn new(
        id: AssetId,
        schemes: &[Name],
        config: &str,
        entropy: &[u8; 32],
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        assert!(
            schemes.len() <= MAX_SCHEMES,
            "more schemes on an account than there may be"
        );
        let network = Network::of(id.kind).ok_or(SchemeError::UnknownScheme)?;
        let config = Config::parse_on(config, network)?;
        let mut account = Self {
            id,
            network,
            schemes: schemes.to_vec(),
            wallet: Wallet::from_entropy(entropy),
            sequence: 0,
            in_flight: Vec::new(),
            never_sent: Vec::new(),
            parked: Vec::new(),
            write_offs: 0,
            anchor: None,
            hint: None,
            unl: Unl::new(config.unl.clone()),
            unl_changes: 0,
            signatures: Box::new(Secp256k1Signatures),
            fetches: alloc::vec![Fetching::default(); config.unl.publishers.len()],
            config,
            round: None,
            stalled_until_ms: 0,
            waiting: None,
            doubts: BTreeMap::new(),
            manifests_from: 0,
            reserve_changed: false,
            walk: None,
            proofs: BTreeMap::new(),
            lists: ProvenLists::new(),
            hints: BTreeSet::new(),
            lookups: BTreeSet::new(),
            tried: BTreeSet::new(),
            proven_sequence: 0,
            issuers_asked: BTreeSet::new(),
            reading: None,
            pools: BTreeMap::new(),
            gaps: 0,
            unproven: 0,
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
            fund_at_ms: 0,
            funding: false,
            failed: None,
            now_ms: 0,
            heard_ms: 0,
            followers: BTreeMap::new(),
            stopped: BTreeSet::new(),
            lines: BTreeMap::new(),
            issuers: BTreeMap::new(),
            funds: BTreeMap::new(),
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

    /// The network it is on.
    #[must_use]
    pub const fn network(&self) -> &'static Network {
        self.network
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

    /// Its anchor: the newest ledger a quorum of its UNL validated that it
    /// acted from.
    #[must_use]
    pub fn anchor(&self) -> Option<crate::unl::Anchor> {
        self.anchor.map(|kept| kept.anchor)
    }

    /// Its UNL, and the lists and manifests that make it.
    #[must_use]
    pub const fn unl(&self) -> &Unl {
        &self.unl
    }

    /// The lies it caught, by kind (spec/designs/xrpl-validation.md#6-the-account-on-proofs).
    #[must_use]
    pub fn doubts(&self, doubt: Doubt) -> u64 {
        self.doubts.get(&doubt).copied().unwrap_or(0)
    }

    /// How often the history it walked had a gap beyond the history bound
    /// that the server no longer had: what fell in it, its schemes matched
    /// through suspense ([`HISTORY_BOUND`]).
    #[must_use]
    pub const fn history_gaps(&self) -> u64 {
        self.gaps
    }

    /// How often one of its transactions past its last ledger could be
    /// proven neither applied nor absent, as when someone else signed for
    /// the account: it stays pending, for the owner.
    #[must_use]
    pub const fn unproven(&self) -> u64 {
        self.unproven
    }

    /// Checks validation signatures with `signatures` from now on, as a
    /// simulation does with the fake ledger's keyed check
    /// (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation):
    /// only a build with the fake ledger can.
    #[cfg(feature = "fake")]
    pub fn check_validations_with(
        &mut self,
        signatures: impl ValidationSignatures + Send + 'static,
    ) {
        self.signatures = Box::new(signatures);
    }

    /// `scheme` settles through the account, and is at `cursor`; `holds`
    /// whether its books hold anything of it. The account walks its history
    /// back to the earliest cursor of its schemes, and an account gone from a
    /// network that never resets stops it if any of them held anything. A
    /// scheme follows only once it has written off what it held through a
    /// write-off it missed ([`write_offs`](Self::write_offs)), so no cursor
    /// of a chain gone holds the account back.
    ///
    /// # Panics
    ///
    /// For a scheme that may not settle through it.
    pub fn follow(&mut self, scheme: Name, cursor: Option<(u32, u32)>, holds: bool) {
        assert!(
            self.schemes.contains(&scheme),
            "a scheme that may settle through the account"
        );
        self.stopped.remove(&scheme);
        self.followers.insert(scheme, Follower { cursor, holds });
    }

    /// How many times the account was written off, with everything the
    /// books held of it (a testnet reset, or the account gone), kept with
    /// its state. A scheme keeps the count it last saw with its own state:
    /// one that sees another missed a write-off while it was not on the
    /// account (stopped, or removed), and writes off what it held before it
    /// follows the account again.
    #[must_use]
    pub const fn write_offs(&self) -> u64 {
        self.write_offs
    }

    /// Follows the trust line `key` for a scheme on the account: it is
    /// proven at every anchor with the account, its history walked with the
    /// account's, and kept as each transaction leaves it. A scheme follows it before the
    /// account connects for it, as the node connects the account anew for
    /// a scheme activated on it.
    ///
    /// # Panics
    ///
    /// Past [`MAX_SCHEMES`] lines: each scheme follows one at most.
    pub fn follow_line(&mut self, key: LineKey) {
        if self.lines.contains_key(&key) {
            return;
        }
        assert!(
            self.lines.len() < MAX_SCHEMES,
            "more lines than schemes on an account"
        );
        self.lines.insert(key, None);
    }

    /// The trust line `key`, as last proven: `None` until it was, then
    /// the line, or `None` if there is none.
    #[must_use]
    pub fn line(&self, key: &LineKey) -> Option<Option<&TrustLine>> {
        self.lines.get(key)?.as_ref().map(Option::as_ref)
    }

    /// Reads the account of `issuer` for a scheme: what it says, proven at
    /// the anchor, comes as [`News::Issuer`]. At most one issuer for each
    /// scheme is asked for at once.
    pub fn read_issuer(&mut self, issuer: AccountId) {
        if self.issuers_asked.contains(&issuer) || self.issuers_asked.len() < MAX_SCHEMES {
            self.issuers_asked.insert(issuer);
            self.read_asked();
        }
    }

    /// Reads the AMM pool of XRP and the token `key` names for a scheme's
    /// peg guard: what it holds comes as [`News::Pool`], both balances
    /// proven at the anchor, as one state proof of the pool's `AMM` object,
    /// which names its account, and of that account's root and line to the
    /// issuer (spec/designs/xrpl-validation.md#4-what-is-proven-and-how). A
    /// pool read for the first time, or whose object names another account,
    /// is read again once its account is known.
    pub fn read_pool(&mut self, key: LineKey) {
        if let Some(watch) = self.pools.get_mut(&key) {
            watch.asked = true;
        } else if self.pools.len() < MAX_SCHEMES {
            self.pools.insert(
                key,
                history::PoolWatch {
                    account: None,
                    asked: true,
                },
            );
        }
        self.read_asked();
    }

    /// Whether transaction `hash` is in flight, or remembered as never
    /// sent: a scheme never looks such a one up.
    #[must_use]
    pub fn holds_transaction(&self, hash: &[u8; 32]) -> bool {
        self.in_flight.iter().any(|t| t.hash == *hash)
            || self.never_sent.iter().any(|n| n.hash == *hash)
    }

    /// Whether transaction `hash` is in flight, remembered as never sent,
    /// or kept for its scheme, validated while the scheme was not on the
    /// account: the account will still tell the scheme what became of it.
    #[must_use]
    pub fn keeps_transaction(&self, hash: &[u8; 32]) -> bool {
        self.holds_transaction(hash) || self.parked.iter().any(|p| p.hash == *hash)
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
    /// What the account holds above its reserve, after what is in flight,
    /// in drops.
    fn spare(&self) -> u64 {
        let in_flight: u64 = self
            .in_flight
            .iter()
            .map(|t| t.drops.saturating_add(t.fee).saturating_add(t.reserve))
            .sum();
        self.balance
            .map(|balance| balance.saturating_sub(self.reserve_drops(self.owner_count)))
            .unwrap_or(0)
            .saturating_sub(in_flight)
    }

    /// Whether `scheme` may spend `gas` drops of XRP it does not settle in:
    /// on an account the XRP scheme settles from, only within the owner's
    /// funds on its ledger, after the gas in flight, and none while the
    /// books stopped that scheme, whose books still hold the XRP; on any
    /// other, the XRP is off the books, and only the reserve bounds it.
    fn gas_allows(&self, scheme: Name, gas: u64) -> bool {
        let xrp = self.network.xrp_scheme;
        if scheme == xrp {
            return true;
        }
        if self.stopped.contains(&xrp) {
            return false;
        }
        if !self.followers.contains_key(&xrp) {
            return true;
        }
        let funds = self.funds.get(&xrp).copied().unwrap_or(0) / UNITS_PER_DROP;
        let in_flight: u64 = self
            .in_flight
            .iter()
            .filter(|t| t.scheme != xrp)
            .map(|t| t.fee.saturating_add(t.reserve))
            .sum();
        gas.saturating_add(in_flight) <= funds
    }

    /// Whether `scheme` may sign a transaction now that pays `drops` of
    /// XRP and `fee`, and may take `reserve` more.
    fn can_sign(&self, scheme: Name, drops: u64, fee: u64, reserve: u64) -> bool {
        let xrp = self.network.xrp_scheme;
        let gas = if scheme == xrp {
            0
        } else {
            drops.saturating_add(fee).saturating_add(reserve)
        };
        // A scheme back on the account reads its own transactions again
        // before it signs more. Nor does one sign while it is being told of
        // its own (a `TrustSet` it signs as it takes the news): should the
        // books refuse what it was told, the account keeps those for it
        // ([`keep_untaken`](Self::keep_untaken)), within what it may have
        // in flight.
        let reading = self.parked.iter().any(|p| p.scheme == scheme) || self.told_own(scheme);
        self.is_synced()
            && !reading
            && self.sequence < u32::MAX
            && self.in_flight.len() < MAX_IN_FLIGHT
            && self.sequence > 0
            && fee <= MAX_FEE_DROPS
            && drops.saturating_add(fee).saturating_add(reserve) <= self.spare()
            && self.gas_allows(scheme, gas)
    }

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
        self.sign_payment_to(scheme, (destination, None), drops, memos)
    }

    /// Signs a Payment of `drops` to `destination` with its destination
    /// tag, if any, as [`sign_payment`](Self::sign_payment) does: how the
    /// owner pays an account that tells its payers apart by tags, such as a
    /// settlement hub's escrow.
    ///
    /// # Errors
    ///
    /// As [`sign_payment`](Self::sign_payment).
    pub fn sign_payment_to(
        &mut self,
        scheme: Name,
        (destination, tag): (AccountId, Option<u32>),
        drops: u64,
        memos: Vec<Memo>,
    ) -> Result<[u8; 32], SchemeError> {
        let fee = self.fee();
        if drops == 0 || !self.can_sign(scheme, drops, fee, 0) {
            return Err(SchemeError::Unavailable);
        }
        let payment = Payment {
            account: self.account(),
            destination,
            destination_tag: tag,
            amount: Amount::Xrp(drops),
            send_max: None,
            fee,
            sequence: self.sequence,
            last_ledger_sequence: Some(self.validated.saturating_add(LEDGER_WINDOW)),
            flags: 0,
            // A transaction names its network only above 1024, and no
            // account's is (`Network::id`).
            network_id: None,
            memos,
            signing: Signing::by(self.wallet.public_key()),
        };
        let signed = payment.sign(&self.wallet);
        Ok(self.keep(scheme, signed.hash, signed.blob, drops, fee, None, 0))
    }

    /// Signs a Payment of a token `amount` to `destination` for `scheme`,
    /// without paths and never partial, and keeps it in flight, not yet
    /// executed, as [`sign_payment`](Self::sign_payment). Its fee is gas.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] unless the account could sign a Payment
    /// of XRP now, the amount is above zero, the line holds it after what
    /// the scheme has in flight, and the fee fits the gas
    /// ([`gas_allows`](Self::gas_allows)).
    pub fn sign_token_payment(
        &mut self,
        scheme: Name,
        destination: AccountId,
        amount: TokenAmount,
        memos: Vec<Memo>,
    ) -> Result<[u8; 32], SchemeError> {
        let fee = self.fee();
        let positive = !amount.value.is_zero() && !amount.value.is_negative();
        if !positive
            || !amount.is_valid()
            || !self.line_holds(scheme, &amount)
            || !self.can_sign(scheme, 0, fee, 0)
        {
            return Err(SchemeError::Unavailable);
        }
        let payment = Payment {
            account: self.account(),
            destination,
            destination_tag: None,
            amount: Amount::Token(amount),
            send_max: None,
            fee,
            sequence: self.sequence,
            last_ledger_sequence: Some(self.validated.saturating_add(LEDGER_WINDOW)),
            flags: 0,
            network_id: None,
            memos,
            signing: Signing::by(self.wallet.public_key()),
        };
        let signed = payment.sign(&self.wallet);
        Ok(self.keep(
            scheme,
            signed.hash,
            signed.blob,
            0,
            fee,
            Some(amount.value),
            0,
        ))
    }

    /// Whether the line of `amount`'s issuer and currency holds `amount`,
    /// as last proven, after the token `scheme` has in flight: a
    /// scheme follows one line, so all it has in flight is on it.
    fn line_holds(&self, scheme: Name, amount: &TokenAmount) -> bool {
        let key = LineKey {
            issuer: amount.issuer,
            currency: amount.currency,
        };
        let Some(balance) = self
            .line(&key)
            .flatten()
            .and_then(|line| line.view(self.account()))
            .map(|view| view.balance)
        else {
            return false;
        };
        let spent = self
            .in_flight
            .iter()
            .filter(|t| t.scheme == scheme)
            .filter_map(|t| t.token)
            .try_fold(amount.value, TokenValue::checked_add);
        spent.is_some_and(|spent| spent <= balance)
    }

    /// Signs a `TrustSet` for `scheme` that sets the account's `limit` on
    /// its line to the limit's issuer, with `flags`, and submits it: it
    /// books nothing on the scheme's ledger, so it waits for nothing. Its
    /// fee is gas, and so is the reserve of a line the account holds none
    /// for yet. Returns its hash.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unavailable`] unless the account could sign now
    /// ([`sign_payment`](Self::sign_payment)) and the fee and the reserve
    /// fit the gas ([`gas_allows`](Self::gas_allows)).
    pub fn sign_trust_set(
        &mut self,
        scheme: Name,
        limit: TokenAmount,
        flags: u32,
    ) -> Result<[u8; 32], SchemeError> {
        let fee = self.fee();
        let key = LineKey {
            issuer: limit.issuer,
            currency: limit.currency,
        };
        let account = self.account();
        let holds_reserve = self
            .line(&key)
            .flatten()
            .and_then(|line| line.view(account))
            .is_some_and(|view| view.ours.reserve);
        let reserve = if holds_reserve { 0 } else { self.reserve_inc };
        if !limit.is_valid() || !self.can_sign(scheme, 0, fee, reserve) {
            return Err(SchemeError::Unavailable);
        }
        let trust_set = TrustSet {
            account,
            limit,
            fee,
            sequence: self.sequence,
            last_ledger_sequence: Some(self.validated.saturating_add(LEDGER_WINDOW)),
            flags,
            network_id: None,
            memos: Vec::new(),
            signing: Signing::by(self.wallet.public_key()),
        };
        let signed = trust_set.sign(&self.wallet);
        let hash = self.keep(scheme, signed.hash, signed.blob, 0, fee, None, reserve);
        if let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
            transaction.trust_set = true;
        }
        let executed = self.execute(scheme, hash);
        assert!(executed, "a TrustSet just signed is in flight");
        Ok(hash)
    }

    /// Keeps a transaction just signed in flight, with the next sequence,
    /// not yet executed. Returns its hash.
    #[allow(clippy::too_many_arguments)]
    fn keep(
        &mut self,
        scheme: Name,
        hash: [u8; 32],
        blob: Vec<u8>,
        drops: u64,
        fee: u64,
        token: Option<TokenValue>,
        reserve: u64,
    ) -> [u8; 32] {
        let sequence = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        self.in_flight.push(InFlight {
            scheme,
            hash,
            blob,
            drops,
            fee,
            token,
            trust_set: false,
            reserve,
            sequence,
            last_ledger: self.validated.saturating_add(LEDGER_WINDOW),
            submitted_at: self.validated,
            executed: false,
            needs_submit: false,
            looked_up: false,
        });
        hash
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

    /// Whether a `TrustSet` of `scheme` is on its way.
    #[must_use]
    pub fn signs_trust_set(&self, scheme: Name) -> bool {
        self.in_flight
            .iter()
            .any(|t| t.trust_set && t.scheme == scheme)
    }

    /// Whether a lookup may be asked now: synced, with room for one.
    #[must_use]
    pub fn can_look_up(&self) -> bool {
        self.phase == Phase::Synced && self.lookups.len() < MAX_LOOKUPS
    }

    /// Whether transaction `hash` is being looked up.
    #[must_use]
    pub fn is_looking_up(&self, hash: &[u8; 32]) -> bool {
        self.lookups.contains(hash)
    }

    /// Looks transaction `hash` up wherever the server finds it: what is
    /// proven in a ledger of the anchor's chain comes as [`News::Found`].
    /// When it was submitted is not kept, and needs not be.
    pub fn look_up(&mut self, hash: [u8; 32]) {
        if self.can_look_up() && !self.is_looking_up(&hash) {
            self.lookups.insert(hash);
            self.dispatch_proofs();
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
            Request::Submit(hash) => {
                let Some(transaction) = self.in_flight.iter().find(|t| t.hash == hash) else {
                    return;
                };
                api::submit(id, &transaction.blob)
            }
            Request::TxProof(hash, ledger) => api::tx_proof(id, &hash, ledger),
            Request::Ledger(at, key) | Request::SkipList(at, key) => {
                api::state_proof(id, &[key], Some(&at))
            }
            Request::Anchor => {
                let Some(Round::Proving(keys)) = &self.round else {
                    return;
                };
                api::state_proof(id, keys, None)
            }
            Request::Read(at) => {
                let Some(keys) = &self.reading else {
                    return;
                };
                api::state_proof(id, keys, Some(&at))
            }
            Request::AccountTx(from, to) => api::account_tx(id, account, from, to, None),
            Request::Validations(hash) => api::validations(id, &hash),
            Request::Manifest(master) => api::manifest(id, &master),
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
        // A faucet's answer may come for an older connection, and be
        // dropped: it is asked again in time. So may a list's.
        self.funding = false;
        self.round = None;
        self.walk = None;
        self.reading = None;
        self.stalled_until_ms = 0;
        self.hints.clear();
        self.lookups.clear();
        self.tried.clear();
        self.lists.clear();
        // A proof whose ledger is not proven yet is proven again on the
        // next connection.
        self.proofs.retain(|_, proof| proof.holds);
        for fetching in &mut self.fetches {
            fetching.forget();
        }
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
        };
        // The network is checked first, so nothing is read or booked from
        // another; then the account is subscribed, for its hints, and
        // anchored on a ledger its UNL validated, which proves what it
        // holds. The lists it lacks are fetched meanwhile.
        self.send(Request::ServerState);
        self.fetch_lists();
    }

    fn message(&mut self, bytes: &[u8]) {
        match api::parse(bytes) {
            Message::Response { id, outcome } => {
                if let Some((request, _)) = self.pending.remove(&id) {
                    self.response(request, outcome);
                }
            }
            Message::Transaction(record) => self.hinted(&record),
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
                    self.anchor_now();
                }
            }
            Request::ServerState => self.server_state(outcome),
            Request::Submit(hash) => self.submitted(hash, outcome.ok().as_ref()),
            Request::TxProof(hash, ledger) => self.tx_proof_read(hash, ledger, outcome),
            Request::Ledger(at, key) => self.ledger_list_read(at, key, outcome),
            Request::Anchor => self.proved_anchor(outcome),
            Request::Read(at) => self.read_answered(at, outcome),
            Request::AccountTx(..) => self.history_hinted(outcome),
            Request::Validations(_) => self.validated_anchor(outcome),
            Request::Manifest(_) => self.manifest_read(outcome),
            Request::SkipList(at, key) => self.skip_list_read((at, key), outcome),
        }
    }

    /// The server's state, the first answer on a connection: a server off
    /// the account's network, or that does not say, stops the account
    /// before anything is read from it, a cheap first check; the UNL is
    /// what binds the account to its network. Its validated ledger is a
    /// hint, which moves nothing: only an anchor does; and so is its fee,
    /// which the account offers up to its ceiling. Then the account is
    /// subscribed.
    fn server_state(&mut self, outcome: Result<Value, ApiError>) {
        let Some(state) = outcome.ok().as_ref().and_then(api::server_state_result) else {
            self.reconnect();
            return;
        };
        let Some(network_id) = state.network_id.filter(|&id| id == self.network.id) else {
            self.failed = Some(self.network.wrong_network);
            self.disconnected();
            return;
        };
        self.network_id = Some(network_id);
        self.fee_base = state.base_fee;
        self.load = (state.load_factor, state.load_base);
        if let Phase::Starting { state, .. } = &mut self.phase {
            *state = true;
            self.send(Request::Subscribe);
        }
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

    /// The account does not exist, as proven at its anchor. On a network
    /// that [`resets`], what the books held is gone with it, and the faucet
    /// is asked; on any other, an account that held anything does not just
    /// go, and the account stops.
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

    /// What the books hold of the account is gone (a testnet reset): what
    /// was in flight can never apply now, since an account made again
    /// starts its sequence anew. Its schemes write off what they held; one
    /// not on the account does when it is back, as the count of write-offs
    /// tells it ([`write_offs`](Self::write_offs)), and what was kept for
    /// it went with the chain: a settlement is cancelled, as one in flight
    /// is.
    fn write_off(&mut self, restarted: bool) {
        assert!(
            !self.holds_anything() || self.network_id.is_some_and(resets),
            "only a network that resets writes the account off"
        );
        for transaction in core::mem::take(&mut self.in_flight) {
            if transaction.executed {
                self.cancelled(transaction.scheme, transaction.hash, !transaction.trust_set);
            } else {
                self.never_sent(transaction.scheme, transaction.hash);
            }
        }
        for parked in core::mem::take(&mut self.parked) {
            if !parked.trust_set {
                self.cancelled(parked.scheme, parked.hash, true);
            }
        }
        self.write_offs = self.write_offs.wrapping_add(1);
        self.tell(News::WrittenOff { restarted });
        for follower in self.followers.values_mut() {
            *follower = Follower {
                cursor: None,
                holds: false,
            };
        }
        self.at = None;
        self.balance = None;
        self.proven_sequence = 0;
        // What was proven of the chain gone goes with it: the lines, the
        // issuers' accounts and the pools are proven again once it is back.
        self.walk = None;
        self.proofs.clear();
        self.lists.clear();
        for state in self.lines.values_mut() {
            *state = None;
        }
        self.issuers.clear();
        for watch in self.pools.values_mut() {
            watch.account = None;
        }
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

    /// Up to date as of `info`, the anchor: the schemes are told the lines
    /// and the account as proven there, then what waits is submitted.
    fn synced(&mut self, info: AccountInfo, lines: &BTreeMap<LineKey, Option<TrustLine>>) {
        self.balance = Some(info.balance);
        self.owner_count = info.owner_count;
        self.at = Some(self.at.map_or((info.ledger_index, u32::MAX), |at| {
            at.max((info.ledger_index, u32::MAX))
        }));
        self.phase = Phase::Synced;
        for (key, line) in lines {
            if let Some(state) = self.lines.get_mut(key) {
                *state = Some(*line);
                self.tell(News::Line {
                    key: *key,
                    line: *line,
                });
            }
        }
        self.tell(News::Synced(info));
        // What was kept for a scheme back on the account, it has now read
        // again from its cursor; what it has not, it is told, and looks up
        // by its hash.
        let followers = &self.followers;
        let (unread, kept): (Vec<Parked>, Vec<Parked>) = core::mem::take(&mut self.parked)
            .into_iter()
            .partition(|p| followers.contains_key(&p.scheme));
        self.parked = kept;
        for parked in unread {
            self.tell(News::Unread {
                hash: parked.hash,
                ours: Ours {
                    scheme: parked.scheme,
                    drops: parked.drops,
                    token: parked.token,
                    settles: !parked.trust_set,
                    again: true,
                },
            });
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

    /// A ledger closed, as the stream says: a hint, which moves nothing the
    /// account takes as validated, and may start an anchoring round, whose
    /// anchor does ([`anchor_on_hint`](Self::anchor_on_hint)). Its fee is
    /// the server's word, which the account offers up to its ceiling; the
    /// reserve is proven at the anchor.
    fn ledger_closed(&mut self, closed: LedgerClosed) {
        self.fee_base = closed.fee_base;
        self.anchor_on_hint();
    }

    /// Our transactions past their last ledger, as of the anchor. One the
    /// node never executed was never sent: it is dropped, and remembered as
    /// never sent. One it executed is cancelled once its absence is proven:
    /// the account's sequence at the anchor, past its last ledger, is not
    /// past its own, so nothing took its number in time, and it never
    /// applied and never will. Otherwise the history told what took the
    /// number, ours or another's, which cancels it; failing that, its own
    /// proof is asked for, and found or not, it is never cancelled on the
    /// server's word (spec/designs/xrpl-validation.md#4-what-is-proven-and-how).
    fn expire(&mut self) {
        let validated = self.validated;
        let (keep, dropped): (Vec<InFlight>, Vec<InFlight>) = core::mem::take(&mut self.in_flight)
            .into_iter()
            .partition(|t| t.executed || validated <= t.last_ledger);
        self.in_flight = keep;
        for transaction in dropped {
            self.never_sent(transaction.scheme, transaction.hash);
        }
        let proven_sequence = self.proven_sequence;
        let past = |t: &InFlight| validated > t.last_ledger;
        let absent: Vec<[u8; 32]> = (self.in_flight.iter())
            .filter(|t| past(t) && proven_sequence <= t.sequence)
            .map(|t| t.hash)
            .collect();
        for hash in absent {
            self.give_up(hash);
        }
        for transaction in &mut self.in_flight {
            if past(transaction) {
                transaction.looked_up = true;
            }
        }
        self.reset_sequence();
        self.dispatch_proofs();
    }

    /// The next sequence: the account's as proven, past what is in flight.
    fn reset_sequence(&mut self) {
        let next = self
            .in_flight
            .iter()
            .map(|t| t.sequence.saturating_add(1))
            .max()
            .unwrap_or(0);
        self.sequence = self.proven_sequence.max(next);
    }

    fn submit(&mut self, hash: [u8; 32]) {
        if let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
            transaction.needs_submit = false;
            self.send(Request::Submit(hash));
        }
    }

    /// What `submit` said: a hint. Even a malformed transaction (`tem…`) is
    /// cancelled only once its absence is proven past its last ledger, as
    /// any is: a server could say so of one it then applies. One that may
    /// still apply later (`ter…`, `tel…`) is submitted again.
    fn submitted(&mut self, hash: [u8; 32], result: Option<&Value>) {
        let Some(code) = result.and_then(api::submit_result) else {
            return;
        };
        if (code.starts_with("ter") || code.starts_with("tel"))
            && let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash)
        {
            transaction.needs_submit = true;
        }
    }

    /// Our transaction never applied, as proven: its scheme cancels its
    /// settlement, and its sequence is the next one's, if it was the last.
    fn give_up(&mut self, hash: [u8; 32]) {
        let Some(position) = self.in_flight.iter().position(|t| t.hash == hash) else {
            return;
        };
        let transaction = self.in_flight.remove(position);
        if transaction.executed {
            self.cancelled(transaction.scheme, transaction.hash, !transaction.trust_set);
        } else {
            self.never_sent(transaction.scheme, transaction.hash);
        }
        self.reset_sequence();
    }

    /// `scheme`'s transaction `hash` can never apply: the scheme is told,
    /// or, while it is not on the account, it is remembered as never sent
    /// until the scheme reconciles it.
    fn cancelled(&mut self, scheme: Name, hash: [u8; 32], settles: bool) {
        if self.followers.contains_key(&scheme) {
            self.tell(News::Cancelled {
                scheme,
                hash,
                settles,
            });
        } else {
            self.never_sent(scheme, hash);
        }
    }

    /// What the account told `scheme` of its own since its schemes were
    /// last told, the scheme did not take: the node lets go of a scheme
    /// before the account forgets what it told ([`told`](ExternalAsset::told))
    /// only when the books refused what it was told, and stopped it. Its
    /// checkpoint stays as it was, so after a restart it reads that again
    /// from its cursor, while the account's state, written on for the other
    /// schemes, no longer has it in flight or kept. So it is kept for the
    /// scheme, as for one not on the account: a transaction the ledger
    /// validated, or one it was told it did not read again, as kept; one
    /// that can never apply as never sent. Otherwise the scheme would read
    /// its own settlement again as a withdrawal, and post it once found.
    fn keep_untaken(&mut self, scheme: Name) {
        let mut parked = Vec::new();
        let mut never_sent = Vec::new();
        for news in &self.news {
            match news {
                News::Transaction {
                    record,
                    ours: Some(ours),
                } if ours.scheme == scheme => parked.push(Parked {
                    scheme,
                    hash: record.hash,
                    drops: ours.drops,
                    token: ours.token,
                    trust_set: !ours.settles,
                }),
                News::Unread { hash, ours } if ours.scheme == scheme => parked.push(Parked {
                    scheme,
                    hash: *hash,
                    drops: ours.drops,
                    token: ours.token,
                    trust_set: !ours.settles,
                }),
                News::Cancelled {
                    scheme: of, hash, ..
                } if *of == scheme => never_sent.push(*hash),
                _ => {}
            }
        }
        for kept in parked {
            if !self.parked.iter().any(|p| p.hash == kept.hash) {
                assert!(self.parked.len() < MAX_PARKED, "kept past its bound");
                self.parked.push(kept);
            }
        }
        for hash in never_sent {
            if !self.never_sent.iter().any(|n| n.hash == hash) {
                self.never_sent(scheme, hash);
            }
        }
    }

    /// Whether the account told `scheme` of its own transactions since its
    /// schemes were last told, which it keeps for the scheme should the
    /// books refuse them ([`keep_untaken`](Self::keep_untaken)).
    fn told_own(&self, scheme: Name) -> bool {
        self.news.iter().any(|news| match news {
            News::Transaction {
                ours: Some(ours), ..
            }
            | News::Unread { ours, .. } => ours.scheme == scheme,
            _ => false,
        })
    }

    /// A proven transaction that touches the account: it moves the holding
    /// if it is newer than what the account told, settles one of ours in
    /// flight whatever its place, and is told to every scheme. One of the
    /// account's that is not ours used up its sequence numbers: one of ours
    /// that took any of them can never apply, and is cancelled.
    fn transaction(&mut self, decoded: &Decoded) {
        let record = &decoded.record;
        assert!(record.validated, "a proven transaction is validated");
        if !self.bookable_record(record) {
            self.failed = Some(TOO_MUCH_XRP);
            return;
        }
        let at = (record.ledger_index, record.index);
        let account = self.account();
        if record.account == account {
            let (first, count) = decoded.uses;
            self.proven_sequence = self.proven_sequence.max(first.saturating_add(count));
        }
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
            for (key, state) in &mut self.lines {
                if let Some(change) = record.line(account, key.issuer, key.currency) {
                    *state = Some(change.after().copied());
                }
            }
        }
        let ours = self.ours(record.hash);
        self.tell(News::Transaction {
            record: record.clone(),
            ours,
        });
        if record.account == account && ours.is_none() {
            let (first, count) = decoded.uses;
            let taken: Vec<[u8; 32]> = (self.in_flight.iter())
                .filter(|t| t.sequence >= first && t.sequence - first < count)
                .map(|t| t.hash)
                .collect();
            for hash in taken {
                self.give_up(hash);
            }
        }
        self.reset_sequence();
    }

    /// Which scheme validated transaction `hash` is of, if it is ours. One
    /// in flight is no longer, and is kept for its scheme while the scheme
    /// is not on the account to be told; one kept so is its scheme's again
    /// once the scheme is back and reads it, told as `again`, so that no
    /// other scheme books it twice.
    fn ours(&mut self, hash: [u8; 32]) -> Option<Ours> {
        if let Some(position) = self.in_flight.iter().position(|t| t.hash == hash) {
            let transaction = self.in_flight.remove(position);
            if !self.followers.contains_key(&transaction.scheme) {
                assert!(self.parked.len() < MAX_PARKED, "kept past its bound");
                self.parked.push(Parked {
                    scheme: transaction.scheme,
                    hash,
                    drops: transaction.drops,
                    token: transaction.token,
                    trust_set: transaction.trust_set,
                });
            }
            return Some(Ours {
                scheme: transaction.scheme,
                drops: transaction.drops,
                token: transaction.token,
                settles: !transaction.trust_set,
                again: false,
            });
        }
        let followers = &self.followers;
        let position = self
            .parked
            .iter()
            .position(|p| p.hash == hash && followers.contains_key(&p.scheme))?;
        let parked = self.parked.remove(position);
        Some(Ours {
            scheme: parked.scheme,
            drops: parked.drops,
            token: parked.token,
            settles: !parked.trust_set,
            again: true,
        })
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
    /// the anchor, which changes as ledgers close, and is written with the
    /// rest: an older anchor of the same chain tells a reset as well.
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
        for parked in &self.parked {
            add(head(&parked.hash));
        }
        add(self.parked.len() as u64);
        add(self.write_offs);
        add(self.unl_changes);
        mix
    }

    fn state_value(&self) -> Value {
        let in_flight: Vec<Value> = self
            .in_flight
            .iter()
            .map(|t| {
                let mut entry = json!({
                    "scheme": t.scheme.as_str(),
                    "blob": hex_upper(&t.blob),
                    "drops": t.drops,
                    "fee": t.fee,
                    "sequence": t.sequence,
                    "last_ledger": t.last_ledger,
                    "submitted_at": t.submitted_at,
                    "executed": t.executed,
                });
                // A Payment of XRP is written as it always was.
                if let Some(object) = entry.as_object_mut() {
                    if let Some(token) = t.token {
                        object.insert("token".into(), token.to_string().into());
                    }
                    if t.trust_set {
                        object.insert("trust_set".into(), true.into());
                        object.insert("reserve".into(), t.reserve.into());
                    }
                }
                entry
            })
            .collect();
        let never_sent: Vec<Value> = self
            .never_sent
            .iter()
            .map(|n| json!({ "scheme": n.scheme.as_str(), "hash": hex_upper(&n.hash) }))
            .collect();
        let parked: Vec<Value> = self
            .parked
            .iter()
            .map(|p| {
                let mut entry = json!({
                    "scheme": p.scheme.as_str(),
                    "hash": hex_upper(&p.hash),
                    "drops": p.drops,
                });
                if let Some(object) = entry.as_object_mut() {
                    if let Some(token) = p.token {
                        object.insert("token".into(), token.to_string().into());
                    }
                    if p.trust_set {
                        object.insert("trust_set".into(), true.into());
                    }
                }
                entry
            })
            .collect();
        json!({
            "sequence": self.sequence,
            "in_flight": in_flight,
            "never_sent": never_sent,
            "parked": parked,
            "write_offs": self.write_offs,
            "anchor": self.anchor.as_ref().map(KeptAnchor::state),
            "newest": self.hint.map(|hint| json!([hint.index, hint.close_time])),
            "unl": self.unl.state(),
            "unl_at_ms": self.now_ms,
        })
    }

    fn restore(&mut self, state: &[u8]) -> Option<()> {
        let value: Value = serde_json::from_slice(state).ok()?;
        let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
        self.sequence = number(value.get("sequence")?)?;
        // Written only since schemes keep the count too: before, none
        // missed a write-off.
        self.write_offs = match value.get("write_offs") {
            None => 0,
            Some(count) => count.as_u64()?,
        };
        self.anchor = match value.get("anchor") {
            None | Some(Value::Null) => None,
            Some(anchor) => Some(KeptAnchor::restore(anchor)?),
        };
        self.validated = self.anchor.map_or(0, |kept| kept.anchor.index);
        // A state written before anchors kept the newest ledger seen
        // without a hash: a hint, which the first anchor replaces.
        self.hint = match value.get("newest") {
            None | Some(Value::Null) => None,
            Some(_) if self.anchor.is_some() => return None,
            Some(newest) => Some(Validated {
                index: number(newest.get(0)?)?,
                close_time: newest.get(1)?.as_u64()?,
            }),
        };
        // Lists and manifests verify again, as of when they were written:
        // what no longer does, as for publishers the owner changed, is
        // dropped and fetched again; what expired since goes as time moves
        // on.
        if let Some(unl) = value.get("unl") {
            let at_ms = value.get("unl_at_ms").and_then(Value::as_u64).unwrap_or(0);
            self.unl = Unl::restore(self.config.unl.clone(), unl, at_ms)?;
        }
        // More than the account keeps is a state it did not write: refused,
        // rather than orphaning a settlement by dropping it.
        let in_flight = value.get("in_flight")?.as_array()?;
        let never_sent = value.get("never_sent")?.as_array()?;
        // Written only since the account kept transactions for schemes not
        // on it.
        let parked = match value.get("parked") {
            None => &[][..],
            Some(parked) => parked.as_array()?.as_slice(),
        };
        if in_flight.len() > MAX_IN_FLIGHT
            || never_sent.len() > MAX_NEVER_SENT
            || parked.len() > MAX_PARKED
        {
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
        for entry in parked {
            let (token, trust_set) = token_and_trust_set(entry)?;
            self.parked.push(Parked {
                scheme: Name::new(entry.get("scheme")?.as_str()?).ok()?,
                hash: from_hex(entry.get("hash")?.as_str()?)?.try_into().ok()?,
                drops: entry
                    .get("drops")?
                    .as_u64()
                    .filter(|&drops| drops <= MAX_ACCOUNT_DROPS)?,
                token,
                trust_set,
            });
        }
        // Each transaction once, in flight, never sent or kept, of a scheme
        // that may settle through the account, no more of a scheme's in
        // flight and kept than it can have in flight, and every one in
        // flight below the next sequence: what the account writes. A scheme
        // the account does not know would otherwise stop the node at every
        // start, as the scheme executes or resumes its settlement.
        let mut seen = BTreeSet::new();
        let distinct = self.hashes().all(|hash| seen.insert(hash));
        let known = self.named().all(|scheme| self.schemes.contains(&scheme));
        let bounded = (self.schemes.iter()).all(|s| self.in_flight_and_parked(*s) <= MAX_IN_FLIGHT);
        let ordered = self.in_flight.iter().all(|t| t.sequence < self.sequence);
        (distinct && known && bounded && ordered).then_some(())
    }

    /// Every transaction the account keeps: in flight, never sent, kept.
    fn hashes(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        (self.in_flight.iter().map(|t| t.hash))
            .chain(self.never_sent.iter().map(|n| n.hash))
            .chain(self.parked.iter().map(|p| p.hash))
    }

    /// The scheme of every transaction the account keeps.
    fn named(&self) -> impl Iterator<Item = Name> + '_ {
        (self.in_flight.iter().map(|t| t.scheme))
            .chain(self.never_sent.iter().map(|n| n.scheme))
            .chain(self.parked.iter().map(|p| p.scheme))
    }

    /// How many of `scheme`'s transactions are in flight or kept.
    fn in_flight_and_parked(&self, scheme: Name) -> usize {
        let in_flight = self.in_flight.iter().filter(|t| t.scheme == scheme);
        let parked = self.parked.iter().filter(|p| p.scheme == scheme);
        in_flight.count() + parked.count()
    }
}

/// What a transaction the account wrote pays of a token, and whether it is
/// a `TrustSet`: neither is written for a Payment of XRP, as before tokens.
fn token_and_trust_set(entry: &Value) -> Option<(Option<TokenValue>, bool)> {
    let token = match entry.get("token") {
        None => None,
        Some(token) => Some(TokenValue::parse(token.as_str()?)?),
    };
    let trust_set = match entry.get("trust_set") {
        None => false,
        Some(trust_set) => trust_set.as_bool()?,
    };
    // A TrustSet pays no token.
    (!(trust_set && token.is_some())).then_some((token, trust_set))
}

/// One transaction in flight, as the account wrote it.
fn restore_in_flight(entry: &Value) -> Option<InFlight> {
    let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
    let blob = from_hex(entry.get("blob")?.as_str()?)?;
    let executed = entry
        .get("executed")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let (token, trust_set) = token_and_trust_set(entry)?;
    let reserve = match entry.get("reserve") {
        None => 0,
        Some(reserve) => reserve.as_u64().filter(|&r| r <= MAX_ACCOUNT_DROPS)?,
    };
    // A TrustSet pays no token, and only a TrustSet takes a reserve.
    if (trust_set && token.is_some()) || (!trust_set && reserve > 0) {
        return None;
    }
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
        token,
        trust_set,
        reserve,
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
        let distinct = self.hashes().all(|hash| seen.insert(hash));
        assert!(
            distinct,
            "each transaction once, in flight, never sent or kept"
        );
        for scheme in self.named().chain(self.followers.keys().copied()) {
            assert!(self.schemes.contains(&scheme), "a scheme of the account");
        }
        for scheme in &self.schemes {
            assert!(
                self.in_flight_and_parked(*scheme) <= MAX_IN_FLIGHT,
                "a scheme reads its own again before it signs more"
            );
        }
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
            assert!(transaction.reserve <= MAX_ACCOUNT_DROPS);
            assert!(
                !transaction.trust_set || (transaction.token.is_none() && transaction.drops == 0),
                "a TrustSet pays nothing"
            );
            assert!(
                transaction.trust_set || transaction.reserve == 0,
                "only a TrustSet takes a reserve"
            );
        }
    }

    fn check_phase(&self) {
        match self.phase {
            Phase::Disconnected => {
                assert!(self.pending.is_empty(), "nothing asked while disconnected");
                assert!(self.network_id.is_none(), "a network is a connection's");
                assert!(!self.funding);
                assert!(self.round.is_none(), "a round is a connection's");
                assert!(self.walk.is_none(), "a walk is a connection's");
                assert!(
                    self.hints.is_empty() && self.lookups.is_empty(),
                    "what is proven for the schemes is a connection's"
                );
                assert_eq!(self.stalled_until_ms, 0, "a stall is a connection's");
                assert!(
                    self.fetches.iter().all(|fetching| !fetching.is_asked()),
                    "a fetch is a connection's"
                );
            }
            Phase::Starting { subscribed, .. } => {
                assert!(
                    subscribed || self.round.is_none(),
                    "anchored only once subscribed"
                );
                assert!(self.walk.is_none(), "a walk starts from an anchor");
            }
            Phase::Synced => {
                assert!(self.at.is_some(), "a synced account read its holding");
            }
            Phase::CatchingUp { .. } => {
                assert!(self.walk.is_some(), "catching up is walking the history");
            }
            Phase::Unfunded => assert!(self.walk.is_none(), "nothing to walk"),
        }
        assert!(
            self.walk.is_none() || self.round.is_none(),
            "no round while the history is walked"
        );
        assert!(
            self.round.is_none() || self.network_id.is_some(),
            "anchored only on the account's network"
        );
        assert!(
            self.round.is_none() || self.stalled_until_ms == 0,
            "no round while stalled"
        );
    }
}

impl ExternalAsset for XrplAccount {
    fn id(&self) -> AssetId {
        self.id
    }

    fn address(&self) -> Option<String> {
        Some(self.account().to_address())
    }

    /// Its XRP, with the reserve, and each line its schemes follow that
    /// the ledger showed, at [`TOKEN_SCALE`].
    fn holdings(&self) -> Vec<Holding> {
        let account = self.account();
        let xrp = self.balance.map(|balance| Holding {
            code: "XRP".to_string(),
            issuer: None,
            amount: u128::from(balance),
            scale: 6,
            reserve: u128::from(self.reserve_drops(self.owner_count)),
            freeze: Freeze::default(),
        });
        let tokens = self.lines.iter().filter_map(move |(key, state)| {
            let line = state.as_ref()?.as_ref()?;
            let view = line.view(account)?;
            Some(Holding {
                code: key.currency.code(),
                issuer: Some(key.issuer.to_address()),
                amount: view
                    .balance
                    .floor_units(TOKEN_SCALE)
                    .map_or(u128::MAX, |units| u128::try_from(units).unwrap_or(0)),
                scale: TOKEN_SCALE,
                reserve: 0,
                freeze: Freeze {
                    line: view.theirs.freeze,
                    deep: view.theirs.deep_freeze,
                    global: self
                        .issuers
                        .get(&key.issuer)
                        .is_some_and(AccountSettings::global_freeze),
                },
            })
        });
        xrp.into_iter().chain(tokens).collect()
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

    /// Why it cannot anchor: no UNL, no quorum, a server that serves no
    /// proofs or refuses them, an older ledger than its anchor, or answers
    /// that do not prove. Until it anchors anew, nothing it takes as
    /// validated moves.
    fn waiting(&self) -> Option<&'static str> {
        self.waiting.filter(|_| self.failed.is_none())
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
            // The next anchor proves whether the account exists.
            LedgerInput::Funded(_) => self.funding = false,
            LedgerInput::Fetched { url, body } => self.fetched(url, body),
            // It names no endpoints: nothing comes from one.
            LedgerInput::From { .. } => {}
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
        let stall = (self.stalled_until_ms > 0).then_some(self.stalled_until_ms);
        [request, fund, silence, stall, self.lists_deadline()]
            .into_iter()
            .flatten()
            .min()
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
        // A connection that went quiet may be half open, and one that
        // refused a proof may reach another server: made anew.
        let silent = self.phase != Phase::Disconnected
            && self.heard_ms.saturating_add(LEDGER_SILENCE_MS) <= now_ms;
        let stalled = self.stalled_until_ms > 0 && self.stalled_until_ms <= now_ms;
        if late || silent || stalled {
            if late && (self.round.is_some() || self.walk.is_some()) {
                self.waiting = Some(anchors::NOT_ANSWERED);
            }
            self.reconnect();
            return;
        }
        self.lists_on_timer();
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

    fn detach(&mut self, scheme: Name, why: Detach) {
        self.followers.remove(&scheme);
        self.keep_untaken(scheme);
        self.funds.remove(&scheme);
        match why {
            Detach::Stopped if self.schemes.contains(&scheme) => {
                self.stopped.insert(scheme);
            }
            Detach::Stopped | Detach::Removed => {
                self.stopped.remove(&scheme);
            }
        }
    }

    fn owner_funds(&mut self, scheme: Name, funds: u64) {
        if self.funds.contains_key(&scheme) || self.funds.len() < MAX_SCHEMES {
            self.funds.insert(scheme, funds);
        }
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
        assert!(self.parked.len() <= MAX_PARKED);
        assert!(self.pending.len() <= MAX_PENDING);
        assert!(self.proofs.len() <= MAX_PROOFS);
        assert!(self.hints.len() <= MAX_HINTS);
        assert!(self.lookups.len() <= MAX_LOOKUPS);
        assert!(self.lists.len() <= MAX_LISTS);
        assert!(self.tried.len() <= MAX_IN_FLIGHT);
        assert!(self.issuers_asked.len() <= MAX_SCHEMES);
        assert!(self.pools.len() <= MAX_SCHEMES);
        if let Some(walk) = &self.walk {
            walk.check_invariants();
        }
        assert!(self.followers.len() <= MAX_SCHEMES);
        assert_eq!(Network::of(self.id.kind), Some(self.network), "its kind's");
        assert!(
            self.stopped.iter().all(|scheme| self.schemes.contains(scheme)
                && !self.followers.contains_key(scheme)),
            "a scheme stopped is one of the account's, and not on it"
        );
        assert!(self.lines.len() <= MAX_SCHEMES);
        assert!(self.funds.len() <= MAX_SCHEMES);
        assert!(
            self.issuers
                .keys()
                .all(|issuer| self.lines.keys().any(|key| key.issuer == *issuer)),
            "only the issuers of followed lines are shown"
        );
        assert!(self.news.len() <= MAX_NEWS);
        assert!(self.effects.len() <= MAX_EFFECTS);
        assert!(self.next_id >= 1, "request IDs start at 1");
        assert_eq!(
            self.fetches.len(),
            self.config.unl.publishers.len(),
            "one fetch for each publisher"
        );
        for (fetching, publisher) in self.fetches.iter().zip(&self.config.unl.publishers) {
            fetching.check_invariants(publisher.sites.len());
        }
        assert_eq!(self.unl.config(), &self.config.unl, "the config's UNL");
        assert!(
            self.anchor.is_none() || self.hint.is_none(),
            "a hint only until the first anchor"
        );
        if let Some(kept) = &self.anchor {
            kept.check_invariants();
            assert!(
                kept.anchor.index <= self.validated,
                "nothing below its anchor is validated"
            );
        }
        self.unl.check_invariants();
        self.check_in_flight();
        self.check_phase();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    use crate::unl::Anchor;
    use anchors::{Judged, judge};

    const NEWEST: Validated = Validated {
        index: 5_000_000,
        close_time: 800_000_000,
    };

    fn ledger(index: u32, close_time: u32, hash: u8) -> Anchor {
        Anchor {
            index,
            close_time,
            hash: [hash; 32],
        }
    }

    // spec/designs/xrpl-validation.md#7-resets-under-a-unl: against the
    // anchor kept, a lower ledger that closed earlier is older, never
    // current; one that closed later, or another of the index that did, is
    // another chain, a reset only where the network resets (`resets`); a
    // higher one that closed earlier is of an older chain, ignored where
    // the network resets and a fork elsewhere; a higher one that closed
    // later is current once its skip lists hold the kept anchor. A state written before anchors kept the newest ledger
    // without its hash, which the chain rule judges alone.
    #[test]
    fn an_anchor_is_judged_against_the_one_kept() {
        let kept = ledger(5_000_000, 800_000_000, 1);
        let judged = |new: Anchor| judge(Some(&kept), None, &new);
        assert_eq!(judged(kept), Judged::Take);
        assert_eq!(judged(ledger(4_999_990, 799_999_960, 2)), Judged::Older);
        assert_eq!(judged(ledger(2_000, 800_000_004, 3)), Judged::Reset);
        assert_eq!(judged(ledger(5_000_000, 800_000_004, 4)), Judged::Reset);
        assert_eq!(judged(ledger(5_000_000, 800_000_000, 4)), Judged::Ignore);
        assert_eq!(judged(ledger(5_000_001, 799_999_996, 5)), Judged::Ignore);
        assert_eq!(judged(ledger(5_000_001, 800_000_004, 6)), Judged::Later);
        let hinted = |new: Anchor| judge(None, Some(NEWEST), &new);
        assert_eq!(hinted(ledger(2_000, 800_000_004, 3)), Judged::Reset);
        assert_eq!(hinted(ledger(4_999_990, 799_999_960, 2)), Judged::Older);
        assert_eq!(hinted(ledger(5_000_001, 800_000_004, 6)), Judged::Take);
        assert_eq!(judge(None, None, &ledger(2_000, 1, 3)), Judged::Take);
        // Only the testnet resets, whatever lets the account run elsewhere.
        assert!(resets(TESTNET_NETWORK_ID));
        for network in [0, 2, 21_338, u32::MAX] {
            assert!(!resets(network));
        }
    }

    // spec/areas/xrpl.md#the-account: schemes on one account take its
    // sequences in order, never the same one; and what a scheme signed and
    // never sent is remembered for that scheme alone, until it reconciles
    // it.
    #[test]
    fn schemes_on_one_account_never_share_a_sequence() {
        let (a, b) = (dassie_ledger::name!("a"), dassie_ledger::name!("b"));
        let mut account = XrplAccount::new(FIRST, &[a, b], "", &[1; 32], &[]).unwrap();
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
        // Past their last ledger, the two never executed are dropped; the
        // one executed is looked for, as the account's sequence moved past
        // its own.
        account.validated = 11 + LEDGER_WINDOW;
        account.proven_sequence = 7;
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
            let mut account = XrplAccount::new(
                FIRST,
                &[dassie_ledger::name!(crate::scheme::SCHEME_ID)],
                "",
                &[1; 32],
                &[],
            )
            .unwrap();
            let mut scheme = crate::XrplScheme::new(&account, node, Some(5_000), &[]).unwrap();
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
                    gain: false,
                    cause: dassie_settlement::ReconcileCause::Unexplained,
                }));
            }
            account.check_invariants();
            scheme.check_invariants();
        }
    }

    /// An account through which `schemes` settle, synced at ledger 10.
    fn synced_account(schemes: &[Name]) -> XrplAccount {
        let mut account = XrplAccount::new(FIRST, schemes, "", &[1; 32], &[]).unwrap();
        account.phase = Phase::Synced;
        account.at = Some((10, u32::MAX));
        account.sequence = 5;
        account.balance = Some(1_000_000_000);
        account.validated = 10;
        account
    }

    // A state that names a scheme that may not settle through the account
    // is one it never wrote: refused as it loads, rather than stopping the
    // node at every start, as the scheme executes or resumes its settlement.
    #[test]
    fn a_state_naming_a_scheme_not_on_the_account_is_refused() {
        let (a, b) = (dassie_ledger::name!("a"), dassie_ledger::name!("b"));
        let mut account = synced_account(&[a, b]);
        let to = Wallet::from_entropy(&[2; 32]).account();
        let first = account.sign_payment(a, to, 1_000, Vec::new()).unwrap();
        assert!(account.execute(a, first));
        account.sign_payment(b, to, 1_000, Vec::new()).unwrap();
        let in_flight = account.state();
        // Past its last ledger, b's, never executed, is never sent.
        account.validated = 11 + LEDGER_WINDOW;
        account.expire();
        let never_sent = account.state();
        let mut parked: Value = serde_json::from_slice(&never_sent).unwrap();
        parked["never_sent"] = json!([]);
        parked["parked"] = json!([{ "scheme": "b", "hash": hex_upper(&[3; 32]), "drops": 1 }]);
        let parked = serde_json::to_vec(&parked).unwrap();
        for state in [in_flight, never_sent, parked] {
            assert!(XrplAccount::new(FIRST, &[a, b], "", &[1; 32], &state).is_ok());
            assert!(XrplAccount::new(FIRST, &[a], "", &[1; 32], &state).is_err());
        }
    }

    // spec/areas/xrpl.md#the-account: what the owner sees of an issuer's
    // freeze is of the chain the account follows. A write-off, at a testnet
    // reset or with the account gone, forgets the issuers' accounts with
    // the lines, and what was proven of the chain gone, so a global freeze
    // of a chain gone is never shown with a line of the new one; the next
    // anchor proves what holds.
    #[test]
    fn a_write_off_forgets_the_issuers_with_the_chain() {
        let dsusd = dassie_ledger::name!(crate::dsusd::SCHEME_ID);
        let issuer = crate::dsusd::ISSUER;
        for restarted in [true, false] {
            let mut account = synced_account(&[dsusd]);
            account.network_id = Some(TESTNET_NETWORK_ID);
            let kept = ledger(NEWEST.index, 800_000_000, 1);
            account.anchor = Some(KeptAnchor::of(kept, None));
            account.validated = NEWEST.index;
            account.follow_line(crate::dsusd::LINE);
            let frozen = AccountSettings {
                flags: api::account_flags::GLOBAL_FREEZE,
                transfer_rate: 0,
                regular_key: None,
                ledger_index: NEWEST.index,
                validated: true,
            };
            account.issuers.insert(issuer, frozen);
            account.read_pool(crate::dsusd::LINE);
            if let Some(watch) = account.pools.get_mut(&crate::dsusd::LINE) {
                watch.account = Some(issuer);
            }
            account.check_invariants();
            if restarted {
                // As a reset does: written off, and connected anew.
                account.write_off(true);
                account.disconnected();
            } else {
                account.absent();
            }
            assert!(account.issuers.is_empty(), "restarted: {restarted}");
            assert!(
                account.pools.values().all(|watch| watch.account.is_none()),
                "a pool's account is proven again"
            );
            assert_eq!(account.lines[&crate::dsusd::LINE], None);
            account.check_invariants();
        }
    }

    // A scheme's state keeps which settlements it looks up it never read,
    // each one it looks up, once: any other is a state it never wrote.
    #[test]
    fn a_scheme_state_keeps_what_it_never_read() {
        use dassie_settlement::SettlementScheme;

        let id = dassie_ledger::name!(crate::scheme::SCHEME_ID);
        let account = synced_account(&[id]);
        let node = dassie_ledger::name!("node");
        let (asked, other) = (hex_upper(&[1; 32]), hex_upper(&[2; 32]));
        let state = |unread: Value| {
            let state = json!({
                "cursor": [10, u32::MAX], "peers": {}, "recovering": [asked], "unread": unread,
            });
            serde_json::to_vec(&state).unwrap()
        };
        let scheme = crate::XrplScheme::new(&account, node, Some(0), &state(json!([asked])));
        let kept: Value = serde_json::from_slice(&scheme.unwrap().state()).unwrap();
        assert_eq!(kept["unread"], json!([asked]));
        for unread in [json!([other]), json!([asked, asked]), json!(asked)] {
            let scheme = crate::XrplScheme::new(&account, node, Some(0), &state(unread));
            assert!(scheme.is_err());
        }
    }
}

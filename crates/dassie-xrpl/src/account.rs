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
//! - each ledger that closes, and each transaction a scheme looked up;
//! - the trust lines its schemes follow, read at each sync as of the same
//!   ledger as the account, and an issuer's account a scheme asked for.
//!
//! Each scheme keeps its own cursor into the transactions, and tells the
//! account where it is ([`XrplAccount::follow`]): on each connection the
//! account reads its history from the earliest cursor of its schemes. What
//! a scheme not on the account (stopped, or removed) misses of its own
//! transactions, the account keeps for it until it is back: one that can
//! never apply as never sent, one the ledger validated as kept, so that the
//! scheme books it as its own when it reads it again, or, when the history
//! no longer reaches back to it, is told it did not, and books it as its
//! own once it has looked it up. So is what it was told of its own in the
//! input whose bookings the books refused, which stopped it.
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
    AssetEffect, AssetId, AssetKind, Detach, ExternalAsset, Freeze, Holding, LedgerInput,
    LedgerStatus, SchemeError,
};
use serde_json::{Value, json};

use crate::amount::{Currency, TokenAmount, TokenValue};
use crate::api::{
    self, AccountInfo, AccountSettings, ApiError, LedgerClosed, Message, TrustLine, TxRecord,
};
use crate::codec::{Amount, Memo, Payment, Signing, Transact, TrustSet};
use crate::keys::{AccountId, Wallet};
use crate::{UNITS_PER_DROP, from_hex, hex_upper};

/// The scale the account reports a token it holds at: fifteen decimals,
/// the most a token's sixteen digits leave below a unit of it.
pub const TOKEN_SCALE: u8 = 15;

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
/// each transaction in flight, the schemes' lookups, the four that keep
/// the account in sync, and for each scheme a line and an issuer read.
const MAX_PENDING: usize = 2 * MAX_IN_FLIGHT + MAX_LOOKUPS + 4 + 2 * MAX_SCHEMES;
/// Most transactions held while catching up.
const MAX_BUFFERED: usize = 256;
/// Most transactions remembered as never sent, until their schemes have
/// reconciled them. Past it, the oldest is forgotten, and its scheme
/// looks it up by its hash, which finds nothing and leaves it pending.
const MAX_NEVER_SENT: usize = 2 * MAX_IN_FLIGHT;
/// Most transactions kept for schemes that stopped, or were removed, after
/// the ledger validated them: each scheme's are what it had in flight, and
/// it signs nothing more until it has read them again.
const MAX_PARKED: usize = MAX_IN_FLIGHT * MAX_SCHEMES;
/// Most news kept between two tellings: one message's worth, at most a page
/// of history, what was held while catching up, what was in flight, and
/// what was kept for schemes and not read again.
// PAGE is a few hundred: no truncation.
#[allow(clippy::cast_possible_truncation)]
const MAX_NEWS: usize =
    api::PAGE as usize + MAX_BUFFERED + MAX_IN_FLIGHT + MAX_PARKED + 4 + 2 * MAX_SCHEMES;
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
/// devnet's 2. Nothing signs for a server that reports another
/// (spec/decisions/real-money.md).
pub const TESTNET_NETWORK_ID: u32 = 1;
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

/// The scheme that settles in the account's XRP: on an account it settles
/// from, what other schemes spend of the XRP as gas is the owner's funds on
/// its ledger (spec/decisions/external-assets.md).
const XRP_SCHEME: &str = crate::scheme::SCHEME_ID;

/// A trust line the account follows for a scheme on it: of its token's
/// issuer, in its currency.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LineKey {
    /// The token's issuer.
    pub issuer: AccountId,
    /// The token's currency.
    pub currency: Currency,
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
    /// account, that it did not read again once back: the server's history
    /// no longer reaches back to it. What it did is in the balance the
    /// scheme synced to; the scheme looks it up by its hash, and books it
    /// as one it never read, by the amount it settled.
    Unread {
        /// The transaction.
        hash: [u8; 32],
        /// What it was, as the scheme signed it.
        ours: Ours,
    },
    /// A transaction a scheme looked up, validated.
    Found {
        /// The hash the scheme looked up.
        hash: [u8; 32],
        /// What the ledger holds under it.
        record: TxRecord,
    },
    /// A trust line a scheme follows, as of the ledger of the
    /// [`Synced`](Self::Synced) that comes right after it: `None` if the
    /// account has none.
    Line {
        /// Which line.
        key: LineKey,
        /// The line.
        line: Option<TrustLine>,
    },
    /// An issuer's account a scheme asked for, as of a validated ledger at
    /// or past the newest the account had seen: `None` if it does not
    /// exist.
    Issuer {
        /// The issuer.
        issuer: AccountId,
        /// What its account says about its tokens.
        settings: Option<AccountSettings>,
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
    /// Past its last ledger and looked for.
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
    AccountInfo,
    AccountTx,
    Tx([u8; 32]),
    Submit([u8; 32]),
    /// A transaction a scheme looks up, wherever the server's history has
    /// it.
    LookUp([u8; 32]),
    /// A line a scheme follows, as of the ledger the account is read at.
    Line(LineKey, u32),
    /// An issuer's account, for a scheme.
    Issuer(AccountId),
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
    /// The schemes the books stopped, until the node restarts: what they
    /// settle in is still on their books.
    stopped: BTreeSet<Name>,
    /// The trust lines its schemes follow, as the ledger last said: `None`
    /// until it said, then the line, or `None` if there is none.
    lines: BTreeMap<LineKey, Option<Option<TrustLine>>>,
    /// The lines read on this connection as of `reads_at`, the ledger the
    /// account was read at.
    reads: BTreeMap<LineKey, Option<TrustLine>>,
    reads_at: u32,
    /// The account of each issuer of a line its schemes follow, as last
    /// read for a scheme: shown with the line, never kept.
    issuers: BTreeMap<AccountId, AccountSettings>,
    /// What the owner may spend on each scheme's ledger, by the books.
    funds: BTreeMap<Name, u64>,
    news: Vec<News>,
    effects: Vec<AssetEffect>,
}

impl XrplAccount {
    /// The account `id`, through which `schemes` may settle, with the
    /// wallet of `entropy`, connecting as `config` says, from the state the
    /// books kept.
    ///
    /// # Errors
    ///
    /// If the config or the state is invalid: a state that names a scheme
    /// not among `schemes` is one the account did not write.
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
        let mut account = Self {
            id,
            schemes: schemes.to_vec(),
            config: Config::parse(config)?,
            wallet: Wallet::from_entropy(entropy),
            sequence: 0,
            in_flight: Vec::new(),
            never_sent: Vec::new(),
            parked: Vec::new(),
            write_offs: 0,
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
            stopped: BTreeSet::new(),
            lines: BTreeMap::new(),
            reads: BTreeMap::new(),
            reads_at: 0,
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

    /// Follows the trust line `key` for a scheme on the account: it is read
    /// at every sync, as of the ledger the account is read at, and kept
    /// as each transaction leaves it. A scheme follows it before the
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

    /// The trust line `key`, as the ledger last said: `None` until it
    /// said, then the line, or `None` if there is none.
    #[must_use]
    pub fn line(&self, key: &LineKey) -> Option<Option<&TrustLine>> {
        self.lines.get(key)?.as_ref().map(Option::as_ref)
    }

    /// Reads the account of `issuer` for a scheme, if synced and not being
    /// read already: what it says comes as [`News::Issuer`], only as of a
    /// validated ledger at or past the newest the account has seen.
    pub fn read_issuer(&mut self, issuer: AccountId) {
        let reading = self
            .pending
            .values()
            .any(|(request, _)| *request == Request::Issuer(issuer));
        if self.phase == Phase::Synced && !reading {
            self.send(Request::Issuer(issuer));
        }
    }

    /// Whether transaction `hash` is in flight, or remembered as never
    /// sent: a scheme never looks such a one up.
    #[must_use]
    pub fn holds_transaction(&self, hash: &[u8; 32]) -> bool {
        self.in_flight.iter().any(|t| t.hash == *hash)
            || self.never_sent.iter().any(|n| n.hash == *hash)
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
        let xrp = dassie_ledger::name!(XRP_SCHEME);
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
        let xrp = dassie_ledger::name!(XRP_SCHEME);
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
            // Only the testnet is signed for, and a transaction names its
            // network only above 1024.
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
    /// as the ledger last said, after the token `scheme` has in flight: a
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
            Request::Line(key, ledger) => {
                api::ripple_state_at(id, account, key.issuer, key.currency, ledger)
            }
            Request::Issuer(issuer) => api::account_info(id, issuer),
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
        self.reads.clear();
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
            Request::LookUp(hash) => self.found(hash, outcome),
            Request::Line(key, ledger) => self.line_read(key, ledger, outcome),
            Request::Issuer(issuer) => self.issuer_read(issuer, outcome),
        }
    }

    /// A line a scheme follows, read as of `ledger`, where the account was
    /// read: kept until the account is synced. An answer as of another
    /// ledger makes the connection anew, which reads the account again.
    fn line_read(&mut self, key: LineKey, ledger: u32, outcome: Result<Value, ApiError>) {
        let reading = matches!(
            self.phase,
            Phase::Starting {
                info: Some(info),
                ..
            } if info.ledger_index == ledger
        );
        if !reading || self.reads_at != ledger {
            return;
        }
        let read = match outcome {
            Ok(result) => match api::ripple_state_result(&result) {
                Some(entry)
                    if entry.ledger_index == ledger
                        && entry
                            .line
                            .is_between(self.account(), key.issuer, key.currency) =>
                {
                    Some(entry.line)
                }
                // As of another ledger than the account: read again, on a
                // new connection.
                Some(_) => {
                    self.reconnect();
                    return;
                }
                None => {
                    self.reconnect();
                    return;
                }
            },
            Err(error)
                if error.error == "entryNotFound"
                    && error.ledger_index.is_none_or(|at| at == ledger) =>
            {
                None
            }
            Err(_) => {
                self.reconnect();
                return;
            }
        };
        self.reads.insert(key, read);
        self.start();
    }

    /// An issuer's account a scheme asked for: news only as of a validated
    /// ledger at or past the newest the account has seen; an older answer
    /// is dropped, and the scheme asks again (spec/decisions/ledger-resets.md).
    fn issuer_read(&mut self, issuer: AccountId, outcome: Result<Value, ApiError>) {
        let seen = self.seen();
        let settings = match outcome {
            Ok(result) => match api::account_settings_result(&result) {
                Some(settings) if settings.validated && settings.ledger_index >= seen => {
                    Some(settings)
                }
                _ => return,
            },
            Err(error)
                if error.error == "actNotFound"
                    && error.validated
                    && error.ledger_index.is_some_and(|at| at >= seen) =>
            {
                None
            }
            Err(_) => return,
        };
        // What the owner sees of the lines it issues.
        if self.lines.keys().any(|key| key.issuer == issuer) {
            match settings {
                Some(settings) => self.issuers.insert(issuer, settings),
                None => self.issuers.remove(&issuer),
            };
        }
        self.tell(News::Issuer { issuer, settings });
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
        // The lines went with the account, and the issuers' accounts with
        // the chain: read again once it is back.
        for state in self.lines.values_mut() {
            *state = None;
        }
        self.issuers.clear();
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
        // The lines its schemes follow, as of the same ledger, so that what
        // a scheme books from them and from the transactions after it adds
        // up.
        if !self.lines_read(info.ledger_index) {
            return;
        }
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

    /// Whether every line the schemes follow was read as of `ledger`;
    /// those not read yet are asked for.
    fn lines_read(&mut self, ledger: u32) -> bool {
        if self.reads_at != ledger {
            self.reads.clear();
            self.reads_at = ledger;
        }
        let missing: Vec<LineKey> = self
            .lines
            .keys()
            .filter(|key| !self.reads.contains_key(key))
            .copied()
            .collect();
        for key in &missing {
            let asking = self
                .pending
                .values()
                .any(|(request, _)| *request == Request::Line(*key, ledger));
            if !asking {
                self.send(Request::Line(*key, ledger));
            }
        }
        missing.is_empty()
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
        for (key, line) in core::mem::take(&mut self.reads) {
            if let Some(state) = self.lines.get_mut(&key) {
                *state = Some(line);
                self.tell(News::Line { key, line });
            }
        }
        self.tell(News::Synced(info));
        for record in core::mem::take(&mut self.buffer) {
            self.transaction(&record);
        }
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
        self.cancelled(transaction.scheme, transaction.hash, !transaction.trust_set);
        self.send(Request::AccountInfo);
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

    /// A scheme's lookup of `hash`, answered: only a transaction found
    /// validated is news, told under the hash asked for. Not found, or not
    /// validated yet, it waits, to be asked again. One that names more XRP
    /// than the books can hold stops the account, as one from the stream
    /// does, before any scheme books it.
    fn found(&mut self, hash: [u8; 32], outcome: Result<Value, ApiError>) {
        let Ok(result) = outcome else {
            return;
        };
        let Some(record) = api::tx_result(&result).filter(|r| r.validated) else {
            return;
        };
        if !self.bookable_record(&record) {
            self.failed = Some(TOO_MUCH_XRP);
            return;
        }
        self.tell(News::Found { hash, record });
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
        for parked in &self.parked {
            add(head(&parked.hash));
        }
        add(self.parked.len() as u64);
        add(self.write_offs);
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
            "newest": self.newest.map(|newest| json!([newest.index, newest.close_time])),
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
        assert!(self.buffer.len() <= MAX_BUFFERED);
        assert!(self.followers.len() <= MAX_SCHEMES);
        assert!(
            self.stopped.iter().all(|scheme| self.schemes.contains(scheme)
                && !self.followers.contains_key(scheme)),
            "a scheme stopped is one of the account's, and not on it"
        );
        assert!(self.lines.len() <= MAX_SCHEMES);
        assert!(self.funds.len() <= MAX_SCHEMES);
        assert!(
            self.reads.keys().all(|key| self.lines.contains_key(key)),
            "only followed lines are read"
        );
        assert!(
            self.issuers
                .keys()
                .all(|issuer| self.lines.keys().any(|key| key.issuer == *issuer)),
            "only the issuers of followed lines are shown"
        );
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
    // the lines, so a global freeze of a chain gone is never shown with a
    // line of the new one; the issuer read again on it says what holds.
    #[test]
    fn a_write_off_forgets_the_issuers_with_the_chain() {
        let rlusd = dassie_ledger::name!(crate::rlusd::SCHEME_ID);
        let issuer = crate::rlusd::ISSUER;
        let answer = |flags: u32, ledger: u32| {
            Ok(json!({
                "account_data": { "Flags": flags },
                "ledger_index": ledger,
                "validated": true,
            }))
        };
        for restarted in [true, false] {
            let mut account = synced_account(&[rlusd]);
            account.network_id = Some(TESTNET_NETWORK_ID);
            account.newest = Some(NEWEST);
            account.follow_line(crate::rlusd::LINE);
            account.issuer_read(
                issuer,
                answer(api::account_flags::GLOBAL_FREEZE, NEWEST.index),
            );
            assert!(account.issuers[&issuer].global_freeze());
            account.check_invariants();
            if restarted {
                // Told by the server's state, the first answer on a
                // connection.
                account.phase = Phase::Starting {
                    subscribed: false,
                    state: false,
                    info: None,
                };
                account.restarted(Validated {
                    index: 2_000,
                    close_time: NEWEST.close_time + 4,
                });
            } else {
                account.absent();
            }
            assert!(account.issuers.is_empty(), "restarted: {restarted}");
            account.check_invariants();
            account.told();
            account.issuer_read(issuer, answer(0, NEWEST.index));
            assert!(!account.issuers[&issuer].global_freeze());
        }
    }

    // A lookup is told under the hash the scheme asked for, whatever hash
    // the server's answer names, and the scheme settles that settlement.
    #[test]
    fn a_lookup_is_told_under_the_hash_asked_for() {
        use dassie_settlement::{Effect, SettlementScheme};

        let id = dassie_ledger::name!(crate::scheme::SCHEME_ID);
        let mut account = synced_account(&[id]);
        let (asked, answered) = ([1; 32], [2; 32]);
        let state =
            json!({ "cursor": [10, u32::MAX], "peers": {}, "recovering": [hex_upper(&asked)] });
        let state = serde_json::to_vec(&state).unwrap();
        let node = dassie_ledger::name!("node");
        let mut scheme = crate::XrplScheme::new(&account, node, Some(0), &state).unwrap();
        account.look_up(asked);
        let request = *account.pending.keys().next().unwrap();
        let answer = json!({
            "type": "response", "id": request, "status": "success",
            "result": {
                "hash": hex_upper(&answered), "ledger_index": 9, "validated": true,
                "Account": account.account().to_address(), "TransactionType": "Payment",
                "Sequence": 4, "Fee": "10",
                "meta": { "TransactionIndex": 0, "TransactionResult": "tesSUCCESS" },
            },
        });
        let answer = serde_json::to_vec(&answer).unwrap();
        account.on_ledger(0, LedgerInput::Message(&answer));
        assert!(
            matches!(account.news(), [News::Found { hash, record }] if *hash == asked && record.hash == answered),
            "{:?}",
            account.news()
        );
        let mut effects = Vec::new();
        scheme.on_asset(0, &mut account, &mut effects);
        let settlement = Name::hex(&asked, true);
        assert_eq!(effects, [Effect::Finalize { settlement }]);
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

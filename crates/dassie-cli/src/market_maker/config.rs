//! The market maker's config: a JSON file, read once and checked whole
//! before anything connects (spec/areas/rate-feed.md#market-maker).
//!
//! ```json
//! {
//!   "socket": "/run/dassie-mm-node/dassie.sock",
//!   "passphraseFile": "/run/dassie-mm/node.phrase",
//!   "xrpl": {
//!     "server": "wss://s.altnet.rippletest.net:51233",
//!     "token": { "currency": "DSUSD", "issuer": "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M" }
//!   },
//!   "xrp": {
//!     "ledger": "hub-xrpl-testnet-xrp+xrp",
//!     "ownerBook": "xrpl-testnet-xrp+xrp",
//!     "tradeSize": 10000000000,
//!     "band": { "low": 20000000000, "high": 200000000000 }
//!   },
//!   "token": {
//!     "ledger": "hub-xrpl-testnet-dsusd+usd",
//!     "ownerBook": "xrpl-testnet-dsusd+usd",
//!     "…": "…"
//!   },
//!   "marginPpm": 5000,
//!   "slippagePpm": 10000
//! }
//! ```
//!
//! `socket` is the IPC socket of the market maker's one node, and
//! `passphraseFile` the file that holds its passphrase, for its XRPL
//! account's keys. Each asset names two of the node's ledgers, each checked
//! whole: `ledger`, the ledger of the settlement hub of the asset, which
//! its swaps pay on and are paid back on, and whose scheme, before the
//! `+`, is the settlement method a target's snapshot names it by, and
//! `ownerBook`, the owner book of the asset on the node's XRP Ledger
//! account, which funds the hub and which its rebalances spend from
//! (spec/areas/rate-feed.md#market-maker). The targets are not named here:
//! they are the node's peers, the uplinks it is pinned to, whose snapshots
//! it reads. Amounts are in the node's ledger units (scale 9). `caFile`
//! adds roots to trust the XRPL server by, besides the platform's, and
//! `allowPrivate` lets it be on a private address, and on `ws://`: both
//! for dev and tests, whose hosts may have no roots of their own. Every problem is a [`ConfigError`]; nothing
//! here panics on what the file says.

use std::fmt;
use std::path::{Path, PathBuf};

use dassie_node::outbound::host_and_port;
use dassie_xrpl::Wallet;
use dassie_xrpl::amount::{Currency, IssuedAsset};
use dassie_xrpl::keys::AccountId;
use dassie_xrpl::network::TESTNET;
use serde_json::{Map, Value};

use super::strategy::{AssetParams, Band, MAX_MARGIN_PPM, MAX_SLIPPAGE_PPM, Params, Side};
use crate::passphrase::{self, ReadProblem, read_bounded};

/// Most bytes in a config file.
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// Most bytes in a ledger's ID: its scheme's, `+` and its asset's.
const MAX_LEDGER_ID_BYTES: usize = 64;

/// Most bytes in a settlement method's ID, its scheme's: more than any
/// scheme a node runs has. A snapshot names a method by a tag of it.
const MAX_METHOD_ID_BYTES: usize = 32;

/// Most bytes in a URL or a path.
const MAX_TEXT_BYTES: usize = 1024;

/// The most a node holds, in its ledger's units: what an owner's balance
/// can be.
pub const MAX_UNITS: u64 = i64::MAX.unsigned_abs();

/// The node's two ledgers of one asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetLedgers {
    /// The ledger of the settlement hub of the asset, which swaps pay on
    /// and are paid back on, such as `hub-xrpl-testnet-xrp+xrp`.
    pub ledger: String,
    /// The settlement method of that hub, as a target's snapshot names it
    /// and `SwapWithPeer` takes it: the ledger's scheme, such as
    /// `hub-xrpl-testnet-xrp`.
    pub method: String,
    /// The ledger of the owner book of the asset on the node's XRP Ledger
    /// account, which funds the hub and which a rebalance spends from, such
    /// as `xrpl-testnet-xrp+xrp`.
    pub owner_book: String,
}

/// The whole config, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The node's IPC socket.
    pub socket: PathBuf,
    /// The file that holds the node's passphrase, for its XRPL account's
    /// keys.
    pub passphrase_file: PathBuf,
    /// The XRPL server, `wss://` (or `ws://` with `allow_private`).
    pub xrpl_server: String,
    /// The token the pool pairs with XRP.
    pub token: IssuedAsset,
    /// The node's ledgers of XRP.
    pub xrp: AssetLedgers,
    /// The node's ledgers of the token.
    pub token_ledgers: AssetLedgers,
    /// The strategy's settings.
    pub params: Params,
    /// Roots to trust the XRPL server by, besides the platform's.
    pub ca_file: Option<PathBuf>,
    /// Whether private addresses, and `ws://`, are allowed.
    pub allow_private: bool,
}

impl Config {
    /// The node's ledgers of `side`'s asset.
    #[must_use]
    pub const fn ledgers(&self, side: Side) -> &AssetLedgers {
        match side {
            Side::Xrp => &self.xrp,
            Side::Token => &self.token_ledgers,
        }
    }
}

/// Why a config is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The file cannot be read.
    Read(String),
    /// It is longer than [`MAX_CONFIG_BYTES`].
    TooLong,
    /// It is not JSON, or not an object.
    Json,
    /// A field it does not know.
    Unknown(String),
    /// A field it needs is missing.
    Missing(String),
    /// A field's value is not what it may be.
    Invalid(String),
    /// Both assets name the same ledger, method or owner book.
    SameLedger(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(problem) => write!(f, "cannot read the config: {problem}"),
            Self::TooLong => write!(f, "the config is longer than {MAX_CONFIG_BYTES} bytes"),
            Self::Json => f.write_str("the config is not a JSON object"),
            Self::Unknown(field) => write!(f, "the config has an unknown field {field:?}"),
            Self::Missing(field) => write!(f, "the config has no {field:?}"),
            Self::Invalid(field) => write!(f, "the config's {field:?} is not valid"),
            Self::SameLedger(what) => write!(f, "the two assets have the same {what}"),
        }
    }
}

/// Reads and checks the config at `path`.
///
/// # Errors
///
/// It cannot be read, or [`parse`] refuses it.
pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = read_bounded(path, MAX_CONFIG_BYTES).map_err(|problem| match problem {
        ReadProblem::TooLong => ConfigError::TooLong,
        ReadProblem::Io(problem) => ConfigError::Read(problem),
    })?;
    parse(&text)
}

/// Checks a config.
///
/// # Errors
///
/// The first thing wrong with it.
pub fn parse(text: &str) -> Result<Config, ConfigError> {
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLong);
    }
    let value: Value = serde_json::from_str(text).map_err(|_| ConfigError::Json)?;
    let root = Fields::of(
        &value,
        "",
        &[
            "socket",
            "passphraseFile",
            "xrpl",
            "xrp",
            "token",
            "marginPpm",
            "slippagePpm",
            "caFile",
            "allowPrivate",
        ],
    )?;
    let allow_private = match root.get("allowPrivate") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| root.invalid("allowPrivate"))?,
    };
    let ca_file = match root.get("caFile") {
        None => None,
        Some(_) => Some(PathBuf::from(root.text("caFile")?)),
    };
    let socket = PathBuf::from(root.text("socket")?);
    let passphrase_file = PathBuf::from(root.text("passphraseFile")?);
    let xrpl = root.object("xrpl", &["server", "token"])?;
    let xrpl_server = websocket_url(&xrpl, "server", allow_private)?;
    let token = token(&xrpl.object("token", &["currency", "issuer"])?)?;
    let (xrp, xrp_params) = asset(&root, "xrp")?;
    let (token_ledgers, token_params) = asset(&root, "token")?;
    if xrp.ledger == token_ledgers.ledger {
        return Err(ConfigError::SameLedger("ledger"));
    }
    if xrp.method == token_ledgers.method {
        return Err(ConfigError::SameLedger("method"));
    }
    if xrp.owner_book == token_ledgers.owner_book {
        return Err(ConfigError::SameLedger("owner book"));
    }
    let params = Params {
        margin_ppm: root.ppm("marginPpm", MAX_MARGIN_PPM)?,
        slippage_ppm: root.ppm("slippagePpm", MAX_SLIPPAGE_PPM)?,
        xrp: xrp_params,
        token: token_params,
    };
    Ok(Config {
        socket,
        passphrase_file,
        xrpl_server,
        token,
        xrp,
        token_ledgers,
        params,
        ca_file,
        allow_private,
    })
}

/// An object's fields, with its path for errors.
struct Fields<'a> {
    map: &'a Map<String, Value>,
    path: String,
}

impl<'a> Fields<'a> {
    /// `value` as an object with only the `known` fields.
    fn of(value: &'a Value, path: &str, known: &[&str]) -> Result<Self, ConfigError> {
        let map = value.as_object().ok_or_else(|| {
            if path.is_empty() {
                ConfigError::Json
            } else {
                ConfigError::Invalid(path.to_owned())
            }
        })?;
        let fields = Self {
            map,
            path: path.to_owned(),
        };
        if let Some(unknown) = map.keys().find(|key| !known.contains(&key.as_str())) {
            return Err(ConfigError::Unknown(fields.name(unknown)));
        }
        Ok(fields)
    }

    fn name(&self, key: &str) -> String {
        if self.path.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.path)
        }
    }

    fn invalid(&self, key: &str) -> ConfigError {
        ConfigError::Invalid(self.name(key))
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        self.map.get(key)
    }

    fn need(&self, key: &str) -> Result<&'a Value, ConfigError> {
        self.get(key)
            .ok_or_else(|| ConfigError::Missing(self.name(key)))
    }

    fn object(&self, key: &str, known: &[&str]) -> Result<Self, ConfigError> {
        Fields::of(self.need(key)?, &self.name(key), known)
    }

    /// A non-empty string of at most [`MAX_TEXT_BYTES`].
    fn text(&self, key: &str) -> Result<&'a str, ConfigError> {
        self.need(key)?
            .as_str()
            .filter(|text| !text.is_empty() && text.len() <= MAX_TEXT_BYTES)
            .ok_or_else(|| self.invalid(key))
    }

    /// A whole number of ledger units, at most [`MAX_UNITS`].
    fn units(&self, key: &str) -> Result<u64, ConfigError> {
        self.need(key)?
            .as_u64()
            .filter(|units| *units <= MAX_UNITS)
            .ok_or_else(|| self.invalid(key))
    }

    /// Parts per million, at most `max`.
    fn ppm(&self, key: &str, max: u32) -> Result<u32, ConfigError> {
        self.need(key)?
            .as_u64()
            .and_then(|ppm| u32::try_from(ppm).ok())
            .filter(|ppm| *ppm <= max)
            .ok_or_else(|| self.invalid(key))
    }
}

/// A WebSocket URL: `wss://`, or `ws://` where private addresses are
/// allowed, with a host.
fn websocket_url(
    fields: &Fields<'_>,
    key: &str,
    allow_private: bool,
) -> Result<String, ConfigError> {
    let text = fields.text(key)?;
    let rest = match (text.strip_prefix("wss://"), text.strip_prefix("ws://")) {
        (Some(rest), _) => rest,
        (None, Some(rest)) if allow_private => rest,
        _ => return Err(fields.invalid(key)),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') || host_and_port(authority, 443).is_none() {
        return Err(fields.invalid(key));
    }
    Ok(text.to_owned())
}

fn token(fields: &Fields<'_>) -> Result<IssuedAsset, ConfigError> {
    let currency =
        parse_currency(fields.text("currency")?).ok_or_else(|| fields.invalid("currency"))?;
    let issuer =
        AccountId::parse(fields.text("issuer")?).ok_or_else(|| fields.invalid("issuer"))?;
    Ok(IssuedAsset { currency, issuer })
}

/// A currency code: three characters, 40 hex digits, or a longer code of
/// up to twenty letters and digits that the ledger holds as its bytes
/// padded with zeros, as DSUSD's.
#[must_use]
pub fn parse_currency(text: &str) -> Option<Currency> {
    if text.len() == 3 || text.len() == 40 {
        return Currency::parse(text);
    }
    if !(4..=20).contains(&text.len()) || !text.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let mut bytes = [0; 20];
    bytes
        .get_mut(..text.len())?
        .copy_from_slice(text.as_bytes());
    let currency = Currency(bytes);
    currency.is_token().then_some(currency)
}

/// The node's ledgers of the asset `key` names, and its strategy's
/// settings.
fn asset(root: &Fields<'_>, key: &str) -> Result<(AssetLedgers, AssetParams), ConfigError> {
    let fields = root.object(key, &["ledger", "ownerBook", "tradeSize", "band"])?;
    let ledger = ledger_id(&fields, "ledger")?;
    let method = method_of(ledger).ok_or_else(|| fields.invalid("ledger"))?;
    let owner_book = ledger_id(&fields, "ownerBook")?;
    // The hub's ledger and the book that funds it are two.
    if owner_book == ledger {
        return Err(fields.invalid("ownerBook"));
    }
    let band_fields = fields.object("band", &["low", "high"])?;
    let band = Band {
        low: band_fields.units("low")?,
        high: band_fields.units("high")?,
    };
    if band.low >= band.high {
        return Err(fields.invalid("band"));
    }
    let trade_size = fields.units("tradeSize")?;
    if trade_size == 0 {
        return Err(fields.invalid("tradeSize"));
    }
    let ledgers = AssetLedgers {
        ledger: ledger.to_owned(),
        method: method.to_owned(),
        owner_book: owner_book.to_owned(),
    };
    Ok((ledgers, AssetParams { trade_size, band }))
}

/// The settlement method of the ledger `ledger`, as a snapshot names it:
/// its scheme, the ID before its one `+`, of at least two and at most
/// [`MAX_METHOD_ID_BYTES`] bytes; `None` for an ID that names no scheme
/// and asset.
fn method_of(ledger: &str) -> Option<&str> {
    let (scheme, asset) = ledger.split_once('+')?;
    let valid = (2..=MAX_METHOD_ID_BYTES).contains(&scheme.len())
        && !asset.is_empty()
        && !asset.contains('+');
    valid.then_some(scheme)
}

/// A ledger's ID, as `ListLedgers` names it: lowercase letters, digits,
/// `-` and `+`, at most [`MAX_LEDGER_ID_BYTES`].
fn ledger_id<'a>(fields: &Fields<'a>, key: &str) -> Result<&'a str, ConfigError> {
    let ledger = fields.text(key)?;
    let valid = ledger.len() <= MAX_LEDGER_ID_BYTES
        && ledger
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'+');
    if valid {
        Ok(ledger)
    } else {
        Err(fields.invalid(key))
    }
}

/// The wallet of the node whose passphrase is in `path`: the node's first
/// XRPL testnet account, derived exactly as the node derives it, from its
/// key (spec/areas/xrpl.md#the-wallet). The phrase is
/// wiped once read, and no error names it.
///
/// # Errors
///
/// The file cannot be read, or holds no passphrase of the test realm that
/// reads as written by a clock that reads `now_ms`.
pub fn wallet_from_file(path: &Path, now_ms: u64) -> Result<Wallet, String> {
    // The market maker trades on the testnet only, so its phrase is the
    // test realm's.
    let key = passphrase::node_key_from_file(path, dassie_identity::Realm::Test, now_ms)?;
    let entropy = key.xrpl_account_entropy(TESTNET.id, 0);
    Ok(Wallet::from_entropy(entropy.as_bytes()))
}

#[cfg(test)]
mod tests {
    use dassie_identity::{Created, Phrase, PhraseSecret, Realm};
    use serde_json::json;

    use super::*;

    fn valid() -> Value {
        json!({
            "socket": "/run/a.sock",
            "passphraseFile": "/etc/a",
            "xrpl": {
                "server": "wss://s.altnet.rippletest.net:51233",
                "token": { "currency": "DSUSD", "issuer": "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M" },
            },
            "xrp": {
                "ledger": "hub-xrpl-testnet-xrp+xrp",
                "ownerBook": "xrpl-testnet-xrp+xrp",
                "tradeSize": 1_000_000_000,
                "band": { "low": 10_000_000_000_u64, "high": 100_000_000_000_u64 },
            },
            "token": {
                "ledger": "hub-xrpl-testnet-dsusd+usd",
                "ownerBook": "xrpl-testnet-dsusd+usd",
                "tradeSize": 500_000_000,
                "band": { "low": 5_000_000_000_u64, "high": 50_000_000_000_u64 },
            },
            "marginPpm": 5_000,
            "slippagePpm": 10_000,
        })
    }

    fn with(edit: impl FnOnce(&mut Value)) -> Result<Config, ConfigError> {
        let mut value = valid();
        edit(&mut value);
        parse(&value.to_string())
    }

    #[test]
    fn a_valid_config_reads_whole() {
        let config = parse(&valid().to_string()).unwrap();
        assert_eq!(config.socket, PathBuf::from("/run/a.sock"));
        assert_eq!(config.passphrase_file, PathBuf::from("/etc/a"));
        assert_eq!(config.token.currency, dassie_xrpl::dsusd::CURRENCY);
        assert_eq!(config.token.issuer, dassie_xrpl::dsusd::ISSUER);
        assert_eq!(config.xrp.ledger, "hub-xrpl-testnet-xrp+xrp");
        assert_eq!(config.xrp.method, "hub-xrpl-testnet-xrp");
        assert_eq!(config.xrp.owner_book, "xrpl-testnet-xrp+xrp");
        assert_eq!(config.token_ledgers.ledger, "hub-xrpl-testnet-dsusd+usd");
        assert_eq!(config.token_ledgers.method, "hub-xrpl-testnet-dsusd");
        assert_eq!(config.token_ledgers.owner_book, "xrpl-testnet-dsusd+usd");
        assert_eq!(config.ledgers(Side::Token), &config.token_ledgers);
        assert_eq!(config.params.margin_ppm, 5_000);
        assert_eq!(config.params.token.trade_size, 500_000_000);
        assert_eq!(config.params.xrp.band.high, 100_000_000_000);
        assert!(!config.allow_private);
        assert_eq!(config.ca_file, None);
        let config = with(|v| v["caFile"] = json!("/etc/ca.pem")).unwrap();
        assert_eq!(config.ca_file, Some(PathBuf::from("/etc/ca.pem")));
    }

    // The targets are the node's peers: a config of the market maker's two
    // nodes, which named its targets, is refused whole rather than half
    // read.
    #[test]
    fn a_config_of_two_nodes_is_refused() {
        assert_eq!(
            with(|v| v["targets"] = json!(["https://node.example"])),
            Err(ConfigError::Unknown("targets".into()))
        );
        assert_eq!(
            with(|v| v["xrp"]["socket"] = json!("/run/b.sock")),
            Err(ConfigError::Unknown("xrp.socket".into()))
        );
        assert_eq!(
            with(|v| v["token"]["passphraseFile"] = json!("/etc/b")),
            Err(ConfigError::Unknown("token.passphraseFile".into()))
        );
    }

    #[test]
    fn every_field_is_checked() {
        let invalid = |field: &str| Err(ConfigError::Invalid(field.into()));
        assert_eq!(parse("[]"), Err(ConfigError::Json));
        assert_eq!(parse("{"), Err(ConfigError::Json));
        assert_eq!(
            with(|v| v["extra"] = json!(1)),
            Err(ConfigError::Unknown("extra".into()))
        );
        assert_eq!(
            with(|v| v["xrp"]["band"]["middle"] = json!(1)),
            Err(ConfigError::Unknown("xrp.band.middle".into()))
        );
        assert_eq!(
            with(|v| {
                v.as_object_mut().unwrap().remove("socket");
            }),
            Err(ConfigError::Missing("socket".into()))
        );
        assert_eq!(
            with(|v| {
                v.as_object_mut().unwrap().remove("passphraseFile");
            }),
            Err(ConfigError::Missing("passphraseFile".into()))
        );
        assert_eq!(with(|v| v["socket"] = json!("")), invalid("socket"));
        assert_eq!(with(|v| v["caFile"] = json!("")), invalid("caFile"));
        assert_eq!(with(|v| v["caFile"] = json!(7)), invalid("caFile"));
        assert_eq!(
            with(|v| v["xrpl"]["server"] = json!("ws://localhost:6006")),
            invalid("xrpl.server")
        );
        assert!(
            with(|v| {
                v["xrpl"]["server"] = json!("ws://localhost:6006");
                v["allowPrivate"] = json!(true);
            })
            .is_ok()
        );
        assert_eq!(
            with(|v| v["xrpl"]["token"]["currency"] = json!("XRP")),
            invalid("xrpl.token.currency")
        );
        assert_eq!(
            with(|v| v["xrpl"]["token"]["issuer"] = json!("rNotAnAddress")),
            invalid("xrpl.token.issuer")
        );
        assert_eq!(
            with(|v| v["allowPrivate"] = json!("yes")),
            invalid("allowPrivate")
        );
    }

    #[test]
    fn every_assets_field_is_checked() {
        let invalid = |field: &str| Err(ConfigError::Invalid(field.into()));
        assert_eq!(
            with(|v| v["xrp"]["ledger"] = json!("XRPL Testnet")),
            invalid("xrp.ledger")
        );
        // A hub's ledger names the method a snapshot names it by: its
        // scheme, and an asset.
        for ledger in [
            "hub-xrpl-testnet-xrp",
            "hub-xrpl-testnet-xrp+",
            "h+xrp",
            "a+b+c",
        ] {
            assert_eq!(
                with(|v| v["xrp"]["ledger"] = json!(ledger)),
                invalid("xrp.ledger"),
                "{ledger}"
            );
        }
        let long = format!("{}+xrp", "a".repeat(MAX_METHOD_ID_BYTES + 1));
        assert_eq!(
            with(|v| v["xrp"]["ledger"] = json!(long)),
            invalid("xrp.ledger")
        );
        assert_eq!(
            with(|v| v["token"]["ownerBook"] = json!("xrpl-testnet-dsusd usd")),
            invalid("token.ownerBook")
        );
        assert_eq!(
            with(|v| {
                v["xrp"].as_object_mut().unwrap().remove("ownerBook");
            }),
            Err(ConfigError::Missing("xrp.ownerBook".into()))
        );
        // A hub is funded from a book of its own.
        assert_eq!(
            with(|v| v["xrp"]["ownerBook"] = json!("hub-xrpl-testnet-xrp+xrp")),
            invalid("xrp.ownerBook")
        );
        assert_eq!(
            with(|v| v["xrp"]["tradeSize"] = json!(0)),
            invalid("xrp.tradeSize")
        );
        assert_eq!(
            with(|v| v["xrp"]["tradeSize"] = json!(1.5)),
            invalid("xrp.tradeSize")
        );
        assert_eq!(
            with(|v| v["xrp"]["tradeSize"] = json!(-1)),
            invalid("xrp.tradeSize")
        );
        assert_eq!(
            with(|v| v["xrp"]["tradeSize"] = json!(u64::MAX)),
            invalid("xrp.tradeSize")
        );
        assert_eq!(
            with(|v| v["xrp"]["band"]["low"] = json!(100_000_000_000_u64)),
            invalid("xrp.band")
        );
        assert_eq!(
            with(|v| v["marginPpm"] = json!(MAX_MARGIN_PPM + 1)),
            invalid("marginPpm")
        );
        assert_eq!(
            with(|v| v["slippagePpm"] = json!(MAX_SLIPPAGE_PPM + 1)),
            invalid("slippagePpm")
        );
    }

    #[test]
    fn the_two_assets_are_two() {
        assert_eq!(
            with(|v| v["token"]["ledger"] = json!("hub-xrpl-testnet-xrp+xrp")),
            Err(ConfigError::SameLedger("ledger"))
        );
        assert_eq!(
            with(|v| v["token"]["ledger"] = json!("hub-xrpl-testnet-xrp+usd")),
            Err(ConfigError::SameLedger("method"))
        );
        assert_eq!(
            with(|v| v["token"]["ownerBook"] = json!("xrpl-testnet-xrp+xrp")),
            Err(ConfigError::SameLedger("owner book"))
        );
    }

    #[test]
    fn currencies_read_as_the_ledger_holds_them() {
        assert_eq!(parse_currency("USD"), Currency::parse("USD"));
        assert_eq!(
            parse_currency("524C555344000000000000000000000000000000"),
            Some(dassie_xrpl::rlusd::CURRENCY)
        );
        assert_eq!(parse_currency("DSUSD"), Some(dassie_xrpl::dsusd::CURRENCY));
        assert_eq!(parse_currency("XRP"), None);
        assert_eq!(parse_currency("DS-USD"), None);
        assert_eq!(parse_currency("ABCDEFGHIJKLMNOPQRSTU"), None);
    }

    #[test]
    fn a_config_too_long_or_missing_is_refused() {
        let long = format!("{{\"x\": \"{}\"}}", "a".repeat(70_000));
        assert_eq!(parse(&long), Err(ConfigError::TooLong));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert!(matches!(load(&path), Err(ConfigError::Read(_))));
        std::fs::write(&path, &long).unwrap();
        assert_eq!(load(&path), Err(ConfigError::TooLong));
        std::fs::write(&path, valid().to_string()).unwrap();
        assert!(load(&path).is_ok());
    }

    #[test]
    fn the_wallet_is_the_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("phrase");
        let created = Created::from_hours(7_000).unwrap();
        let now_ms = created.unix_ms();
        let phrase = Phrase::new(Realm::Test, created, &PhraseSecret::from_bytes([3; 16]));
        std::fs::write(&path, format!("{}\n", phrase.text().as_str())).unwrap();
        let wallet = wallet_from_file(&path, now_ms).unwrap();
        // As the node derives it from its key.
        let key = phrase.master_seed(Realm::Test).unwrap().node_key();
        let entropy = key.xrpl_account_entropy(TESTNET.id, 0);
        assert_eq!(
            wallet.account(),
            Wallet::from_entropy(entropy.as_bytes()).account()
        );
        std::fs::write(&path, "not a phrase").unwrap();
        let problem = wallet_from_file(&path, now_ms).err().unwrap();
        assert!(!problem.contains("not a phrase"), "{problem}");
        std::fs::write(&path, "a".repeat(4_096)).unwrap();
        assert!(wallet_from_file(&path, now_ms).is_err());
        // A live phrase is no market maker's.
        let live = Phrase::new(Realm::Live, created, &PhraseSecret::from_bytes([3; 16]));
        std::fs::write(&path, live.text().as_str()).unwrap();
        let problem = wallet_from_file(&path, now_ms).err().unwrap();
        assert!(problem.contains("live realm"), "{problem}");
    }
}

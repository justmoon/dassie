//! The market maker's config: a JSON file, read once and checked whole
//! before anything connects (spec/areas/rate-feed.md#market-maker).
//!
//! ```json
//! {
//!   "targets": ["https://a.example", { "url": "https://b.example", "feedPort": 7768 }],
//!   "xrpl": {
//!     "server": "wss://s.altnet.rippletest.net:51233",
//!     "token": { "currency": "DSUSD", "issuer": "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M" }
//!   },
//!   "xrp": {
//!     "socket": "/run/mm-xrp/dassie.sock",
//!     "ledger": "xrpl-testnet-xrp+xrp",
//!     "passphraseFile": "/etc/dassie-mm/xrp.phrase",
//!     "tradeSize": 10000000000,
//!     "band": { "low": 20000000000, "high": 200000000000 }
//!   },
//!   "token": { "socket": "…", "ledger": "xrpl-testnet-dsusd+usd", "…": "…" },
//!   "marginPpm": 5000,
//!   "slippagePpm": 10000
//! }
//! ```
//!
//! `targets` lists the core nodes to trade through, at least one and at
//! most [`MAX_TARGETS`], each once: a URL, or `{ url, feedPort }` for a
//! target whose discovery document names no feed port. Amounts are in the
//! nodes' ledger units (scale 9). `caFile` adds roots to trust the targets
//! by, and `allowPrivate` lets the targets and the XRPL server be on
//! private addresses, and the server on `ws://`: both for dev and tests.
//! Every problem is a [`ConfigError`]; nothing here panics
//! on what the file says.

use std::fmt;
use std::path::{Path, PathBuf};

use dassie_node::outbound::host_and_port;
use dassie_xrpl::Wallet;
use dassie_xrpl::amount::{Currency, IssuedAsset};
use dassie_xrpl::keys::AccountId;
use dassie_xrpl::network::TESTNET;
use serde_json::{Map, Value};

use super::strategy::{Band, MAX_MARGIN_PPM, MAX_SLIPPAGE_PPM, NodeParams, Params};
use crate::passphrase::{self, ReadProblem, read_bounded};

/// Most bytes in a config file.
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// Most bytes in a ledger's ID, as a feed names it.
const MAX_LEDGER_ID_BYTES: usize = dassie_feed::MAX_LEDGER_ID_BYTES;

/// Most bytes in a URL or a path.
const MAX_TEXT_BYTES: usize = 1024;

/// The most a node holds, in its ledger's units: what an owner's balance
/// can be.
pub const MAX_UNITS: u64 = i64::MAX.unsigned_abs();

/// Most targets a config names: no more than a node pins, since both nodes
/// pin every target (spec/areas/rate-feed.md#market-maker).
pub const MAX_TARGETS: usize = 128;

const _: () = assert!(MAX_TARGETS <= dassie_node::env::MAX_UPLINKS);

/// A target node, as the config names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// Its URL, `https://host[:port]`, without a trailing slash.
    pub url: String,
    /// Its host, as the URL names it, without brackets.
    pub host: String,
    /// Its port for HTTPS.
    pub port: u16,
    /// Its feed's UDP port, for a target whose discovery document names
    /// none.
    pub feed_port: Option<u16>,
}

/// One of the market maker's nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeConfig {
    /// Its IPC socket.
    pub socket: PathBuf,
    /// The ledger it runs, such as `xrpl-testnet-xrp+xrp`.
    pub ledger: String,
    /// The file that holds its passphrase, for its XRPL account's keys.
    pub passphrase_file: PathBuf,
}

/// The whole config, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// The nodes to trade through, in the config's order: at least one, at
    /// most [`MAX_TARGETS`], none twice.
    pub targets: Vec<Target>,
    /// The XRPL server, `wss://` (or `ws://` with `allow_private`).
    pub xrpl_server: String,
    /// The token the pool pairs with XRP.
    pub token: IssuedAsset,
    /// The node that holds XRP.
    pub xrp: NodeConfig,
    /// The node that holds the token.
    pub token_node: NodeConfig,
    /// The strategy's settings.
    pub params: Params,
    /// Roots to trust the target by, besides the platform's.
    pub ca_file: Option<PathBuf>,
    /// Whether private addresses, and `ws://`, are allowed.
    pub allow_private: bool,
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
    /// Both nodes run the same ledger, or use the same socket or
    /// passphrase.
    SameNode(&'static str),
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
            Self::SameNode(what) => write!(f, "the two nodes have the same {what}"),
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
            "targets",
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
    let targets = targets(&root)?;
    let xrpl = root.object("xrpl", &["server", "token"])?;
    let xrpl_server = websocket_url(&xrpl, "server", allow_private)?;
    let token = token(&xrpl.object("token", &["currency", "issuer"])?)?;
    let (xrp, xrp_params) = node(&root, "xrp")?;
    let (token_node, token_params) = node(&root, "token")?;
    if xrp.ledger == token_node.ledger {
        return Err(ConfigError::SameNode("ledger"));
    }
    if xrp.socket == token_node.socket {
        return Err(ConfigError::SameNode("socket"));
    }
    if xrp.passphrase_file == token_node.passphrase_file {
        return Err(ConfigError::SameNode("passphrase file"));
    }
    let params = Params {
        margin_ppm: root.ppm("marginPpm", MAX_MARGIN_PPM)?,
        slippage_ppm: root.ppm("slippagePpm", MAX_SLIPPAGE_PPM)?,
        xrp: xrp_params,
        token: token_params,
    };
    let ca_file = match root.get("caFile") {
        None => None,
        Some(_) => Some(PathBuf::from(root.text("caFile")?)),
    };
    Ok(Config {
        targets,
        xrpl_server,
        token,
        xrp,
        token_node,
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

/// The targets: the list `targets`.
fn targets(root: &Fields<'_>) -> Result<Vec<Target>, ConfigError> {
    let entries = root
        .need("targets")?
        .as_array()
        .filter(|entries| !entries.is_empty() && entries.len() <= MAX_TARGETS)
        .ok_or_else(|| root.invalid("targets"))?;
    let mut list = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let path = format!("targets[{index}]");
        // A bare URL is a target that names nothing else.
        let wrapped = entry
            .is_string()
            .then(|| serde_json::json!({ "url": entry }));
        let fields = Fields::of(
            wrapped.as_ref().unwrap_or(entry),
            &path,
            &["url", "feedPort"],
        )?;
        list.push(target(&fields)?);
    }
    // Each once: one host and port twice would subscribe to one feed twice,
    // and pin one uplink twice.
    for (index, target) in list.iter().enumerate() {
        let twice = list.iter().take(index).any(|earlier| {
            earlier.port == target.port && earlier.host.eq_ignore_ascii_case(&target.host)
        });
        if twice {
            return Err(root.invalid("targets"));
        }
    }
    assert!(
        !list.is_empty() && list.len() <= MAX_TARGETS,
        "checked above"
    );
    Ok(list)
}

fn target(fields: &Fields<'_>) -> Result<Target, ConfigError> {
    let text = fields.text("url")?;
    let invalid = || fields.invalid("url");
    let rest = text.strip_prefix("https://").ok_or_else(invalid)?;
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        return Err(invalid());
    }
    let (host, port) = host_and_port(rest, 443).ok_or_else(invalid)?;
    let feed_port = match fields.get("feedPort") {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or_else(|| fields.invalid("feedPort"))?,
        ),
    };
    Ok(Target {
        url: format!("https://{rest}"),
        host: host.to_owned(),
        port,
        feed_port,
    })
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

/// The node `key` names, and its strategy's settings.
fn node(root: &Fields<'_>, key: &str) -> Result<(NodeConfig, NodeParams), ConfigError> {
    let fields = root.object(
        key,
        &["socket", "ledger", "passphraseFile", "tradeSize", "band"],
    )?;
    let ledger = fields.text("ledger")?;
    let valid_ledger = ledger.len() <= MAX_LEDGER_ID_BYTES
        && ledger
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'+');
    if !valid_ledger {
        return Err(fields.invalid("ledger"));
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
    let config = NodeConfig {
        socket: PathBuf::from(fields.text("socket")?),
        ledger: ledger.to_owned(),
        passphrase_file: PathBuf::from(fields.text("passphraseFile")?),
    };
    Ok((config, NodeParams { trade_size, band }))
}

/// The wallet of the node whose passphrase is in `path`: the node's first
/// XRPL testnet account, derived exactly as the node derives it, from its
/// key (spec/areas/xrpl.md#the-wallet). The phrase is
/// wiped once read, and no error names it.
///
/// # Errors
///
/// The file cannot be read, or holds no passphrase.
pub fn wallet_from_file(path: &Path) -> Result<Wallet, String> {
    // The market maker trades on the testnet only, so its phrases are the
    // test realm's.
    let key = passphrase::node_key_from_file(path, dassie_identity::Realm::Test)?;
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
            "targets": ["https://node.example/"],
            "xrpl": {
                "server": "wss://s.altnet.rippletest.net:51233",
                "token": { "currency": "DSUSD", "issuer": "rURgPoYNN7tGqBzGXFeCLmKYgpSjYgJP6M" },
            },
            "xrp": {
                "socket": "/run/a.sock",
                "ledger": "xrpl-testnet-xrp+xrp",
                "passphraseFile": "/etc/a",
                "tradeSize": 1_000_000_000,
                "band": { "low": 10_000_000_000_u64, "high": 100_000_000_000_u64 },
            },
            "token": {
                "socket": "/run/b.sock",
                "ledger": "xrpl-testnet-dsusd+usd",
                "passphraseFile": "/etc/b",
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
        let [target] = config.targets.as_slice() else {
            panic!("one target");
        };
        assert_eq!(target.url, "https://node.example");
        assert_eq!(target.host, "node.example");
        assert_eq!(target.port, 443);
        assert_eq!(target.feed_port, None);
        assert_eq!(config.token.currency, dassie_xrpl::dsusd::CURRENCY);
        assert_eq!(config.token.issuer, dassie_xrpl::dsusd::ISSUER);
        assert_eq!(config.xrp.ledger, "xrpl-testnet-xrp+xrp");
        assert_eq!(config.params.margin_ppm, 5_000);
        assert_eq!(config.params.token.trade_size, 500_000_000);
        assert_eq!(config.params.xrp.band.high, 100_000_000_000);
        assert!(!config.allow_private);
        let config = with(|v| {
            v["targets"] = json!([
                "https://a.example",
                { "url": "https://[::1]:8443", "feedPort": 7768 },
            ]);
        })
        .unwrap();
        let hosts: Vec<_> = config
            .targets
            .iter()
            .map(|t| (t.host.as_str(), t.port, t.feed_port))
            .collect();
        assert_eq!(hosts, [("a.example", 443, None), ("::1", 8443, Some(7768))]);
    }

    #[test]
    fn the_targets_are_checked() {
        let invalid = |field: &str| Err(ConfigError::Invalid(field.into()));
        let targets = |list: Value| with(|v| v["targets"] = list);
        assert_eq!(
            with(|v| {
                v.as_object_mut().unwrap().remove("targets");
            }),
            Err(ConfigError::Missing("targets".into()))
        );
        // `targets` is the only form, however many there are.
        assert_eq!(
            with(|v| {
                v.as_object_mut().unwrap().remove("targets");
                v["target"] = json!({ "url": "https://node.example" });
            }),
            Err(ConfigError::Unknown("target".into()))
        );
        assert_eq!(targets(json!([])), invalid("targets"));
        assert_eq!(targets(json!("https://a.example")), invalid("targets"));
        let most: Vec<String> = (0..MAX_TARGETS)
            .map(|i| format!("https://n{i}.example"))
            .collect();
        assert_eq!(targets(json!(most)).unwrap().targets.len(), MAX_TARGETS);
        let too_many: Vec<String> = (0..=MAX_TARGETS)
            .map(|i| format!("https://n{i}.example"))
            .collect();
        assert_eq!(targets(json!(too_many)), invalid("targets"));
        // The same host and port twice, however it is written.
        assert_eq!(
            targets(json!(["https://a.example", "https://A.example:443/"])),
            invalid("targets")
        );
        assert!(targets(json!(["https://a.example", "https://a.example:8443"])).is_ok());
        // Each entry's problem names it.
        assert_eq!(
            targets(json!(["https://a.example", "http://b.example"])),
            invalid("targets[1].url")
        );
        assert_eq!(
            targets(json!(["https://a.example", { "url": "https://b.example/path" }])),
            invalid("targets[1].url")
        );
        assert_eq!(
            targets(json!([{ "url": "https://a.example", "feedPort": 0 }])),
            invalid("targets[0].feedPort")
        );
        assert_eq!(
            targets(json!([{ "url": "https://a.example", "feedPort": 70_000 }])),
            invalid("targets[0].feedPort")
        );
        assert_eq!(
            targets(json!([{ "url": "https://a.example", "port": 1 }])),
            Err(ConfigError::Unknown("targets[0].port".into()))
        );
        assert_eq!(
            targets(json!([{ "feedPort": 7768 }])),
            Err(ConfigError::Missing("targets[0].url".into()))
        );
        assert_eq!(targets(json!([7])), invalid("targets[0]"));
        assert_eq!(targets(json!([""])), invalid("targets[0].url"));
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
                v["xrp"].as_object_mut().unwrap().remove("socket");
            }),
            Err(ConfigError::Missing("xrp.socket".into()))
        );
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
    fn every_nodes_field_is_checked() {
        let invalid = |field: &str| Err(ConfigError::Invalid(field.into()));
        assert_eq!(
            with(|v| v["xrp"]["ledger"] = json!("XRPL Testnet")),
            invalid("xrp.ledger")
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
    fn the_two_nodes_are_two() {
        assert_eq!(
            with(|v| v["token"]["ledger"] = json!("xrpl-testnet-xrp+xrp")),
            Err(ConfigError::SameNode("ledger"))
        );
        assert_eq!(
            with(|v| v["token"]["socket"] = json!("/run/a.sock")),
            Err(ConfigError::SameNode("socket"))
        );
        assert_eq!(
            with(|v| v["token"]["passphraseFile"] = json!("/etc/a")),
            Err(ConfigError::SameNode("passphrase file"))
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
        let phrase = Phrase::new(Realm::Test, created, &PhraseSecret::from_bytes([3; 16]));
        std::fs::write(&path, format!("{}\n", phrase.text().as_str())).unwrap();
        let wallet = wallet_from_file(&path).unwrap();
        // As the node derives it from its key.
        let key = phrase.master_seed(Realm::Test).unwrap().node_key();
        let entropy = key.xrpl_account_entropy(TESTNET.id, 0);
        assert_eq!(
            wallet.account(),
            Wallet::from_entropy(entropy.as_bytes()).account()
        );
        std::fs::write(&path, "not a phrase").unwrap();
        let problem = wallet_from_file(&path).err().unwrap();
        assert!(!problem.contains("not a phrase"), "{problem}");
        std::fs::write(&path, "a".repeat(4_096)).unwrap();
        assert!(wallet_from_file(&path).is_err());
    }
}

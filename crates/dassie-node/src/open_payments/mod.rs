//! Open Payments around the engine (D9): the URLs of the node's wallet
//! addresses and resources, and which ledger a wallet address names.
//!
//! Each active ledger has a wallet address at `https://host/pay/<slug>`.
//! `$host`, that is `https://host/.well-known/pay`, names the default
//! ledger: the one the owner chose, or else the lowest active one. Every
//! URL is built from the configured hostname, so the node serves Open
//! Payments only once it has one.

pub mod auth;
pub mod keys;

use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};

use dassie_identity::NodeKey;
use dassie_ledger::LedgerId;
use dassie_node_core::rate_limit::{ClientKey as RateKey, RateLimitPolicy, RateLimiter};
use dassie_node_core::urls;

use crate::engine::ledger_slug;
use crate::node::Node;
use crate::store::{PaymentId, StoreError};

/// The path `$host` stands for.
pub const WELL_KNOWN_PATH: &str = dassie_payments::pointer::WELL_KNOWN_PATH;
/// The prefix of each ledger's wallet address path.
pub const PAY_PREFIX: &str = "/pay/";
/// The authorization server's grant endpoint.
pub const GRANT_PATH: &str = "/gnap";
/// The prefix of incoming payments' paths.
pub const INCOMING_PREFIX: &str = "/incoming-payments";

/// Every Open Payments request, per client address.
const REQUESTS: RateLimitPolicy = RateLimitPolicy {
    client_burst: 120,
    client_refill_ms: 250,
    client_slots: 1024,
};

/// Grant requests and new incoming payments, per client address: each
/// makes the node fetch a key set or keep a row.
const CREATIONS: RateLimitPolicy = RateLimitPolicy {
    client_burst: 20,
    client_refill_ms: 3_000,
    client_slots: 1024,
};

/// What Open Payments keeps beside the database: clients' key sets, the
/// node's own key, and the rate limits.
pub struct State {
    /// Clients' key sets.
    pub keys: Mutex<keys::KeyCache>,
    client_key: OnceLock<Result<keys::ClientKey, String>>,
    requests: Mutex<RateLimiter>,
    creations: Mutex<RateLimiter>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            keys: Mutex::new(keys::KeyCache::default()),
            client_key: OnceLock::new(),
            requests: Mutex::new(RateLimiter::new(REQUESTS)),
            creations: Mutex::new(RateLimiter::new(CREATIONS)),
        }
    }
}

impl State {
    /// The node's Open Payments key, derived on first use.
    pub fn client_key(&self, node_key: &NodeKey) -> Option<&keys::ClientKey> {
        self.client_key
            .get_or_init(|| keys::ClientKey::derive(node_key))
            .as_ref()
            .ok()
    }

    /// Charges a request from `address`; false once it is over its limit.
    pub fn allow_request(&self, address: IpAddr, now_ms: u64) -> bool {
        crate::node::lock(&self.requests)
            .check(RateKey::from_ip(address), now_ms)
            .is_ok()
    }

    /// Charges a grant request or a new incoming payment from `address`;
    /// false once it is over its limit.
    pub fn allow_creation(&self, address: IpAddr, now_ms: u64) -> bool {
        crate::node::lock(&self.creations)
            .check(RateKey::from_ip(address), now_ms)
            .is_ok()
    }
}

/// The node's public URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Urls {
    origin: String,
}

impl Urls {
    /// The URLs, once the node has a hostname.
    #[must_use]
    pub fn of(node: &Node) -> Option<Self> {
        let config = node.config();
        let origin = urls::node_url(config.hostname.as_ref(), node.ports().1)?;
        Some(Self { origin })
    }

    /// From an origin, `https://host[:port]`.
    #[must_use]
    pub fn from_origin(origin: &str) -> Self {
        Self {
            origin: origin.trim_end_matches('/').to_string(),
        }
    }

    /// `https://host[:port]`, which is also the resource server's URL.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// A wallet address path: the well-known one for the default ledger.
    #[must_use]
    pub fn wallet_address_path(ledger: LedgerId, is_default: bool) -> String {
        if is_default {
            WELL_KNOWN_PATH.to_string()
        } else {
            format!("{PAY_PREFIX}{}", ledger_slug(ledger))
        }
    }

    /// A wallet address.
    #[must_use]
    pub fn wallet_address(&self, ledger: LedgerId, is_default: bool) -> String {
        format!(
            "{}{}",
            self.origin,
            Self::wallet_address_path(ledger, is_default)
        )
    }

    /// A wallet address as a payment pointer.
    #[must_use]
    pub fn payment_pointer(&self, ledger: LedgerId, is_default: bool) -> String {
        let authority = self.origin.strip_prefix("https://").unwrap_or(&self.origin);
        if is_default {
            format!("${authority}")
        } else {
            format!("${authority}{PAY_PREFIX}{}", ledger_slug(ledger))
        }
    }

    /// The authorization server's grant endpoint.
    #[must_use]
    pub fn auth_server(&self) -> String {
        format!("{}{GRANT_PATH}", self.origin)
    }

    /// An incoming payment.
    #[must_use]
    pub fn incoming_payment(&self, id: PaymentId) -> String {
        format!("{}{INCOMING_PREFIX}/{id}", self.origin)
    }
}

/// The node's wallet addresses as of one moment: the active ledgers and
/// which is the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletAddresses {
    /// The active ledgers, in order.
    pub ledgers: Vec<LedgerId>,
    /// The ledger `$host` names, if any is active.
    pub default: Option<LedgerId>,
}

impl WalletAddresses {
    /// From the active ledgers and the stored choice of default.
    #[must_use]
    pub fn new(mut ledgers: Vec<LedgerId>, chosen: Option<LedgerId>) -> Self {
        ledgers.sort();
        let default = chosen
            .filter(|ledger| ledgers.contains(ledger))
            .or_else(|| ledgers.first().copied());
        Self { ledgers, default }
    }

    /// Reads them from the engine and the store.
    ///
    /// # Errors
    ///
    /// The engine is down, or the store failed.
    pub async fn of(node: &Node) -> Result<Self, AddressError> {
        let engine = node.engine().ok_or(AddressError::Unavailable)?;
        let views = engine
            .ledgers()
            .await
            .map_err(|_| AddressError::Unavailable)?;
        let chosen = node.store().default_ledger()?;
        Ok(Self::new(
            views.iter().map(|view| view.id).collect(),
            chosen,
        ))
    }

    /// Whether `ledger` is the default.
    #[must_use]
    pub fn is_default(&self, ledger: LedgerId) -> bool {
        self.default == Some(ledger)
    }

    /// The ledger a wallet address path names.
    #[must_use]
    pub fn ledger_of_path(&self, path: &str) -> Option<LedgerId> {
        if path == WELL_KNOWN_PATH {
            return self.default;
        }
        let slug = path.strip_prefix(PAY_PREFIX)?;
        self.ledgers
            .iter()
            .copied()
            .find(|&ledger| ledger_slug(ledger) == slug)
    }

    /// The ledger a wallet address URL of this node names.
    #[must_use]
    pub fn ledger_of_url(&self, urls: &Urls, url: &str) -> Option<LedgerId> {
        let path = url.strip_prefix(urls.origin())?;
        self.ledger_of_path(path)
    }
}

/// Why the wallet addresses are not known.
#[derive(Debug)]
pub enum AddressError {
    /// The engine is not running.
    Unavailable,
    /// The database failed.
    Store(StoreError),
}

impl From<StoreError> for AddressError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn names_ledgers_by_path() {
        let addresses =
            WalletAddresses::new(vec![LedgerId::XrplTestnetXrp, LedgerId::StubUsd], None);
        assert_eq!(addresses.default, Some(LedgerId::StubUsd));
        assert_eq!(
            addresses.ledger_of_path("/.well-known/pay"),
            Some(LedgerId::StubUsd)
        );
        assert_eq!(
            addresses.ledger_of_path("/pay/stub"),
            Some(LedgerId::StubUsd)
        );
        assert_eq!(
            addresses.ledger_of_path("/pay/xrpl-testnet"),
            Some(LedgerId::XrplTestnetXrp)
        );
        assert_eq!(addresses.ledger_of_path("/pay/xrpl"), None);
        assert_eq!(addresses.ledger_of_path("/pay/stub/"), None);
        let chosen = WalletAddresses::new(
            vec![LedgerId::StubUsd, LedgerId::XrplTestnetXrp],
            Some(LedgerId::XrplTestnetXrp),
        );
        assert_eq!(
            chosen.ledger_of_path(WELL_KNOWN_PATH),
            Some(LedgerId::XrplTestnetXrp)
        );
        let gone = WalletAddresses::new(vec![LedgerId::StubUsd], Some(LedgerId::XrplXrp));
        assert_eq!(gone.default, Some(LedgerId::StubUsd));
    }

    #[test]
    fn builds_urls_and_pointers() {
        let urls = Urls::from_origin("https://d1.localhost:5001");
        assert_eq!(
            urls.wallet_address(LedgerId::StubUsd, true),
            "https://d1.localhost:5001/.well-known/pay"
        );
        assert_eq!(
            urls.wallet_address(LedgerId::StubUsd, false),
            "https://d1.localhost:5001/pay/stub"
        );
        assert_eq!(
            urls.payment_pointer(LedgerId::StubUsd, true),
            "$d1.localhost:5001"
        );
        assert_eq!(
            urls.payment_pointer(LedgerId::XrplTestnetXrp, false),
            "$d1.localhost:5001/pay/xrpl-testnet"
        );
        assert_eq!(urls.auth_server(), "https://d1.localhost:5001/gnap");
        let addresses = WalletAddresses::new(vec![LedgerId::StubUsd], None);
        assert_eq!(
            addresses.ledger_of_url(&urls, "https://d1.localhost:5001/pay/stub"),
            Some(LedgerId::StubUsd)
        );
        assert_eq!(
            addresses.ledger_of_url(&urls, "https://d2.localhost:5002/pay/stub"),
            None
        );
    }
}

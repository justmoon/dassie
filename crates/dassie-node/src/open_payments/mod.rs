//! Open Payments around the engine (spec/decisions/open-payments.md): the
//! URLs of the node's wallet addresses and resources, and which ledger a
//! wallet address names.
//!
//! Each active ledger has a wallet address at `https://host/pay/<slug>`.
//! `$host`, that is `https://host/.well-known/pay`, names the default
//! ledger: the one the owner chose, or else the lowest active one.
//!
//! Every URL is built from one of the node's names ([`Names`]): a node
//! with a hostname has only that; a node without one answers to its
//! default front door name, the names its owner added, and the names it
//! holds at the default front door, each request under the name it came
//! through (spec/areas/payments.md#the-nodes-names).
//!
//! The original's Open Payments had no GNAP, served no incoming payment by
//! its ID, fixed the asset at USD/2, linked no money it received to a
//! payment, quoted 1:1, and counted outgoing payments twice (ILP-10). Here
//! it is served as its specs define it. SPSP stays out on purpose
//! (spec/decisions/open-payments.md), and each read still hands out fresh
//! credentials, now for a STREAM connection per payment
//! (spec/areas/payments.md#receiving).

pub mod auth;
pub mod client;
pub mod consent;
pub mod keys;
pub mod spending;

use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};

use dassie_identity::{NodeId, NodeKey};
use dassie_ledger::LedgerId;
use dassie_node_core::config::Hostname;
use dassie_node_core::origin::{self, Host};
use dassie_node_core::rate_limit::{ClientKey as RateKey, RateLimitPolicy, RateLimiter};
use dassie_node_core::urls::{self, DEFAULT_HTTPS_PORT};
use dassie_peer::names::Name;
use tokio::sync::watch;

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
/// The default front door's domain: a node without a hostname answers to
/// `<label>.dassiepay.net` first.
pub const DEFAULT_FRONT_DOOR_DOMAIN: &str = "dassiepay.net";

/// Every Open Payments request, per client address. A front door charges
/// its clients the same.
pub const REQUESTS: RateLimitPolicy = RateLimitPolicy {
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

/// Most of the owner's lookups of recipients at once: each may fetch from
/// other wallets as the node.
const MAX_LOOKUPS: usize = 4;

/// Most names the node holds that it answers to: more than a registrar
/// grants one node (`RegistrarConfig::max_names_per_node`, asserted in
/// the engine). A view that gives the node more answers to the first, by
/// name.
pub const MAX_HELD_NAMES: usize = 64;

/// Most front doors whose domains the node answers to its held names at,
/// the default one among them. A front door reaches only the nodes that
/// keep a connection to it, and a node keeps one relay, so a few leave
/// room for one the owner moves to.
pub const MAX_FRONT_DOORS: usize = 4;

/// What Open Payments keeps beside the database: the names the owner
/// added and those the node holds, clients' key sets, the node's own key,
/// its tokens at other wallets, and the rate limits.
pub struct State {
    /// The names the owner added, in their order.
    public_names: Vec<Hostname>,
    /// The names the node holds in its view, by name, as the engine last
    /// said (spec/areas/network.md#holders).
    held_names: watch::Sender<Arc<Vec<Name>>>,
    /// Clients' key sets.
    pub keys: Mutex<keys::KeyCache>,
    /// The node's own tokens at other wallets' authorization servers.
    pub tokens: client::Tokens,
    /// Permits for the owner's lookups of recipients; the rest wait.
    pub lookups: tokio::sync::Semaphore,
    client_key: OnceLock<Result<keys::ClientKey, String>>,
    requests: Mutex<RateLimiter>,
    creations: Mutex<RateLimiter>,
}

impl Default for State {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl State {
    /// With the names the owner added, which a node without a hostname
    /// answers to beside its default one.
    #[must_use]
    pub fn new(public_names: Vec<Hostname>) -> Self {
        Self {
            public_names,
            held_names: watch::Sender::new(Arc::new(Vec::new())),
            keys: Mutex::new(keys::KeyCache::default()),
            tokens: Mutex::new(client::TokenCache::default()),
            lookups: tokio::sync::Semaphore::new(MAX_LOOKUPS),
            client_key: OnceLock::new(),
            requests: Mutex::new(RateLimiter::new(REQUESTS)),
            creations: Mutex::new(RateLimiter::new(CREATIONS)),
        }
    }

    /// The names the owner added, in their order.
    #[must_use]
    pub fn public_names(&self) -> &[Hostname] {
        &self.public_names
    }

    /// The names the node holds in its view, by name: it answers to each
    /// at the front doors it is named at.
    #[must_use]
    pub fn held_names(&self) -> Arc<Vec<Name>> {
        Arc::clone(&self.held_names.borrow())
    }

    /// Says which names the node holds now, at most [`MAX_HELD_NAMES`];
    /// those who follow hear of it only if that changed.
    pub fn set_held_names(&self, mut names: Vec<Name>) {
        names.truncate(MAX_HELD_NAMES);
        self.held_names.send_if_modified(|held| {
            let changed = **held != names;
            if changed {
                *held = Arc::new(names);
            }
            changed
        });
    }

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

/// The names the node answers Open Payments to
/// (spec/areas/payments.md#the-nodes-names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Names {
    /// A node with a hostname answers under it alone, whatever `Host`
    /// says: its origin, `https://host[:port]`.
    Hostname(String),
    /// A node without one answers under each of these, the main one first:
    /// its default front door name, then the names its owner added, then
    /// `<name>.<domain>` for each name it holds, at each front door it is
    /// named at.
    List(Vec<Hostname>),
}

impl Names {
    /// The node's names, once it has a hostname or a key.
    #[must_use]
    pub fn of(node: &Node) -> Option<Self> {
        let config = node.config();
        let id = node.node_key().map(|key| key.node_id());
        let open_payments = node.open_payments();
        Self::new(
            config.hostname.as_ref(),
            node.ports().1,
            id.as_ref(),
            open_payments.public_names(),
            &open_payments.held_names(),
        )
    }

    /// The names of a node with `hostname`, or else with `id`, the names
    /// its owner `added` and the names it holds, `held`.
    ///
    /// The default front door name stays the main one whatever the node
    /// holds: other wallets know the node as a client by it, and a name
    /// can be lost or expire (spec/areas/payments.md#the-nodes-names).
    /// The node answers to the names it holds at the default front door's
    /// domain, and at the domain of each name its owner added that is the
    /// node's label there, `<label>.<domain>`: another front door, which
    /// passes a name on to its holder under that domain.
    #[must_use]
    pub fn new(
        hostname: Option<&Hostname>,
        https_port: u16,
        id: Option<&NodeId>,
        added: &[Hostname],
        held: &[Name],
    ) -> Option<Self> {
        if let Some(origin) = urls::node_url(hostname, https_port) {
            return Some(Self::Hostname(origin));
        }
        // Before setup, a node without a hostname is no one yet.
        let id = id?;
        let default = default_name(id);
        let mut names: Vec<Hostname> = default.into_iter().collect();
        let doors = front_doors(id, added);
        let held = doors.iter().flat_map(|domain| {
            held.iter()
                .take(MAX_HELD_NAMES)
                .filter_map(move |name| Hostname::parse(&format!("{name}.{domain}")).ok())
        });
        for name in added.iter().cloned().chain(held) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        (!names.is_empty()).then_some(Self::List(names))
    }

    /// Every origin of the node's, the main one first.
    #[must_use]
    pub fn origins(&self) -> Vec<String> {
        match self {
            Self::Hostname(origin) => vec![origin.clone()],
            Self::List(names) => names.iter().map(|name| format!("https://{name}")).collect(),
        }
    }

    /// The URLs under the main name: how the node names itself outside any
    /// request, to its owner and as a client of other wallets.
    #[must_use]
    pub fn main(&self) -> Urls {
        let own = self.origins();
        let origin = own.first().cloned().unwrap_or_default();
        Urls { origin, own }
    }

    /// The URLs to answer a request for `host` (its `Host` or authority)
    /// under: the node's hostname whatever `host` says, or else the name
    /// `host` names, on the default port. None for a name not on the list.
    #[must_use]
    pub fn answering(&self, host: Option<&str>) -> Option<Urls> {
        let Self::List(names) = self else {
            return Some(self.main());
        };
        let authority = origin::parse_authority(host?)?;
        if authority
            .port
            .is_some_and(|port| port != DEFAULT_HTTPS_PORT)
        {
            return None;
        }
        let Host::Name(asked) = authority.host else {
            return None;
        };
        let name = names.iter().find(|name| name.as_str() == asked)?;
        Some(Urls {
            origin: format!("https://{name}"),
            own: self.origins(),
        })
    }
}

/// `<label>.dassiepay.net`, for a node ID a key gives.
fn default_name(id: &NodeId) -> Option<Hostname> {
    let label = id.label().ok()?;
    Hostname::parse(&format!("{label}.{DEFAULT_FRONT_DOOR_DOMAIN}")).ok()
}

/// The domains of the front doors the node is named at: the default one,
/// then each of a name in `added` that is the node's label there, at most
/// [`MAX_FRONT_DOORS`].
fn front_doors(id: &NodeId, added: &[Hostname]) -> Vec<String> {
    let mut doors = vec![DEFAULT_FRONT_DOOR_DOMAIN.to_string()];
    let Ok(label) = id.label() else {
        return doors;
    };
    let label = label.to_string();
    for name in added {
        if doors.len() == MAX_FRONT_DOORS {
            break;
        }
        let domain = name
            .as_str()
            .strip_prefix(label.as_str())
            .and_then(|rest| rest.strip_prefix('.'));
        if let Some(domain) = domain
            && !doors.iter().any(|door| door == domain)
        {
            doors.push(domain.to_string());
        }
    }
    doors
}

/// The node's public URLs, under one of its names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Urls {
    origin: String,
    /// Every origin of the node's, `origin` among them.
    own: Vec<String>,
}

impl Urls {
    /// The URLs under the node's main name, once it has one.
    #[must_use]
    pub fn of(node: &Node) -> Option<Self> {
        Names::of(node).map(|names| names.main())
    }

    /// From an origin, `https://host[:port]`, the node's only one.
    #[must_use]
    pub fn from_origin(origin: &str) -> Self {
        let origin = origin.trim_end_matches('/').to_string();
        Self {
            own: vec![origin.clone()],
            origin,
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

    /// The path of `url` if it is at one of the node's origins, compared
    /// as origins are: whatever the case, and with or without the scheme's
    /// own port.
    #[must_use]
    pub fn own_path(&self, url: &str) -> Option<String> {
        use dassie_payments::uri::Uri;
        let uri = Uri::parse(url).ok()?;
        let origin = uri.origin();
        self.own
            .iter()
            .filter_map(|own| Uri::parse(own).ok())
            .any(|own| own.origin() == origin)
            .then(|| uri.path.to_string())
    }

    /// A URL the node wrote under any of its names, such as a wallet
    /// address a grant or a resource keeps, under this one: its path at
    /// this origin. Text that is no URL stays as it is.
    #[must_use]
    pub fn rebase(&self, url: &str) -> String {
        match dassie_payments::uri::Uri::parse(url) {
            Ok(uri) => format!("{}{}", self.origin, uri.path),
            Err(_) => url.to_string(),
        }
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

    /// The ledger a wallet address URL of this node names, under any of
    /// its names.
    #[must_use]
    pub fn ledger_of_url(&self, urls: &Urls, url: &str) -> Option<LedgerId> {
        urls.own
            .iter()
            .find_map(|origin| url.strip_prefix(origin.as_str()))
            .and_then(|path| self.ledger_of_path(path))
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

    fn hostname(text: &str) -> Hostname {
        Hostname::parse(text).unwrap()
    }

    /// The node of the label vector in `dassie-identity`, with two names
    /// its owner added, one of them twice.
    fn listed() -> (Names, String) {
        let id = NodeId::parse("dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW").unwrap();
        let default = "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq.dassiepay.net".to_string();
        let added = [
            hostname("pay.example.com"),
            hostname(&default),
            hostname("other.example"),
        ];
        (
            Names::new(None, 443, Some(&id), &added, &[]).unwrap(),
            default,
        )
    }

    #[test]
    fn a_node_with_a_hostname_answers_under_it_alone() {
        let id = NodeId::parse("dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW").unwrap();
        let names = Names::new(
            Some(&hostname("node.example")),
            8443,
            Some(&id),
            &[hostname("pay.example.com")],
            &[Name::parse("alice").unwrap()],
        )
        .unwrap();
        let origin = |urls: Option<Urls>| urls.map(|urls| urls.origin().to_string());
        let main = Some("https://node.example:8443".to_string());
        assert_eq!(origin(Some(names.main())), main);
        for host in [
            None,
            Some("pay.example.com"),
            Some("alice.dassiepay.net"),
            Some("elsewhere.example"),
        ] {
            assert_eq!(origin(names.answering(host)), main, "{host:?}");
        }
    }

    #[test]
    fn a_node_without_one_answers_under_each_of_its_names() {
        let (names, default) = listed();
        let Names::List(list) = &names else {
            panic!("{names:?}")
        };
        let list: Vec<&str> = list.iter().map(Hostname::as_str).collect();
        assert_eq!(list, [default.as_str(), "pay.example.com", "other.example"]);
        assert_eq!(names.main().origin(), format!("https://{default}"));
        let origin = |host: &str| {
            names
                .answering(Some(host))
                .map(|urls| urls.origin().to_string())
        };
        assert_eq!(
            origin("pay.example.com").as_deref(),
            Some("https://pay.example.com")
        );
        assert_eq!(
            origin("PAY.Example.com:443").as_deref(),
            Some("https://pay.example.com")
        );
        assert_eq!(
            origin(&default.to_ascii_uppercase()),
            Some(format!("https://{default}"))
        );
        for refused in [
            "elsewhere.example",
            "pay.example.com:8443",
            "pay.example.com.",
            "sub.pay.example.com",
            "192.0.2.1",
            "[::1]",
            "",
        ] {
            assert_eq!(origin(refused), None, "{refused}");
        }
        assert_eq!(names.answering(None), None);
        // Without a key or a hostname, a node has no names yet.
        assert_eq!(
            Names::new(None, 443, None, &[hostname("pay.example.com")], &[]),
            None
        );
    }

    // spec/areas/payments.md#the-nodes-names: a node answers to the names
    // it holds at the default front door, after the names its owner added,
    // and stops when it no longer holds them.
    #[test]
    fn a_node_answers_to_the_names_it_holds() {
        let id = NodeId::parse("dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW").unwrap();
        let default = "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq.dassiepay.net";
        let added = [hostname("pay.example.com"), hostname("bob.dassiepay.net")];
        let held = [Name::parse("alice").unwrap(), Name::parse("bob").unwrap()];
        let names = Names::new(None, 443, Some(&id), &added, &held).unwrap();
        assert_eq!(
            names.origins(),
            [
                format!("https://{default}"),
                "https://pay.example.com".to_string(),
                "https://bob.dassiepay.net".to_string(),
                "https://alice.dassiepay.net".to_string(),
            ]
        );
        // The default name stays the main one.
        assert_eq!(names.main().origin(), format!("https://{default}"));
        let origin = |names: &Names, host: &str| {
            names
                .answering(Some(host))
                .map(|urls| urls.origin().to_string())
        };
        assert_eq!(
            origin(&names, "Alice.dassiepay.net").as_deref(),
            Some("https://alice.dassiepay.net")
        );
        for refused in [
            "alice",
            "alice.dassiepay.net.example",
            "carol.dassiepay.net",
        ] {
            assert_eq!(origin(&names, refused), None, "{refused}");
        }
        let lost = Names::new(None, 443, Some(&id), &added, &held[1..]).unwrap();
        assert_eq!(origin(&lost, "alice.dassiepay.net"), None);
        // Only so many held names are answered to.
        let many: Vec<Name> = (0..MAX_HELD_NAMES + 8)
            .map(|index| Name::parse(&format!("name{index:03}")).unwrap())
            .collect();
        let Names::List(list) = Names::new(None, 443, Some(&id), &[], &many).unwrap() else {
            panic!("no list")
        };
        assert_eq!(list.len(), 1 + MAX_HELD_NAMES);
    }

    // spec/areas/payments.md#the-nodes-names: a front door on another
    // domain passes a name on to its holder under that domain, so a node
    // its owner names there answers to its names there too.
    #[test]
    fn a_node_answers_to_its_names_at_each_front_door_it_is_named_at() {
        let id = NodeId::parse("dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW").unwrap();
        let label = "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq";
        let added = [
            hostname("pay.example.com"),
            hostname(&format!("{label}.pay.example")),
            hostname(&format!("{label}.dassiepay.net")),
            hostname(&format!("sub.{label}.other.example")),
        ];
        let held = [Name::parse("alice").unwrap()];
        let names = Names::new(None, 443, Some(&id), &added, &held).unwrap();
        assert_eq!(
            names.origins(),
            [
                format!("https://{label}.dassiepay.net"),
                "https://pay.example.com".to_string(),
                format!("https://{label}.pay.example"),
                format!("https://sub.{label}.other.example"),
                "https://alice.dassiepay.net".to_string(),
                "https://alice.pay.example".to_string(),
            ]
        );
        let origin = |names: &Names, host: &str| {
            names
                .answering(Some(host))
                .map(|urls| urls.origin().to_string())
        };
        assert_eq!(
            origin(&names, "alice.pay.example").as_deref(),
            Some("https://alice.pay.example")
        );
        // Not at a domain of the owner's that does not name the node by
        // its label, and not once the node no longer holds the name.
        for refused in ["alice.example.com", "alice.other.example"] {
            assert_eq!(origin(&names, refused), None, "{refused}");
        }
        let lost = Names::new(None, 443, Some(&id), &added, &[]).unwrap();
        assert_eq!(origin(&lost, "alice.pay.example"), None);
        // Only so many front doors.
        let doors: Vec<Hostname> = (0..MAX_FRONT_DOORS + 2)
            .map(|index| hostname(&format!("{label}.door{index}.example")))
            .collect();
        let many = Names::new(None, 443, Some(&id), &doors, &held).unwrap();
        let answered = (0..MAX_FRONT_DOORS + 2)
            .filter(|index| origin(&many, &format!("alice.door{index}.example")).is_some())
            .count();
        assert_eq!(answered, MAX_FRONT_DOORS - 1);
    }

    #[test]
    fn held_names_change_only_when_they_do() {
        let state = State::default();
        let mut follower = state.held_names.subscribe();
        let alice = Name::parse("alice").unwrap();
        state.set_held_names(vec![alice]);
        assert!(follower.has_changed().unwrap());
        follower.mark_unchanged();
        state.set_held_names(vec![alice]);
        assert!(!follower.has_changed().unwrap());
        assert_eq!(*state.held_names(), [alice]);
        let many: Vec<Name> = (0..MAX_HELD_NAMES + 1)
            .map(|index| Name::parse(&format!("name{index:03}")).unwrap())
            .collect();
        state.set_held_names(many);
        assert_eq!(state.held_names().len(), MAX_HELD_NAMES);
    }

    #[test]
    fn a_node_knows_its_own_under_every_name() {
        let (names, default) = listed();
        let urls = names.answering(Some("pay.example.com")).unwrap();
        let addresses = WalletAddresses::new(vec![LedgerId::StubUsd], None);
        for url in [
            "https://pay.example.com/pay/stub",
            "https://other.example/pay/stub",
            &format!("https://{default}/.well-known/pay"),
        ] {
            assert_eq!(
                addresses.ledger_of_url(&urls, url),
                Some(LedgerId::StubUsd),
                "{url}"
            );
        }
        assert_eq!(
            addresses.ledger_of_url(&urls, "https://elsewhere.example/pay/stub"),
            None
        );
        assert_eq!(
            urls.own_path("https://OTHER.example:443/incoming-payments/x")
                .as_deref(),
            Some("/incoming-payments/x")
        );
        assert_eq!(
            urls.own_path("https://elsewhere.example/incoming-payments/x"),
            None
        );
        // What the node wrote under one name, it writes under this one.
        assert_eq!(
            urls.rebase(&format!("https://{default}/pay/stub")),
            "https://pay.example.com/pay/stub"
        );
        assert_eq!(urls.rebase("not a url"), "not a url");
    }

    mod properties {
        use proptest::prelude::*;

        use super::*;

        const ORIGINS: &[&str] = &[
            "https://node.example",
            "https://d1.localhost:5001",
            "https://[::1]:8443",
        ];

        const PATHS: &[&str] = &[
            WELL_KNOWN_PATH,
            "/pay/stub",
            "/pay/xrpl-testnet",
            "/pay/xrpl",
            "/pay/",
            "/pay/stub/",
            "/PAY/STUB",
            "/pay/stub?x#y",
            "/pay/%73tub",
            "/incoming-payments/x",
            "",
        ];

        /// A path: one of the node's, nearly, or any text.
        fn path() -> impl Strategy<Value = String> {
            prop_oneof![
                proptest::sample::select(PATHS).prop_map(String::from),
                (proptest::sample::select(PATHS), "\\PC{0,8}")
                    .prop_map(|(path, rest)| format!("{path}{rest}")),
                any::<String>(),
            ]
        }

        /// A URL: at this node's origin, written another way, at another
        /// origin, or any text.
        fn url(origin: &'static str) -> impl Strategy<Value = String> {
            let at = prop_oneof![
                Just(origin.to_string()),
                Just(origin.to_ascii_uppercase()),
                Just(format!("{origin}:443")),
                Just(format!("{origin}x")),
                Just(origin.replace("https", "http")),
                Just(String::new()),
                "[ -~]{0,16}",
            ];
            prop_oneof![
                4 => (at, path()).prop_map(|(at, path)| format!("{at}{path}")),
                1 => any::<String>(),
            ]
        }

        fn ledgers() -> impl Strategy<Value = (Vec<LedgerId>, Option<LedgerId>)> {
            (
                proptest::sample::subsequence(LedgerId::ALL.to_vec(), 0..=LedgerId::ALL.len()),
                proptest::option::of(proptest::sample::select(LedgerId::ALL.to_vec())),
            )
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

            // Paths and URLs come from clients' requests and grants: a
            // ledger is found only for one of the node's wallet addresses,
            // and anything else is none, without a panic
            // (spec/decisions/tigerstyle.md).
            #[test]
            fn any_path_names_an_active_ledger_or_none(path in path(), (ledgers, chosen) in ledgers()) {
                let addresses = WalletAddresses::new(ledgers, chosen);
                if let Some(ledger) = addresses.ledger_of_path(&path) {
                    prop_assert!(addresses.ledgers.contains(&ledger));
                    let is_default = path == WELL_KNOWN_PATH;
                    prop_assert!(!is_default || addresses.is_default(ledger));
                    prop_assert_eq!(path, Urls::wallet_address_path(ledger, is_default));
                }
            }

            #[test]
            fn any_url_names_an_own_ledger_or_none(
                origin in proptest::sample::select(ORIGINS),
                (ledgers, chosen) in ledgers(),
                url in proptest::sample::select(ORIGINS).prop_flat_map(url),
            ) {
                let urls = Urls::from_origin(origin);
                let addresses = WalletAddresses::new(ledgers, chosen);
                if let Some(ledger) = addresses.ledger_of_url(&urls, &url) {
                    let is_default = url.ends_with(WELL_KNOWN_PATH);
                    prop_assert_eq!(url, urls.wallet_address(ledger, is_default));
                }
            }

            #[test]
            fn any_url_is_own_or_not(origin in proptest::sample::select(ORIGINS), url in proptest::sample::select(ORIGINS).prop_flat_map(url)) {
                let urls = Urls::from_origin(origin);
                if let Some(path) = urls.own_path(&url) {
                    prop_assert!(url.to_ascii_lowercase().starts_with("https://"));
                    prop_assert!(url.contains(&path));
                }
                for ledger in LedgerId::ALL {
                    let own = urls.wallet_address(ledger, false);
                    prop_assert_eq!(urls.own_path(&own), Some(Urls::wallet_address_path(ledger, false)));
                }
            }
        }
    }
}

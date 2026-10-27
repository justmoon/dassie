//! Which hubs the owner has the node validate
//! (spec/areas/hubs.md#the-validator-in-the-node): a file named by
//! `DASSIE_VALIDATOR_CONFIG`, read and checked at start, so a bad one stops
//! the daemon with a message that names it.
//!
//! ```json
//! {
//!   "sponsors": ["<node ID>", ...],
//!   "hubs": [{ "rules": "<hex>", "genesisMs": 1800000000000, "xrpl": "wss://..." }],
//!   "xrpl": "wss://...",
//!   "network": "testnet",
//!   "publishers": [{ "key": "<hex>", "sites": ["https://..."] }]
//! }
//! ```
//!
//! Validating is something an owner turns on, and only for sponsors it
//! lists (spec/decisions/settlement-hubs.md): a hub whose sponsor is not
//! listed is refused, so nobody makes the node keep a hub it never wanted.
//! `rules` are the hub's genesis rules in canonical OER, as hex; every
//! validator starts the hub from the same `genesisMs`, the genesis block's
//! timestamp, which is part of its digest; `xrpl` is the XRPL server the
//! node follows the escrow on: the hub's own, else the file's, which hubs
//! founded through the node use too (spec/areas/hubs.md#founding-a-hub),
//! else the network's default one. `hubs` may be empty: a sponsor the file
//! lists founds hubs with the node over the hub's founding stream.
//! `network` is the XRP Ledger network of every hub the node validates, by
//! name, the testnet unless it says otherwise; a network of real money is
//! refused while the live realm is (spec/decisions/real-money.md).
//! `publishers` are the validator list publishers whose lists make the UNL
//! the node proves the escrows' ledgers under, as an XRPL account's config
//! names them (spec/designs/xrpl-validation.md#10-configuration-and-state):
//! by default the network's own, which only a test or a dev network
//! replaces.

use dassie_hub::Rules;
use dassie_identity::NodeId;
use dassie_ledger::Realm;
use dassie_node_core::config::LiveRealm;
use dassie_oer::Reader;
use dassie_xrpl::Network;
use dassie_xrpl::network::{NETWORKS, TESTNET};
use dassie_xrpl::validator_list::UnlConfig;
use serde::Deserialize;

/// The most hubs a node validates: its own limit
/// (spec/designs/settlement-hubs.md#2-founding-a-hub).
pub const MAX_VALIDATED_HUBS: usize = 4;

/// The most sponsors a node validates for.
pub const MAX_SPONSORS: usize = 64;

/// The largest configuration file read.
pub const MAX_CONFIG_BYTES: u64 = 256 * 1024;

/// The most bytes of an XRPL server's URL.
const MAX_URL_BYTES: usize = 2048;

/// The validator role's configuration.
#[derive(Clone, Debug)]
pub struct ValidatorConfig {
    /// The sponsors the node validates hubs for.
    pub sponsors: Vec<NodeId>,
    /// The hubs it validates, each of a listed sponsor.
    pub hubs: Vec<HubConfig>,
    /// The XRPL server it follows the escrows of hubs founded through it
    /// on, and of hubs that name none.
    pub xrpl: String,
    /// The XRP Ledger network every hub it validates is on: it accepts
    /// terms on no other.
    pub network: &'static Network,
    /// The validator list publishers whose lists make the UNL it proves
    /// that network's ledgers under.
    pub unl: UnlConfig,
}

/// One hub the node validates.
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// Its genesis rules, checked.
    pub rules: Rules,
    /// Its genesis block's timestamp.
    pub genesis_ms: u64,
    /// The XRPL server the node follows its escrow on: `wss://`, or `ws://`
    /// where private addresses are allowed.
    pub xrpl: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct File {
    sponsors: Vec<String>,
    hubs: Vec<HubEntry>,
    xrpl: Option<String>,
    network: Option<String>,
    publishers: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HubEntry {
    rules: String,
    genesis_ms: u64,
    xrpl: Option<String>,
}

/// Reads a configuration from the file's text, for a node that runs the
/// live realm as `live` says.
///
/// # Errors
///
/// What is wrong with it, for people.
pub fn parse(text: &str, live: LiveRealm) -> Result<ValidatorConfig, String> {
    let file: File = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let network = network(file.network.as_deref(), live)?;
    let unl = unl(file.publishers.as_ref(), network)?;
    if file.sponsors.len() > MAX_SPONSORS {
        return Err(format!("lists more than {MAX_SPONSORS} sponsors"));
    }
    let sponsors = file
        .sponsors
        .iter()
        .map(|id| NodeId::parse(id).map_err(|_| format!("names an invalid sponsor {id:?}")))
        .collect::<Result<Vec<NodeId>, String>>()?;
    if file.hubs.len() > MAX_VALIDATED_HUBS {
        return Err(format!("lists more than {MAX_VALIDATED_HUBS} hubs"));
    }
    let xrpl = server(file.xrpl.clone().unwrap_or_else(|| network.server.into()))?;
    let mut hubs: Vec<HubConfig> = Vec::with_capacity(file.hubs.len());
    for (index, entry) in file.hubs.iter().enumerate() {
        let hub = hub(entry, &sponsors, &xrpl, network)
            .map_err(|problem| format!("hub {index}: {problem}"))?;
        if hubs.iter().any(|other| other.rules.id() == hub.rules.id()) {
            return Err(format!("hub {index}: listed twice"));
        }
        hubs.push(hub);
    }
    Ok(ValidatorConfig {
        sponsors,
        hubs,
        xrpl,
        network,
        unl,
    })
}

/// The publishers `publishers` names, else `network`'s own.
fn unl(publishers: Option<&serde_json::Value>, network: &Network) -> Result<UnlConfig, String> {
    let own = UnlConfig::for_network(network.id);
    let Some(publishers) = publishers else {
        return own.ok_or_else(|| {
            format!(
                "names no validator list publishers, and the network {:?} has none built in",
                network.name
            )
        });
    };
    let base = own.unwrap_or(UnlConfig {
        publishers: Vec::new(),
    });
    base.with_override(&serde_json::json!({ "publishers": publishers }))
        .ok_or_else(|| "its validator list publishers are not ones the node follows".into())
}

/// The network `name` names, the testnet if none: only one of the live
/// realm where it runs (spec/decisions/mainnet-schemes.md), so no file
/// turns a node to validating hubs of real money.
fn network(name: Option<&str>, live: LiveRealm) -> Result<&'static Network, String> {
    let Some(name) = name else {
        return Ok(&TESTNET);
    };
    let network = NETWORKS
        .into_iter()
        .find(|network| network.name == name)
        .ok_or_else(|| {
            let known: Vec<&str> = NETWORKS.iter().map(|network| network.name).collect();
            format!(
                "names the network {name:?}, which is none of {}",
                known.join(", ")
            )
        })?;
    if network.realm == Realm::Live && !live.allowed() {
        return Err(format!(
            "names the network {name:?}, which holds real money: the live realm is not supported yet"
        ));
    }
    Ok(network)
}

/// `url`, if it is a WebSocket URL the node connects to.
fn server(url: String) -> Result<String, String> {
    let websocket = url.starts_with("wss://") || url.starts_with("ws://");
    if !websocket || url.len() > MAX_URL_BYTES {
        return Err(format!("its XRPL server {url:?} is no WebSocket URL"));
    }
    Ok(url)
}

fn hub(
    entry: &HubEntry,
    sponsors: &[NodeId],
    xrpl: &str,
    network: &Network,
) -> Result<HubConfig, String> {
    let bytes = hex(&entry.rules).ok_or("its rules are not hex")?;
    let mut reader = Reader::new(&bytes);
    let rules = Rules::decode(&mut reader).map_err(|error| format!("its rules: {error:?}"))?;
    reader
        .finish()
        .map_err(|_| "its rules have bytes after them")?;
    rules
        .check()
        .map_err(|error| format!("its rules do not check: {error:?}"))?;
    let sponsor = rules.terms.sponsor.node_id();
    if !sponsors.contains(&sponsor) {
        return Err(format!(
            "its sponsor {sponsor} is not one the node validates for (add it to \"sponsors\")"
        ));
    }
    if rules.terms.network != network.id {
        return Err(format!(
            "its rules are on network {}, not the file's {:?}",
            rules.terms.network, network.name
        ));
    }
    if entry.genesis_ms == 0 {
        return Err("its genesisMs is 0".into());
    }
    let xrpl = server(entry.xrpl.clone().unwrap_or_else(|| xrpl.into()))?;
    Ok(HubConfig {
        rules,
        genesis_ms: entry.genesis_ms,
        xrpl,
    })
}

/// Bytes from hex, either case, whitespace around it ignored.
fn hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim().as_bytes();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_chunks::<2>()
        .0
        .iter()
        .map(|[high, low]| {
            let digit = |byte: u8| char::from(byte).to_digit(16);
            u8::try_from(digit(*high)? * 16 + digit(*low)?).ok()
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use dassie_hub::escrow::{ValidatorKeys, escrow_master};
    use dassie_hub::ledger::TESTNET_NETWORK;
    use dassie_hub::rules::{Acceptance, Fees, Limits, Terms, Timers};
    use dassie_identity::NodeKey;
    use dassie_oer::Encode;

    use super::*;

    /// The nonce of the hubs tests make.
    pub(crate) const NONCE: [u8; 32] = [7; 32];

    /// Terms that check, sponsored by `sponsor`, on the testnet.
    pub(crate) fn terms(sponsor: &NodeKey) -> Terms {
        terms_on(sponsor, TESTNET_NETWORK)
    }

    /// The same on `network`.
    fn terms_on(sponsor: &NodeKey, network: u32) -> Terms {
        Terms {
            version: dassie_hub::rules::HUB_VERSION,
            nonce: NONCE,
            sponsor: sponsor.public_key(),
            network,
            escrow: escrow_master(sponsor, network, &NONCE).account().0,
            asset: dassie_hub::rules::Asset::Xrp,
            fees: Fees {
                operation: 10,
                ledger: 100,
                gas: 0,
                reserve_share_ppm: 100_000,
                account_reserve: 2_000_000,
                hold_reserve: 200_000,
                starting_balance: 1_000,
                least_payout: 1_000_000,
            },
            limits: Limits {
                accounts: 100,
                holds: 100,
                holds_per_account: 8,
                withdrawals: 16,
                pending_deposits: 64,
                recent_deposits: 256,
                block_operations: 64,
                deadline_operations: 16,
                claimed_per_hold: 1_100,
                commitments_per_claim: 16,
                vote_window: 32,
            },
            timers: Timers {
                hold_notice: 60_000,
                shutdown_notice: 60_000,
                reconfigure_notice: 120_000,
                absence: 86_400_000,
                stall: 3_600_000,
                margin: 600,
                allowance: 300,
            },
        }
    }

    /// The rules of a hub of `nodes`, each at its endpoint.
    pub(crate) fn rules(sponsor: &NodeKey, nodes: &[(NodeKey, String)]) -> Rules {
        rules_on(sponsor, nodes, TESTNET_NETWORK)
    }

    /// The same on `network`.
    fn rules_on(sponsor: &NodeKey, nodes: &[(NodeKey, String)], network: u32) -> Rules {
        let terms = terms_on(sponsor, network);
        let validators: Vec<Acceptance> = nodes
            .iter()
            .map(|(node, endpoint)| {
                ValidatorKeys::derive(node, network, &NONCE)
                    .unwrap()
                    .accept(node, &terms, endpoint.clone())
                    .unwrap()
            })
            .collect();
        Rules { terms, validators }
    }

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn file(sponsors: &[String], rules: &Rules) -> String {
        serde_json::json!({
            "sponsors": sponsors,
            "hubs": [{ "rules": hex_of(&rules.to_vec()), "genesisMs": 1_800_000_000_000_u64 }],
        })
        .to_string()
    }

    /// The same, naming `network`.
    fn file_on(sponsors: &[String], rules: &Rules, network: &str) -> String {
        serde_json::json!({
            "sponsors": sponsors,
            "hubs": [{ "rules": hex_of(&rules.to_vec()), "genesisMs": 1_800_000_000_000_u64 }],
            "network": network,
        })
        .to_string()
    }

    fn nodes() -> Vec<(NodeKey, String)> {
        (1..=4)
            .map(|i| (NodeKey::from_bytes(&[i; 32]), format!("v{i}.example:443")))
            .collect()
    }

    #[test]
    fn a_hub_of_a_listed_sponsor_is_taken() {
        let refused = LiveRealm::Refused;
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let rules = rules(&sponsor, &nodes());
        let listed = vec![sponsor.node_id().as_str().to_string()];
        let config = parse(&file(&listed, &rules), refused).unwrap();
        assert_eq!(config.hubs.len(), 1);
        assert_eq!(config.hubs[0].rules, rules);
        assert_eq!(config.hubs[0].xrpl, dassie_xrpl::account::DEFAULT_SERVER);
        assert_eq!(config.network, &TESTNET);

        // Validating is opt-in, per sponsor.
        let error = parse(&file(&[], &rules), refused).unwrap_err();
        assert!(
            error.contains("is not one the node validates for"),
            "{error}"
        );
        let mut broken = rules.clone();
        broken.validators[1].endpoint = "evil.example:443".into();
        let error = parse(&file(&listed, &broken), refused).unwrap_err();
        assert!(error.contains("do not check"), "{error}");
        assert!(parse("{\"sponsors\": [], \"hubs\": [], \"extra\": 1}", refused).is_err());
        let bad_rules = r#"{"sponsors": [], "hubs": [{"rules": "zz", "genesisMs": 1}]}"#;
        assert!(parse(bad_rules, refused).is_err());
        // Its keys are camelCase, like the environment's JSON.
        let snake = file(&listed, &rules).replace("genesisMs", "genesis_ms");
        let error = parse(&snake, refused).unwrap_err();
        assert!(error.contains("genesis_ms"), "{error}");
    }

    // The network is the testnet unless the file names another; one of
    // real money only where the live realm runs, and then only its hubs.
    #[test]
    fn the_network_is_the_files() {
        use dassie_xrpl::network::MAINNET;

        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let listed = vec![sponsor.node_id().as_str().to_string()];
        let testnet = rules(&sponsor, &nodes());
        let mainnet = rules_on(&sponsor, &nodes(), MAINNET.id);
        let config = parse(&file_on(&listed, &testnet, "testnet"), LiveRealm::Refused).unwrap();
        assert_eq!(config.network, &TESTNET);

        // Mainnet stays refused while the live realm is.
        let error = parse(&file_on(&listed, &mainnet, "mainnet"), LiveRealm::Refused).unwrap_err();
        assert!(error.contains("real money"), "{error}");
        let error = parse(&file_on(&listed, &mainnet, "devnet"), LiveRealm::Refused).unwrap_err();
        assert!(error.contains("none of testnet, mainnet"), "{error}");

        // A hub on another network than the file's is refused.
        let error = parse(&file(&listed, &mainnet), LiveRealm::Refused).unwrap_err();
        assert!(error.contains("not the file's \"testnet\""), "{error}");

        if LiveRealm::DevSwitch.allowed() {
            let live = LiveRealm::DevSwitch;
            let config = parse(&file_on(&listed, &mainnet, "mainnet"), live).unwrap();
            assert_eq!(config.network, &MAINNET);
            assert_eq!(config.xrpl, MAINNET.server);
            assert_eq!(config.hubs[0].xrpl, MAINNET.server);
            let error = parse(&file_on(&listed, &testnet, "mainnet"), live).unwrap_err();
            assert!(error.contains("not the file's \"mainnet\""), "{error}");
        }
    }

    // The validators' UNL is the network's, unless the file names other
    // publishers, as a dev network's fake ledger has its own.
    #[test]
    fn the_publishers_are_the_networks_unless_named() {
        let refused = LiveRealm::Refused;
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let rules = rules(&sponsor, &nodes());
        let listed = vec![sponsor.node_id().as_str().to_string()];
        let config = parse(&file(&listed, &rules), refused).unwrap();
        assert_eq!(config.unl, UnlConfig::testnet());

        let fake = dassie_xrpl::fake::FakeLedger::new();
        let publishers = fake.publishers(dassie_xrpl::fake::LIST_SITE);
        let mut named: serde_json::Value = serde_json::from_str(&file(&listed, &rules)).unwrap();
        named["publishers"] = publishers["publishers"].clone();
        let config = parse(&named.to_string(), refused).unwrap();
        assert_eq!(
            config.unl,
            UnlConfig::testnet().with_override(&publishers).unwrap()
        );
        assert_ne!(config.unl, UnlConfig::testnet());

        named["publishers"] = serde_json::json!([]);
        let error = parse(&named.to_string(), refused).unwrap_err();
        assert!(error.contains("publishers"), "{error}");
    }

    #[test]
    fn hex_reads_both_cases() {
        assert_eq!(hex(" 0aFf "), Some(vec![0x0a, 0xff]));
        assert_eq!(hex("abc"), None);
        assert_eq!(hex("gg"), None);
    }
}

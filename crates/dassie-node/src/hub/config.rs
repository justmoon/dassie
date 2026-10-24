//! Which hubs the owner has the node validate
//! (spec/areas/hubs.md#the-validator-in-the-node): a file named by
//! `DASSIE_VALIDATOR_CONFIG`, read and checked at start, so a bad one stops
//! the daemon with a message that names it.
//!
//! ```json
//! {
//!   "sponsors": ["<node ID>", ...],
//!   "hubs": [{ "rules": "<hex>", "genesis_ms": 1800000000000, "xrpl": "wss://..." }],
//!   "xrpl": "wss://..."
//! }
//! ```
//!
//! Validating is something an owner turns on, and only for sponsors it
//! lists (spec/decisions/settlement-hubs.md): a hub whose sponsor is not
//! listed is refused, so nobody makes the node keep a hub it never wanted.
//! `rules` are the hub's genesis rules in canonical OER, as hex; every
//! validator starts the hub from the same `genesis_ms`, the genesis block's
//! timestamp, which is part of its digest; `xrpl` is the XRPL server the
//! node follows the escrow on: the hub's own, else the file's, which hubs
//! founded through the node use too (spec/areas/hubs.md#founding-a-hub),
//! else the testnet's public one. `hubs` may be empty: a sponsor the file
//! lists founds hubs with the node over the hub's founding stream.

use dassie_hub::Rules;
use dassie_identity::NodeId;
use dassie_oer::Reader;
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
#[serde(deny_unknown_fields)]
struct File {
    sponsors: Vec<String>,
    hubs: Vec<HubEntry>,
    xrpl: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HubEntry {
    rules: String,
    genesis_ms: u64,
    xrpl: Option<String>,
}

/// Reads a configuration from the file's text.
///
/// # Errors
///
/// What is wrong with it, for people.
pub fn parse(text: &str) -> Result<ValidatorConfig, String> {
    let file: File = serde_json::from_str(text).map_err(|error| error.to_string())?;
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
    let xrpl = server(
        file.xrpl
            .clone()
            .unwrap_or_else(|| dassie_xrpl::account::DEFAULT_SERVER.into()),
    )?;
    let mut hubs: Vec<HubConfig> = Vec::with_capacity(file.hubs.len());
    for (index, entry) in file.hubs.iter().enumerate() {
        let hub =
            hub(entry, &sponsors, &xrpl).map_err(|problem| format!("hub {index}: {problem}"))?;
        if hubs.iter().any(|other| other.rules.id() == hub.rules.id()) {
            return Err(format!("hub {index}: listed twice"));
        }
        hubs.push(hub);
    }
    Ok(ValidatorConfig {
        sponsors,
        hubs,
        xrpl,
    })
}

/// `url`, if it is a WebSocket URL the node connects to.
fn server(url: String) -> Result<String, String> {
    let websocket = url.starts_with("wss://") || url.starts_with("ws://");
    if !websocket || url.len() > MAX_URL_BYTES {
        return Err(format!("its XRPL server {url:?} is no WebSocket URL"));
    }
    Ok(url)
}

fn hub(entry: &HubEntry, sponsors: &[NodeId], xrpl: &str) -> Result<HubConfig, String> {
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
    if entry.genesis_ms == 0 {
        return Err("its genesis_ms is 0".into());
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
    use dassie_hub::rules::{Acceptance, Fees, Limits, Terms, Timers};
    use dassie_identity::NodeKey;
    use dassie_oer::Encode;

    use super::*;

    /// The nonce of the hubs tests make.
    pub(crate) const NONCE: [u8; 32] = [7; 32];

    /// Terms that check, sponsored by `sponsor`, on the testnet.
    pub(crate) fn terms(sponsor: &NodeKey) -> Terms {
        Terms {
            nonce: NONCE,
            sponsor: sponsor.public_key(),
            network: 1,
            escrow: escrow_master(sponsor, &NONCE).unwrap().account().0,
            fees: Fees {
                operation: 10,
                ledger: 100,
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
                precision: 100,
                margin: 500,
                allowance: 300,
                view_timeout: 200,
            },
        }
    }

    /// The rules of a hub of `nodes`, each at its endpoint.
    pub(crate) fn rules(sponsor: &NodeKey, nodes: &[(NodeKey, String)]) -> Rules {
        let terms = terms(sponsor);
        let validators: Vec<Acceptance> = nodes
            .iter()
            .map(|(node, endpoint)| {
                ValidatorKeys::derive(node, &NONCE)
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
            "hubs": [{ "rules": hex_of(&rules.to_vec()), "genesis_ms": 1_800_000_000_000_u64 }],
        })
        .to_string()
    }

    #[test]
    fn a_hub_of_a_listed_sponsor_is_taken() {
        let sponsor = NodeKey::from_bytes(&[100; 32]);
        let nodes: Vec<(NodeKey, String)> = (1..=4)
            .map(|i| (NodeKey::from_bytes(&[i; 32]), format!("v{i}.example:443")))
            .collect();
        let rules = rules(&sponsor, &nodes);
        let listed = vec![sponsor.node_id().as_str().to_string()];
        let config = parse(&file(&listed, &rules)).unwrap();
        assert_eq!(config.hubs.len(), 1);
        assert_eq!(config.hubs[0].rules, rules);
        assert_eq!(config.hubs[0].xrpl, dassie_xrpl::account::DEFAULT_SERVER);

        // Validating is opt-in, per sponsor.
        let error = parse(&file(&[], &rules)).unwrap_err();
        assert!(
            error.contains("is not one the node validates for"),
            "{error}"
        );
        let mut broken = rules.clone();
        broken.validators[1].endpoint = "evil.example:443".into();
        let error = parse(&file(&listed, &broken)).unwrap_err();
        assert!(error.contains("do not check"), "{error}");
        assert!(parse("{\"sponsors\": [], \"hubs\": [], \"extra\": 1}").is_err());
        assert!(parse(r#"{"sponsors": [], "hubs": [{"rules": "zz", "genesis_ms": 1}]}"#).is_err());
    }

    #[test]
    fn hex_reads_both_cases() {
        assert_eq!(hex(" 0aFf "), Some(vec![0x0a, 0xff]));
        assert_eq!(hex("abc"), None);
        assert_eq!(hex("gg"), None);
    }
}

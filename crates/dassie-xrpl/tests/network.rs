//! Each network's account against the fake ledger: the network it is on
//! decides the server it connects to, the network ID its server must
//! report, and whether it asks a faucet (spec/decisions/mainnet-schemes.md).

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use dassie_settlement::{AssetEffect, ExternalAsset, LedgerInput, LedgerStatus, SchemeError};
use dassie_xrpl::account::{self, XrplAccount};
use dassie_xrpl::fake::FakeLedger;
use dassie_xrpl::network::{MAINNET, NETWORKS, Network, TESTNET};
use dassie_xrpl::{XrplMainnetScheme, XrplScheme};

/// Most rounds of requests and answers a connection takes here.
const MAX_ROUNDS: usize = 64;

/// An account on `network` with the config `config`, which only the
/// network's XRP scheme may settle through, trusting the validators of the
/// network's fake ledger.
fn account_on(network: &Network, config: &str) -> Result<XrplAccount, SchemeError> {
    XrplAccount::new(
        network.first(),
        &[network.xrp_scheme],
        &dassie_xrpl::fake::with_publishers(config, network),
        &[7; 32],
        &[],
    )
}

/// What a connection did: the commands the account sent, in order, and
/// the faucets it asked.
struct Connection {
    asked: Vec<String>,
    funds: Vec<String>,
}

/// Connects `account` to `ledger` and answers what it asks until it asks
/// nothing more.
fn connect(account: &mut XrplAccount, ledger: &mut FakeLedger) -> Connection {
    let mut connection = Connection {
        asked: Vec::new(),
        funds: Vec::new(),
    };
    account.on_ledger(ledger.now_ms(), LedgerInput::Connected);
    answer(account, ledger, &mut connection);
    connection
}

/// Answers what `account` asks until it asks nothing more.
fn answer(account: &mut XrplAccount, ledger: &mut FakeLedger, connection: &mut Connection) {
    for _ in 0..MAX_ROUNDS {
        account.told();
        account.check_invariants();
        let mut effects = Vec::new();
        account.take_effects(&mut effects);
        if effects.is_empty() {
            return;
        }
        for effect in effects {
            match effect {
                AssetEffect::Send { message } => {
                    let request: serde_json::Value = serde_json::from_slice(&message).unwrap();
                    let command = request["command"].as_str().unwrap_or_default();
                    connection.asked.push(command.to_string());
                    let reply = ledger.request(1, &message);
                    account.on_ledger(ledger.now_ms(), LedgerInput::Message(&reply));
                }
                AssetEffect::Fund { url, .. } => connection.funds.push(url),
                AssetEffect::Fetch { url, .. } => {
                    let body = ledger.fetch(&url);
                    let fetched = LedgerInput::Fetched {
                        url: &url,
                        body: body.as_deref(),
                    };
                    account.on_ledger(ledger.now_ms(), fetched);
                }
                other => panic!("{other:?}"),
            }
        }
    }
    panic!("the connection did not settle in {MAX_ROUNDS} rounds");
}

// The account connects to its network's server by default, and runs only
// where the server reports its network: a server of the other network, or
// one that names none, stops it before the account is read, with the
// reason naming the network it is on.
#[test]
fn an_account_runs_only_on_its_own_network() {
    for network in NETWORKS {
        let reports = |id: u32| network.id == id;
        for reported in [Some(TESTNET.id), Some(MAINNET.id), Some(2), None] {
            let mut ledger = fake_of(network);
            ledger.set_network_id(reported);
            let mut account = account_on(network, "").unwrap();
            assert_eq!(account.connection(), Some(network.server));
            let connection = connect(&mut account, &mut ledger);
            if reported.is_some_and(reports) {
                assert_eq!(
                    connection.asked[..4],
                    ["server_state", "subscribe", "state_proof", "validations"]
                );
                assert_eq!(account.status(), LedgerStatus::Unfunded, "{reported:?}");
            } else {
                assert_eq!(connection.asked, ["server_state"], "{reported:?}");
                assert_eq!(
                    account.status(),
                    LedgerStatus::Failed(network.wrong_network),
                    "{reported:?}"
                );
            }
        }
    }
}

/// The fake ledger of `network`.
fn fake_of(network: &Network) -> FakeLedger {
    let ledger = if *network == MAINNET {
        FakeLedger::mainnet()
    } else {
        FakeLedger::new()
    };
    assert_eq!(ledger.network(), network);
    ledger
}

// Mainnet has no faucet: an account there without funds waits for the
// owner's deposit and never asks one, and the owner cannot ask one either.
// The testnet's asks its faucet, as it always has. Either syncs once its
// owner deposits, on its network's fake ledger.
#[test]
fn only_the_testnet_account_asks_a_faucet() {
    for network in NETWORKS {
        let mut ledger = fake_of(network);
        let mut account = account_on(network, "").unwrap();
        let mut connection = connect(&mut account, &mut ledger);
        assert_eq!(account.status(), LedgerStatus::Unfunded);
        let faucet: Vec<String> = network.faucet.iter().map(ToString::to_string).collect();
        assert_eq!(connection.funds, faucet);
        assert_eq!(account.has_faucet(), network.faucet.is_some());
        if network.faucet.is_none() {
            assert_eq!(account.ask_faucet(), Err(SchemeError::Unsupported));
        }
        ledger.deposit(account.account(), 50_000_000);
        for (_, message) in ledger.close() {
            account.on_ledger(ledger.now_ms(), LedgerInput::Message(&message));
            answer(&mut account, &mut ledger, &mut connection);
        }
        assert_eq!(account.status(), LedgerStatus::Synced, "{}", network.name);
    }
}

// The fake's mainnet reports mainnet's network ID, has no faucet and never
// resets; its testnet reports the testnet's.
#[test]
#[should_panic(expected = "only the testnet resets")]
fn the_mainnet_fake_never_resets() {
    let mut testnet = FakeLedger::new();
    testnet.reset();
    FakeLedger::mainnet().reset();
}

#[test]
#[should_panic(expected = "mainnet has no faucet")]
fn the_mainnet_fake_has_no_faucet() {
    let account = account_on(&MAINNET, "").unwrap().account();
    FakeLedger::new().fund(account);
    FakeLedger::mainnet().fund(account);
}

// A network without a faucet takes none in the config but `null`; one
// with a faucet takes another. A server may be named on either.
#[test]
fn a_config_names_a_faucet_only_where_there_is_one() {
    let faucet = r#"{"faucet": "https://faucet.example/accounts"}"#;
    assert!(account_on(&TESTNET, faucet).is_ok());
    assert_eq!(
        account_on(&MAINNET, faucet).err(),
        Some(SchemeError::InvalidConfig)
    );
    for network in NETWORKS {
        let config = account_on(network, r#"{"faucet": null}"#).unwrap();
        assert_eq!(config.config().faucet, None);
        let server = r#"{"server": "wss://xrpl.example"}"#;
        let named = account_on(network, server).unwrap();
        assert_eq!(named.config().server, "wss://xrpl.example");
    }
    let config = account::Config::parse("").unwrap();
    assert_eq!(config, account::Config::parse_on("", &TESTNET).unwrap());
    assert_eq!(config.server, account::DEFAULT_SERVER);
    assert_eq!(config.faucet.as_deref(), Some(account::DEFAULT_FAUCET));
}

// spec/designs/xrpl-validation.md#10-configuration-and-state: each network
// trusts its own list publishers unless the owner names others, which
// replace them entirely. What is not a publisher an account can follow,
// and a floor, are refused.
#[test]
fn a_config_names_the_validator_list_publishers() {
    use dassie_xrpl::validator_list::UnlConfig;

    for network in [&TESTNET, &MAINNET] {
        let config = account::Config::parse_on("", network).unwrap();
        assert_eq!(config.unl, UnlConfig::for_network(network.id).unwrap());
        let key = dassie_xrpl::fake::publisher_key(dassie_xrpl::fake::DEFAULT_SEED);
        let own = serde_json::json!({
            "publishers": [{
                "key": key.to_node_public(),
                "sites": ["https://lists.example/vl"],
            }],
        });
        let config = account::Config::parse_on(&own.to_string(), network).unwrap();
        assert_eq!(config.unl.publishers.len(), 1);
        assert_eq!(config.unl.publishers[0].master_key, key);
        let mut floored = own.clone();
        floored["floor"] = 3.into();
        for bad in [
            floored,
            serde_json::json!({ "floor": 3 }),
            serde_json::json!({ "publishers": [] }),
            serde_json::json!({ "publishers": [{ "key": "ED00", "sites": ["https://a.example"] }] }),
        ] {
            assert_eq!(
                account::Config::parse_on(&bad.to_string(), network),
                Err(SchemeError::InvalidConfig),
                "{bad}"
            );
        }
    }
}

// A scheme settles only through an account on its own network: the
// testnet's XRP scheme refuses the mainnet's account.
#[test]
fn a_scheme_refuses_an_account_of_another_network() {
    let mainnet = account_on(&MAINNET, "").unwrap();
    assert_eq!(
        XrplScheme::new(&mainnet, None, &[]).err(),
        Some(SchemeError::InvalidConfig)
    );
    assert_eq!(
        dassie_xrpl::DsusdScheme::new(&mainnet, None, &[]).err(),
        Some(SchemeError::InvalidConfig)
    );
    let testnet = account_on(&TESTNET, "").unwrap();
    assert!(XrplScheme::new(&testnet, None, &[]).is_ok());
    // Mainnet's XRP scheme the other way round.
    assert_eq!(
        XrplMainnetScheme::new(&testnet, None, &[]).err(),
        Some(SchemeError::InvalidConfig)
    );
    assert!(XrplMainnetScheme::new(&mainnet, None, &[]).is_ok());
    // And mainnet's RLUSD scheme.
    assert_eq!(
        dassie_xrpl::RlusdScheme::new(&testnet, None, &[]).err(),
        Some(SchemeError::InvalidConfig)
    );
    assert!(dassie_xrpl::RlusdScheme::new(&mainnet, None, &[]).is_ok());
    // An asset of no XRPL network is no account.
    let hub = dassie_settlement::AssetId::sole(dassie_settlement::AssetKind::HubXrplTestnetXrp);
    assert_eq!(
        XrplAccount::new(hub, &[], "", &[7; 32], &[]).err(),
        Some(SchemeError::UnknownScheme)
    );
}

/// The requests `account` sends, in order, once told what it learnt.
fn sent(account: &mut XrplAccount) -> Vec<serde_json::Value> {
    account.told();
    account.check_invariants();
    let mut effects = Vec::new();
    account.take_effects(&mut effects);
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            AssetEffect::Send { message } => Some(serde_json::from_slice(&message).unwrap()),
            // Its validator lists, fetched meanwhile.
            AssetEffect::Fetch { .. } => None,
            other => panic!("{other:?}"),
        })
        .collect()
}

// The table's network IDs are the ones live servers report: `server_state`
// as mainnet's public servers and the testnet's, and each network's proof
// server, its default, answered it (tests/data/server-state.json). Each network's account goes
// past the check on its own network's servers, and stops on the other's
// before it reads the account.
#[test]
fn live_servers_report_the_tables_network_ids() {
    let text = std::fs::read_to_string("tests/data/server-state.json").unwrap();
    let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
    let servers = fixture["servers"].as_object().unwrap();
    assert_eq!(servers.len(), 6);
    assert!(servers.contains_key(MAINNET.server) && servers.contains_key(TESTNET.server));
    for (server, reply) in servers {
        let on = if server.contains("altnet") || server.contains("testnet") {
            &TESTNET
        } else {
            &MAINNET
        };
        let state = dassie_xrpl::api::server_state_result(&reply["result"]).unwrap();
        assert_eq!(state.network_id, Some(on.id), "{server}");
        for network in NETWORKS {
            let mut account = account_on(network, "").unwrap();
            account.on_ledger(1_000, LedgerInput::Connected);
            let requests = sent(&mut account);
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["command"], "server_state");
            let mut answer = reply.clone();
            answer["id"] = requests[0]["id"].clone();
            let answer = serde_json::to_vec(&answer).unwrap();
            account.on_ledger(1_000, LedgerInput::Message(&answer));
            let requests = sent(&mut account);
            if network == on {
                assert_eq!(requests[0]["command"], "subscribe", "{server}");
                assert!(!matches!(account.status(), LedgerStatus::Failed(_)));
            } else {
                assert!(requests.is_empty(), "{server}");
                assert_eq!(
                    account.status(),
                    LedgerStatus::Failed(network.wrong_network),
                    "{server}"
                );
            }
        }
    }
}

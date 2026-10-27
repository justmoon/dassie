//! Founding a settlement hub between daemons on loopback, over real QUIC
//! (spec/areas/hubs.md#founding-a-hub, spec/areas/hubs.md#payout-bundles):
//! four validator daemons list the sponsor in their validator file; the
//! sponsor's owner founds a hub with them through the RPC; they accept,
//! sign its genesis and run it once the certificate reaches them; the
//! sponsor funds and sets up the escrow on the fake XRP Ledger from its own
//! account; the validators' watchers check it and attest it; and once the
//! hub stalls, a member fetches its validators' payout bundles and checks
//! them. A node of the live realm, by the dev switch, founds hubs on the
//! fake ledger's mainnet, funded from its mainnet XRP.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::clock::ManualClock;
use dassie_node::fake_ledger::{Options, Server};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_xrpl::network::{MAINNET, Network, TESTNET};
use support::pair::{CA, CERT, KEY};
use support::{Bound, START_MS, TestNode, post_json, send, set_up, setup_body_in};

mod support;

/// Sets `node` up with `key` in the realm of `network`, with a hostname
/// and a certificate: what a validator needs to name its endpoint.
async fn prepare(node: &TestNode, key: &NodeKey, network: &Network) {
    if network.realm == dassie_ledger::Realm::Live {
        let token = node.setup_token().await;
        let body = setup_body_in(&token, key, "live");
        let reply = send(node.https, true, post_json(node, "/api/setup", &body)).await;
        assert_eq!(reply.status.as_u16(), 200, "{}", reply.text());
    } else {
        set_up(node, key).await;
    }
    let mut owner = node.ipc().await;
    let hostname = owner
        .call(Method::SetHostname(proto::SetHostnameRequest {
            hostname: "localhost".into(),
        }))
        .await;
    assert!(hostname.is_ok(), "{hostname:?}");
    let certificate = owner
        .call(Method::SetCertificate(proto::SetCertificateRequest {
            certificate_chain_pem: std::fs::read_to_string(CERT).unwrap(),
            private_key_pem: std::fs::read_to_string(KEY).unwrap(),
        }))
        .await;
    assert!(certificate.is_ok(), "{certificate:?}");
}

/// Adds settlement scheme `id` with `config`, once the engine runs.
async fn add_scheme(node: &TestNode, id: &str, config: &str) {
    node.engine_started().await;
    let added = node
        .ipc()
        .await
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: id.into(),
                config: config.into(),
            },
        ))
        .await;
    assert!(added.is_ok(), "{id} is not added: {added:?}");
}

async fn ledgers(node: &TestNode) -> Vec<proto::Ledger> {
    match node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await
    {
        Ok(response::Method::ListLedgers(list)) => list.ledgers,
        other => panic!("{other:?}"),
    }
}

async fn founding(node: &TestNode) -> proto::HubFounding {
    match node
        .ipc()
        .await
        .call(Method::ListHubFoundings(proto::ListHubFoundingsRequest {}))
        .await
    {
        Ok(response::Method::ListHubFoundings(list)) => list.foundings[0].clone(),
        other => panic!("{other:?}"),
    }
}

async fn validating(node: &TestNode) -> Vec<proto::HubValidator> {
    match node
        .ipc()
        .await
        .call(Method::GetStatus(proto::GetStatusRequest {}))
        .await
    {
        Ok(response::Method::GetStatus(status)) => status.validating,
        other => panic!("{other:?}"),
    }
}

/// Moves the shared clock on by `step_ms` at a time, giving the daemons
/// real time to talk, until `done` holds.
async fn until(clock: &ManualClock, step_ms: u64, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..1_500 {
        if done().await {
            return;
        }
        clock.advance(step_ms);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("not reached in time");
}

/// What every validator's file says: the sponsor it validates for, no hub
/// of its own, the network and the fake ledger to follow escrows on, and
/// the publisher of its validators, whose UNL the escrow's ledgers are
/// proven under.
fn validator_file(
    dir: &std::path::Path,
    sponsor: &NodeKey,
    (network, xrpl): (&Network, &str),
    publishers: Option<serde_json::Value>,
) -> String {
    let path = dir.join("validator.json");
    let mut file = serde_json::json!({
        "sponsors": [sponsor.node_id().as_str()],
        "hubs": [],
        "network": network.name,
        "xrpl": xrpl,
    });
    if let (Some(file), Some(serde_json::Value::Object(publishers))) =
        (file.as_object_mut(), publishers)
    {
        file.extend(publishers);
    }
    std::fs::write(&path, file.to_string()).unwrap();
    path.to_str().unwrap().to_owned()
}

/// What the owner deposits to the sponsor's mainnet account, which has no
/// faucet: enough for a few escrows.
const MAINNET_DEPOSIT_DROPS: u64 = 100_000_000;

/// What every daemon of a net on `network` runs with: the live realm's
/// dev switch on mainnet (spec/decisions/real-money.md).
fn env_on(network: &Network) -> Vec<(&'static str, &'static str)> {
    let mut env = vec![
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_EXTRA_CA_FILE", CA),
    ];
    if network.realm == dassie_ledger::Realm::Live {
        env.push(("DASSIE_DEV_LIVE_REALM", "1"));
    }
    env
}

/// A fake XRP Ledger, four validator daemons whose file lists the
/// sponsor, and the sponsor's daemon, its XRP Ledger account funded and
/// synced, all at one clock: on the testnet, or on mainnet in the live
/// realm.
struct Net {
    xrpl: Server,
    clock: ManualClock,
    sponsor: TestNode,
    validators: Vec<TestNode>,
    keys: Vec<NodeKey>,
    network: &'static Network,
    _files: tempfile::TempDir,
}

impl Net {
    async fn start(before_sync: impl FnOnce(&Server)) -> Self {
        Self::start_on(&TESTNET, before_sync).await
    }

    async fn start_on(network: &'static Network, before_sync: impl FnOnce(&Server)) -> Self {
        // Its HTTPS listener serves its validators' list; the sponsor asks
        // no faucet.
        let xrpl = Server::start(Options {
            network,
            listen: "127.0.0.1:0".parse().unwrap(),
            faucet: Some((
                "127.0.0.1:0".parse().unwrap(),
                dassie_node::store::StoredCertificate {
                    source: dassie_node_core::certificate::CertificateSource::Manual,
                    chain_pem: std::fs::read_to_string(CERT).unwrap(),
                    key_pem: std::fs::read_to_string(KEY).unwrap(),
                },
            )),
            close_every: Duration::from_millis(100),
        })
        .await
        .unwrap();
        let clock = ManualClock::new(START_MS);
        xrpl.close_on(std::sync::Arc::new(clock.clone()));
        let sponsor_key = NodeKey::from_bytes(&[70; 32]);
        let files = tempfile::tempdir().unwrap();
        let file = validator_file(
            files.path(),
            &sponsor_key,
            (network, &xrpl.url),
            xrpl.publishers(),
        );
        let env = env_on(network);
        let mut validator_env = env.clone();
        validator_env.push(("DASSIE_VALIDATOR_CONFIG", file.as_str()));
        let mut validators = Vec::new();
        let keys: Vec<NodeKey> = (71..=74)
            .map(|seed| NodeKey::from_bytes(&[seed; 32]))
            .collect();
        for (key, seed) in keys.iter().zip(71_u64..) {
            let node = Bound::new()
                .start(clock.clone(), seed, Policies::DEV, &validator_env)
                .await;
            prepare(&node, key, network).await;
            validators.push(node);
        }
        let sponsor = Bound::new()
            .start(clock.clone(), 70, Policies::DEV, &env)
            .await;
        prepare(&sponsor, &sponsor_key, network).await;
        let mut xrpl_config = serde_json::json!({ "server": xrpl.url, "faucet": null });
        if let (Some(config), Some(serde_json::Value::Object(publishers))) =
            (xrpl_config.as_object_mut(), xrpl.publishers())
        {
            config.extend(publishers);
        }
        support::pair::set_asset_config(
            &sponsor,
            &network.first().to_string(),
            &xrpl_config.to_string(),
        )
        .await;
        // The XRP book, peering with nobody, as `dassie setup` adds it.
        let book = network.xrp_scheme.as_str();
        let ledger = format!("{book}+xrp");
        add_scheme(&sponsor, book, r#"{"peers":false}"#).await;
        let address = ledgers(&sponsor)
            .await
            .into_iter()
            .find(|l| l.id == ledger)
            .unwrap()
            .deposit_address;
        // Mainnet has no faucet: the owner deposits.
        let address = dassie_xrpl::AccountId::parse(&address).unwrap();
        if network.faucet.is_some() {
            xrpl.ledger().fund(address);
        } else {
            xrpl.ledger().deposit(address, MAINNET_DEPOSIT_DROPS);
        }
        before_sync(&xrpl);
        until(&clock, 250, async || {
            ledgers(&sponsor)
                .await
                .iter()
                .any(|l| l.id == ledger && l.connection() == proto::LedgerConnection::Synced)
        })
        .await;
        Self {
            xrpl,
            clock,
            sponsor,
            validators,
            keys,
            network,
            _files: files,
        }
    }

    /// The validators, as the owner invites them.
    fn invites(&self) -> Vec<proto::HubFoundingInvite> {
        self.keys
            .iter()
            .zip(&self.validators)
            .map(|(key, node)| proto::HubFoundingInvite {
                node: key.node_id().as_str().into(),
                endpoint: node.host(),
            })
            .collect()
    }

    /// Stops the sponsor's daemon and starts it again on its database.
    async fn restart_sponsor(self) -> Self {
        let (dir, clock) = self.sponsor.stop().await;
        let env = env_on(self.network);
        let sponsor = Bound::bind_in(dir)
            .start(clock, 70, Policies::DEV, &env)
            .await;
        Self { sponsor, ..self }
    }

    async fn stop(self) {
        self.sponsor.stop().await;
        for node in self.validators {
            node.stop().await;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sponsor_founds_a_hub_with_four_validator_daemons() {
    let net = Net::start(|_| {}).await;
    let invites = net.invites();
    let Net {
        xrpl,
        clock,
        sponsor,
        validators,
        _files,
        ..
    } = net;

    // A stranger's founding is refused; the sponsor's goes through.
    let mut owner = sponsor.ipc().await;
    let refused = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites[..3].to_vec(),
            terms: String::new(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert_eq!(
        refused.unwrap_err().code(),
        proto::ErrorCode::InvalidArgument,
        "three validators are no set"
    );
    // An escrow funded with less than the XRP Ledger needs to create it
    // could never be set up.
    let refused = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites.clone(),
            terms: String::new(),
            escrow_funding_drops: Some(1_000),
            xrpl_server: String::new(),
        }))
        .await;
    assert_eq!(
        refused.unwrap_err().code(),
        proto::ErrorCode::InvalidArgument,
        "too little to create the escrow"
    );
    // The shortest stall the rules allow, so that the hub stalls within
    // the test once validators are gone.
    let terms = serde_json::json!({ "timers": { "stall": dassie_hub::consensus::MIN_STALL_MS } });
    let Ok(response::Method::FoundHub(found)) = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites.clone(),
            terms: terms.to_string(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await
    else {
        panic!("not founding");
    };
    let started = found.founding.unwrap();
    assert_eq!(started.stage(), proto::HubFoundingStage::Accepting);

    // Every validator accepts, signs, runs the hub; the escrow is funded
    // from the sponsor's account, set up, and checked by a quorum.
    until(&clock, 250, async || {
        let status = founding(&sponsor).await;
        status.stage() == proto::HubFoundingStage::Founded
    })
    .await;
    let founded = founding(&sponsor).await;
    assert!(
        founded.escrow_funded && founded.escrow_set_up,
        "{founded:?}"
    );
    assert!(founded.validators.iter().all(|v| v.accepted && v.running));
    let hub = founded.hub.clone();
    for node in &validators {
        let hubs = validating(node).await;
        assert_eq!(hubs.len(), 1);
        assert_eq!(hubs[0].hub, hub);
    }
    until(&clock, 250, async || {
        let mut checked = 0;
        for node in &validators {
            checked += usize::from(validating(node).await.iter().all(|h| h.escrow_checked));
        }
        checked == validators.len()
    })
    .await;
    // The escrow on the ledger is the rules': its master key disabled, and
    // the validators' signer list.
    let escrow = dassie_xrpl::AccountId::parse(&founded.escrow).unwrap();
    assert!(xrpl.ledger().balance(escrow).is_some());

    // A member: the sponsor takes the hub's scheme. Two validators go, and
    // the two left declare the hub stalled once no block is final for the
    // rules' stall, and sign the payout set; the member fetches each one's
    // bundle and keeps it once it checks.
    let hub_config = serde_json::json!({ "rules": founded.rules }).to_string();
    add_scheme(&sponsor, dassie_hub::member::SCHEME_ID, &hub_config).await;
    let mut validators = validators.into_iter();
    let (first, second) = (validators.next().unwrap(), validators.next().unwrap());
    let left: Vec<TestNode> = validators.collect();
    first.stop().await;
    second.stop().await;
    // The stall is the rules' shortest, minutes on the hub's clock, which
    // moves in steps of seconds: the views it holds time out within it.
    until(&clock, 5_000, async || {
        let reply = sponsor
            .ipc()
            .await
            .call(Method::GetHubPayout(proto::GetHubPayoutRequest::default()))
            .await;
        matches!(reply, Ok(response::Method::GetHubPayout(payout)) if payout.bundles.len() == 2)
    })
    .await;
    // The node's only hub, unnamed or named by its ID; no other.
    let Ok(response::Method::GetHubPayout(payout)) = sponsor
        .ipc()
        .await
        .call(Method::GetHubPayout(proto::GetHubPayoutRequest {
            hub_id: hub.clone(),
        }))
        .await
    else {
        panic!("no payout");
    };
    let unnamed = sponsor
        .ipc()
        .await
        .call(Method::GetHubPayout(proto::GetHubPayoutRequest::default()))
        .await;
    assert!(
        matches!(&unnamed, Ok(response::Method::GetHubPayout(only)) if *only == payout),
        "{unnamed:?}"
    );
    let other = sponsor
        .ipc()
        .await
        .call(Method::GetHubPayout(proto::GetHubPayoutRequest {
            hub_id: "00".repeat(32),
        }))
        .await;
    assert_eq!(
        other.unwrap_err().code(),
        proto::ErrorCode::NotFound,
        "a hub the node is not in"
    );
    assert_eq!(payout.hub, hub);
    assert_eq!(payout.quorum, 3);
    assert_eq!(
        payout
            .bundles
            .iter()
            .map(|b| b.validator)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert!(payout.ready_height.is_none(), "two of three");
    for node in &left {
        assert!(validating(node).await[0].stalled);
    }

    sponsor.stop().await;
    for node in left {
        node.stop().await;
    }
}

// spec/areas/hubs.md#founding-a-hub: a hub of DSUSD is founded as one of
// XRP is, its terms naming the token; its escrow is funded in XRP from the
// sponsor's account, and its setup makes the trust line to DSUSD's issuer
// beside the signer list, before the master key goes; a quorum checks it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sponsor_founds_a_dsusd_hub() {
    let net = Net::start(|_| {}).await;
    let invites = net.invites();
    let mut owner = net.sponsor.ipc().await;
    let asset = serde_json::json!({
        "currency": dassie_xrpl::dsusd::CURRENCY.to_string(),
        "issuer": dassie_xrpl::dsusd::ISSUER.to_address(),
    });
    // Too little for the account, its signer list and its line, and the
    // fees of four setup transactions at the hub's gas.
    let least = dassie_hub::rules::LEDGER_BASE_RESERVE
        + 2 * dassie_node::hub::sponsor::LEDGER_OWNER_RESERVE
        + 4 * dassie_hub::rules::LEAST_TOKEN_GAS_DROPS;
    let refused = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites.clone(),
            terms: serde_json::json!({ "asset": asset }).to_string(),
            escrow_funding_drops: Some(least - 1),
            xrpl_server: String::new(),
        }))
        .await;
    assert_eq!(
        refused.unwrap_err().code(),
        proto::ErrorCode::InvalidArgument,
        "too little to create the escrow with its line"
    );
    let found = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: invites,
            terms: serde_json::json!({ "asset": asset }).to_string(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert!(
        matches!(found, Ok(response::Method::FoundHub(_))),
        "{found:?}"
    );
    until(&net.clock, 250, async || {
        founding(&net.sponsor).await.stage() == proto::HubFoundingStage::Founded
    })
    .await;
    let founded = founding(&net.sponsor).await;
    assert!(
        founded.escrow_funded && founded.escrow_set_up,
        "{founded:?}"
    );
    let rules = rules_of(&founded);
    assert_eq!(
        rules.terms.asset,
        dassie_hub::rules::Asset::Token {
            currency: dassie_xrpl::dsusd::CURRENCY,
            issuer: dassie_xrpl::dsusd::ISSUER,
        }
    );
    assert_eq!(
        rules.terms.fees.gas,
        dassie_hub::rules::LEAST_TOKEN_GAS_DROPS
    );
    until(&net.clock, 250, async || {
        let mut checked = 0;
        for node in &net.validators {
            checked += usize::from(validating(node).await.iter().all(|h| h.escrow_checked));
        }
        checked == net.validators.len()
    })
    .await;
    // Its signer list and its line to the issuer: two objects.
    let escrow = dassie_xrpl::AccountId::parse(&founded.escrow).unwrap();
    assert_eq!(net.xrpl.ledger().owner_count(escrow), Some(2));
    net.stop().await;
}

/// The rules of `founded`, decoded.
fn rules_of(founded: &proto::HubFounding) -> dassie_hub::Rules {
    let bytes = dassie_xrpl::from_hex(&founded.rules).unwrap();
    let mut reader = dassie_oer::Reader::new(&bytes);
    let rules = dassie_hub::Rules::decode(&mut reader).unwrap();
    reader.finish().unwrap();
    rules
}

/// Founds a hub of `terms` as `net`'s sponsor, and waits until it is
/// founded, its escrow funded and set up, and every validator checked it:
/// the founding.
async fn found_and_check(net: &Net, terms: &serde_json::Value) -> proto::HubFounding {
    let before = net
        .sponsor
        .ipc()
        .await
        .call(Method::ListHubFoundings(proto::ListHubFoundingsRequest {}))
        .await;
    let Ok(response::Method::ListHubFoundings(before)) = before else {
        panic!("{before:?}");
    };
    let found = net
        .sponsor
        .ipc()
        .await
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: net.invites(),
            terms: terms.to_string(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    let Ok(response::Method::FoundHub(found)) = found else {
        panic!("{found:?}");
    };
    let nonce = found.founding.unwrap().nonce;
    assert!(before.foundings.iter().all(|f| f.nonce != nonce));
    let this = async || {
        let reply = net
            .sponsor
            .ipc()
            .await
            .call(Method::ListHubFoundings(proto::ListHubFoundingsRequest {}))
            .await;
        let Ok(response::Method::ListHubFoundings(list)) = reply else {
            panic!("{reply:?}");
        };
        list.foundings
            .into_iter()
            .find(|f| f.nonce == nonce)
            .unwrap()
    };
    until(&net.clock, 250, async || {
        this().await.stage() == proto::HubFoundingStage::Founded
    })
    .await;
    let founded = this().await;
    assert!(
        founded.escrow_funded && founded.escrow_set_up,
        "{founded:?}"
    );
    until(&net.clock, 250, async || {
        let mut checked = 0;
        for node in &net.validators {
            let hubs = validating(node).await;
            let hub = hubs.iter().find(|h| h.hub == founded.hub);
            checked += usize::from(hub.is_some_and(|h| h.escrow_checked));
        }
        checked == net.validators.len()
    })
    .await;
    founded
}

// spec/areas/hubs.md#founding-a-hub: a node of the live realm, by the dev
// switch, founds hubs on mainnet: its terms name mainnet, its escrow's key
// derives at the path that names mainnet
// (spec/decisions/hub-keys-by-network.md), and its escrow is funded from
// the sponsor's `xrpl-xrp` book on the fake ledger's mainnet; validators
// whose file names mainnet accept, sign, run and check it. An RLUSD hub's
// terms name RLUSD's issuer, and its setup makes the line to it. A token
// no mainnet scheme joins is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_live_realm_sponsor_founds_hubs_on_mainnet() {
    use dassie_hub::rules::Asset;

    let net = Net::start_on(&MAINNET, |_| {}).await;
    let sponsor_key = NodeKey::from_bytes(&[70; 32]);
    let entropy = sponsor_key.xrpl_account_entropy(MAINNET.id, 0);
    let mainnet_account = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account();
    let funded = net.xrpl.ledger().balance(mainnet_account).unwrap();

    // DSUSD is no mainnet hub's asset.
    let dsusd = serde_json::json!({ "asset": {
        "currency": dassie_xrpl::dsusd::CURRENCY.to_string(),
        "issuer": dassie_xrpl::dsusd::ISSUER.to_address(),
    }});
    let refused = net
        .sponsor
        .ipc()
        .await
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: net.invites(),
            terms: dsusd.to_string(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert_eq!(
        refused.unwrap_err().code(),
        proto::ErrorCode::InvalidArgument,
        "no mainnet scheme joins a DSUSD hub"
    );

    let xrp = found_and_check(&net, &serde_json::json!({})).await;
    let rules = rules_of(&xrp);
    assert_eq!(rules.terms.network, MAINNET.id);
    assert_eq!(rules.terms.asset, Asset::Xrp);
    assert_eq!(
        dassie_hub::member::scheme_of(&rules.terms).map(|(scheme, _)| scheme),
        Some(dassie_hub::member::MAINNET_SCHEME_ID)
    );
    let master = dassie_hub::escrow::escrow_master(&sponsor_key, MAINNET.id, &rules.terms.nonce);
    assert_eq!(master.account().0, rules.terms.escrow, "mainnet's key path");
    let testnet_master =
        dassie_hub::escrow::escrow_master(&sponsor_key, TESTNET.id, &rules.terms.nonce);
    assert_ne!(testnet_master.account().0, rules.terms.escrow);
    // Paid from the sponsor's mainnet account.
    let escrow = dassie_xrpl::AccountId::parse(&xrp.escrow).unwrap();
    assert!(net.xrpl.ledger().balance(escrow).is_some());
    assert!(net.xrpl.ledger().balance(mainnet_account).unwrap() < funded);

    let rlusd = serde_json::json!({ "asset": {
        "currency": dassie_xrpl::rlusd::CURRENCY.to_string(),
        "issuer": dassie_xrpl::rlusd::ISSUER.to_address(),
    }});
    let token = found_and_check(&net, &rlusd).await;
    let rules = rules_of(&token);
    assert_eq!(rules.terms.network, MAINNET.id);
    assert_eq!(
        rules.terms.asset,
        Asset::Token {
            currency: dassie_xrpl::rlusd::CURRENCY,
            issuer: dassie_xrpl::rlusd::ISSUER,
        }
    );
    assert_eq!(
        dassie_hub::member::scheme_of(&rules.terms).map(|(scheme, _)| scheme),
        Some(dassie_hub::member::RLUSD_SCHEME_ID)
    );
    // Its signer list and its line to RLUSD's issuer: two objects.
    let escrow = dassie_xrpl::AccountId::parse(&token.escrow).unwrap();
    assert_eq!(net.xrpl.ledger().owner_count(escrow), Some(2));
    for node in &net.validators {
        assert_eq!(validating(node).await.len(), 2);
    }
    net.stop().await;
}

/// A QUIC endpoint that only dials: a node of `key`'s, as a stranger or a
/// sponsor would found from.
struct Dialer;

impl dassie_node::quic::Handler for Dialer {
    fn client(&self, _address: std::net::IpAddr) -> Option<dassie_kit::ClientKey> {
        None
    }

    fn limits(&self) -> dassie_node_core::connections::ConnectionLimits {
        dassie_node_core::connections::ConnectionLimits {
            max_total: 8,
            max_per_client: 8,
        }
    }

    fn answer<'a>(
        &'a self,
        _client: std::net::IpAddr,
        _from: dassie_identity::PublicKey,
        _body: dassie_node::quic::Body<'a>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = dassie_node::peer_server::Answer> + Send + 'a>,
    > {
        Box::pin(async {
            dassie_node::peer_server::Answer {
                status: 404,
                body: Vec::new(),
            }
        })
    }
}

/// Sends a founding request as the node of `key` to `to`, and returns the
/// answer's bytes.
async fn found_as(
    key: &NodeKey,
    to: (&NodeKey, &TestNode),
    shutdown: &tokio_util::sync::CancellationToken,
) -> Result<Vec<u8>, dassie_node::hub::HubExchangeError> {
    use dassie_oer::Encode;
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let (identity, receiver) = tokio::sync::watch::channel(Some(std::sync::Arc::new(
        NodeKey::from_bytes(key.as_bytes()),
    )));
    let quic = std::sync::Arc::new(dassie_node::quic::Quic::new(socket, receiver, true).unwrap());
    tokio::spawn(std::sync::Arc::clone(&quic).serve(std::sync::Arc::new(Dialer), shutdown.clone()));
    let client = dassie_node::outbound::Resolver::new(true);
    let terms = dassie_hub::founding::starting_terms([3; 32], key.public_key(), 1, [4; 20]);
    let request = dassie_hub::founding::FoundingRequest::Propose(terms).to_vec();
    let (node, endpoint) = (to.0.node_id(), to.1.host());
    // The connection's timeout is real time, so it only guards against a
    // hang: a short one could end an exchange as unreachable on a busy
    // machine, which every check here fails.
    let answer = quic
        .hub_founding_exchange(node, &endpoint, &request, &client, support::HANG)
        .await;
    drop(identity);
    answer
}

// spec/areas/peer-protocol.md#the-hubs-stream: a founding stream from a
// node the validator's file does not list, or to a node with no validator
// file, is reset before its request is read; a listed sponsor's is read and
// answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_strangers_founding_stream_is_reset_unread() {
    use dassie_node::hub::HubExchangeError;
    use dassie_oer::Decode;

    let shutdown = tokio_util::sync::CancellationToken::new();
    let clock = ManualClock::new(START_MS);
    let sponsor = NodeKey::from_bytes(&[80; 32]);
    let stranger = NodeKey::from_bytes(&[81; 32]);
    let files = tempfile::tempdir().unwrap();
    let file = validator_file(files.path(), &sponsor, (&TESTNET, "ws://127.0.0.1:1"), None);
    let env = [
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_EXTRA_CA_FILE", CA),
    ];
    let validator_env = [
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_VALIDATOR_CONFIG", file.as_str()),
    ];
    let (validator_key, plain_key) = (
        NodeKey::from_bytes(&[82; 32]),
        NodeKey::from_bytes(&[83; 32]),
    );
    let validator = Bound::new()
        .start(clock.clone(), 82, Policies::DEV, &validator_env)
        .await;
    set_up(&validator, &validator_key).await;
    let plain = Bound::new()
        .start(clock.clone(), 83, Policies::DEV, &env)
        .await;
    set_up(&plain, &plain_key).await;

    for to in [(&validator_key, &validator), (&plain_key, &plain)] {
        let answer = found_as(&stranger, to, &shutdown).await;
        assert!(
            matches!(answer, Err(HubExchangeError::Failed(_))),
            "{answer:?}"
        );
    }
    let answer = found_as(&sponsor, (&plain_key, &plain), &shutdown).await;
    assert!(
        matches!(answer, Err(HubExchangeError::Failed(_))),
        "no validator file: {answer:?}"
    );
    // The listed sponsor is read and answered: the node has no hostname
    // yet to name in an acceptance.
    let answer = found_as(&sponsor, (&validator_key, &validator), &shutdown).await;
    let answer = dassie_hub::founding::FoundingAnswer::decode_exact(&answer.expect("answered"));
    assert_eq!(
        answer,
        Ok(dassie_hub::founding::FoundingAnswer::Refused(
            dassie_hub::founding::Refusal::NoEndpoint
        ))
    );

    shutdown.cancel();
    validator.stop().await;
    plain.stop().await;
}

/// The payments from `from` to `to` the fake ledger holds, each with its
/// result.
fn payments(
    xrpl: &Server,
    from: dassie_xrpl::AccountId,
    to: dassie_xrpl::AccountId,
) -> Vec<String> {
    use dassie_xrpl::api;
    let reply = xrpl
        .ledger()
        .request(77, &api::account_tx(5, from, 0, u32::MAX, None));
    let api::Message::Response {
        outcome: Ok(result),
        ..
    } = api::parse(&reply)
    else {
        return Vec::new();
    };
    api::account_tx_result(&result).map_or_else(Vec::new, |records| {
        records
            .transactions
            .iter()
            .filter(|record| record.is_payment && record.destination == Some(to))
            .map(|record| record.result.clone())
            .collect()
    })
}

// spec/areas/hubs.md#founding-a-hub: when the ledger's reserve is more
// than the sponsor funds the escrow with, the payment fails for being too
// little; the sponsor says so and pays nothing again, which would only fail
// alike and cost a fee each time.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_funding_too_little_for_the_escrow_is_paid_once() {
    // The network voted its reserve up to 5 XRP, past the 2 the sponsor
    // funds the escrow with by default.
    let net = Net::start(|xrpl| xrpl.ledger().set_reserve(5_000_000, 200_000)).await;
    let mut owner = net.sponsor.ipc().await;
    let found = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: net.invites(),
            terms: String::new(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert!(found.is_ok(), "{found:?}");
    until(&net.clock, 250, async || {
        founding(&net.sponsor).await.problem.contains("too little")
    })
    .await;
    let status = founding(&net.sponsor).await;
    let escrow = dassie_xrpl::AccountId::parse(&status.escrow).unwrap();
    let address = ledgers(&net.sponsor)
        .await
        .into_iter()
        .find(|l| l.id == "xrpl-testnet-xrp+xrp")
        .unwrap()
        .deposit_address;
    let sponsor = dassie_xrpl::AccountId::parse(&address).unwrap();
    // Until the setup, connecting again after it stopped, looked at the
    // escrow once more.
    looked_again(&net, escrow).await;
    assert_eq!(
        payments(&net.xrpl, sponsor, escrow),
        vec!["tecNO_DST_INSUF_XRP".to_string()],
        "paid once"
    );
    assert!(!founding(&net.sponsor).await.escrow_set_up);
    drop(owner);
    // Nor after a restart, when the server no longer holds the payment:
    // the founding kept why it stopped, and asks the ledger nothing.
    net.xrpl.ledger().forget_history();
    let net = net.restart_sponsor().await;
    looked_again(&net, escrow).await;
    assert!(
        payments(&net.xrpl, sponsor, escrow).is_empty(),
        "paid again after a restart"
    );
    assert!(founding(&net.sponsor).await.problem.contains("too little"));
    net.stop().await;
}

/// Runs the clock until the sponsor's escrow setup has read `escrow` from
/// the ledger again, and so decided whether to fund it, then a few steps
/// longer, for a payment it made to reach the ledger. Its callers check
/// that none did: nothing else follows a read that stops there, so there
/// is nothing to wait for, and these steps only give a wrong payment the
/// time to show. A slow machine can only let one go unseen, never fail
/// the test.
async fn looked_again(net: &Net, escrow: dassie_xrpl::AccountId) {
    let reads = net.xrpl.ledger().reads_of(escrow);
    until(&net.clock, 250, async || {
        net.xrpl.ledger().reads_of(escrow) > reads
    })
    .await;
    for _ in 0..8 {
        net.clock.advance(250);
        tokio::time::sleep(Duration::from_millis(125)).await;
    }
}

// spec/areas/hubs.md#founding-a-hub: a funding payment the XRP scheme
// cancels before it reaches the ledger moved nothing, so the sponsor pays
// again, once, and the founding goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cancelled_funding_is_paid_again() {
    use dassie_xrpl::api;
    use dassie_xrpl::codec::{Amount, Payment, Signing, Transact};

    let net = Net::start(|_| {}).await;
    let address = ledgers(&net.sponsor)
        .await
        .into_iter()
        .find(|l| l.id == "xrpl-testnet-xrp+xrp")
        .unwrap()
        .deposit_address;
    let sponsor = dassie_xrpl::AccountId::parse(&address).unwrap();
    // A transaction of another wallet of the owner's takes the account's
    // next sequence behind the scheme's back, in the ledger its funding
    // payment would go into: that payment can never apply, which the
    // history proves once it tells the other, and the scheme cancels it.
    let reply = net
        .xrpl
        .ledger()
        .request(77, &api::account_info(5, sponsor));
    let api::Message::Response {
        outcome: Ok(result),
        ..
    } = api::parse(&reply)
    else {
        panic!("no account");
    };
    let sequence = api::account_info_result(&result).unwrap().sequence;
    let entropy = NodeKey::from_bytes(&[70; 32]).xrpl_account_entropy(TESTNET.id, 0);
    let wallet = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes());
    assert_eq!(wallet.account(), sponsor);
    let other = Payment {
        account: sponsor,
        destination: dassie_xrpl::Wallet::from_entropy(&[9; 32]).account(),
        destination_tag: None,
        amount: Amount::Xrp(10 * dassie_xrpl::DROPS_PER_XRP),
        send_max: None,
        fee: dassie_xrpl::fake::FEE_BASE,
        sequence,
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(wallet.public_key()),
    }
    .sign(&wallet);
    net.xrpl.ledger().request(77, &api::submit(6, &other.blob));
    let mut owner = net.sponsor.ipc().await;
    let found = owner
        .call(Method::FoundHub(proto::FoundHubRequest {
            validators: net.invites(),
            terms: String::new(),
            escrow_funding_drops: None,
            xrpl_server: String::new(),
        }))
        .await;
    assert!(found.is_ok(), "{found:?}");
    until(&net.clock, 250, async || {
        founding(&net.sponsor).await.stage() == proto::HubFoundingStage::Founded
    })
    .await;
    let escrow = dassie_xrpl::AccountId::parse(&founding(&net.sponsor).await.escrow).unwrap();
    assert_eq!(
        payments(&net.xrpl, sponsor, escrow),
        vec!["tesSUCCESS".to_string()],
        "the cancelled one never reached the ledger; the second paid"
    );
    drop(owner);
    net.stop().await;
}

//! Two daemons that peer, over real HTTPS with a shared clock: what
//! `DASSIE_BOOTSTRAP_NODES` names, setting each up as a peer, and moving
//! their time on until something holds.

#![allow(clippy::panic)]

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::clock::ManualClock;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};

use super::{Bound, START_MS, TestNode, set_up};

pub const CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/root.pem");
pub const CERT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/cert.pem");
pub const KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/key.pem");

/// The config of an XRPL scheme on the fake ledger `server`: its server
/// and faucet, and the publisher of its validators, whose list it serves.
pub fn xrpl_config(server: &dassie_node::fake_ledger::Server) -> String {
    let mut config = serde_json::json!({
        "server": server.url,
        "faucet": server.faucet_url,
    });
    if let (Some(object), Some(serde_json::Value::Object(publishers))) =
        (config.as_object_mut(), server.publishers())
    {
        object.extend(publishers);
    }
    config.to_string()
}

pub fn key(seed: u8) -> NodeKey {
    NodeKey::from_bytes(&[seed; 32])
}

/// `DASSIE_BOOTSTRAP_NODES` naming `node` at `port`.
pub fn bootstrap(node: &NodeKey, port: u16) -> String {
    let mut encoded = [0u8; 43];
    let length = dassie_identity::base64url::encode(node.public_key().as_bytes(), &mut encoded);
    let public = std::str::from_utf8(&encoded[..length]).unwrap();
    format!(
        r#"[{{"id":"{}","url":"https://localhost:{port}","publicKey":"{public}"}}]"#,
        node.node_id()
    )
}

/// Sets a node up as a peer: owner, hostname, certificate, stub scheme.
pub async fn prepare(node: &TestNode, key: &NodeKey) {
    prepare_with(node, key, "stub", "").await;
}

/// Sets a node up as a peer, settling with `scheme` set up with `config`.
pub async fn prepare_with(node: &TestNode, key: &NodeKey, scheme: &str, config: &str) {
    set_up(node, key).await;
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
    for _ in 0..200 {
        let added = owner
            .call(Method::AddSettlementScheme(
                proto::AddSettlementSchemeRequest {
                    id: scheme.into(),
                    config: config.into(),
                },
            ))
            .await;
        if added.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the {scheme} scheme did not start");
}

/// The node's ledger accounts' paths.
pub async fn accounts(node: &TestNode) -> Vec<String> {
    let mut owner = node.ipc().await;
    for _ in 0..200 {
        let reply = owner
            .call(Method::ListAccounts(proto::ListAccountsRequest {}))
            .await;
        match reply {
            Ok(response::Method::ListAccounts(list)) => {
                return list
                    .accounts
                    .into_iter()
                    .map(|account| account.path)
                    .collect();
            }
            // The engine is still starting.
            Err(error) if error.code() == proto::ErrorCode::Unavailable => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            other => panic!("{other:?}"),
        }
    }
    panic!("the engine did not start");
}

/// Moves the shared clock on in small steps, giving the daemons real time
/// to talk, until `done` holds.
pub async fn until(clock: &ManualClock, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..600 {
        if done().await {
            return;
        }
        clock.advance(250);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("not reached in time");
}

/// Gives the daemons a few seconds of their time to exchange link states.
pub async fn settle(clock: &ManualClock) {
    for _ in 0..20 {
        clock.advance(250);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub async fn peered(a: &TestNode, b: &NodeKey) -> bool {
    peered_on(a, b, "stub+usd").await
}

/// Whether `a` holds accounts for `b` on `ledger`.
pub async fn peered_on(a: &TestNode, b: &NodeKey, ledger: &str) -> bool {
    let path = format!("{ledger}:assets/interledger/{}", b.node_id());
    accounts(a).await.contains(&path)
}

/// Two nodes that peer, A the bootstrap node, at the same clock; set up
/// as peers and peered.
pub struct Pair {
    pub a: TestNode,
    pub b: TestNode,
    pub key_a: NodeKey,
    pub key_b: NodeKey,
    pub clock: ManualClock,
}

impl Pair {
    pub async fn start(seed_a: u8, seed_b: u8) -> Self {
        let (key_a, key_b) = (key(seed_a), key(seed_b));
        let (bound_a, bound_b) = (Bound::new(), Bound::new());
        let nodes = bootstrap(&key_a, bound_a.https_port());
        let env = [
            ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
            ("DASSIE_PEER_CA_FILE", CA),
            ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ];
        let clock = ManualClock::new(START_MS);
        let a = bound_a
            .start(clock.clone(), u64::from(seed_a), Policies::DEV, &env)
            .await;
        let b = bound_b
            .start(clock.clone(), u64::from(seed_b), Policies::DEV, &env)
            .await;
        prepare(&a, &key_a).await;
        prepare(&b, &key_b).await;
        until(&clock, async || {
            peered(&a, &key_b).await && peered(&b, &key_a).await
        })
        .await;
        settle(&clock).await;
        Self {
            a,
            b,
            key_a,
            key_b,
            clock,
        }
    }
}

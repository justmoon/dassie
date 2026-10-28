//! Two daemons that peer, over real HTTPS with a shared clock: what
//! `DASSIE_BOOTSTRAP_NODES` names, setting each up as a peer, and moving
//! their time on until something holds.

#![allow(clippy::panic)]

use std::time::Duration;

use dassie_identity::IdentityKey;
use dassie_node::clock::ManualClock;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};

use super::{Bound, START_MS, STUB_UNLIMITED, TestNode, set_up, stub_with_commitments};

pub const CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/root.pem");
pub const CERT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/cert.pem");
pub const KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/key.pem");

/// The node's XRPL testnet account, as external assets are named.
pub const XRPL_ACCOUNT: &str = "xrpl-testnet-account/0";

/// The config of an XRPL account on the fake ledger `server`: its server
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

pub fn key(seed: u8) -> IdentityKey {
    IdentityKey::from_bytes(&[seed; 32])
}

/// `DASSIE_BOOTSTRAP_NODES` naming `node` at `port`.
pub fn bootstrap(node: &IdentityKey, port: u16) -> String {
    let entry =
        dassie_node::front_door::key::relay_entry(node, &format!("https://localhost:{port}"));
    format!("[{entry}]")
}

/// Sets a node up as a peer: owner, hostname, certificate, and the stub
/// with unlimited trust, for a test whose subject is not settlement.
pub async fn prepare(node: &TestNode, key: &IdentityKey) {
    prepare_with(node, key, "stub", STUB_UNLIMITED).await;
}

/// Sets a node up as a peer on the stub with commitments, holding at most
/// `limit` packets in flight per peer each way: for a test of settlement,
/// whose settlements fund holds as a hub member's do.
pub async fn prepare_settling(node: &TestNode, key: &IdentityKey, limit: usize) {
    prepare_with(node, key, "stub", &stub_with_commitments(limit)).await;
}

/// Sets a node up as a peer, settling with `scheme` set up with `config`.
pub async fn prepare_with(node: &TestNode, key: &IdentityKey, scheme: &str, config: &str) {
    prepare_base(node, key).await;
    add_scheme(node, scheme, config).await;
}

/// Sets a node up as a peer, settling with the XRPL scheme `scheme`
/// through its testnet account, whose config is `account`.
pub async fn prepare_on_xrpl(node: &TestNode, key: &IdentityKey, scheme: &str, account: &str) {
    prepare_base(node, key).await;
    set_asset_config(node, XRPL_ACCOUNT, account).await;
    add_scheme(node, scheme, "").await;
}

/// Sets the owner's config of the node's external asset `asset`, once the
/// engine runs.
pub async fn set_asset_config(node: &TestNode, asset: &str, config: &str) {
    node.engine_started().await;
    let set = node
        .ipc()
        .await
        .call(Method::SetAssetConfig(proto::SetAssetConfigRequest {
            asset: asset.into(),
            config: config.into(),
        }))
        .await;
    assert!(set.is_ok(), "the config of {asset} was not set: {set:?}");
}

/// Sets a node up as a peer: owner, hostname and certificate.
async fn prepare_base(node: &TestNode, key: &IdentityKey) {
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
}

/// Adds settlement scheme `scheme` with `config`, once the engine runs.
async fn add_scheme(node: &TestNode, scheme: &str, config: &str) {
    node.engine_started().await;
    let added = node
        .ipc()
        .await
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: scheme.into(),
                config: config.into(),
            },
        ))
        .await;
    assert!(
        added.is_ok(),
        "the {scheme} scheme did not start: {added:?}"
    );
}

/// The node's ledger accounts' paths, once its engine runs.
pub async fn accounts(node: &TestNode) -> Vec<String> {
    node.engine_started().await;
    let reply = node
        .ipc()
        .await
        .call(Method::ListAccounts(proto::ListAccountsRequest {}))
        .await;
    let Ok(response::Method::ListAccounts(list)) = reply else {
        panic!("{reply:?}")
    };
    list.accounts
        .into_iter()
        .map(|account| account.path)
        .collect()
}

/// Moves the shared clock on in small steps, giving the daemons real time
/// to talk, until `done` holds.
pub async fn until(clock: &ManualClock, done: impl AsyncFnMut() -> bool) {
    until_by(clock, 250, done).await;
}

/// [`until`], with the clock moving `step_ms` at a time: for a wait of
/// many seconds on the daemons' clock, such as a backoff.
pub async fn until_by(clock: &ManualClock, step_ms: u64, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..600 {
        if done().await {
            return;
        }
        clock.advance(step_ms);
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

/// Whether `a` holds accounts for `b` on the stub's ledger: a core peer's
/// open as its peering is made.
pub async fn peered(a: &TestNode, b: &IdentityKey) -> bool {
    peered_on(a, b, "stub+usd").await
}

/// Whether `uplink` holds a peering with its downlink `downlink`. Its
/// accounts for a downlink open only once value moves with it, which on the
/// stub with unlimited trust is its first packet, as nothing settles
/// (spec/decisions/lazy-downlink-accounts.md).
pub async fn peered_downlink(uplink: &TestNode, downlink: &IdentityKey) -> bool {
    let reply = uplink
        .ipc()
        .await
        .call(Method::ListPeers(proto::ListPeersRequest {}))
        .await;
    let Ok(response::Method::ListPeers(list)) = reply else {
        panic!("{reply:?}")
    };
    let id = downlink.node_id().to_string();
    list.peers
        .iter()
        .any(|peer| peer.node == id && peer.inbound && peer.state == "peered")
}

/// Whether `a` holds accounts for `b` on `ledger`.
pub async fn peered_on(a: &TestNode, b: &IdentityKey, ledger: &str) -> bool {
    let path = format!("{ledger}:assets/interledger/{}", b.node_id());
    accounts(a).await.contains(&path)
}

/// Two nodes that peer, A the bootstrap node, at the same clock; set up
/// as peers on the stub with unlimited trust, and peered.
pub struct Pair {
    pub a: TestNode,
    pub b: TestNode,
    pub key_a: IdentityKey,
    pub key_b: IdentityKey,
    pub clock: ManualClock,
}

impl Pair {
    pub async fn start(seed_a: u8, seed_b: u8) -> Self {
        let (key_a, key_b) = (key(seed_a), key(seed_b));
        let (bound_a, bound_b) = (Bound::new(), Bound::new());
        let nodes = bootstrap(&key_a, bound_a.https_port());
        let env = [
            ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
            ("DASSIE_EXTRA_CA_FILE", CA),
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

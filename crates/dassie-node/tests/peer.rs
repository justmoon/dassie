//! Two daemons in one process, peering over real HTTPS (spec 7 §4.7): they
//! register, learn of each other, peer, and keep their peering across a
//! restart.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use support::btp_client::{BtpClient, pay_stream};
use support::{Bound, START_MS, TestNode, set_up};

/// 0.25 USD, more than one packet's worth.
const PAYMENT: u64 = 250_000_000;

/// The credit a node extends to a peer: 1.1 USD.
const CREDIT: u64 = 1_100_000_000;

const CA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../testdata/pebble/root.pem"
);
const CERT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../testdata/pebble/cert.pem"
);
const KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/pebble/key.pem");

fn key(seed: u8) -> NodeKey {
    NodeKey::from_bytes(&[seed; 32])
}

/// `DASSIE_BOOTSTRAP_NODES` naming `node` at `port`.
fn bootstrap(node: &NodeKey, port: u16) -> String {
    let mut encoded = [0u8; 43];
    let length = dassie_identity::base64url::encode(node.public_key().as_bytes(), &mut encoded);
    let public = std::str::from_utf8(&encoded[..length]).unwrap();
    format!(
        r#"[{{"id":"{}","url":"https://localhost:{port}","publicKey":"{public}"}}]"#,
        node.node_id()
    )
}

/// Sets a node up as a peer: owner, hostname, certificate, stub scheme.
async fn prepare(node: &TestNode, key: &NodeKey) {
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
                    id: "stub".into(),
                    config: String::new(),
                },
            ))
            .await;
        if added.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the stub scheme did not start");
}

/// The node's ledger accounts' paths.
async fn accounts(node: &TestNode) -> Vec<String> {
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
async fn until(clock: &ManualClock, mut done: impl AsyncFnMut() -> bool) {
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
async fn settle(clock: &ManualClock) {
    for _ in 0..20 {
        clock.advance(250);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The net credit of `node`'s account for its peer `peer`.
async fn net(node: &TestNode, peer: &NodeKey) -> i128 {
    let path = format!("stub+usd:assets/interledger/{}", peer.node_id());
    let reply = node
        .ipc()
        .await
        .call(Method::ListAccounts(proto::ListAccountsRequest {}))
        .await;
    let Ok(response::Method::ListAccounts(list)) = reply else {
        panic!("{reply:?}")
    };
    let account = list.accounts.into_iter().find(|a| a.path == path).unwrap();
    i128::from(account.credits_posted) - i128::from(account.debits_posted)
}

/// What the network methods say of a pair: one peer, one route, the
/// other node known with its URL, and the registration with A.
async fn check_network(node: &TestNode, own: &NodeKey, other: &NodeKey) {
    let mut owner = node.ipc().await;
    let Ok(response::Method::ListPeers(peers)) = owner
        .call(Method::ListPeers(proto::ListPeersRequest {}))
        .await
    else {
        panic!("no peers")
    };
    assert_eq!(peers.peers.len(), 1);
    let peer = &peers.peers[0];
    assert_eq!(peer.node, other.node_id().to_string());
    assert_eq!((peer.scheme.as_str(), peer.version), ("stub", 1));
    assert_eq!((peer.credit_in, peer.credit_out), (CREDIT, CREDIT));
    let Ok(response::Method::ListRoutes(routes)) = owner
        .call(Method::ListRoutes(proto::ListRoutesRequest {}))
        .await
    else {
        panic!("no routes")
    };
    assert_eq!(routes.routes.len(), 1);
    assert_eq!(
        routes.routes[0].prefix,
        format!("test.das.{}", other.node_id())
    );
    let Ok(response::Method::ListNodes(nodes)) = owner
        .call(Method::ListNodes(proto::ListNodesRequest {}))
        .await
    else {
        panic!("no nodes")
    };
    let them = nodes
        .nodes
        .iter()
        .find(|n| n.id == other.node_id().to_string())
        .unwrap();
    assert!(them.url.starts_with("https://localhost:") && them.peer && them.majority);
    let Ok(response::Method::GetRegistrationStatus(status)) = owner
        .call(Method::GetRegistrationStatus(
            proto::GetRegistrationStatusRequest {},
        ))
        .await
    else {
        panic!("no status")
    };
    assert_eq!(status.node, own.node_id().to_string());
    assert_eq!(status.bootstrap.len(), 1);
    assert!(status.bootstrap[0].listed);
}

async fn peered(a: &TestNode, b: &NodeKey) -> bool {
    let path = format!("stub+usd:assets/interledger/{}", b.node_id());
    accounts(a).await.contains(&path)
}

#[tokio::test(flavor = "multi_thread")]
async fn two_daemons_peer_and_keep_their_peering() {
    if let Ok(debug) = std::env::var("DEBUG") {
        dassie_node::log::init(dassie_node::log::LogFilter::new(
            dassie_node::log::LogLevel::Debug,
            &debug,
        ));
    }
    let (key_a, key_b) = (key(21), key(22));
    let (bound_a, bound_b) = (Bound::new(), Bound::new());
    let nodes = bootstrap(&key_a, bound_a.https_port());
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 1, Policies::DEV, &env).await;
    let b = bound_b.start(clock.clone(), 2, Policies::DEV, &env).await;
    prepare(&a, &key_a).await;
    prepare(&b, &key_b).await;
    until(&clock, async || {
        peered(&a, &key_b).await && peered(&b, &key_a).await
    })
    .await;
    // Both link states name the other, so each has a route to the other.
    settle(&clock).await;
    check_network(&a, &key_a, &key_b).await;
    check_network(&b, &key_b, &key_a).await;
    // Found in review: removing a scheme that peers use left their
    // peerings without a ledger, and the engine could not start again.
    let removed = a
        .ipc()
        .await
        .call(Method::RemoveSettlementScheme(
            proto::RemoveSettlementSchemeRequest { id: "stub".into() },
        ))
        .await;
    assert_eq!(
        removed.unwrap_err().code(),
        proto::ErrorCode::FailedPrecondition
    );
    // A client of A pays B's receiver, one hop away, within B's credit.
    let mut owner_a = a.ipc().await;
    let Ok(response::Method::CreateBtpToken(token)) = owner_a
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "wallet".into(),
            ledger: "stub+usd".into(),
        }))
        .await
    else {
        panic!("no token")
    };
    let Ok(response::Method::CreateStreamCredentials(credentials)) = b
        .ipc()
        .await
        .call(Method::CreateStreamCredentials(
            proto::CreateStreamCredentialsRequest { tag: "hop".into() },
        ))
        .await
    else {
        panic!("no credentials")
    };
    let mut client = BtpClient::connect(&a, &token.secret).await.unwrap();
    pay_stream(
        &clock,
        &mut client,
        &credentials.destination_account,
        &credentials.shared_secret,
        PAYMENT,
    )
    .await;
    // Each side's account for the other moved by the payment, mirrored:
    // A owes B what B is owed by A.
    let at_a = net(&a, &key_b).await;
    let at_b = net(&b, &key_a).await;
    assert_eq!(at_a, i128::from(CREDIT) + i128::from(PAYMENT), "A's books");
    assert_eq!(at_b, i128::from(CREDIT) - i128::from(PAYMENT), "B's books");
    // Restarted, B still has its peering and its accounts: nothing to redo.
    let (dir, clock_b) = b.stop().await;
    assert_eq!(clock_b.now_ms(), clock.now_ms());
    let b = Bound::bind_in(dir)
        .start(clock.clone(), 3, Policies::DEV, &env)
        .await;
    assert!(peered(&b, &key_a).await);
    a.stop().await;
    b.stop().await;
}

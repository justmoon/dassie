//! Open Payments over real HTTPS (D9): wallet addresses and keys, grants
//! for clients known by their wallet address or only by their key, signed
//! requests, and incoming payments that take exactly what they ask for.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeKey;
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::open_payments::keys::ClientKey;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_payments::SigningKey;
use dassie_payments::httpsig;
use dassie_payments::jwk::Jwk;
use http_body_util::Full;
use hyper::{Request, StatusCode};
use serde_json::{Value, json};
use support::btp_client::{BtpClient, send_stream};
use support::{Bound, Reply, START_MS, TestNode, send, set_up};

const CA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../testdata/pebble/root.pem"
);
const CERT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../testdata/pebble/cert.pem"
);
const KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/pebble/key.pem");

/// A client of the node's Open Payments: its key and how it names itself.
struct Client {
    key: SigningKey,
    kid: String,
    identity: Value,
}

impl Client {
    /// A client known only by its key.
    fn keyed(seed: u8) -> Self {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let kid = format!("key-{seed}");
        let jwk = Jwk::ed25519(&kid, &key.verifying_key());
        Self {
            identity: json!({ "jwk": jwk }),
            key,
            kid,
        }
    }
}

/// A node with a hostname, a certificate its own HTTPS client trusts, and
/// the stub ledger: an Open Payments server.
async fn start() -> (TestNode, NodeKey) {
    let key = NodeKey::from_bytes(&[31; 32]);
    let env = [
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 3, Policies::DEV, &env)
        .await;
    set_up(&node, &key).await;
    let mut owner = node.ipc().await;
    for method in [
        Method::SetHostname(proto::SetHostnameRequest {
            hostname: "localhost".into(),
        }),
        Method::SetCertificate(proto::SetCertificateRequest {
            certificate_chain_pem: std::fs::read_to_string(CERT).unwrap(),
            private_key_pem: std::fs::read_to_string(KEY).unwrap(),
        }),
    ] {
        owner.call(method).await.unwrap();
    }
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
            owner
                .call(Method::StubDeposit(proto::StubDepositRequest {
                    amount: 100_000_000_000,
                }))
                .await
                .unwrap();
            return (node, key);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the stub scheme did not start");
}

/// A request to the node, signed by `client` with `token` if given.
fn signed(
    node: &TestNode,
    client: &Client,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Request<Full<Bytes>> {
    let url = format!("{}{path}", node.origin());
    let body = body.map(|body| serde_json::to_vec(body).unwrap());
    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(token) = token {
        fields.push(("Authorization".into(), format!("GNAP {token}")));
    }
    if let Some(body) = &body {
        fields.push(("Content-Type".into(), "application/json".into()));
        fields.push(("Content-Digest".into(), httpsig::content_digest(body)));
        fields.push(("Content-Length".into(), body.len().to_string()));
    }
    let refs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    let request = httpsig::Request {
        method,
        target_uri: &url,
        fields: &refs,
    };
    let components = httpsig::profile(&request, body.is_some());
    let created = node.clock.now_ms() / 1_000;
    let signature =
        httpsig::sign(&request, &components, &client.key, &client.kid, created).unwrap();
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", node.host())
        .header("signature-input", signature.signature_input)
        .header("signature", signature.signature);
    for (name, value) in &fields {
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .unwrap()
}

fn get(node: &TestNode, path: &str) -> Request<Full<Bytes>> {
    Request::get(path)
        .header("host", node.host())
        .body(Full::new(Bytes::new()))
        .unwrap()
}

fn body(reply: &Reply) -> Value {
    serde_json::from_slice(&reply.body).unwrap_or_else(|_| panic!("not JSON: {}", reply.text()))
}

/// A grant for `access`; returns its access token.
async fn grant(node: &TestNode, client: &Client, access: Value) -> String {
    let request = json!({ "access_token": { "access": access }, "client": client.identity });
    let reply = send(
        node.https,
        true,
        signed(node, client, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let grant = body(&reply);
    assert!(
        grant["continue"]["uri"]
            .as_str()
            .unwrap()
            .contains("/gnap/continue/")
    );
    grant["access_token"]["value"].as_str().unwrap().to_string()
}

fn incoming_access(node: &TestNode) -> Value {
    json!([{
        "type": "incoming-payment",
        "actions": ["create", "read", "list", "complete"],
        "identifier": format!("{}/.well-known/pay", node.origin()),
    }])
}

#[tokio::test(flavor = "multi_thread")]
async fn wallet_addresses_and_their_keys() {
    let (node, key) = start().await;
    let reply = send(node.https, true, get(&node, "/.well-known/pay")).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.header("access-control-allow-origin"), Some("*"));
    let address = body(&reply);
    let origin = node.origin();
    assert_eq!(address["id"], format!("{origin}/.well-known/pay"));
    assert_eq!(address["assetCode"], "USD");
    assert_eq!(address["assetScale"], 9);
    assert_eq!(address["authServer"], format!("{origin}/gnap"));
    assert_eq!(address["resourceServer"], origin);
    let by_ledger = body(&send(node.https, true, get(&node, "/pay/stub")).await);
    assert_eq!(by_ledger["id"], format!("{origin}/pay/stub"));
    let missing = send(node.https, true, get(&node, "/pay/xrpl")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(body(&missing)["error"]["code"], "not_found");
    let jwks = body(&send(node.https, true, get(&node, "/pay/stub/jwks.json")).await);
    let own = ClientKey::derive(&key).unwrap();
    assert_eq!(jwks["keys"][0]["kid"], own.kid());
    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/incoming-payments")
        .header("host", node.host())
        .header("origin", "https://elsewhere.example")
        .header("access-control-request-method", "POST")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = send(node.https, true, preflight).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(
        reply
            .header("access-control-allow-headers")
            .unwrap()
            .contains("Signature-Input")
    );
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_creates_and_reads_incoming_payments() {
    let (node, _) = start().await;
    let client = Client::keyed(7);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({
        "walletAddress": format!("{}/pay/stub", node.origin()),
        "incomingAmount": { "value": "250000000", "assetCode": "USD", "assetScale": 9 },
        "metadata": { "description": "coffee" },
    });
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let payment = body(&reply);
    let id = payment["id"].as_str().unwrap().to_string();
    let path = id.strip_prefix(&node.origin()).unwrap().to_string();
    assert_eq!(payment["completed"], false);
    assert_eq!(payment["receivedAmount"]["value"], "0");
    assert_eq!(payment["metadata"]["description"], "coffee");
    let method = &payment["methods"][0];
    assert_eq!(method["type"], "ilp");
    // Anyone may see what it received, and where to get a grant.
    let public = body(&send(node.https, true, get(&node, &path)).await);
    assert_eq!(
        public,
        json!({
            "receivedAmount": { "value": "0", "assetCode": "USD", "assetScale": 9 },
            "authServer": format!("{}/gnap", node.origin()),
        })
    );
    // Paying more than it asks for gets it exactly what it asks for.
    let destination = method["ilpAddress"].as_str().unwrap().to_string();
    let secret = base64_url(method["sharedSecret"].as_str().unwrap());
    let received = pay(&node, &destination, &secret, 400_000_000).await;
    assert_eq!(received, 250_000_000);
    let read = body(
        &send(
            node.https,
            true,
            signed(&node, &client, "GET", &path, Some(&token), None),
        )
        .await,
    );
    assert_eq!(read["receivedAmount"]["value"], "250000000");
    assert_eq!(read["completed"], true);
    assert_eq!(read["methods"], json!([]));
    // Completed, it takes nothing more.
    assert_eq!(pay(&node, &destination, &secret, 1).await, 0);
    let list_path = format!(
        "/incoming-payments?wallet-address={}&first=10",
        dassie_payments::uri::percent_encode(&format!("{}/.well-known/pay", node.origin()))
    );
    let list = body(
        &send(
            node.https,
            true,
            signed(&node, &client, "GET", &list_path, Some(&token), None),
        )
        .await,
    );
    assert_eq!(list["result"][0]["id"], id);
    assert_eq!(list["pagination"]["hasNextPage"], false);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_completes_an_open_payment() {
    let (node, _) = start().await;
    let client = Client::keyed(8);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let payment = body(
        &send(
            node.https,
            true,
            signed(
                &node,
                &client,
                "POST",
                "/incoming-payments",
                Some(&token),
                Some(&create),
            ),
        )
        .await,
    );
    let path = payment["id"]
        .as_str()
        .unwrap()
        .strip_prefix(&node.origin())
        .unwrap()
        .to_string();
    let method = &payment["methods"][0];
    let destination = method["ilpAddress"].as_str().unwrap().to_string();
    let secret = base64_url(method["sharedSecret"].as_str().unwrap());
    assert_eq!(pay(&node, &destination, &secret, 3_000).await, 3_000);
    let complete = format!("{path}/complete");
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", &complete, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(body(&reply)["completed"], true);
    assert_eq!(body(&reply)["receivedAmount"]["value"], "3000");
    assert_eq!(pay(&node, &destination, &secret, 1).await, 0);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_named_by_its_wallet_address() {
    // The node is its own client here: its wallet address publishes the
    // key, which the authorization server fetches over HTTPS.
    let (node, key) = start().await;
    let own = ClientKey::derive(&key).unwrap();
    let client = Client {
        key: own.signing_key().clone(),
        kid: own.kid().to_string(),
        identity: json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) }),
    };
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_without_a_good_token_and_signature_are_refused() {
    let (node, _) = start().await;
    let client = Client::keyed(9);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    // No token: 401, and where to get one.
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            None,
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        reply.header("www-authenticate"),
        Some(format!("GNAP as_uri={}/gnap", node.origin()).as_str())
    );
    // Another key with the token.
    let thief = Client::keyed(10);
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &thief,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    // A body the signature does not match.
    let mut tampered = signed(
        &node,
        &client,
        "POST",
        "/incoming-payments",
        Some(&token),
        Some(&create),
    );
    // The same length, so that only the digest tells.
    let changed = serde_json::to_string(&create)
        .unwrap()
        .replace("pay\"", "paz\"");
    *tampered.body_mut() = Full::new(Bytes::from(changed));
    let reply = send(node.https, true, tampered).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    // A signature from long ago.
    let stale = signed(
        &node,
        &client,
        "POST",
        "/incoming-payments",
        Some(&token),
        Some(&create),
    );
    node.clock.advance(10 * 60 * 1_000);
    let reply = send(node.https, true, stale).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn grants_reach_only_what_they_name() {
    let (node, _) = start().await;
    let client = Client::keyed(11);
    let other = Client::keyed(12);
    let read_only = grant(
        &node,
        &client,
        json!([{ "type": "incoming-payment", "actions": ["read"] }]),
    )
    .await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&read_only),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    // `read` reaches only what the client created.
    let token = grant(&node, &other, incoming_access(&node)).await;
    let payment = body(
        &send(
            node.https,
            true,
            signed(
                &node,
                &other,
                "POST",
                "/incoming-payments",
                Some(&token),
                Some(&create),
            ),
        )
        .await,
    );
    let path = payment["id"]
        .as_str()
        .unwrap()
        .strip_prefix(&node.origin())
        .unwrap()
        .to_string();
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "GET", &path, Some(&read_only), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    // A wallet address that is not the node's, and grants that need the
    // owner, are not offered.
    let foreign = json!([{ "type": "incoming-payment", "actions": ["create"], "identifier": "https://elsewhere.example/alice" }]);
    let request = json!({ "access_token": { "access": foreign }, "client": client.identity });
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(body(&reply)["error"]["code"], "invalid_request");
    let list_all = json!([{ "type": "incoming-payment", "actions": ["list-all"] }]);
    let request = json!({
        "access_token": { "access": list_all },
        "client": client.identity,
        "interact": { "start": ["redirect"] },
    });
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    node.stop().await;
}

/// Pays `amount` to a STREAM destination from a BTP client of the node,
/// and returns what arrived.
async fn pay(node: &TestNode, destination: &str, secret: &[u8], amount: u64) -> u64 {
    let mut owner = node.ipc().await;
    let Ok(response::Method::CreateBtpToken(token)) = owner
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "payer".into(),
            ledger: "stub+usd".into(),
            cap: None,
        }))
        .await
    else {
        panic!("no token")
    };
    let mut client = BtpClient::connect(node, &token.secret).await.unwrap();
    send_stream(&node.clock, &mut client, destination, secret, amount, 2_000).await
}

fn base64_url(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .unwrap()
}

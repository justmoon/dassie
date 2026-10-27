//! Open Payments as a client sees it: a node with a hostname and the stub
//! ledger, signed requests, grants, and paying incoming payments.

#![allow(clippy::panic)]

use bytes::Bytes;
use dassie_identity::NodeKey;
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_payments::SigningKey;
use dassie_payments::httpsig;
use dassie_payments::jwk::Jwk;
use http_body_util::Full;
use hyper::{Request, StatusCode};
use serde_json::{Value, json};

use super::btp_client::{BtpClient, send_stream, send_stream_reported};
use super::{Bound, Reply, START_MS, TestNode, send, set_up};

pub const CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/root.pem");
pub const CERT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/cert.pem");
pub const KEY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testdata/tls/key.pem");

/// A client of the node's Open Payments: its key and how it names itself.
pub struct Client {
    pub key: SigningKey,
    pub kid: String,
    pub identity: Value,
}

impl Client {
    /// A client known only by its key.
    pub fn keyed(seed: u8) -> Self {
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
pub async fn start() -> (TestNode, NodeKey) {
    start_with(Policies::DEV).await
}

pub async fn start_with(policies: Policies) -> (TestNode, NodeKey) {
    start_keyed(policies, 31).await
}

/// As [`start`], with the node key made from `seed`: for a second node.
pub async fn start_keyed(policies: Policies, seed: u8) -> (TestNode, NodeKey) {
    let key = NodeKey::from_bytes(&[seed; 32]);
    let env = [
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 3, policies, &env)
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
    add_stub(&node).await;
    (node, key)
}

/// A node without a hostname, with the node key made from `seed`, the
/// stub ledger, and `names` added to its default one: an Open Payments
/// server that clients reach through front doors.
pub async fn start_unnamed(seed: u8, names: &str) -> (TestNode, NodeKey) {
    let key = NodeKey::from_bytes(&[seed; 32]);
    let env = [
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_PUBLIC_NAMES", names),
    ];
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 3, Policies::DEV, &env)
        .await;
    set_up(&node, &key).await;
    add_stub(&node).await;
    (node, key)
}

/// `<label>.dassiepay.net`: the name of the node with `key` at the default
/// front door.
pub fn default_name(key: &NodeKey) -> String {
    format!("{}.dassiepay.net", key.node_id().label().unwrap())
}

/// Adds the stub ledger, with money in it, once the engine runs.
pub async fn add_stub(node: &TestNode) {
    node.engine_started().await;
    let mut owner = node.ipc().await;
    let added = owner
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: "stub".into(),
                config: String::new(),
            },
        ))
        .await;
    assert!(added.is_ok(), "the stub scheme did not start: {added:?}");
    owner
        .call(Method::StubDeposit(proto::StubDepositRequest {
            amount: 100_000_000_000,
        }))
        .await
        .unwrap();
}

/// A request to the node, signed by `client` with `token` if given.
pub fn signed(
    node: &TestNode,
    client: &Client,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Request<Full<Bytes>> {
    let at = (node.origin(), node.host());
    signed_at(node, &at, client, method, path, token, body)
}

/// A request to the node under `name`, as a front door passes it on,
/// signed by `client` for `https://<name>`.
pub fn signed_under(
    node: &TestNode,
    name: &str,
    client: &Client,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Request<Full<Bytes>> {
    let at = (format!("https://{name}"), name.to_string());
    signed_at(node, &at, client, method, path, token, body)
}

/// A request signed for `origin` with `Host: host`.
fn signed_at(
    node: &TestNode,
    (origin, host): &(String, String),
    client: &Client,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Request<Full<Bytes>> {
    let url = format!("{origin}{path}");
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
        .header("host", host.as_str())
        .header("signature-input", signature.signature_input)
        .header("signature", signature.signature);
    for (name, value) in &fields {
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .unwrap()
}

pub fn get(node: &TestNode, path: &str) -> Request<Full<Bytes>> {
    get_under(&node.host(), path)
}

/// A GET for `path` with `Host: host`.
pub fn get_under(host: &str, path: &str) -> Request<Full<Bytes>> {
    Request::get(path)
        .header("host", host)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

pub fn body(reply: &Reply) -> Value {
    serde_json::from_slice(&reply.body).unwrap_or_else(|_| panic!("not JSON: {}", reply.text()))
}

/// A grant for `access`; returns its access token.
pub async fn grant(node: &TestNode, client: &Client, access: Value) -> String {
    full_grant(node, client, access).await["access_token"]["value"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A grant for `access`, as the authorization server answered.
pub async fn full_grant(node: &TestNode, client: &Client, access: Value) -> Value {
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
    grant
}

/// The path of a URL of the node.
pub fn path_of(node: &TestNode, url: &Value) -> String {
    url.as_str()
        .unwrap()
        .strip_prefix(&node.origin())
        .unwrap()
        .to_string()
}

/// Creates an incoming payment as `client`; returns it.
pub async fn create(node: &TestNode, client: &Client, token: &str) -> Value {
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let reply = send(
        node.https,
        true,
        signed(
            node,
            client,
            "POST",
            "/incoming-payments",
            Some(token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    body(&reply)
}

pub fn incoming_access(node: &TestNode) -> Value {
    json!([{
        "type": "incoming-payment",
        "actions": ["create", "read", "list", "complete"],
        "identifier": format!("{}/.well-known/pay", node.origin()),
    }])
}

/// Pays `amount` to a STREAM destination from a BTP client of the node,
/// and returns what arrived.
pub async fn pay(node: &TestNode, destination: &str, secret: &[u8], amount: u64) -> u64 {
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

/// As [`pay`], returning also what the receiver told the sender arrived.
pub async fn pay_reported(
    node: &TestNode,
    destination: &str,
    secret: &[u8],
    amount: u64,
) -> (u64, u64) {
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
    send_stream_reported(&node.clock, &mut client, destination, secret, amount, 2_000).await
}

pub fn base64_url(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .unwrap()
}

//! A front door on loopback (spec/areas/http.md#front-doors): a node
//! without a public address names it as its relay, and Open Payments
//! clients reach the node at `https://<label>.dassiepay.net` through the
//! front door's HTTPS, with the node's signature intact.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

mod support;

use std::future::Future;
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::{NodeId, NodeKey};
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::front_door::nodes::STREAMS_PER_NODE;
use dassie_node::front_door::{self, CertificateFiles, Parts, Sockets, key};
use dassie_node::http::open_payments::MAX_BODY_BYTES;
use dassie_node::http::peer::Answer;
use dassie_node::node::Policies;
use dassie_node::peer_client::PeerClient;
use dassie_node::quic::relay::{KeepWhile, Purpose, RelayError, ServedStream};
use dassie_node::quic::{Body, Handler, Quic};
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node_core::connections::ConnectionLimits;
use dassie_node_core::rate_limit::ClientKey;
use dassie_payments::httpsig;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, ServerName};
use serde_json::{Value, json};
use support::op::{CA, Client, add_stub, signed_under};
use support::{Bound, Reply, START_MS, TestNode, pair, set_up};
use tokio::sync::watch;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;

const DOMAIN: &str = "dassiepay.net";

/// A front door on loopback, with a certificate for `*.dassiepay.net`
/// from a CA of the test's own.
struct Door {
    https: SocketAddr,
    id: NodeId,
    /// `DASSIE_RELAY` naming it.
    entry: String,
    url: String,
    roots: Arc<rustls::RootCertStore>,
    /// The front door's clock, for its rate limits: polls move it on, so
    /// that they do not use up a client's allowance.
    clock: ManualClock,
    _dir: tempfile::TempDir,
}

impl Door {
    async fn start(shutdown: &CancellationToken) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (certificate, roots) = wildcard(dir.path());
        let https = TcpListener::bind("127.0.0.1:0").unwrap();
        let quic = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = https.local_addr().unwrap();
        let url = format!("https://{}", quic.local_addr().unwrap());
        let door_key = NodeKey::from_bytes(&[90; 32]);
        let id = door_key.node_id();
        let entry = key::relay_entry(&door_key, &url);
        let parts = Parts {
            domain: front_door::settings::parse_domain(DOMAIN).unwrap(),
            key: door_key,
            certificate,
        };
        let sockets = Sockets { https, quic };
        let clock = ManualClock::new(START_MS);
        let serving = front_door::serve(parts, sockets, Arc::new(clock.clone()), shutdown.clone());
        tokio::spawn(async move { serving.await.unwrap() });
        Self {
            https: address,
            id,
            entry,
            url,
            roots: Arc::new(roots),
            clock,
            _dir: dir,
        }
    }

    /// Sends `request` for `name` over a fresh connection, checking the
    /// front door's certificate for that name.
    async fn send(&self, name: &str, request: Request<Full<Bytes>>) -> Reply {
        let config = rustls::ClientConfig::builder_with_provider(dassie_node::tls::provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(Arc::clone(&self.roots))
            .with_no_client_auth();
        let stream = tokio::net::TcpStream::connect(self.https).await.unwrap();
        let server = ServerName::try_from(name.to_string()).unwrap();
        let stream = TlsConnector::from(Arc::new(config))
            .connect(server, stream)
            .await
            .unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let response = sender.send_request(request).await.unwrap();
        let (parts, body) = response.into_parts();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body: body.collect().await.unwrap().to_bytes(),
        }
    }

    /// A GET of `path` under `name`, with a nonce for the node's signature.
    async fn get(&self, name: &str, path: &str) -> Reply {
        let request = Request::get(path)
            .header("host", name)
            .header(httpsig::NONCE_FIELD, NONCE)
            .body(Full::new(Bytes::new()))
            .unwrap();
        self.send(name, request).await
    }

    /// Waits until the node `name` names answers through the front door.
    async fn until_reached(&self, name: &str) {
        for _ in 0..500 {
            if self.get(name, "/.well-known/pay").await.status != StatusCode::SERVICE_UNAVAILABLE {
                return;
            }
            self.clock.advance(1_000);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the node never connected to the front door");
    }
}

const NONCE: &str = "front-door-nonce";

/// A CA, and a certificate it issued for `*.dassiepay.net`, in files
/// under `dir`.
fn wildcard(dir: &Path) -> (CertificateFiles, rustls::RootCertStore) {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_certificate = ca.self_signed(&ca_key).unwrap();
    let issuer = Issuer::from_params(&ca, &ca_key);
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec![format!("*.{DOMAIN}"), DOMAIN.to_string()])
        .unwrap()
        .signed_by(&leaf_key, &issuer)
        .unwrap();
    let files = CertificateFiles {
        chain: dir.join("fullchain.pem"),
        key: dir.join("privkey.pem"),
    };
    std::fs::write(&files.chain, leaf.pem() + &ca_certificate.pem()).unwrap();
    std::fs::write(&files.key, leaf_key.serialize_pem()).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca_certificate.der().to_vec()))
        .unwrap();
    (files, roots)
}

/// `<label>.dassiepay.net`.
fn name_of(id: &NodeId) -> String {
    format!("{}.{DOMAIN}", id.label().unwrap())
}

/// A node without a hostname that names `door` as its relay, with the
/// stub ledger.
async fn node_behind(door: &Door, seed: u8) -> (TestNode, NodeKey) {
    let key = NodeKey::from_bytes(&[seed; 32]);
    let env = [
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_RELAY", door.entry.as_str()),
    ];
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 3, Policies::DEV, &env)
        .await;
    set_up(&node, &key).await;
    add_stub(&node).await;
    (node, key)
}

/// Checks the node's signature on `reply` to a GET of `target` with
/// [`NONCE`], as a Dassie-aware client does: by the label in the URL.
fn signer(target: &str, reply: &Reply, now_s: u64) -> Result<NodeId, httpsig::NodeResponseError> {
    let nonce = [(httpsig::NONCE_FIELD, NONCE)];
    let request = httpsig::Request {
        method: "GET",
        target_uri: target,
        fields: &nonce,
    };
    let fields: Vec<(&str, &str)> = reply
        .headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
        .collect();
    let answer = httpsig::Response {
        status: reply.status.as_u16(),
        fields: &fields,
    };
    httpsig::check_node_response(&request, &answer, &reply.body, None, now_s)
}

async fn status_of(node: &TestNode) -> proto::GetStatusResponse {
    let request = Method::GetStatus(proto::GetStatusRequest {});
    match node.ipc().await.call(request).await {
        Ok(response::Method::GetStatus(status)) => status,
        other => panic!("{other:?}"),
    }
}

fn json_of(reply: &Reply) -> Value {
    serde_json::from_slice(&reply.body).unwrap_or_else(|_| panic!("not JSON: {}", reply.text()))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_is_reached_at_its_name_through_the_front_door() {
    let shutdown = CancellationToken::new();
    let door = Door::start(&shutdown).await;
    let (node, key) = node_behind(&door, 61).await;
    let name = name_of(&key.node_id());
    door.until_reached(&name).await;
    let reply = door.get(&name, "/.well-known/pay").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let target = format!("https://{name}/.well-known/pay");
    let address = json_of(&reply);
    assert_eq!(address["id"], target);
    assert_eq!(
        address["dassie"],
        json!({ "nodeId": key.node_id().as_str() })
    );
    assert_eq!(reply.header("access-control-allow-origin"), Some("*"));
    // The front door's policy replaced the node's, which the node's
    // signature does not cover: it still verifies below.
    assert_eq!(
        reply.header("content-security-policy"),
        Some("default-src 'none'; sandbox")
    );
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    // The answer is the node's, signed with its key: the label checks it.
    let now_s = node.clock.now_ms() / 1_000;
    assert_eq!(signer(&target, &reply, now_s), Ok(key.node_id()));
    // A few requests in a row reuse the node's streams.
    for _ in 0..8 {
        let again = door.get(&name, "/.well-known/pay").await;
        assert_eq!(again.status, StatusCode::OK, "{}", again.text());
    }
    // An error is the node's too, and signed.
    let missing = door.get(&name, "/pay/xrpl").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let target = format!("https://{name}/pay/xrpl");
    assert_eq!(signer(&target, &missing, now_s), Ok(key.node_id()));
    node.stop().await;
    shutdown.cancel();
}

// A periphery node keeps its uplink and its front door apart: it peers
// with the one as its downlink, and is reached through the other, which
// is no uplink (spec/areas/node.md#a-chosen-relay).
#[tokio::test(flavor = "multi_thread")]
async fn a_periphery_node_with_an_uplink_is_reached_through_its_front_door() {
    let shutdown = CancellationToken::new();
    let door = Door::start(&shutdown).await;
    let (key_a, key_p) = (pair::key(66), pair::key(67));
    let (bound_a, bound_p) = (Bound::new(), Bound::new());
    let nodes = pair::bootstrap(&key_a, bound_a.https_port());
    let core = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let mut periphery = core.to_vec();
    periphery.push(("DASSIE_RELAY", door.entry.as_str()));
    let clock = ManualClock::new(START_MS);
    let a = bound_a.start(clock.clone(), 1, Policies::DEV, &core).await;
    let p = bound_p
        .start(clock.clone(), 2, Policies::DEV, &periphery)
        .await;
    pair::prepare(&a, &key_a).await;
    set_up(&p, &key_p).await;
    add_stub(&p).await;
    pair::until(&clock, async || {
        pair::peered(&a, &key_p).await && pair::peered(&p, &key_a).await
    })
    .await;
    let name = name_of(&key_p.node_id());
    door.until_reached(&name).await;
    let reply = door.get(&name, "/.well-known/pay").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let target = format!("https://{name}/.well-known/pay");
    let now_s = p.clock.now_ms() / 1_000;
    assert_eq!(signer(&target, &reply, now_s), Ok(key_p.node_id()));
    // Still its uplink's downlink, beside the front door.
    assert!(pair::peered(&p, &key_a).await);
    // What `dassie status` reads: a periphery node, its relay connected, and
    // its name at the front door; a core node, at its hostname, without one.
    let status = status_of(&p).await;
    assert!(status.periphery);
    let relay = status.relay.unwrap();
    assert_eq!(relay.node, door.id.as_str());
    assert_eq!(relay.url, door.url);
    assert!(relay.connected);
    assert_eq!(status.open_payments_origins, [format!("https://{name}")]);
    let status = status_of(&a).await;
    assert!(!status.periphery);
    assert_eq!(status.relay, None);
    assert_eq!(status.open_payments_origins, [a.origin()]);
    a.stop().await;
    p.stop().await;
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_front_door_serves_only_its_names_and_open_payments() {
    let shutdown = CancellationToken::new();
    let door = Door::start(&shutdown).await;
    let (node, key) = node_behind(&door, 62).await;
    let name = name_of(&key.node_id());
    door.until_reached(&name).await;
    // Paths outside Open Payments, and upgrades, reach no node.
    for path in ["/", "/index.html", "/stats", "/api/login", "/peer", "/rpc"] {
        let reply = door.get(&name, path).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(json_of(&reply)["error"]["code"], "not_found", "{path}");
        assert!(reply.header("signature").is_none(), "{path}");
    }
    let upgrade = Request::get("/.well-known/pay")
        .header("host", &name)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .body(Full::new(Bytes::new()))
        .unwrap();
    assert_eq!(
        door.send(&name, upgrade).await.status,
        StatusCode::NOT_FOUND
    );
    // Names under the domain that are no node's label find nothing.
    let other = NodeKey::from_bytes(&[63; 32]).node_id();
    let deeper = format!("x.{name}");
    for host in ["nope.dassiepay.net", DOMAIN, deeper.as_str()] {
        let request = Request::get("/.well-known/pay")
            .header("host", host)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let reply = door.send(&name, request).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{host}");
        assert_eq!(json_of(&reply)["error"]["code"], "not_found");
    }
    // Names outside it, or on another port, are refused.
    let elsewhere = format!("{}.example.net", key.node_id().label().unwrap());
    let ported = format!("{name}:8443");
    for host in [elsewhere.as_str(), ported.as_str(), "127.0.0.1"] {
        let request = Request::get("/.well-known/pay")
            .header("host", host)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let reply = door.send(&name, request).await;
        assert_eq!(reply.status, StatusCode::MISDIRECTED_REQUEST, "{host}");
    }
    // A node that is not connected cannot be reached now.
    let reply = door.get(&name_of(&other), "/.well-known/pay").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_of(&reply)["error"]["code"], "unavailable");
    // The consent page is the owner app's: the front door answers itself.
    let reply = door.get(&name, "/gnap/interact/grant/key").await;
    assert_eq!(reply.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(json_of(&reply)["error"]["code"], "interaction_unavailable");
    assert!(reply.header("signature").is_none());
    // Once the node stops, it cannot be reached either.
    node.stop().await;
    let mut gone = false;
    for _ in 0..200 {
        let status = door.get(&name, "/.well-known/pay").await.status;
        if status == StatusCode::SERVICE_UNAVAILABLE {
            gone = true;
            break;
        }
        door.clock.advance(1_000);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(gone, "the front door still reaches a node that stopped");
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_incoming_payment_made_through_the_front_door_reads_back() {
    let shutdown = CancellationToken::new();
    let door = Door::start(&shutdown).await;
    let (node, key) = node_behind(&door, 64).await;
    let name = name_of(&key.node_id());
    door.until_reached(&name).await;
    let client = Client::keyed(7);
    let access = json!([{
        "type": "incoming-payment",
        "actions": ["create", "read"],
        "identifier": format!("https://{name}/.well-known/pay"),
    }]);
    let grant = json!({ "access_token": { "access": access }, "client": client.identity });
    let request = signed_under(&node, &name, &client, "POST", "/gnap", None, Some(&grant));
    let reply = door.send(&name, request).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let token = json_of(&reply)["access_token"]["value"]
        .as_str()
        .unwrap()
        .to_string();
    let create = json!({ "walletAddress": format!("https://{name}/.well-known/pay") });
    let request = signed_under(
        &node,
        &name,
        &client,
        "POST",
        "/incoming-payments",
        Some(&token),
        Some(&create),
    );
    let reply = door.send(&name, request).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let created = json_of(&reply);
    let id = created["id"].as_str().unwrap();
    let path = id.strip_prefix(&format!("https://{name}")).unwrap();
    let request = signed_under(&node, &name, &client, "GET", path, Some(&token), None);
    let reply = door.send(&name, request).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let read = json_of(&reply);
    assert_eq!(read["id"], created["id"]);
    assert_eq!(read["walletAddress"], created["walletAddress"]);
    // The node's GNAP challenge passes, and its answer verifies.
    let reply = door.get(&name, "/incoming-payments").await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    assert_eq!(
        reply.header("www-authenticate"),
        Some(format!("GNAP as_uri=https://{name}/gnap").as_str())
    );
    let target = format!("https://{name}/incoming-payments");
    let now_s = node.clock.now_ms() / 1_000;
    assert_eq!(signer(&target, &reply, now_s), Ok(key.node_id()));
    // A body over the node's bound never reaches it.
    let large = Request::post("/incoming-payments")
        .header("host", &name)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(vec![b' '; MAX_BODY_BYTES + 1])))
        .unwrap();
    let reply = door.send(&name, large).await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(reply.header("signature").is_none());
    node.stop().await;
    shutdown.cancel();
}

/// A node that serves whatever the path asks for on a relayed stream:
/// a page, a JSON answer with a cookie, or one over the front door's
/// bound. A real node never does; the front door must not pass it on.
struct Misbehaving;

impl Handler for Misbehaving {
    fn client(&self, address: IpAddr) -> Option<ClientKey> {
        Some(ClientKey::from_ip(address))
    }

    fn limits(&self) -> ConnectionLimits {
        ConnectionLimits {
            max_total: 16,
            max_per_client: 16,
        }
    }

    fn answer<'a>(
        &'a self,
        _client: IpAddr,
        _body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async {
            Answer {
                status: 200,
                body: Vec::new(),
            }
        })
    }

    fn serves(&self, purpose: Purpose) -> bool {
        purpose == Purpose::Payments
    }

    fn serve_relayed<'a>(
        &'a self,
        _purpose: Purpose,
        _client: NodeId,
        _relay: SocketAddr,
        stream: ServedStream,
        _shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let service = hyper::service::service_fn(misbehave);
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        })
    }
}

/// What a misbehaving node answers, by path: a page, a page behind a
/// JSON type, a redirect, an answer over the front door's bound, a sign-in
/// dialog, a slow answer, or JSON with its own lax policy; and a cookie
/// with each.
async fn misbehave(
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let page = || Bytes::from_static(b"<script>steal()</script>");
    let builder = Response::builder().header("set-cookie", "session=stolen");
    let response = match request.uri().path() {
        "/pay/page" => builder.header("content-type", "text/html").body(page()),
        "/pay/two-types" => builder
            .header("content-type", "application/json")
            .header("content-type", "text/html")
            .body(page()),
        "/pay/redirect" => builder
            .status(StatusCode::FOUND)
            .header("location", "https://elsewhere.example/")
            .body(Bytes::new()),
        "/pay/basic" => builder
            .status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", "Basic realm=\"Sign in to Dassie\"")
            .body(Bytes::new()),
        "/pay/mixed" => builder
            .status(StatusCode::UNAUTHORIZED)
            .header("www-authenticate", "GNAP as_uri=https://a.example/gnap")
            .header("www-authenticate", "Basic realm=\"Sign in to Dassie\"")
            .body(Bytes::new()),
        "/pay/huge" => builder
            .header("content-type", "application/json")
            .body(Bytes::from(vec![b' '; 1024 * 1024])),
        path => {
            if path == "/pay/slow" {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            builder
                .header("content-type", "application/json; charset=utf-8")
                .header("content-security-policy", "default-src *")
                .header("x-content-type-options", "none")
                .header("alt-svc", "h3=\":443\"")
                .header(
                    "content-disposition",
                    "attachment; filename=\"invoice.pdf.exe\"",
                )
                .header("x-node-says", "hello")
                .body(Bytes::from_static(b"{}"))
        }
    };
    Ok(response.unwrap().map(Full::new))
}

#[tokio::test]
async fn the_front_door_passes_back_only_json_without_cookies() {
    let shutdown = CancellationToken::new();
    let door = Door::start(&shutdown).await;
    let key = Arc::new(NodeKey::from_bytes(&[65; 32]));
    let id = key.node_id();
    let (_identity, keys) = watch::channel(Some(key));
    let quic = Arc::new(Quic::new(UdpSocket::bind("127.0.0.1:0").unwrap(), keys, true).unwrap());
    tokio::spawn(Arc::clone(&quic).serve(Arc::new(Misbehaving), shutdown.clone()));
    let roots = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/pebble/root.pem"
    );
    let resolver = PeerClient::new(Some(Path::new(roots)), true).unwrap();
    tokio::spawn(Arc::clone(&quic).keep_relay(
        door.id,
        door.url.clone(),
        resolver.clone(),
        KeepWhile::Always,
        shutdown.clone(),
    ));
    let name = name_of(&id);
    door.until_reached(&name).await;
    let reply = door.get(&name, "/pay/fine").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.body.as_ref(), b"{}");
    assert!(reply.header("set-cookie").is_none());
    for dropped in ["alt-svc", "content-disposition", "x-node-says"] {
        assert!(reply.header(dropped).is_none(), "{dropped}");
    }
    // The front door's own type and policy replace the node's.
    let all = |name: &str| -> Vec<&str> {
        reply
            .headers
            .get_all(name)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect()
    };
    assert_eq!(all("content-type"), ["application/json"]);
    assert_eq!(
        all("content-security-policy"),
        ["default-src 'none'; sandbox"]
    );
    assert_eq!(all("x-content-type-options"), ["nosniff"]);
    // A redirect goes nowhere: its `Location` stays behind.
    let reply = door.get(&name, "/pay/redirect").await;
    assert_eq!(reply.status, StatusCode::FOUND);
    assert!(reply.header("location").is_none());
    // No sign-in dialog of a node's, alone or beside GNAP's challenge.
    for path in ["/pay/basic", "/pay/mixed"] {
        let reply = door.get(&name, path).await;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{path}");
        assert!(reply.header("www-authenticate").is_none(), "{path}");
    }
    for path in ["/pay/page", "/pay/two-types", "/pay/huge"] {
        let reply = door.get(&name, path).await;
        assert_eq!(reply.status, StatusCode::BAD_GATEWAY, "{path}");
        assert_eq!(json_of(&reply)["error"]["code"], "bad_gateway");
    }
    // More requests at once than the node has streams wait for one.
    let parallel = (0..STREAMS_PER_NODE + 2).map(|_| door.get(&name, "/pay/slow"));
    for reply in futures_util::future::join_all(parallel).await {
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    }
    // The front door carries no relay requests: it is no node's relay but
    // its own.
    let (_other, other_keys) = watch::channel(Some(Arc::new(NodeKey::from_bytes(&[66; 32]))));
    let client =
        Arc::new(Quic::new(UdpSocket::bind("127.0.0.1:0").unwrap(), other_keys, true).unwrap());
    tokio::spawn(Arc::clone(&client).serve(Arc::new(Misbehaving), shutdown.clone()));
    let relayed = client
        .open_relayed((door.id, &door.url), id, Purpose::Payments, &resolver)
        .await;
    assert_eq!(relayed.err(), Some(RelayError::Refused));
    shutdown.cancel();
}

//! A front door on loopback (spec/areas/http.md#front-doors): a node
//! without a public address names it as its relay, and Open Payments
//! clients reach the node at `https://<node ID>.dassiepay.net` through the
//! front door's HTTPS, with the node's signature intact, and at
//! `https://<name>.dassiepay.net` for a name the node holds, as the front
//! door reads the registrars' lists; and over a tunnel at `/quic` on the
//! relay's host where the node's UDP does not reach the front door.

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
use dassie_identity::{IdentityKey, NodeId};
use dassie_kit::ClientKey;
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::env::Env;
use dassie_node::front_door::nodes::STREAMS_PER_NODE;
use dassie_node::front_door::relay::NODES_PER_ADDRESS;
use dassie_node::front_door::{self, CertificateFiles, Parts, Registrars, Sockets, key};
use dassie_node::http::open_payments::MAX_BODY_BYTES;
use dassie_node::node::Policies;
use dassie_node::outbound::Resolver;
use dassie_node::peer_server::Answer;
use dassie_node::quic::relay::{KeepWhile, Purpose, RelayError, ServedStream};
use dassie_node::quic::tunnel::{self, TunnelBounds};
use dassie_node::quic::{Body, Handler, Quic};
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node_core::connections::ConnectionLimits;
use dassie_payments::httpsig;
use dassie_peer::wire::Endpoint;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, ServerName};
use serde_json::{Value, json};
use support::op::{CA, Client, add_stub, signed_under};
use support::{Bound, HANG, Reply, START_MS, TestNode, eventually, https_and_quic, pair, set_up};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest};
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
    /// that they do not use up a client's allowance. Some tests share it
    /// with their nodes, and then a poll moves them on too.
    clock: ManualClock,
    _dir: tempfile::TempDir,
    /// Where its URL leads over UDP when that drops everything.
    _blackhole: Option<UdpSocket>,
}

/// A front door's sockets, and the URL nodes dial it at.
struct Listening {
    https: TcpListener,
    quic: UdpSocket,
    url: String,
    blackhole: Option<UdpSocket>,
}

impl Listening {
    /// HTTPS and QUIC on ports of their own, with the URL at QUIC's.
    fn apart() -> Self {
        let quic = UdpSocket::bind("127.0.0.1:0").unwrap();
        Self {
            https: TcpListener::bind("127.0.0.1:0").unwrap(),
            url: format!("https://{}", quic.local_addr().unwrap()),
            quic,
            blackhole: None,
        }
    }

    /// The URL at the HTTPS port, whose UDP socket drops everything, as a
    /// network that blocks UDP does, while QUIC listens elsewhere: the
    /// tunnels at `/quic` are the only way in.
    fn udp_blocked() -> Self {
        let (https, blackhole) = https_and_quic();
        Self {
            url: format!("https://{}", https.local_addr().unwrap()),
            https,
            quic: UdpSocket::bind("127.0.0.1:0").unwrap(),
            blackhole: Some(blackhole),
        }
    }
}

impl Door {
    /// A front door that reads no registrar's names, on a clock of its own.
    async fn start(shutdown: &CancellationToken) -> Self {
        let registrars = Registrars {
            nodes: Vec::new(),
            config: dassie_peer::Config::DEV,
        };
        Self::start_with(shutdown, registrars, ManualClock::new(START_MS)).await
    }

    /// A front door that reads the names `registrars` list, on `clock`.
    async fn start_with(
        shutdown: &CancellationToken,
        registrars: Registrars,
        clock: ManualClock,
    ) -> Self {
        let listening = Listening::apart();
        Self::start_on(
            shutdown,
            registrars,
            clock,
            listening,
            TunnelBounds::DEFAULT,
        )
        .await
    }

    /// A front door that reads no registrar's names, at a URL whose UDP
    /// drops everything, whose tunnels have `bounds`.
    async fn start_udp_blocked(shutdown: &CancellationToken, bounds: TunnelBounds) -> Self {
        let registrars = Registrars {
            nodes: Vec::new(),
            config: dassie_peer::Config::DEV,
        };
        let clock = ManualClock::new(START_MS);
        Self::start_on(
            shutdown,
            registrars,
            clock,
            Listening::udp_blocked(),
            bounds,
        )
        .await
    }

    /// A front door on `listening` that reads the names `registrars` list,
    /// on `clock`, whose tunnels have `bounds`.
    async fn start_on(
        shutdown: &CancellationToken,
        registrars: Registrars,
        clock: ManualClock,
        listening: Listening,
        bounds: TunnelBounds,
    ) -> Self {
        let Listening {
            https,
            quic,
            url,
            blackhole,
        } = listening;
        let dir = tempfile::tempdir().unwrap();
        let (certificate, roots) = wildcard(dir.path());
        let address = https.local_addr().unwrap();
        let door_key = IdentityKey::from_bytes(&[90; 32]);
        let id = door_key.node_id();
        let entry = key::relay_entry(&door_key, &url);
        let parts = Parts {
            domain: front_door::settings::parse_domain(DOMAIN).unwrap(),
            key: door_key,
            certificate,
            relay_url: url.clone(),
            registrars,
            allow_private: true,
            tunnel_bounds: bounds,
        };
        let sockets = Sockets { https, quic };
        let time = Arc::new(clock.clone()) as Arc<dyn Clock>;
        let serving = front_door::serve(parts, sockets, time, shutdown.clone());
        tokio::spawn(async move { serving.await.unwrap() });
        Self {
            https: address,
            id,
            entry,
            url,
            roots: Arc::new(roots),
            clock,
            _dir: dir,
            _blackhole: blackhole,
        }
    }

    /// The `Host` nodes dial the front door at as their relay.
    fn relay_host(&self) -> &str {
        self.url.strip_prefix("https://").unwrap()
    }

    /// A fresh TLS connection, checking the front door's certificate for
    /// `name`; none if the front door closes it, as it does a client's
    /// past its share of connections. It does so on arrival, and the
    /// handshake fails at once, and one it takes completes: the timeout is
    /// a hang's guard ([`HANG`]), never what decides.
    async fn try_tls(&self, name: &str) -> Option<TlsStream<tokio::net::TcpStream>> {
        let config = rustls::ClientConfig::builder_with_provider(dassie_node::tls::provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(Arc::clone(&self.roots))
            .with_no_client_auth();
        let stream = tokio::net::TcpStream::connect(self.https).await.ok()?;
        let server = ServerName::try_from(name.to_string()).unwrap();
        let connect = TlsConnector::from(Arc::new(config)).connect(server, stream);
        tokio::time::timeout(HANG, connect)
            .await
            .expect("a TLS handshake hung")
            .ok()
    }

    /// Opens a tunnel for QUIC at `/quic` on the relay's host; the status
    /// of a refusal. The place of the connection upgraded before comes back
    /// once the front door has seen it go, which the IO layer does in its
    /// own time, so this tries for one until a hang ([`HANG`]).
    async fn tunnel(
        &self,
    ) -> Result<WebSocketStream<TlsStream<tokio::net::TcpStream>>, StatusCode> {
        let relay = format!("relay.{DOMAIN}");
        let stream = eventually("a place for an HTTPS connection", async || {
            self.try_tls(&relay).await
        })
        .await;
        let mut request = format!("wss://{}{}", self.relay_host(), tunnel::PATH)
            .into_client_request()
            .unwrap();
        let value = tunnel::SUBPROTOCOL.parse().unwrap();
        request
            .headers_mut()
            .insert("sec-websocket-protocol", value);
        let config = Some(tunnel::websocket_config());
        match tokio_tungstenite::client_async_with_config(request, stream, config).await {
            Ok((socket, _)) => Ok(socket),
            Err(tungstenite::Error::Http(response)) => Err(response.status()),
            Err(error) => panic!("{error}"),
        }
    }

    /// Sends `request` for `name` over a fresh connection, checking the
    /// front door's certificate for that name.
    async fn send(&self, name: &str, request: Request<Full<Bytes>>) -> Reply {
        let stream = self.try_tls(name).await.unwrap();
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
    /// Its connection to the front door is the IO layer's, made in real
    /// time (a QUIC handshake, and the backoff between tries), so this
    /// waits for it until a hang ([`HANG`]).
    async fn until_reached(&self, name: &str) {
        eventually("the node connecting to the front door", async || {
            let status = self.get(name, "/.well-known/pay").await.status;
            if status != StatusCode::SERVICE_UNAVAILABLE {
                return Some(());
            }
            self.clock.advance(1_000);
            tokio::time::sleep(Duration::from_millis(10)).await;
            None
        })
        .await;
    }

    /// Waits until the front door has seen a node that stopped go: a
    /// request for `path` under `name` gives what `gone` looks for. The
    /// node's connection closes as the IO layer closes it, in real time,
    /// so this waits until a hang ([`HANG`]); each try moves the front
    /// door's clock on, as [`until_reached`](Self::until_reached) does.
    async fn until_gone(&self, name: &str, path: &str, gone: impl Fn(&Reply) -> bool) -> Reply {
        eventually("the front door seeing the node go", async || {
            let reply = self.get(name, path).await;
            if gone(&reply) {
                return Some(reply);
            }
            self.clock.advance(1_000);
            tokio::time::sleep(Duration::from_millis(40)).await;
            None
        })
        .await
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

/// `<node ID>.dassiepay.net`.
fn name_of(id: &NodeId) -> String {
    format!("{}.{DOMAIN}", id)
}

/// A node without a hostname that names `door` as its relay, with the
/// stub ledger.
async fn node_behind(door: &Door, seed: u8) -> (TestNode, IdentityKey) {
    let key = IdentityKey::from_bytes(&[seed; 32]);
    let env = [
        ("DASSIE_EXTRA_CA_FILE", CA),
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
    let missing = door.get(&name, "/pay/xrp").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    let target = format!("https://{name}/pay/xrp");
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
        ("DASSIE_EXTRA_CA_FILE", CA),
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
        pair::peered_downlink(&a, &key_p).await && pair::peered(&p, &key_a).await
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
    assert_eq!(
        relay.connection,
        i32::from(proto::RelayConnection::Connected)
    );
    assert_eq!(status.open_payments_origins, [format!("https://{name}")]);
    let status = status_of(&a).await;
    assert!(!status.periphery);
    assert_eq!(status.relay, None);
    assert_eq!(status.open_payments_origins, [a.origin()]);
    a.stop().await;
    p.stop().await;
    shutdown.cancel();
}

/// A tunnel's WebSocket handshake at `/quic` for `host`, with the tunnel's
/// subprotocol or without.
fn tunnel_handshake(host: &str, subprotocol: bool) -> Request<Full<Bytes>> {
    let mut request = Request::get(tunnel::PATH)
        .header("host", host)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    if subprotocol {
        request = request.header("sec-websocket-protocol", tunnel::SUBPROTOCOL);
    }
    request.body(Full::new(Bytes::new())).unwrap()
}

// spec/areas/http.md#front-doors: a node whose UDP does not reach its
// front door keeps it as its relay over a tunnel at `/quic` on the relay's
// host, and is reached through it as over UDP. Under every other name the
// front door answers `/quic` as any path outside Open Payments.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_whose_udp_is_blocked_keeps_its_front_door_over_a_tunnel() {
    let shutdown = CancellationToken::new();
    let door = Door::start_udp_blocked(&shutdown, TunnelBounds::DEFAULT).await;
    let (node, key) = node_behind(&door, 70).await;
    let name = name_of(&key.node_id());
    door.until_reached(&name).await;
    let reply = door.get(&name, "/.well-known/pay").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let target = format!("https://{name}/.well-known/pay");
    let now_s = node.clock.now_ms() / 1_000;
    assert_eq!(signer(&target, &reply, now_s), Ok(key.node_id()));
    // Its connection to the front door is over the tunnel, the only way in.
    assert_eq!(node.quic.as_ref().unwrap().open_tunnels(), 1);
    let relay = status_of(&node).await.relay.unwrap();
    assert_eq!(relay.node, door.id.as_str());
    // On the relay's host, `/quic` is a tunnel's WebSocket and nothing else.
    let relay_host = door.relay_host();
    let reply = door.send(&name, tunnel_handshake(relay_host, false)).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(json_of(&reply)["error"]["code"], "invalid_request");
    let plain = Request::get(tunnel::PATH)
        .header("host", relay_host)
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = door.send(&name, plain).await;
    assert_eq!(reply.status, StatusCode::UPGRADE_REQUIRED);
    let broken = Request::get(tunnel::PATH)
        .header("host", relay_host)
        .header("connection", "upgrade")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = door.send(&name, broken).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    // Under a node's name it is a path outside Open Payments, and on
    // another port a host the front door does not serve: no tunnel.
    let ported = format!("{}:{}", relay_host.rsplit_once(':').unwrap().0, 443);
    for (host, status) in [
        (name.as_str(), StatusCode::NOT_FOUND),
        (ported.as_str(), StatusCode::MISDIRECTED_REQUEST),
    ] {
        let reply = door.send(&name, tunnel_handshake(host, true)).await;
        assert_eq!(reply.status, status, "{host}");
    }
    // Its requests still reach the node over the tunnel.
    let again = door.get(&name, "/.well-known/pay").await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.text());
    node.stop().await;
    shutdown.cancel();
}

// spec/areas/http.md#front-doors: as at a node's listener, an upgraded
// tunnel leaves the front door's HTTPS places and holds the place of the
// QUIC connection it carries, first one among those being set up, by its
// client address. So a client holds as many tunnels as it may set up QUIC
// connections, whatever its share of HTTPS connections; past that it is
// 503, until a tunnel ends and gives its place back. A browser's request,
// which carries an origin, is refused with 403, and holds no place.
#[tokio::test(flavor = "multi_thread")]
async fn tunnels_at_the_front_door_hold_the_places_of_their_connections() {
    let shutdown = CancellationToken::new();
    // Bounds as long as a hang ([`HANG`]), so that each tunnel holds its
    // place until the test ends it. They are real time, and that a tunnel
    // without a connection ends by them is `tests/tunnel.rs`'s to check.
    let held = TunnelBounds {
        first_packet: HANG,
        connect: HANG,
    };
    let door = Door::start_udp_blocked(&shutdown, held).await;
    let (mut browsers, body) = tunnel_handshake(door.relay_host(), true).into_parts();
    let origin = "https://page.example".parse().unwrap();
    browsers.headers.insert("origin", origin);
    let relay = format!("relay.{DOMAIN}");
    let reply = door.send(&relay, Request::from_parts(browsers, body)).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(json_of(&reply)["error"]["code"], "forbidden");
    // At least as many as the client's HTTPS connections, so the request
    // past them needs a place the upgraded ones gave back.
    const _: () = assert!(NODES_PER_ADDRESS >= Policies::DEFAULT.http_connections_per_client);
    let mut tunnels = Vec::new();
    for _ in 0..NODES_PER_ADDRESS {
        tunnels.push(door.tunnel().await.unwrap());
    }
    assert_eq!(
        door.tunnel().await.err(),
        Some(StatusCode::SERVICE_UNAVAILABLE)
    );
    // A tunnel its client ends gives its place back, once the front door
    // has seen it go, in the IO layer's own time.
    drop(tunnels.pop());
    eventually("a tunnel's place coming back", async || {
        door.tunnel().await.ok()
    })
    .await;
    drop(tunnels);
    shutdown.cancel();
}

/// Each of `hosts` with a wallet address's path and with the root.
fn hosts_and_paths<'a>(hosts: &[&'a str]) -> Vec<(&'a str, &'static str)> {
    hosts
        .iter()
        .flat_map(|host| [(*host, "/.well-known/pay"), (*host, "/")])
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_front_door_serves_only_its_names_and_open_payments() {
    let shutdown = CancellationToken::new();
    let door = Door::start(&shutdown).await;
    let (node, key) = node_behind(&door, 62).await;
    let name = name_of(&key.node_id());
    door.until_reached(&name).await;
    // Paths outside Open Payments, and upgrades, reach no node. The root
    // is the front door's own page, but only for GET and HEAD.
    for path in [
        "/index.html",
        "/.well-known/dassie/node",
        "/api/login",
        "/rpc",
    ] {
        let reply = door.get(&name, path).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(json_of(&reply)["error"]["code"], "not_found", "{path}");
        assert!(reply.header("signature").is_none(), "{path}");
    }
    for method in ["POST", "PUT", "DELETE", "OPTIONS"] {
        let request = Request::builder()
            .method(method)
            .uri("/")
            .header("host", &name)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let reply = door.send(&name, request).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{method}");
        assert_eq!(json_of(&reply)["error"]["code"], "not_found", "{method}");
    }
    let upgrade = Request::get("/")
        .header("host", &name)
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .body(Full::new(Bytes::new()))
        .unwrap();
    assert_eq!(
        door.send(&name, upgrade).await.status,
        StatusCode::NOT_FOUND
    );
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
    let other = IdentityKey::from_bytes(&[63; 32]).node_id();
    let deeper = format!("x.{name}");
    // Their roots show no page either.
    for (host, path) in hosts_and_paths(&["nope.dassiepay.net", DOMAIN, deeper.as_str()]) {
        let request = Request::get(path)
            .header("host", host)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let reply = door.send(&name, request).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{host}{path}");
        assert_eq!(json_of(&reply)["error"]["code"], "not_found");
    }
    // Names outside it, or on another port, are refused.
    let elsewhere = format!("{}.example.net", key.node_id());
    let ported = format!("{name}:8443");
    for (host, path) in hosts_and_paths(&[elsewhere.as_str(), ported.as_str(), "127.0.0.1"]) {
        let request = Request::get(path)
            .header("host", host)
            .body(Full::new(Bytes::new()))
            .unwrap();
        let reply = door.send(&name, request).await;
        assert_eq!(
            reply.status,
            StatusCode::MISDIRECTED_REQUEST,
            "{host}{path}"
        );
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
    door.until_gone(&name, "/.well-known/pay", |reply| {
        reply.status == StatusCode::SERVICE_UNAVAILABLE
    })
    .await;
    shutdown.cancel();
}

/// Checks that `reply` is the front door's page about `id`, connected or
/// not, and returns its text.
fn page_of(reply: &Reply, id: &NodeId, connected: bool) -> String {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(
        reply.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        reply.header("content-security-policy"),
        Some(front_door::page::CONTENT_SECURITY_POLICY)
    );
    let policy = front_door::page::CONTENT_SECURITY_POLICY;
    assert!(policy.starts_with("default-src 'none';"), "{policy}");
    assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
    assert!(policy.ends_with("; sandbox"), "{policy}");
    assert!(!policy.contains("script-src"), "{policy}");
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    for absent in ["set-cookie", "signature", "access-control-allow-origin"] {
        assert!(reply.header(absent).is_none(), "{absent}");
    }
    let page = reply.text();
    let name = name_of(id);
    assert!(page.starts_with("<!doctype html>"));
    assert!(!page.to_ascii_lowercase().contains("<script"));
    assert!(page.contains(&format!("<code>{name}</code>")), "{page}");
    assert!(page.contains(&format!("<code>{}</code>", id.as_str())));
    let address = format!("https://{name}/.well-known/pay");
    assert!(page.contains(&format!("<a href=\"{address}\">{address}</a>")));
    assert!(page.contains("<a href=\"https://dassie.land\">"));
    assert!(page.contains("address of the Dassie node with the ID below"));
    assert!(page.contains("Open Payments only"));
    assert!(page.contains("own wallet, not here"));
    let state = if connected {
        "Connected to this front door now."
    } else {
        "Not connected to this front door now."
    };
    assert!(page.contains(state), "{page}");
    page
}

#[tokio::test(flavor = "multi_thread")]
async fn the_root_of_a_name_is_a_page_the_front_door_writes() {
    let shutdown = CancellationToken::new();
    let door = Door::start(&shutdown).await;
    let (node, key) = node_behind(&door, 68).await;
    let id = key.node_id();
    let name = name_of(&id);
    door.until_reached(&name).await;
    page_of(&door.get(&name, "/").await, &id, true);
    // The same page whatever the client accepts, and under the name in
    // capitals, which DNS does not tell apart.
    let request = Request::get("/?q=1")
        .header("host", name.to_ascii_uppercase())
        .header("accept", "application/json")
        .body(Full::new(Bytes::new()))
        .unwrap();
    page_of(&door.send(&name, request).await, &id, true);
    // HEAD has the page's head and no body.
    let head = Request::head("/")
        .header("host", &name)
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = door.send(&name, head).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        reply.header("content-security-policy"),
        Some(front_door::page::CONTENT_SECURITY_POLICY)
    );
    assert!(reply.body.is_empty());
    // A node that never connected has its page too, which says so.
    let other = IdentityKey::from_bytes(&[69; 32]).node_id();
    page_of(&door.get(&name_of(&other), "/").await, &other, false);
    // And so does one that stopped, once the front door has seen it go.
    node.stop().await;
    let reply = door
        .until_gone(&name, "/", |reply| reply.text().contains("Not connected"))
        .await;
    page_of(&reply, &id, false);
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

/// Four bootstrap daemons, the registrars, set up as core nodes with the
/// keys of `seeds`, on `clock`; not started yet, so that a front door can
/// be told where they will be. Returns their listeners and keys, and
/// `DASSIE_BOOTSTRAP_NODES` naming them.
fn registrars(seeds: [u8; 4]) -> (Vec<Bound>, [IdentityKey; 4], String) {
    let keys = seeds.map(pair::key);
    let bounds: Vec<Bound> = (0..4).map(|_| Bound::new()).collect();
    let entries: Vec<String> = keys
        .iter()
        .zip(&bounds)
        .map(|(key, bound)| {
            let one = pair::bootstrap(key, bound.https_port());
            one[1..one.len() - 1].to_string()
        })
        .collect();
    (bounds, keys, format!("[{}]", entries.join(",")))
}

/// Moves `clock` on a second at a time, giving the daemons and the front
/// door real time to talk, until `done` holds.
async fn until(clock: &ManualClock, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..1_500 {
        if done().await {
            return;
        }
        clock.advance(1_000);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("not reached in time");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_reaches_its_holder_through_the_front_door() {
    let shutdown = CancellationToken::new();
    let clock = ManualClock::new(START_MS);
    let (bounds, keys, nodes) = registrars([71, 72, 73, 74]);
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    // The front door reads the registrars as every node is configured with
    // them, on the dev timers the daemons run.
    let configured = Env::from_pairs([("DASSIE_BOOTSTRAP_NODES", nodes.as_str())]);
    let registrars = Registrars {
        nodes: dassie_node::env::bootstrap_nodes(&configured).unwrap(),
        config: dassie_peer::Config::DEV,
    };
    let door = Door::start_with(&shutdown, registrars, clock.clone()).await;
    let mut started = Vec::new();
    for ((bound, key), seed) in bounds.into_iter().zip(&keys).zip(71u64..) {
        let node = bound.start(clock.clone(), seed, Policies::DEV, &env).await;
        pair::prepare(&node, key).await;
        started.push(node);
    }
    // A periphery node, whose relay is the front door.
    let key_p = pair::key(75);
    let id_p = key_p.node_id();
    let mut periphery = env.to_vec();
    periphery.push(("DASSIE_RELAY", door.entry.as_str()));
    let p = Bound::new()
        .start(clock.clone(), 75, Policies::DEV, &periphery)
        .await;
    set_up(&p, &key_p).await;
    add_stub(&p).await;
    door.until_reached(&name_of(&id_p)).await;
    // No node holds `alice` yet: the name finds nothing, as a label that is
    // no node's, at its root too.
    const ALICE: &str = "alice.dassiepay.net";
    for path in ["/.well-known/pay", "/"] {
        let reply = door.get(ALICE, path).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        let description = &json_of(&reply)["error"]["description"];
        assert_eq!(description, "no node has that name", "{path}");
        assert!(reply.header("signature").is_none());
    }
    let registered = p
        .ipc()
        .await
        .call(Method::RegisterName(proto::RegisterNameRequest {
            name: "alice".into(),
        }))
        .await;
    assert!(registered.is_ok(), "{registered:?}");
    until(&clock, async || {
        door.get(ALICE, "/.well-known/pay").await.status == StatusCode::OK
    })
    .await;
    // The holder answers under the name, through the front door, and signs
    // its answer: a client that knows the holder checks it.
    let reply = door.get(ALICE, "/.well-known/pay").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let target = format!("https://{ALICE}/.well-known/pay");
    let address = json_of(&reply);
    assert_eq!(address["id"], target);
    assert_eq!(address["dassie"], json!({ "nodeId": id_p.as_str() }));
    let now_s = clock.now_ms() / 1_000;
    assert_eq!(signed_by(&target, &reply, id_p, now_s), Ok(id_p));
    // The root of the name is the front door's page, about the holder.
    let page = door.get(ALICE, "/").await;
    assert_eq!(page.status, StatusCode::OK);
    let text = page.text();
    assert!(text.contains(&format!("<code>{ALICE}</code>")), "{text}");
    assert!(text.contains(&format!("<code>{}</code>", id_p.as_str())));
    assert!(text.contains("while the network's registrars list it"));
    assert!(text.contains("Connected to this front door now."));
    // Names nobody holds, and labels that are no name, find nothing.
    for host in [
        "bob.dassiepay.net",
        "xn--alice.dassiepay.net",
        "al.dassiepay.net",
    ] {
        let reply = door.get(host, "/.well-known/pay").await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{host}");
    }
    // One registrar of four goes away: three fresh lists are still a
    // quorum, and the name still reaches its holder.
    let gone = started.pop().unwrap();
    gone.stop().await;
    clock.advance(2 * dassie_peer::Config::DEV.list_fresh_ms);
    until(&clock, async || {
        door.get(ALICE, "/.well-known/pay").await.status == StatusCode::OK
    })
    .await;
    // The holder stops, and so renews nothing: once the name expires, the
    // registrars left stop listing it. Only the last list of the one that
    // went away still names the holder, and it is not fresh: no quorum
    // lists the name, and it finds nothing.
    p.stop().await;
    clock.advance(dassie_peer::Config::DEV.names.ttl_ms);
    until(&clock, async || {
        door.get(ALICE, "/.well-known/pay").await.status == StatusCode::NOT_FOUND
    })
    .await;
    for path in ["/.well-known/pay", "/"] {
        let reply = door.get(ALICE, path).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{path}");
        let description = &json_of(&reply)["error"]["description"];
        assert_eq!(description, "no node has that name", "{path}");
    }
    // At once: each waits a moment for its peers to hear it closed.
    futures_util::future::join_all(started.into_iter().map(TestNode::stop)).await;
    shutdown.cancel();
}

/// Checks the node's signature on `reply` to a GET of `target` with
/// [`NONCE`], as a client does that knows the node to expect: for a name,
/// its holder.
fn signed_by(
    target: &str,
    reply: &Reply,
    node: NodeId,
    now_s: u64,
) -> Result<NodeId, httpsig::NodeResponseError> {
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
    httpsig::check_node_response(&request, &answer, &reply.body, Some(node), now_s)
}

/// Moves `clock` on 100 ms at a time, giving the front door real time to
/// work between moves, until `answer` is in: for a request the front door
/// holds until its clock passes a deadline. Gives up after a number of
/// steps of the clock, far past the 2 s it waits for a stream.
async fn until_answered(clock: &ManualClock, answer: impl Future<Output = Reply>) -> Reply {
    let mut answer = std::pin::pin!(answer);
    for _ in 0..1_000 {
        tokio::select! {
            reply = answer.as_mut() => return reply,
            () = tokio::time::sleep(Duration::from_millis(10)) => {}
        }
        clock.advance(100);
    }
    panic!("no answer in 100 s of the front door's clock");
}

/// Sends a GET of `path` under `name` to the front door over a fresh
/// connection, in full, and returns the connection, to read the answer
/// from.
async fn sent(door: &Door, name: &str, path: &str) -> TlsStream<tokio::net::TcpStream> {
    let mut stream = door.try_tls(name).await.unwrap();
    let head = format!("GET {path} HTTP/1.1\r\nhost: {name}\r\nconnection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    stream
}

/// The status line of the answer on `stream`. It comes as the IO layer
/// carries it, so this waits until a hang ([`HANG`]).
async fn status_line(stream: &mut TlsStream<tokio::net::TcpStream>) -> String {
    let read = async {
        let mut head = Vec::new();
        let mut buffer = [0; 256];
        while !head.windows(2).any(|pair| pair == b"\r\n") {
            let read = stream.read(&mut buffer).await.unwrap();
            assert_ne!(read, 0, "the connection closed without an answer");
            head.extend_from_slice(&buffer[..read]);
        }
        let end = head.windows(2).position(|pair| pair == b"\r\n").unwrap();
        String::from_utf8_lossy(&head[..end]).into_owned()
    };
    tokio::time::timeout(HANG, read).await.expect("no answer")
}

/// A node that serves whatever the path asks for on a relayed stream:
/// a page, a JSON answer with a cookie, or one over the front door's
/// bound. A real node never does; the front door must not pass it on.
#[derive(Default)]
struct Misbehaving {
    held: Arc<Held>,
}

/// The requests for `/pay/held` a misbehaving node holds: the first
/// [`STREAMS_PER_NODE`] wait until the test lets them go, each on a stream
/// of its own; any after them are answered at once.
#[derive(Default)]
struct Held {
    /// How many came.
    came: watch::Sender<usize>,
    go: CancellationToken,
}

impl Held {
    async fn hold(&self) {
        let mut came = 0;
        self.came.send_modify(|count| {
            *count += 1;
            came = *count;
        });
        if came <= STREAMS_PER_NODE {
            self.go.cancelled().await;
        }
    }

    /// Waits until the node holds all it holds. They reach it as the IO
    /// layer carries them, in its own time, so until a hang ([`HANG`]).
    async fn until_all_held(&self) {
        let mut came = self.came.subscribe();
        let all = came.wait_for(|count| *count >= STREAMS_PER_NODE);
        tokio::time::timeout(HANG, all)
            .await
            .expect("the requests never reached the node")
            .unwrap();
    }
}

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
        _from: dassie_identity::NodeIdentity,
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
        let held = Arc::clone(&self.held);
        Box::pin(async move {
            let service =
                hyper::service::service_fn(move |request| misbehave(request, Arc::clone(&held)));
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        })
    }
}

/// What a misbehaving node answers, by path: a page, a page behind a
/// JSON type, a redirect, an answer over the front door's bound, a sign-in
/// dialog, an answer it holds ([`Held`]), or JSON with its own lax policy;
/// and a cookie with each.
async fn misbehave(
    request: Request<hyper::body::Incoming>,
    held: Arc<Held>,
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
            if path == "/pay/held" {
                held.hold().await;
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
    let key = Arc::new(IdentityKey::from_bytes(&[65; 32]));
    let id = key.node_id();
    let (_identity, keys) = watch::channel(Some(key));
    let quic = Arc::new(Quic::new(UdpSocket::bind("127.0.0.1:0").unwrap(), keys, true).unwrap());
    let node = Arc::new(Misbehaving::default());
    let held = Arc::clone(&node.held);
    tokio::spawn(Arc::clone(&quic).serve(node, shutdown.clone()));
    let resolver = Resolver::new(true);
    tokio::spawn(Arc::clone(&quic).keep_relay(
        door.id,
        Endpoint::of_url(&door.url).unwrap(),
        resolver,
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
    // More requests at once than the node has streams wait for one, on
    // the front door's clock. The node holds the first, one on each stream
    // it has, and the test waits for that.
    let first = (0..STREAMS_PER_NODE).map(|_| door.get(&name, "/pay/held"));
    let more = async {
        held.until_all_held().await;
        // One that finds every stream busy is answered 503 once the front
        // door's clock passed its time for a stream to come free.
        let busy = until_answered(&door.clock, door.get(&name, "/pay/held")).await;
        assert_eq!(busy.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json_of(&busy)["error"]["description"], "the node is busy");
        // Two more are sent, and then the first let go. The front door's
        // clock stands still from here on, so the two wait for a stream
        // however long their way takes, and get one.
        let mut more = Vec::new();
        for _ in 0..2 {
            more.push(sent(&door, &name, "/pay/held").await);
        }
        held.go.cancel();
        let mut statuses = Vec::new();
        for stream in &mut more {
            statuses.push(status_line(stream).await);
        }
        statuses
    };
    let (first, more) =
        futures_util::future::join(futures_util::future::join_all(first), more).await;
    for reply in &first {
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    }
    for status in more {
        assert!(status.starts_with("HTTP/1.1 200 "), "{status}");
    }
    // The front door carries no relay requests: it is no node's relay but
    // its own.
    let (_other, other_keys) = watch::channel(Some(Arc::new(IdentityKey::from_bytes(&[66; 32]))));
    let client =
        Arc::new(Quic::new(UdpSocket::bind("127.0.0.1:0").unwrap(), other_keys, true).unwrap());
    tokio::spawn(Arc::clone(&client).serve(Arc::new(Misbehaving::default()), shutdown.clone()));
    let relayed = client
        .open_relayed(
            (door.id, &Endpoint::of_url(&door.url).unwrap()),
            id,
            Purpose::Payments,
            &resolver,
        )
        .await;
    assert_eq!(relayed.err(), Some(RelayError::Refused));
    shutdown.cancel();
}

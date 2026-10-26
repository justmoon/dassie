//! A daemon in-process, on loopback ports in a temporary directory, with a
//! manual clock and a seeded RNG; and clients for its HTTPS, WebSocket and
//! IPC sides.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

pub mod btp_client;
pub mod op;
pub mod pair;
pub mod rates;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use dassie_kit::sim::SeededRng;
use dassie_node::clock::ManualClock;
use dassie_node::daemon::{self, DaemonError, Prepared};
use dassie_node::env::{Env, Settings};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, client_message, request, response, server_message};
use dassie_node::store::Store;
use dassie_node::systemd::Notifier;
use dassie_node::{ipc, tls};
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use prost::Message as _;
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio::net::{TcpStream, UnixStream};
use tokio::task::JoinHandle;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tokio_util::sync::CancellationToken;

/// 2027-01-15, a plausible now.
pub const START_MS: u64 = 1_800_000_000_000;

/// A daemon's listeners, bound before it starts, so that other nodes can
/// be told where it will be.
pub struct Bound {
    dir: tempfile::TempDir,
    http: std::net::TcpListener,
    https: std::net::TcpListener,
    ipc_path: PathBuf,
    ipc: std::os::unix::net::UnixListener,
    quic: std::net::UdpSocket,
    /// The rate feed's socket, as systemd would pass it.
    feed: std::net::UdpSocket,
    /// Whether the daemon gets the feed's socket.
    with_feed: bool,
    notify: Option<PathBuf>,
    /// A UDP socket on the HTTPS listener's port that never reads, where
    /// the QUIC endpoint is elsewhere.
    blackhole: Option<std::net::UdpSocket>,
}

/// An HTTPS listener and a UDP socket for QUIC on the same port number, as
/// a node has them. The OS picks the TCP port, which may be taken on UDP:
/// then another is tried.
pub fn https_and_quic() -> (std::net::TcpListener, std::net::UdpSocket) {
    for _ in 0..16 {
        let https = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        if let Ok(quic) = std::net::UdpSocket::bind(https.local_addr().unwrap()) {
            return (https, quic);
        }
    }
    panic!("no port free on both TCP and UDP");
}

impl Bound {
    /// Listeners on fresh ports and a fresh directory.
    pub fn new() -> Self {
        Self::bind_in(tempfile::tempdir().unwrap())
    }

    /// Listeners on fresh ports, for a daemon on `dir`.
    pub fn bind_in(dir: tempfile::TempDir) -> Self {
        let http = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let (https, quic) = https_and_quic();
        let ipc_path = dir.path().join("ipc.sock");
        let ipc = ipc::bind(&ipc_path).unwrap();
        let feed = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        Self {
            dir,
            http,
            https,
            quic,
            feed,
            with_feed: true,
            ipc_path,
            ipc,
            notify: None,
            blackhole: None,
        }
    }

    /// QUIC on another loopback address, `127.0.0.2`, at the HTTPS port
    /// number, so that the node still says it takes QUIC; and on
    /// `127.0.0.1`, where its URL leads, a UDP socket that drops
    /// everything, as a network that blocks UDP does. Its tunnels at
    /// `/quic` are the only way in.
    pub fn udp_blocked(mut self) -> Self {
        let port = self.https_port();
        let quic = std::net::UdpSocket::bind(("127.0.0.2", port)).unwrap();
        self.blackhole = Some(std::mem::replace(&mut self.quic, quic));
        self
    }

    /// Where the daemon's rate feed will take datagrams.
    pub fn feed_address(&self) -> SocketAddr {
        self.feed.local_addr().unwrap()
    }

    /// The daemon gets no socket for the rate feed, as under systemd
    /// without `dassie-feed.socket`.
    pub fn without_feed(mut self) -> Self {
        self.with_feed = false;
        self
    }

    /// The daemon reports readiness to the datagram socket at `path`, as to
    /// systemd's.
    pub fn notifying(mut self, path: &std::path::Path) -> Self {
        self.notify = Some(path.to_owned());
        self
    }

    /// The HTTPS port the daemon will have.
    pub fn https_port(&self) -> u16 {
        self.https.local_addr().unwrap().port()
    }

    /// Starts the daemon, with `env` beside its state directory.
    pub async fn start(
        self,
        clock: ManualClock,
        seed: u64,
        policies: Policies,
        env: &[(&str, &str)],
    ) -> TestNode {
        let Self {
            dir,
            http,
            https,
            quic,
            feed,
            with_feed,
            ipc_path,
            ipc,
            notify,
            blackhole,
        } = self;
        let feed_address = feed.local_addr().unwrap();
        let state = dir.path().join("state");
        let state_text = state.to_str().unwrap().to_owned();
        // No bootstrap nodes and no relay unless the test names some: the
        // default list is the public testnet's, and the default relay the
        // public front door (spec/decisions/default-relay.md).
        let pairs = [("DASSIE_BOOTSTRAP_NODES", "[]"), ("DASSIE_RELAY", "")]
            .into_iter()
            .chain(env.iter().copied())
            .chain([("DASSIE_STATE_DIRECTORY", state_text.as_str())]);
        let settings = Settings::from_env(&Env::from_pairs(pairs)).unwrap();
        let store = Store::open(&state).unwrap();
        let (http_address, https_address) =
            (http.local_addr().unwrap(), https.local_addr().unwrap());
        let (hand_out, handed) = tokio::sync::oneshot::channel();
        let (node_out, node_handed) = tokio::sync::oneshot::channel();
        let mut prepared = Prepared::from_parts(settings, store, (http, https), ipc, quic)
            .handing_out(hand_out)
            .handing_out_node(node_out);
        if with_feed {
            prepared = prepared.with_feed(feed);
        }
        if let Some(path) = notify {
            prepared = prepared.notifying(Notifier::Path(path));
        }
        let shutdown = CancellationToken::new();
        let handle = tokio::spawn(daemon::run(
            prepared,
            Arc::new(clock.clone()),
            Box::new(SeededRng::new(seed)),
            policies,
            shutdown.clone(),
        ));
        let mut node = TestNode {
            dir,
            http: http_address,
            https: https_address,
            feed: feed_address,
            ipc: ipc_path,
            clock,
            quic: None,
            node: std::sync::Weak::new(),
            shutdown,
            handle: Some(handle),
            _blackhole: blackhole,
        };
        node.wait_until_up().await;
        node.quic = handed.await.ok();
        node.node = node_handed.await.unwrap_or_default();
        node
    }
}

/// A running daemon.
pub struct TestNode {
    pub dir: tempfile::TempDir,
    pub http: SocketAddr,
    pub https: SocketAddr,
    /// Where the rate feed takes datagrams.
    pub feed: SocketAddr,
    pub ipc: PathBuf,
    pub clock: ManualClock,
    /// The QUIC endpoint, once the daemon made it.
    pub quic: Option<Arc<dassie_node::quic::Quic>>,
    /// The node, held weakly so that stopping the daemon ends it: for a
    /// test that reaches what no RPC does.
    #[allow(clippy::disallowed_types)]
    pub node: std::sync::Weak<dassie_node::node::Node>,
    shutdown: CancellationToken,
    handle: Option<JoinHandle<Result<(), DaemonError>>>,
    _blackhole: Option<std::net::UdpSocket>,
}

impl TestNode {
    /// A daemon on a fresh directory.
    pub async fn start() -> Self {
        Self::start_in(tempfile::tempdir().unwrap(), ManualClock::new(START_MS), 1).await
    }

    /// A daemon on a fresh directory, with other limits.
    pub async fn start_with(policies: Policies) -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self::start_in_with(dir, ManualClock::new(START_MS), 1, policies).await
    }

    /// A daemon on `dir`, which may hold a database already.
    pub async fn start_in(dir: tempfile::TempDir, clock: ManualClock, seed: u64) -> Self {
        Self::start_in_with(dir, clock, seed, Policies::DEFAULT).await
    }

    async fn start_in_with(
        dir: tempfile::TempDir,
        clock: ManualClock,
        seed: u64,
        policies: Policies,
    ) -> Self {
        Bound::bind_in(dir).start(clock, seed, policies, &[]).await
    }

    async fn wait_until_up(&self) {
        for _ in 0..200 {
            if UnixStream::connect(&self.ipc).await.is_ok()
                && TcpStream::connect(self.https).await.is_ok()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the daemon did not come up");
    }

    /// Waits for the daemon to end by itself, and returns how it ended and
    /// its directory.
    pub async fn ended(mut self) -> (Result<(), DaemonError>, tempfile::TempDir) {
        let handle = self.handle.take().unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), handle)
            .await
            .expect("the daemon did not end")
            .unwrap();
        let _ = std::fs::remove_file(&self.ipc);
        (outcome, self.dir)
    }

    /// Stops the daemon and returns its directory and clock, to start again.
    pub async fn stop(mut self) -> (tempfile::TempDir, ManualClock) {
        self.shutdown.cancel();
        self.handle.take().unwrap().await.unwrap().unwrap();
        let _ = std::fs::remove_file(&self.ipc);
        (self.dir, self.clock)
    }

    /// The origin of the HTTPS listener, as the browser would name it.
    pub fn origin(&self) -> String {
        format!("https://localhost:{}", self.https.port())
    }

    pub fn host(&self) -> String {
        format!("localhost:{}", self.https.port())
    }

    pub async fn ipc(&self) -> RpcClient<Framed<UnixStream, LengthDelimitedCodec>> {
        RpcClient::ipc(&self.ipc).await
    }

    /// The setup token, from the setup URL.
    pub async fn setup_token(&self) -> String {
        let response = self
            .ipc()
            .await
            .call(request::Method::GetSetupUrl(proto::GetSetupUrlRequest {}))
            .await;
        let Ok(response::Method::GetSetupUrl(setup)) = response else {
            panic!("{response:?}")
        };
        setup.url.rsplit('/').next().unwrap().to_string()
    }
}

/// Accepts any server certificate, and keeps the last one it saw.
#[derive(Debug, Default)]
pub struct Recorder(pub Mutex<Option<CertificateDer<'static>>>);

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.0.lock().unwrap() = Some(end_entity.clone().into_owned());
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = tls::provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, certificate, signature, &algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = tls::provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, certificate, signature, &algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        tls::provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A TLS connection to the node, and the certificate it presented.
pub async fn connect_tls(address: SocketAddr) -> (TlsStream<TcpStream>, CertificateDer<'static>) {
    let recorder = Arc::new(Recorder::default());
    let config = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(recorder.clone())
        .with_no_client_auth();
    let stream = TcpStream::connect(address).await.unwrap();
    let name = ServerName::try_from("localhost").unwrap();
    let stream = TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
        .unwrap();
    let certificate = recorder.0.lock().unwrap().clone().unwrap();
    (stream, certificate)
}

/// A response, read in full.
#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: hyper::HeaderMap,
    pub body: Bytes,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|value| value.to_str().unwrap())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The `name=value` of the `Set-Cookie` header.
    pub fn cookie(&self) -> String {
        self.header("set-cookie")
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }
}

/// Sends `request` over a fresh connection, TLS or not.
pub async fn send(address: SocketAddr, tls: bool, request: Request<Full<Bytes>>) -> Reply {
    if tls {
        let (stream, _) = connect_tls(address).await;
        exchange(TokioIo::new(stream), request).await
    } else {
        exchange(
            TokioIo::new(TcpStream::connect(address).await.unwrap()),
            request,
        )
        .await
    }
}

async fn exchange<S>(io: TokioIo<S>, request: Request<Full<Bytes>>) -> Reply
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await.unwrap();
    tokio::spawn(connection);
    let response = sender.send_request(request).await.unwrap();
    let (parts, body) = response.into_parts();
    let body = body.collect().await.unwrap().to_bytes();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body,
    }
}

/// A `POST` of `json` from the node's own pages.
pub fn post_json(node: &TestNode, path: &str, json: &str) -> Request<Full<Bytes>> {
    Request::post(path)
        .header("host", node.host())
        .header("origin", node.origin())
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(json.to_string())))
        .unwrap()
}

/// The setup body for `key`, with the right proof.
pub fn setup_body(token: &str, key: &dassie_identity::NodeKey) -> String {
    let hex = dassie_node_core::hex::encode;
    format!(
        r#"{{"setupAuthorizationToken":"{token}","rawDassieKeyHex":"{}","loginAuthorizationSignature":"{}"}}"#,
        hex(key.as_bytes()),
        hex(key.login_secret().as_bytes())
    )
}

pub fn login_body(key: &dassie_identity::NodeKey) -> String {
    let proof = dassie_node_core::hex::encode(key.login_secret().as_bytes());
    format!(r#"{{"loginAuthorizationSignature":"{proof}"}}"#)
}

/// Sets the node up with `key` and returns the session cookie.
pub async fn set_up(node: &TestNode, key: &dassie_identity::NodeKey) -> String {
    let token = node.setup_token().await;
    let reply = send(
        node.https,
        true,
        post_json(node, "/api/setup", &setup_body(&token, key)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    reply.cookie()
}

/// An RPC client over any message transport.
pub struct RpcClient<T> {
    transport: T,
    next_call: u32,
}

impl RpcClient<Framed<UnixStream, LengthDelimitedCodec>> {
    pub async fn ipc(path: &std::path::Path) -> Self {
        let stream = UnixStream::connect(path).await.unwrap();
        Self {
            transport: Framed::new(stream, ipc::codec()),
            next_call: 1,
        }
    }
}

/// A browser's RPC WebSocket.
pub type Socket = WebSocketStream<TlsStream<TcpStream>>;

/// Opens `/rpc` with `cookie` (if any) and `origin`.
pub async fn websocket(
    node: &TestNode,
    cookie: Option<&str>,
    origin: &str,
) -> Result<Socket, String> {
    let (stream, _) = connect_tls(node.https).await;
    let mut request = format!("wss://{}/rpc", node.host())
        .into_client_request()
        .unwrap();
    let headers = request.headers_mut();
    headers.insert("origin", origin.parse().unwrap());
    headers.insert("sec-websocket-protocol", "dassie.ui.v1".parse().unwrap());
    if let Some(cookie) = cookie {
        headers.insert("cookie", cookie.parse().unwrap());
    }
    tokio_tungstenite::client_async(request, stream)
        .await
        .map(|(socket, _)| socket)
        .map_err(|error| error.to_string())
}

/// Something that carries whole messages.
pub trait Transport {
    async fn send_bytes(&mut self, bytes: Bytes);
    /// The next message, or `None` when the other end closed.
    async fn receive_bytes(&mut self) -> Option<Bytes>;
}

impl Transport for Framed<UnixStream, LengthDelimitedCodec> {
    async fn send_bytes(&mut self, bytes: Bytes) {
        self.send(bytes).await.unwrap();
    }

    async fn receive_bytes(&mut self) -> Option<Bytes> {
        self.next().await.map(|frame| frame.unwrap().freeze())
    }
}

impl Transport for Socket {
    async fn send_bytes(&mut self, bytes: Bytes) {
        self.send(Message::Binary(bytes)).await.unwrap();
    }

    async fn receive_bytes(&mut self) -> Option<Bytes> {
        loop {
            match self.next().await? {
                Ok(Message::Binary(bytes)) => return Some(bytes),
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }
}

impl<T: Transport> RpcClient<T> {
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_call: 1,
        }
    }

    pub fn into_inner(self) -> T {
        self.transport
    }

    pub async fn send_raw(&mut self, message: proto::ClientMessage) {
        self.transport
            .send_bytes(Bytes::from(message.encode_to_vec()))
            .await;
    }

    pub async fn receive(&mut self) -> Option<proto::ServerMessage> {
        let bytes = self.transport.receive_bytes().await?;
        Some(proto::ServerMessage::decode(bytes).unwrap())
    }

    /// Calls a unary method; `Err` carries the error.
    pub async fn call(
        &mut self,
        method: request::Method,
    ) -> Result<response::Method, proto::Error> {
        let call_id = self.next_call;
        self.next_call += 1;
        let request = proto::Request {
            method: Some(method),
        };
        self.send_raw(proto::ClientMessage {
            call_id,
            kind: Some(client_message::Kind::Request(request)),
        })
        .await;
        let message = self.receive().await.expect("the connection closed");
        assert_eq!(message.call_id, call_id);
        match message.kind.unwrap() {
            server_message::Kind::Response(response) => Ok(response.method.unwrap()),
            server_message::Kind::Error(error) => Err(error),
            other => panic!("unexpected {other:?}"),
        }
    }

    /// A new incoming payment on the default ledger, of at most `amount`:
    /// the STREAM credentials to pay it with. Gives the node the hostname
    /// `localhost` first if it has none, since Open Payments' URLs need one.
    pub async fn incoming_payment(&mut self, amount: Option<u64>) -> proto::IncomingPayment {
        let Ok(response::Method::GetConfig(config)) = self
            .call(request::Method::GetConfig(proto::GetConfigRequest {}))
            .await
        else {
            panic!("no config")
        };
        if config
            .config
            .is_none_or(|config| config.hostname.is_empty())
        {
            self.call(request::Method::SetHostname(proto::SetHostnameRequest {
                hostname: "localhost".into(),
            }))
            .await
            .unwrap();
        }
        match self
            .call(request::Method::CreateIncomingPayment(
                proto::CreateIncomingPaymentRequest {
                    incoming_amount: amount,
                    ..proto::CreateIncomingPaymentRequest::default()
                },
            ))
            .await
        {
            Ok(response::Method::CreateIncomingPayment(created)) => created.payment.unwrap(),
            other => panic!("no incoming payment: {other:?}"),
        }
    }

    /// `GetBasicState`'s state.
    pub async fn state(&mut self) -> proto::NodeState {
        match self
            .call(request::Method::GetBasicState(
                proto::GetBasicStateRequest {},
            ))
            .await
        {
            Ok(response::Method::GetBasicState(state)) => state.state(),
            other => panic!("{other:?}"),
        }
    }
}

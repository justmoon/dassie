//! QUIC over WebSockets, where UDP does not get through
//! (spec/areas/peer-protocol.md#quic-over-websocket,
//! spec/decisions/quic-over-websocket.md).
//!
//! A periphery node whose QUIC cannot reach a core node over UDP carries the
//! same packets over a WebSocket at [`PATH`] on the core node's HTTPS
//! listener, one packet per binary message. The QUIC side is a second quinn
//! endpoint, whose socket is a [`TunnelSocket`]: each tunnel is an address
//! there, and a packet sent to that address goes down its WebSocket. A
//! tunnel a node takes has its TCP connection's address, so that places per
//! client address hold as over UDP; one it dials has the server's address,
//! with a port of its own.
//!
//! Each tunnel carries one QUIC connection, and the two end together: a
//! tunnel that ends closes its connection at once, and a connection that
//! closes ends its tunnel once its close has gone out.
//!
//! The outer TLS is not checked: the QUIC handshake inside pins the node
//! key, so whoever holds the outer TLS sees only QUIC's ciphertext, and a
//! node with a self-signed certificate, or behind a proxy that intercepts
//! TLS, is reached all the same.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, SocketAddrV6};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeId;
use dassie_node_core::connections::{ConnectionLimits, Refused};
use dassie_node_core::rate_limit::ClientKey;
use futures_util::{SinkExt, StreamExt};
use quinn::congestion::CubicConfig;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, EndpointConfig, Runtime, TransportConfig, UdpPoller,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{WebPkiSupportedAlgorithms, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::SEC_WEBSOCKET_PROTOCOL;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::{CONNECT_TIMEOUT, MAX_LINKS};
use crate::log::target;
use crate::node::lock;
use crate::slots::{ClientSlot, ClientSlots};
use crate::tls;

/// Where a core node's HTTPS listener takes tunnels.
pub const PATH: &str = "/quic";

/// The WebSocket subprotocol of a tunnel: QUIC packets, one per binary
/// message.
pub const SUBPROTOCOL: &str = "dassie-quic/1";

/// The outer TLS's ALPN: HTTP/1.1, which the WebSocket upgrade takes.
const TLS_ALPN: &[u8] = b"http/1.1";

/// The size of the QUIC packets over a tunnel, which nothing fragments: a
/// TLS record's largest plaintext, 16 KiB, less the largest header of a
/// client's frame of that size, 8 bytes, so that a packet and its frame fit
/// one record. Every packet may be this large from the first, and MTU
/// discovery is off: there is no path to probe.
pub const DATAGRAM_BYTES: u16 = 16_376;

/// Packets waiting to go down one tunnel. Past them, more are dropped, as a
/// full UDP socket drops them, and QUIC's congestion control slows down; so
/// a tunnel holds at most this many packets of [`DATAGRAM_BYTES`].
const QUEUE_PACKETS: usize = 16;

/// Packets from all tunnels waiting for the endpoint to read them. A tunnel
/// that finds them full waits before it reads more, which TCP passes back to
/// its sender.
const RECEIVED_PACKETS: usize = 256;

/// Most tunnels a node takes at once: as many as its HTTPS listener has
/// connections, since each is a TCP connection as theirs are, and so room
/// for every downlink a core node takes by default. The QUIC connection
/// inside holds a place by its key as over UDP ([`super::places`]), so the
/// places, not this bound, decide whose connections stay.
pub const MAX_TUNNELS: usize = crate::http::MAX_CONNECTIONS;

/// Most tunnels one client (an IPv4 address or an IPv6 /56) holds at once:
/// as many connections as the HTTPS listener takes from one client by
/// default, so that the periphery nodes behind one NAT each reach the node.
pub const MAX_TUNNELS_PER_CLIENT: u32 = crate::node::Policies::DEFAULT.http_connections_per_client;

/// Most tunnels the socket holds: those the node takes, and those it dials,
/// one per node it links to at most.
const MAX_ADDRESSES: usize = MAX_TUNNELS + MAX_LINKS;

/// Time a tunnel lingers once its connection closed, so that the close quinn
/// sends goes out before the WebSocket ends, unless the other side ends it
/// first.
const CLOSE_LINGER: Duration = Duration::from_millis(100);

/// Time to send what is queued and close the WebSocket, at a tunnel's end.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

const _: () = assert!(MAX_TUNNELS >= dassie_peer::Config::DEFAULT.max_downlinks);
const _: () = assert!(MAX_ADDRESSES < u16::MAX as usize);
const _: () = assert!(DATAGRAM_BYTES as usize + 8 == 16 * 1024);

/// The WebSocket's settings for a tunnel, either side: no message larger
/// than a packet.
#[must_use]
pub fn websocket_config() -> WebSocketConfig {
    let largest = usize::from(DATAGRAM_BYTES);
    WebSocketConfig::default()
        .max_message_size(Some(largest))
        .max_frame_size(Some(largest))
}

/// QUIC's settings over a tunnel: `transport`, with packets of
/// [`DATAGRAM_BYTES`] and no MTU discovery. The congestion window starts at
/// two of them, as RFC 9002 has it for packets this large: quinn's default,
/// sized for UDP's, would not fit the first.
pub(super) fn over_tunnel(mut transport: TransportConfig) -> TransportConfig {
    transport.initial_mtu(DATAGRAM_BYTES);
    transport.min_mtu(DATAGRAM_BYTES);
    transport.mtu_discovery_config(None);
    let mut congestion = CubicConfig::default();
    congestion.initial_window(2 * u64::from(DATAGRAM_BYTES));
    transport.congestion_controller_factory(Arc::new(congestion));
    transport
}

/// The endpoint's settings over tunnels: `config`, which holds the node's
/// key for stateless resets, taking packets of [`DATAGRAM_BYTES`].
pub(super) fn endpoint_config(mut config: EndpointConfig) -> io::Result<EndpointConfig> {
    config
        .max_udp_payload_size(DATAGRAM_BYTES)
        .map_err(io::Error::other)?;
    Ok(config)
}

/// The node's tunnels: the endpoint over them, the places for those it
/// takes, and the tasks that carry them.
pub(super) struct Tunnels {
    endpoint: Endpoint,
    socket: Arc<TunnelSocket>,
    /// The outer TLS's settings for the tunnels it dials.
    tls: Arc<rustls::ClientConfig>,
    /// Places for the tunnels it takes, in all and per client address.
    slots: ClientSlots,
    tasks: TaskTracker,
    /// Ends every tunnel, once the endpoint is closed.
    stop: CancellationToken,
}

impl Tunnels {
    /// An endpoint over tunnels with `config` and, for the connections it
    /// takes, `server`.
    ///
    /// # Errors
    ///
    /// The TLS settings cannot be made, or the endpoint cannot start.
    pub(super) fn new(
        config: EndpointConfig,
        server: quinn::ServerConfig,
        runtime: Arc<dyn Runtime>,
    ) -> io::Result<Self> {
        let socket = Arc::new(TunnelSocket::new());
        let abstract_socket: Arc<dyn AsyncUdpSocket> = Arc::clone(&socket) as _;
        let endpoint =
            Endpoint::new_with_abstract_socket(config, Some(server), abstract_socket, runtime)?;
        Ok(Self {
            endpoint,
            socket,
            tls: tls_config().map_err(io::Error::other)?,
            slots: ClientSlots::new(ConnectionLimits {
                max_total: MAX_TUNNELS,
                max_per_client: MAX_TUNNELS_PER_CLIENT,
            }),
            tasks: TaskTracker::new(),
            stop: CancellationToken::new(),
        })
    }

    /// The endpoint over the tunnels.
    pub(super) const fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Tunnels open now, taken and dialled.
    pub(super) fn count(&self) -> usize {
        lock(&self.socket.tunnels).by_address.len()
    }

    /// A place for a tunnel from `client`, before its upgrade.
    ///
    /// # Errors
    ///
    /// All places are taken, or the client holds its share.
    pub(super) fn slot(&self, client: Option<ClientKey>) -> Result<ClientSlot, Refused> {
        self.slots.take(client)
    }

    /// The tunnel at `from` for the connection arriving on it, which then
    /// holds what ends them together; `None` when there is no tunnel there,
    /// or another connection took it: a tunnel carries one.
    pub(super) fn claim(&self, from: SocketAddr) -> Option<Inside> {
        let mut table = lock(&self.socket.tunnels);
        let tunnel = table.by_address.get_mut(&v6(from))?;
        if tunnel.claimed {
            return None;
        }
        tunnel.claimed = true;
        Some(Inside(tunnel.ends.clone()))
    }

    /// Carries the packets of `ws`, a WebSocket from `peer` upgraded at
    /// [`PATH`], to and from the endpoint, holding `slot` while it lasts. A
    /// tunnel no connection took within the time to connect ends.
    pub(super) fn serve<S>(&self, ws: WebSocketStream<S>, peer: SocketAddr, slot: ClientSlot)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let address = v6(peer);
        let Some((queue, ends)) = self.socket.open(address) else {
            tracing::debug!(target: target::PEER, %peer, "no room for a tunnel; closing");
            return;
        };
        let carried = carry(
            Arc::clone(&self.socket),
            ws,
            (address, queue),
            ends,
            Some(CONNECT_TIMEOUT),
            self.stop.clone(),
        );
        self.tasks.spawn(async move {
            let _slot = slot;
            carried.await;
        });
    }

    /// Makes a connection to `to` with `config` over a tunnel to `host` and
    /// `port`, at each of `addresses` in turn with a share of the time left
    /// until `deadline`.
    pub(super) async fn dial(
        &self,
        to: NodeId,
        (host, port): (&str, u16),
        addresses: &[SocketAddr],
        config: &quinn::ClientConfig,
        deadline: tokio::time::Instant,
    ) -> Option<Connection> {
        for (index, address) in addresses.iter().enumerate() {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let tries_left = u32::try_from(addresses.len() - index).unwrap_or(1).max(1);
            let share = left / tries_left;
            if share.is_zero() {
                break;
            }
            let dialled = self.dial_at(*address, (host, port), config.clone());
            match tokio::time::timeout(share, dialled).await {
                Ok(Ok(connection)) => return Some(connection),
                Ok(Err(error)) => {
                    tracing::debug!(target: target::PEER, %to, %address, %error, "QUIC over a WebSocket failed");
                }
                Err(_) => {
                    tracing::debug!(target: target::PEER, %to, %address, "QUIC over a WebSocket timed out");
                }
            }
        }
        None
    }

    /// A tunnel to `address`, as `host`, and a connection over it.
    async fn dial_at(
        &self,
        address: SocketAddr,
        (host, port): (&str, u16),
        config: quinn::ClientConfig,
    ) -> Result<Connection, String> {
        let tcp = TcpStream::connect(address)
            .await
            .map_err(|e| e.to_string())?;
        let _ = tcp.set_nodelay(true);
        let name = ServerName::try_from(host.to_owned()).map_err(|e| e.to_string())?;
        let tls = tokio_rustls::TlsConnector::from(Arc::clone(&self.tls))
            .connect(name, tcp)
            .await
            .map_err(|e| e.to_string())?;
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let mut request = format!("wss://{authority}{PATH}")
            .into_client_request()
            .map_err(|e| e.to_string())?;
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(SUBPROTOCOL),
        );
        // A node of an earlier release answers 404, a failed dial.
        let (ws, _) =
            tokio_tungstenite::client_async_with_config(request, tls, Some(websocket_config()))
                .await
                .map_err(|e| e.to_string())?;
        let (local, queue, ends) = self
            .socket
            .open_dialled(address.ip())
            .ok_or("too many tunnels")?;
        let inside = Inside(ends.clone());
        let carried = carry(
            Arc::clone(&self.socket),
            ws,
            (local, queue),
            ends,
            None,
            self.stop.clone(),
        );
        self.tasks.spawn(carried);
        let connection = self
            .endpoint
            .connect_with(config, SocketAddr::V6(local), host)
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?;
        self.tasks.spawn(tie(connection.clone(), inside));
        Ok(connection)
    }

    /// Ends every tunnel, once the endpoint is closed, waiting a bounded
    /// time for their WebSockets to close.
    pub(super) async fn stop(&self) {
        self.stop.cancel();
        self.tasks.close();
        if tokio::time::timeout(2 * CLOSE_TIMEOUT, self.tasks.wait())
            .await
            .is_err()
        {
            tracing::warn!(target: target::PEER, "tunnels still open after the close timeout");
        }
    }
}

/// Holds `connection`, dialled over a tunnel, with it: closes it at once
/// when the tunnel ends, and once it closes, lets the tunnel end.
async fn tie(connection: Connection, inside: Inside) {
    tokio::select! {
        _ = connection.closed() => {}
        () = inside.tunnel_ended() => connection.close(0u32.into(), b"tunnel closed"),
    }
}

/// How a tunnel and the connection inside it end together.
#[derive(Clone, Default)]
struct Ends {
    /// The tunnel ended: its connection closes at once.
    tunnel: CancellationToken,
    /// The connection is done: the tunnel ends once its close has gone out.
    connection: CancellationToken,
}

/// What the connection inside a tunnel holds while it lasts: dropping it
/// lets the tunnel end.
pub(super) struct Inside(Ends);

impl Inside {
    /// Returns once the tunnel has ended, when its connection is to close.
    pub(super) async fn tunnel_ended(&self) {
        self.0.tunnel.cancelled().await;
    }
}

impl Drop for Inside {
    fn drop(&mut self) {
        self.0.connection.cancel();
    }
}

/// Returns once the tunnel `inside` names has ended; never without one.
pub(super) async fn tunnel_ended(inside: Option<&Inside>) {
    match inside {
        Some(inside) => inside.tunnel_ended().await,
        None => std::future::pending().await,
    }
}

/// Carries packets between `ws` and the socket, where the tunnel is at
/// `address` with `queue`, until the WebSocket ends or breaks, the
/// connection inside is done, no connection took it `within` (when given),
/// or `stop`. Then sends what is queued, and closes the WebSocket.
async fn carry<S>(
    socket: Arc<TunnelSocket>,
    ws: WebSocketStream<S>,
    (address, mut queue): (SocketAddrV6, mpsc::Receiver<Bytes>),
    ends: Ends,
    within: Option<Duration>,
    stop: CancellationToken,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    {
        let received = socket.receive.clone();
        let reading = async {
            while let Some(Ok(message)) = stream.next().await {
                match message {
                    Message::Binary(packet) => {
                        let packet = Received {
                            from: address,
                            packet,
                        };
                        if received.send(packet).await.is_err() {
                            break;
                        }
                    }
                    // Control frames are tungstenite's business.
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                    // Only binary messages carry packets.
                    Message::Text(_) | Message::Close(_) => break,
                }
            }
        };
        let writing = async {
            while let Some(packet) = queue.recv().await {
                if sink.send(Message::Binary(packet)).await.is_err() {
                    break;
                }
            }
        };
        let done = async {
            ends.connection.cancelled().await;
            tokio::time::sleep(CLOSE_LINGER).await;
        };
        let unclaimed = async {
            if let Some(within) = within {
                tokio::time::sleep(within).await;
                if !socket.claimed(address) {
                    return;
                }
            }
            std::future::pending::<()>().await;
        };
        tokio::select! {
            () = reading => {}
            () = writing => {}
            () = done => {}
            () = unclaimed => {}
            () = stop.cancelled() => {}
        }
    }
    socket.close(address);
    ends.tunnel.cancel();
    let closing = async {
        while let Ok(packet) = queue.try_recv() {
            sink.feed(Message::Binary(packet)).await?;
        }
        sink.send(Message::Close(None)).await?;
        sink.close().await
    };
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, closing).await;
}

/// A packet that came down a tunnel, and the tunnel's address.
struct Received {
    from: SocketAddrV6,
    packet: Bytes,
}

/// The endpoint's socket over tunnels: an address per tunnel, each with its
/// queue of packets to send, and one queue of the packets received.
pub(super) struct TunnelSocket {
    tunnels: Mutex<Table>,
    received: Mutex<mpsc::Receiver<Received>>,
    receive: mpsc::Sender<Received>,
}

/// The tunnels by address.
struct Table {
    by_address: BTreeMap<SocketAddrV6, Tunnel>,
    /// The port a tunnel dialled next tries first.
    next_port: u16,
}

/// A tunnel the socket sends on.
struct Tunnel {
    queue: mpsc::Sender<Bytes>,
    ends: Ends,
    /// Whether a connection took it: only one does.
    claimed: bool,
}

impl TunnelSocket {
    fn new() -> Self {
        let (receive, received) = mpsc::channel(RECEIVED_PACKETS);
        Self {
            tunnels: Mutex::new(Table {
                by_address: BTreeMap::new(),
                next_port: 1,
            }),
            received: Mutex::new(received),
            receive,
        }
    }

    /// Opens a tunnel at `address`, which a connection is still to take;
    /// `None` when one is there, or the socket holds its most.
    fn open(&self, address: SocketAddrV6) -> Option<(mpsc::Receiver<Bytes>, Ends)> {
        let mut table = lock(&self.tunnels);
        if table.by_address.len() >= MAX_ADDRESSES || table.by_address.contains_key(&address) {
            return None;
        }
        Some(insert(&mut table, address, false))
    }

    /// Opens a tunnel dialled to `server`, taken by the connection we make
    /// over it, at the server's address and a port no other tunnel there
    /// has; `None` when the socket holds its most.
    fn open_dialled(&self, server: IpAddr) -> Option<(SocketAddrV6, mpsc::Receiver<Bytes>, Ends)> {
        let ip = match server {
            IpAddr::V4(ip) => ip.to_ipv6_mapped(),
            IpAddr::V6(ip) => ip,
        };
        let mut table = lock(&self.tunnels);
        if table.by_address.len() >= MAX_ADDRESSES {
            return None;
        }
        // Of more ports in a row than the socket holds tunnels, one is free.
        for _ in 0..=MAX_ADDRESSES {
            let port = table.next_port;
            table.next_port = port.checked_add(1).unwrap_or(1);
            let address = SocketAddrV6::new(ip, port, 0, 0);
            if !table.by_address.contains_key(&address) {
                let (queue, ends) = insert(&mut table, address, true);
                return Some((address, queue, ends));
            }
        }
        None
    }

    /// Whether a connection took the tunnel at `address`.
    fn claimed(&self, address: SocketAddrV6) -> bool {
        lock(&self.tunnels)
            .by_address
            .get(&address)
            .is_some_and(|tunnel| tunnel.claimed)
    }

    /// Closes the tunnel at `address`: what is sent to it from now on is
    /// dropped.
    fn close(&self, address: SocketAddrV6) {
        lock(&self.tunnels).by_address.remove(&address);
    }
}

/// Puts a tunnel at `address`, free, into `table`.
fn insert(
    table: &mut Table,
    address: SocketAddrV6,
    claimed: bool,
) -> (mpsc::Receiver<Bytes>, Ends) {
    let (queue, queued) = mpsc::channel(QUEUE_PACKETS);
    let ends = Ends::default();
    let tunnel = Tunnel {
        queue,
        ends: ends.clone(),
        claimed,
    };
    let replaced = table.by_address.insert(address, tunnel);
    assert!(replaced.is_none());
    assert!(table.by_address.len() <= MAX_ADDRESSES);
    (queued, ends)
}

/// `address` as quinn has it on an endpoint whose socket is IPv6, as
/// [`TunnelSocket`]'s is.
fn v6(address: SocketAddr) -> SocketAddrV6 {
    match address {
        SocketAddr::V4(address) => {
            SocketAddrV6::new(address.ip().to_ipv6_mapped(), address.port(), 0, 0)
        }
        SocketAddr::V6(address) => address,
    }
}

impl fmt::Debug for TunnelSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TunnelSocket").finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for TunnelSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable)
    }

    /// Queues the packets for their tunnel, or drops them, as UDP would,
    /// when the tunnel is gone or its queue is full.
    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let table = lock(&self.tunnels);
        let Some(tunnel) = table.by_address.get(&v6(transmit.destination)) else {
            return Ok(());
        };
        let size = transmit
            .segment_size
            .unwrap_or(transmit.contents.len())
            .max(1);
        for packet in transmit.contents.chunks(size) {
            let _ = tunnel.queue.try_send(Bytes::copy_from_slice(packet));
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut received = lock(&self.received);
        let mut filled = 0;
        while let (Some(buf), Some(meta)) = (bufs.get_mut(filled), meta.get_mut(filled)) {
            // Waits for the first; takes the others that are there already.
            let next = if filled == 0 {
                match received.poll_recv(cx) {
                    Poll::Ready(Some(next)) => next,
                    Poll::Ready(None) | Poll::Pending => return Poll::Pending,
                }
            } else {
                match received.try_recv() {
                    Ok(next) => next,
                    Err(_) => break,
                }
            };
            let length = next.packet.len();
            // Never larger than the buffer, which holds the largest packet
            // the WebSocket passes; dropped if it were.
            let Some(into) = buf.get_mut(..length) else {
                continue;
            };
            into.copy_from_slice(&next.packet);
            *meta = RecvMeta {
                addr: SocketAddr::V6(next.from),
                len: length,
                stride: length,
                ecn: None,
                dst_ip: None,
            };
            filled += 1;
        }
        if filled == 0 {
            Poll::Pending
        } else {
            Poll::Ready(Ok(filled))
        }
    }

    /// No address of its own: an IPv6 one, so that it takes addresses of
    /// both families.
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)))
    }

    /// A packet is a message, whole or not at all.
    fn may_fragment(&self) -> bool {
        false
    }
}

/// The socket never makes a sender wait: a full tunnel drops.
#[derive(Debug)]
struct Writable;

impl UdpPoller for Writable {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// The outer TLS's settings for the tunnels a node dials: any certificate,
/// since the QUIC handshake inside proves the node key.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, rustls::Error> {
    let provider = tls::provider();
    let algorithms = provider.signature_verification_algorithms;
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(algorithms)))
        .with_no_client_auth();
    config.alpn_protocols = vec![TLS_ALPN.to_vec()];
    Ok(Arc::new(config))
}

/// Takes any certificate, and checks only that the server holds its key.
#[derive(Debug)]
struct AnyCertificate(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// The first of `udp` and `tunnel` to make a connection to `to`, the tunnel
/// dialled only once `udp` has had `head_start` without one, or failed: the
/// other is dropped. `None` only when both fail.
pub(super) async fn race(
    to: NodeId,
    udp: impl Future<Output = Option<Connection>>,
    tunnel: impl Future<Output = Option<Connection>>,
    head_start: Duration,
) -> Option<Connection> {
    let mut udp = std::pin::pin!(udp);
    let udp_failed = tokio::select! {
        connection = &mut udp => match connection {
            Some(connection) => return Some(connection),
            None => true,
        },
        () = tokio::time::sleep(head_start) => false,
    };
    let mut tunnel = std::pin::pin!(tunnel);
    let tunnelled = if udp_failed {
        tunnel.await
    } else {
        tokio::select! {
            connection = &mut udp => match connection {
                Some(connection) => return Some(connection),
                None => tunnel.await,
            },
            connection = &mut tunnel => match connection {
                Some(connection) => Some(connection),
                None => return udp.await,
            },
        }
    };
    if tunnelled.is_some() {
        tracing::info!(target: target::PEER, %to, "connected over a WebSocket: QUIC over UDP did not get through");
    }
    tunnelled
}

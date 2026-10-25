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
//! A tunnel holds the place of the connection it carries
//! (spec/decisions/places-by-proved-key.md): once upgraded, one among the
//! connections being set up, by its client address, until its connection
//! has proved a key, which it must within the time to connect, having
//! carried a first packet within [`FIRST_PACKET_TIMEOUT`]; then only its
//! connection's, in its class. It has no places of its own. Only a node's
//! client takes a tunnel: a browser's request, which carries an origin, is
//! refused, so that no page can have its visitors hold those places.
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
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::{ORIGIN, SEC_WEBSOCKET_PROTOCOL};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::{CONNECT_TIMEOUT, MAX_LINKS};
use crate::log::target;
use crate::node::lock;
use crate::slots::ClientSlot;
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

/// Most tunnels the node dials at once: one per node it links to. Those it
/// takes are bounded by the endpoint's places ([`Tunnels::serve`]).
const MAX_DIALLED: usize = MAX_LINKS;

/// Time a tunnel lingers once its connection closed, so that the close quinn
/// sends goes out before the WebSocket ends, unless the other side ends it
/// first.
const CLOSE_LINGER: Duration = Duration::from_millis(100);

/// Time to send what is queued and close the WebSocket, at a tunnel's end.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// Time a tunnel taken has, from its upgrade, to carry its first packet;
/// one that carries none by then ends, rather than at the time to connect.
/// A node's client sends the first packet of its handshake as soon as the
/// upgrade completes, so this is a round trip on a slow link, with room to
/// spare: only a client that sends nothing holds its place among the
/// connections being set up for this long, rather than for
/// [`CONNECT_TIMEOUT`].
pub const FIRST_PACKET_TIMEOUT: Duration = Duration::from_secs(2);

/// The bytes of a packet's WebSocket frame, with the largest header a frame
/// may have.
const FRAME_BYTES: usize = DATAGRAM_BYTES as usize + 14;

/// Room in a tunnel's write buffer for control frames beside its packets:
/// pongs, and the close.
const CONTROL_BYTES: usize = 1024;

const _: () = assert!(MAX_DIALLED < u16::MAX as usize);
const _: () = assert!(DATAGRAM_BYTES as usize + 8 == 16 * 1024);
const _: () = assert!(FIRST_PACKET_TIMEOUT.as_millis() < CONNECT_TIMEOUT.as_millis());

/// The WebSocket's settings for a tunnel, either side: no message larger
/// than a packet, and buffers for about two of them.
///
/// tungstenite's defaults, made for throughput, keep 128 KiB to read,
/// allocated up front, and as much to write, which a node that takes a
/// tunnel per downlink would hold for each. A tunnel reads at most two
/// packets at a time. It writes one packet at a time, flushed before it
/// takes the next from its queue ([`QUEUE_PACKETS`] bounds those), so its
/// write buffer holds at most the packet being written, the next one, and
/// control frames. Its bound keeps a client that sends pings but does not
/// read from growing it: tungstenite holds back a pong that does not fit,
/// rather than buffer it without end, as it would by default.
#[must_use]
pub fn websocket_config() -> WebSocketConfig {
    let largest = usize::from(DATAGRAM_BYTES);
    WebSocketConfig::default()
        .read_buffer_size(2 * FRAME_BYTES)
        .write_buffer_size(FRAME_BYTES)
        .max_write_buffer_size(2 * FRAME_BYTES + CONTROL_BYTES)
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

/// The endpoint's settings over tunnels: `config`, which holds the key for
/// stateless resets over tunnels, taking packets of [`DATAGRAM_BYTES`].
pub(super) fn endpoint_config(mut config: EndpointConfig) -> io::Result<EndpointConfig> {
    config
        .max_udp_payload_size(DATAGRAM_BYTES)
        .map_err(io::Error::other)?;
    Ok(config)
}

/// The node's tunnels: the endpoint over them, and the tasks that carry
/// them.
pub(super) struct Tunnels {
    endpoint: Endpoint,
    socket: Arc<TunnelSocket>,
    /// The outer TLS's settings for the tunnels it dials.
    tls: Arc<rustls::ClientConfig>,
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

    /// The tunnel taken at `from` for the connection arriving on it, which
    /// then holds what ends them together, and the place among the
    /// connections being set up that the tunnel held for it; `None` when
    /// there is no tunnel there, or another connection took it: a tunnel
    /// carries one.
    pub(super) fn claim(&self, from: SocketAddr) -> Option<(Inside, ClientSlot)> {
        let mut table = lock(&self.socket.tunnels);
        let tunnel = table.by_address.get_mut(&v6(from))?;
        if tunnel.claimed {
            return None;
        }
        let setting_up = tunnel.setting_up.take()?;
        tunnel.claimed = true;
        Some((Inside(tunnel.ends.clone()), setting_up))
    }

    /// Carries the packets of `ws`, a WebSocket from `peer` upgraded at
    /// [`PATH`], to and from the endpoint. The tunnel holds `setting_up`, a
    /// place among the QUIC connections being set up, for the connection
    /// that arrives on it, and ends unless that connection has proved a key
    /// within the time to connect ([`Inside::settled`]).
    ///
    /// `places` is how many places the endpoint has, of every kind: those
    /// for connections being set up and those of every class. It bounds
    /// the tunnels taken, so that the socket's table is bounded, and is
    /// derived from the places so that it never binds before they do:
    /// every tunnel taken holds one of them, or its connection does, until
    /// its connection is done (but for the moment a connection that lost
    /// its place takes to close), and a new tunnel holds one already. So
    /// when the socket holds `places` tunnels taken, one of them only
    /// lingers for its close to go out ([`CLOSE_LINGER`]), holding no
    /// place, and gives way to the new one.
    pub(super) fn serve<S>(
        &self,
        ws: WebSocketStream<S>,
        peer: SocketAddr,
        setting_up: ClientSlot,
        places: usize,
    ) where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let address = v6(peer);
        let Some((id, queue, ends)) = self.socket.open(address, setting_up, places) else {
            tracing::debug!(target: target::PEER, %peer, "no room for a tunnel; closing");
            return;
        };
        let carried = carry(
            Arc::clone(&self.socket),
            ws,
            (address, id, queue),
            ends,
            Some(CONNECT_TIMEOUT),
            self.stop.clone(),
        );
        self.tasks.spawn(carried);
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
        // A node of an earlier release answers 404, a failed dial.
        let (ws, _) = tokio_tungstenite::client_async_with_config(
            handshake(host, port)?,
            tls,
            Some(websocket_config()),
        )
        .await
        .map_err(|e| e.to_string())?;
        self.connect_over(ws, address.ip(), host, config).await
    }

    /// A connection with `config` to `host` over `ws`, a WebSocket dialled
    /// to `server` and upgraded at [`PATH`].
    pub(super) async fn connect_over<S>(
        &self,
        ws: WebSocketStream<S>,
        server: IpAddr,
        host: &str,
        config: quinn::ClientConfig,
    ) -> Result<Connection, String>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (local, id, queue, ends) =
            self.socket.open_dialled(server).ok_or("too many tunnels")?;
        let inside = Inside(ends.clone());
        let carried = carry(
            Arc::clone(&self.socket),
            ws,
            (local, id, queue),
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
        self.hold(connection.clone(), inside);
        Ok(connection)
    }

    /// Holds `connection`, over the tunnel `inside` names, with it until
    /// either ends ([`tie`]).
    pub(super) fn hold(&self, connection: Connection, inside: Inside) {
        self.tasks.spawn(tie(connection, inside));
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

/// The request that opens a tunnel to `host` and `port`: with the tunnel's
/// subprotocol, and without an origin, which only a browser sends and a
/// node refuses ([`PATH`]).
fn handshake(host: &str, port: u16) -> Result<Request, String> {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!("wss://{authority}{PATH}")
        .into_client_request()
        .map_err(|e| e.to_string())?;
    let headers = request.headers_mut();
    headers.insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(SUBPROTOCOL),
    );
    assert!(!headers.contains_key(ORIGIN));
    Ok(request)
}

/// Holds `connection`, over a tunnel, with it: closes it at once when the
/// tunnel ends, and once it closes, lets the tunnel end.
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
    /// The connection proved a key, and holds a place in its class: a
    /// tunnel taken lasts past the time to connect.
    settled: CancellationToken,
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

    /// The connection proved a key, and holds a place in its class: the
    /// tunnel lasts as long as it does.
    pub(super) fn settled(&self) {
        self.0.settled.cancel();
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

/// Carries packets between `ws` and the socket, where the tunnel `id` is at
/// `address` with `queue`, until the WebSocket ends or breaks, the
/// connection inside is done, none proved a key on it `within` (when
/// given; nor any packet came within [`FIRST_PACKET_TIMEOUT`]), the socket
/// drops it, or `stop`. Then sends what is queued, closes the WebSocket,
/// and only then leaves the socket.
async fn carry<S>(
    socket: Arc<TunnelSocket>,
    ws: WebSocketStream<S>,
    (address, id, mut queue): (SocketAddrV6, u64, mpsc::Receiver<Bytes>),
    ends: Ends,
    within: Option<Duration>,
    stop: CancellationToken,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    {
        let received = socket.receive.clone();
        let carried_one = CancellationToken::new();
        let reading = async {
            while let Some(Ok(message)) = stream.next().await {
                match message {
                    Message::Binary(packet) => {
                        carried_one.cancel();
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
        // Ends too once the socket drops the tunnel, which drops the
        // queue's sender, after what it holds.
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
        let unsettled = async {
            if let Some(within) = within {
                let deadline = tokio::time::Instant::now() + within;
                tokio::select! {
                    () = tokio::time::sleep(FIRST_PACKET_TIMEOUT.min(within)) => return,
                    () = carried_one.cancelled() => {}
                }
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => return,
                    () = ends.settled.cancelled() => {}
                }
            }
            std::future::pending::<()>().await;
        };
        tokio::select! {
            () = reading => {}
            () = writing => {}
            () = done => {}
            () = unsettled => {}
            () = stop.cancelled() => {}
        }
    }
    ends.tunnel.cancel();
    // What is queued goes out, and nothing after it.
    queue.close();
    let closing = async {
        while let Ok(packet) = queue.try_recv() {
            sink.feed(Message::Binary(packet)).await?;
        }
        sink.send(Message::Close(None)).await?;
        sink.close().await
    };
    let _ = tokio::time::timeout(CLOSE_TIMEOUT, closing).await;
    // Only now, its TCP connection about to go, does it leave the socket:
    // while it closes, it counts among the tunnels, and one taken whose
    // connection never arrived still holds its place among those being set
    // up. A client that does not read cannot keep tunnels closing beyond
    // the places, however many it ends.
    socket.close(address, id);
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
    /// How many of them the node took; the others it dialled.
    taken: usize,
    /// The port a tunnel dialled next tries first.
    next_port: u16,
    /// The next tunnel's ID, which tells it from one at the same address
    /// before it.
    next_id: u64,
}

/// A tunnel the socket sends on.
struct Tunnel {
    id: u64,
    queue: mpsc::Sender<Bytes>,
    ends: Ends,
    /// Whether the node took it, rather than dialled it.
    taken: bool,
    /// For a tunnel taken, its place among the connections being set up,
    /// until the connection arriving on it takes it over.
    setting_up: Option<ClientSlot>,
    /// Whether a connection took it: only one does.
    claimed: bool,
}

impl Tunnel {
    /// Whether it only lingers, its connection done, for its close to go
    /// out: it holds no place, and gives way to a new tunnel.
    fn lingers(&self) -> bool {
        self.ends.connection.is_cancelled()
    }
}

impl TunnelSocket {
    fn new() -> Self {
        let (receive, received) = mpsc::channel(RECEIVED_PACKETS);
        Self {
            tunnels: Mutex::new(Table {
                by_address: BTreeMap::new(),
                taken: 0,
                next_port: 1,
                next_id: 0,
            }),
            received: Mutex::new(received),
            receive,
        }
    }

    /// Opens a tunnel taken at `address`, holding `setting_up` for the
    /// connection still to arrive on it, with at most `places` tunnels
    /// taken ([`Tunnels::serve`]): the oldest that lingers gives way to it,
    /// as one lingering at its address does. `None` when one that does not
    /// linger is at its address, or when the socket holds `places` and
    /// none of them lingers.
    fn open(
        &self,
        address: SocketAddrV6,
        setting_up: ClientSlot,
        places: usize,
    ) -> Option<(u64, mpsc::Receiver<Bytes>, Ends)> {
        let mut table = lock(&self.tunnels);
        if let Some(there) = table.by_address.get(&address) {
            if !(there.taken && there.lingers()) {
                return None;
            }
            remove(&mut table, address);
        }
        if table.taken >= places {
            let oldest = table
                .by_address
                .iter()
                .filter(|(_, tunnel)| tunnel.taken && tunnel.lingers())
                .min_by_key(|(_, tunnel)| tunnel.id)
                .map(|(address, _)| *address)?;
            remove(&mut table, oldest);
        }
        let opened = insert(&mut table, address, Some(setting_up));
        assert!(table.taken <= places);
        Some(opened)
    }

    /// Opens a tunnel dialled to `server`, taken by the connection we make
    /// over it, at the server's address and a port no other tunnel there
    /// has; `None` when the socket holds its most.
    fn open_dialled(
        &self,
        server: IpAddr,
    ) -> Option<(SocketAddrV6, u64, mpsc::Receiver<Bytes>, Ends)> {
        let ip = match server {
            IpAddr::V4(ip) => ip.to_ipv6_mapped(),
            IpAddr::V6(ip) => ip,
        };
        let mut table = lock(&self.tunnels);
        if table.by_address.len() - table.taken >= MAX_DIALLED {
            return None;
        }
        // Of more ports in a row than the socket holds tunnels, one is free,
        // while it holds fewer than there are ports.
        for _ in 0..=table.by_address.len() {
            let port = table.next_port;
            table.next_port = port.checked_add(1).unwrap_or(1);
            let address = SocketAddrV6::new(ip, port, 0, 0);
            if !table.by_address.contains_key(&address) {
                let (id, queue, ends) = insert(&mut table, address, None);
                assert!(table.by_address.len() - table.taken <= MAX_DIALLED);
                return Some((address, id, queue, ends));
            }
        }
        None
    }

    /// Closes the tunnel `id` at `address`, unless it is gone already: what
    /// is sent to it from now on is dropped.
    fn close(&self, address: SocketAddrV6, id: u64) {
        let mut table = lock(&self.tunnels);
        if table
            .by_address
            .get(&address)
            .is_some_and(|tunnel| tunnel.id == id)
        {
            remove(&mut table, address);
        }
    }
}

/// Puts a tunnel at `address` into `table`: one taken, holding
/// `setting_up` for the connection still to arrive on it, or, without, one
/// dialled, which the connection we make over it takes.
fn insert(
    table: &mut Table,
    address: SocketAddrV6,
    setting_up: Option<ClientSlot>,
) -> (u64, mpsc::Receiver<Bytes>, Ends) {
    let (queue, queued) = mpsc::channel(QUEUE_PACKETS);
    let ends = Ends::default();
    let id = table.next_id;
    table.next_id += 1;
    let taken = setting_up.is_some();
    let tunnel = Tunnel {
        id,
        queue,
        ends: ends.clone(),
        taken,
        setting_up,
        claimed: !taken,
    };
    let replaced = table.by_address.insert(address, tunnel);
    assert!(replaced.is_none());
    if taken {
        table.taken += 1;
    }
    assert!(table.taken <= table.by_address.len());
    (id, queued, ends)
}

/// Takes the tunnel at `address` out of `table`: the place it held still,
/// if any, goes back, and its queue's sender drops, which ends its carrier
/// once the queue is sent ([`carry`]).
fn remove(table: &mut Table, address: SocketAddrV6) {
    if let Some(tunnel) = table.by_address.remove(&address)
        && tunnel.taken
    {
        assert!(table.taken > 0);
        table.taken -= 1;
    }
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

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use dassie_node_core::connections::ConnectionLimits;

    use super::*;
    use crate::slots::ClientSlots;

    /// Places for the tunnels of these tests, as many as they take.
    fn places() -> ClientSlots {
        ClientSlots::new(ConnectionLimits {
            max_total: 8,
            max_per_client: 8,
        })
    }

    fn at(port: u16) -> SocketAddrV6 {
        v6(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
    }

    // A node's request for a tunnel asks for its subprotocol, and carries no
    // origin, with which a node refuses it as a browser's.
    #[test]
    fn a_nodes_handshake_carries_no_origin() {
        for (host, port) in [("a.example", 443), ("2001:db8::1", 8443)] {
            let request = handshake(host, port).unwrap();
            assert_eq!(request.uri().path(), PATH);
            assert!(!request.headers().contains_key(ORIGIN));
            assert_eq!(
                request.headers().get(SEC_WEBSOCKET_PROTOCOL).unwrap(),
                SUBPROTOCOL
            );
        }
    }

    // A packet for a tunnel whose queue is full is dropped, as UDP drops
    // it: the endpoint never waits for a client that does not read.
    #[test]
    fn a_full_tunnel_drops_rather_than_waits() {
        let socket = TunnelSocket::new();
        let (address, _id, mut queue, _ends) =
            socket.open_dialled(Ipv4Addr::LOCALHOST.into()).unwrap();
        for byte in 0..u8::try_from(QUEUE_PACKETS + 4).unwrap() {
            let packet = [byte; 32];
            let transmit = Transmit {
                destination: SocketAddr::V6(address),
                ecn: None,
                contents: &packet,
                segment_size: None,
                src_ip: None,
            };
            socket.try_send(&transmit).unwrap();
        }
        let mut queued = 0;
        while let Ok(packet) = queue.try_recv() {
            assert_eq!(packet[0], u8::try_from(queued).unwrap());
            queued += 1;
        }
        assert_eq!(queued, QUEUE_PACKETS);
    }

    // The tunnels taken are bounded by the endpoint's places: one whose
    // connection is not done holds a place, and is never displaced; one
    // that only lingers for its close to go out gives way, and its place
    // goes back.
    #[test]
    fn only_a_lingering_tunnel_gives_way() {
        let socket = TunnelSocket::new();
        let slots = places();
        let (_, mut first, ends) = socket.open(at(1), slots.take(None).unwrap(), 1).unwrap();
        assert!(socket.open(at(2), slots.take(None).unwrap(), 1).is_none());
        assert_eq!(slots.open_count(), 1);
        // The connection on it is done: the tunnel lingers.
        ends.connection.cancel();
        let (_, _second, _) = socket.open(at(2), slots.take(None).unwrap(), 1).unwrap();
        assert_eq!(slots.open_count(), 1);
        let table = lock(&socket.tunnels);
        assert_eq!(table.taken, 1);
        assert!(table.by_address.contains_key(&at(2)));
        drop(table);
        // Its carrier ends.
        assert!(first.try_recv().is_err());
        assert!(first.is_closed());
    }

    // A tunnel that gave way, closing once its carrier ends, leaves the one
    // at its address since alone.
    #[test]
    fn a_tunnel_closes_only_itself() {
        let socket = TunnelSocket::new();
        let slots = places();
        let (first, _queue, ends) = socket.open(at(1), slots.take(None).unwrap(), 4).unwrap();
        ends.connection.cancel();
        let (second, _queue, _ends) = socket.open(at(1), slots.take(None).unwrap(), 4).unwrap();
        assert_ne!(first, second);
        socket.close(at(1), first);
        assert!(lock(&socket.tunnels).by_address.contains_key(&at(1)));
        socket.close(at(1), second);
        assert!(lock(&socket.tunnels).by_address.is_empty());
        assert_eq!(slots.open_count(), 0);
    }
}

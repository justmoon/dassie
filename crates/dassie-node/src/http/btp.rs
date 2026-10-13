//! The BTP server at `/btp` (RFC 23, spec 3 §14): ILP clients such as
//! `ilp-plugin-btp` connect with `btp+wss://:<token>@<host>/btp`.
//!
//! The first message must authenticate within a few seconds: a BTP Message
//! whose first entry is an empty `auth` and which carries the token as
//! `auth_token`. Then each Message's `ilp` entry is a Prepare for the
//! connector, and each Response's `ilp` entry answers one of ours. A BTP
//! Transfer is refused: money moves only in ILP packets. Clients are not
//! browsers, so there is no Origin check; the token is the authentication.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use dassie_ilp::btp::{self, Body as BtpBody, ContentType, ProtocolData, ProtocolDataEntry};
use dassie_oer::{Decode, Encode};
use futures_util::{SinkExt, StreamExt};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::response::{self, Body};
use super::ws::{handshake_key, not_a_handshake, switching_protocols};
use crate::engine::{Engine, Outbound};
use crate::log::target;
use crate::node::Node;

/// The largest BTP message: an ILP packet and its envelope.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Messages queued for a client before it counts as not reading.
const OUTBOUND_QUEUE: usize = 256;

type Socket = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;

/// Answers the handshake, and serves the client once upgraded.
pub fn upgrade(
    node: &Arc<Node>,
    mut request: Request<Incoming>,
    peer: SocketAddr,
    tasks: &TaskTracker,
    shutdown: &CancellationToken,
) -> Response<Body> {
    let Some(key) = handshake_key(&request) else {
        return not_a_handshake();
    };
    let Some(engine) = node.engine() else {
        return response::text(StatusCode::SERVICE_UNAVAILABLE, "The ledger is not running");
    };
    if !node.btp_auth_allowed(peer.ip()) {
        return response::text(StatusCode::TOO_MANY_REQUESTS, "Too many attempts");
    }
    let Some(permit) = node.btp_permit() else {
        return response::text(StatusCode::SERVICE_UNAVAILABLE, "Too many BTP clients");
    };
    let upgrade = hyper::upgrade::on(&mut request);
    let auth_timeout = Duration::from_millis(node.policies().btp_auth_timeout_ms);
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let _permit = permit;
        match upgrade.await {
            Ok(upgraded) => {
                let config = WebSocketConfig::default()
                    .max_message_size(Some(MAX_MESSAGE_BYTES))
                    .max_frame_size(Some(MAX_MESSAGE_BYTES));
                let io = TokioIo::new(upgraded);
                let socket = WebSocketStream::from_raw_socket(io, Role::Server, Some(config)).await;
                serve(engine, socket, auth_timeout, shutdown).await;
            }
            Err(error) => tracing::debug!(target: target::BTP, %error, "BTP upgrade failed"),
        }
    });
    switching_protocols(&key, None)
}

async fn serve(
    engine: Engine,
    mut socket: Socket,
    auth_timeout: Duration,
    shutdown: CancellationToken,
) {
    let (outbound, mut queue) = mpsc::channel(OUTBOUND_QUEUE);
    let link = match authenticate(&engine, &mut socket, auth_timeout, outbound).await {
        Ok(link) => link,
        Err(reason) => {
            tracing::debug!(target: target::BTP, reason, "BTP client refused");
            close(&mut socket, CloseCode::Policy, reason).await;
            return;
        }
    };
    let end = loop {
        tokio::select! {
            // At shutdown the engine stops too: say why the node went.
            biased;
            () = shutdown.cancelled() => break Some((CloseCode::Away, "node shutting down")),
            () = engine.generation().cancelled() => break Some((CloseCode::Error, "ledger restarting")),
            message = queue.recv() => match message {
                Some(Outbound::Close) | None => break Some((CloseCode::Policy, "disconnected")),
                Some(message) => {
                    if socket.send(Message::Binary(encode(&message).into())).await.is_err() {
                        break None;
                    }
                }
            },
            incoming = socket.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    if let Err(reason) = receive(&engine, link, &mut socket, &bytes).await {
                        break Some((CloseCode::Protocol, reason));
                    }
                }
                Some(Ok(Message::Text(_))) => break Some((CloseCode::Unsupported, "BTP is binary")),
                Some(Ok(Message::Close(_)) | Err(_)) | None => break None,
                Some(Ok(_)) => {}
            },
        }
    };
    engine.btp_closed(link).await;
    if let Some((code, reason)) = end {
        close(&mut socket, code, reason).await;
    }
}

/// Waits for the auth message and checks its token.
async fn authenticate(
    engine: &Engine,
    socket: &mut Socket,
    timeout: Duration,
    outbound: mpsc::Sender<Outbound>,
) -> Result<u64, &'static str> {
    let first = tokio::time::timeout(timeout, socket.next())
        .await
        .map_err(|_| "no authentication")?;
    let Some(Ok(Message::Binary(bytes))) = first else {
        return Err("expected a BTP authentication message");
    };
    let packet = btp::Packet::decode_exact(&bytes).map_err(|_| "not a BTP packet")?;
    let token = btp::auth_token(&packet).ok_or("expected a BTP authentication message")?;
    let session = engine
        .btp_connect(token.to_string(), outbound)
        .await
        .map_err(|_| "authentication failed")?;
    let accepted = btp::Packet {
        request_id: packet.request_id,
        body: BtpBody::Response(ProtocolData::EMPTY),
    };
    socket
        .send(Message::Binary(accepted.to_vec().into()))
        .await
        .map_err(|_| "connection lost")?;
    Ok(session.link)
}

/// Passes a client's message on, or answers it with a BTP Error.
async fn receive(
    engine: &Engine,
    link: u64,
    socket: &mut Socket,
    bytes: &[u8],
) -> Result<(), &'static str> {
    let packet = btp::Packet::decode_exact(bytes).map_err(|_| "not a BTP packet")?;
    let ilp = packet.body.protocol_data().find("ilp");
    match (packet.body, ilp) {
        (BtpBody::Message(_), Some(ilp)) => {
            engine
                .btp_packet(link, packet.request_id, ilp.data.to_vec(), false)
                .await;
        }
        (BtpBody::Response(_), Some(ilp)) => {
            engine
                .btp_packet(link, packet.request_id, ilp.data.to_vec(), true)
                .await;
        }
        // An acknowledgement without ILP data; nothing waits for it.
        (BtpBody::Response(_), None) => {}
        (BtpBody::Error(error), _) => {
            tracing::debug!(target: target::BTP, link, code = error.code, name = error.name, "BTP error from client");
        }
        (BtpBody::Transfer { .. }, _) => {
            let refusal = Outbound::Error {
                request_id: packet.request_id,
                message: "BTP transfers are not supported: send ILP packets",
            };
            let _ = socket.send(Message::Binary(encode(&refusal).into())).await;
        }
        (BtpBody::Message(_), None) => {
            let refusal = Outbound::Error {
                request_id: packet.request_id,
                message: "expected ILP data",
            };
            let _ = socket.send(Message::Binary(encode(&refusal).into())).await;
        }
    }
    Ok(())
}

fn ilp(packet: &[u8]) -> ProtocolDataEntry<'_> {
    ProtocolDataEntry {
        protocol_name: "ilp",
        content_type: ContentType::OCTET_STREAM,
        data: packet,
    }
}

/// A message for the client, as a BTP packet.
fn encode(message: &Outbound) -> Vec<u8> {
    match message {
        Outbound::Prepare { request_id, packet } => {
            let entries = [ilp(packet)];
            btp::Packet {
                request_id: *request_id,
                body: BtpBody::Message(ProtocolData::Entries(&entries)),
            }
            .to_vec()
        }
        Outbound::Response { request_id, packet } => {
            let entries = [ilp(packet)];
            btp::Packet {
                request_id: *request_id,
                body: BtpBody::Response(ProtocolData::Entries(&entries)),
            }
            .to_vec()
        }
        Outbound::Error {
            request_id,
            message,
        } => btp::Packet {
            request_id: *request_id,
            body: BtpBody::Error(btp::Error {
                code: "F00",
                name: "NotAcceptedError",
                triggered_at: "",
                data: message.as_bytes(),
                protocol_data: ProtocolData::EMPTY,
            }),
        }
        .to_vec(),
        Outbound::Close => Vec::new(),
    }
}

async fn close(socket: &mut Socket, code: CloseCode, reason: &'static str) {
    let frame = CloseFrame {
        code,
        reason: reason.into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
    let _ = socket.close(None).await;
}

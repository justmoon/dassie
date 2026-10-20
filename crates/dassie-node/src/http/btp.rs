//! The BTP server at `/btp` (RFC 23, spec/areas/connector.md#the-btp-server):
//! ILP clients such as `ilp-plugin-btp` connect with
//! `btp+wss://:<token>@<host>/btp`.
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

use bytes::Bytes;
use dassie_ilp::btp::{self, Body as BtpBody, ContentType, ProtocolData, ProtocolDataEntry};
use dassie_ilp::{ErrorCode, Reject};
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

// The largest ILP packet of any type fits, with room for BTP's few octets
// of framing.
const _: () = assert!(dassie_ilp::MAX_PACKET_LENGTH + 1024 <= MAX_MESSAGE_BYTES);

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
    if node.btp_auth_blocked(peer.ip()) {
        return response::text(StatusCode::TOO_MANY_REQUESTS, "Too many failed attempts");
    }
    let Some(permit) = node.btp_permit() else {
        return response::text(StatusCode::SERVICE_UNAVAILABLE, "Too many BTP clients");
    };
    let upgrade = hyper::upgrade::on(&mut request);
    let node = Arc::clone(node);
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
                serve(&node, peer, engine, socket, shutdown).await;
            }
            Err(error) => tracing::debug!(target: target::BTP, %error, "BTP upgrade failed"),
        }
    });
    switching_protocols(&key, None)
}

async fn serve(
    node: &Node,
    peer: SocketAddr,
    engine: Engine,
    mut socket: Socket,
    shutdown: CancellationToken,
) {
    let (outbound, mut queue) = mpsc::channel(OUTBOUND_QUEUE);
    let auth_timeout = Duration::from_millis(node.policies().btp_auth_timeout_ms);
    let link = match authenticate(&engine, &mut socket, auth_timeout, outbound).await {
        Ok(link) => link,
        Err(Refused { reason, failed }) => {
            tracing::debug!(target: target::BTP, reason, "BTP client refused");
            // Only failed logins count against the client's address, so
            // that honest clients may reconnect as often as they need.
            if failed {
                node.btp_auth_failed(peer.ip());
            }
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

/// Why a client was not let in.
struct Refused {
    reason: &'static str,
    /// It failed to log in, rather than going away.
    failed: bool,
}

const fn failed(reason: &'static str) -> Refused {
    Refused {
        reason,
        failed: true,
    }
}

/// Waits for the auth message and checks its token.
async fn authenticate(
    engine: &Engine,
    socket: &mut Socket,
    timeout: Duration,
    outbound: mpsc::Sender<Outbound>,
) -> Result<u64, Refused> {
    // Control frames may come first: ilp-plugin-btp pings as soon as it
    // connects.
    let first = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => return other,
            }
        }
    };
    let first = tokio::time::timeout(timeout, first)
        .await
        .map_err(|_| failed("no authentication"))?;
    let bytes = match first {
        Some(Ok(Message::Binary(bytes))) => bytes,
        Some(Ok(Message::Close(_)) | Err(_)) | None => {
            return Err(Refused {
                reason: "connection lost",
                failed: false,
            });
        }
        Some(Ok(_)) => return Err(failed("expected a binary BTP message")),
    };
    let packet = btp::Packet::decode_exact(&bytes).map_err(|_| failed("not a BTP packet"))?;
    let token = btp::auth_token(&packet).ok_or(failed("expected BTP authentication"))?;
    let session = engine
        .btp_connect(token.to_string(), outbound)
        .await
        .map_err(|_| failed("authentication failed"))?;
    let accepted = btp::Packet {
        request_id: packet.request_id,
        body: BtpBody::Response(ProtocolData::EMPTY),
    };
    if socket
        .send(Message::Binary(accepted.to_vec().into()))
        .await
        .is_err()
    {
        // The engine has the link already: give it back.
        engine.btp_closed(session.link).await;
        return Err(Refused {
            reason: "connection lost",
            failed: false,
        });
    }
    Ok(session.link)
}

/// `part` of `whole`, sharing its memory rather than copying it: what the
/// BTP decoder borrows is a slice of the message. Were it not, it would be
/// copied rather than panic.
fn shared(whole: &Bytes, part: &[u8]) -> Bytes {
    let (outer, inner) = (whole.as_ptr_range(), part.as_ptr_range());
    if outer.start <= inner.start && inner.end <= outer.end {
        whole.slice_ref(part)
    } else {
        Bytes::copy_from_slice(part)
    }
}

/// Passes a client's message on, or answers it with a BTP Error.
async fn receive(
    engine: &Engine,
    link: u64,
    socket: &mut Socket,
    bytes: &Bytes,
) -> Result<(), &'static str> {
    let packet = btp::Packet::decode_exact(bytes).map_err(|_| "not a BTP packet")?;
    let ilp = packet.body.protocol_data().find("ilp");
    match (packet.body, ilp) {
        (BtpBody::Message(_), Some(ilp)) => {
            engine
                .btp_packet(link, packet.request_id, shared(bytes, ilp.data), false)
                .await;
        }
        (BtpBody::Response(_), Some(ilp)) => {
            engine
                .btp_packet(link, packet.request_id, shared(bytes, ilp.data), true)
                .await;
        }
        // An acknowledgement without ILP data; nothing waits for it.
        (BtpBody::Response(_), None) => {}
        // RFC 23: an Error answers our Message as a failure. The Prepare it
        // answers is rejected now, rather than held until it expires.
        (BtpBody::Error(error), _) => {
            tracing::debug!(target: target::BTP, link, code = error.code, name = error.name, "BTP error from client");
            let code = <[u8; 3]>::try_from(error.code.as_bytes())
                .ok()
                .and_then(ErrorCode::new)
                .unwrap_or(ErrorCode::T00_INTERNAL_ERROR);
            let reject = Reject {
                code,
                triggered_by: None,
                message: "the BTP client answered with an error",
                data: &[],
            };
            engine
                .btp_packet(link, packet.request_id, reject.to_vec().into(), true)
                .await;
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

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;

    // A packet goes to the engine as a slice of its WebSocket message,
    // without a copy; anything else is copied.
    #[test]
    fn a_slice_of_the_message_is_shared() {
        let whole = Bytes::from_static(b"0123456789");
        let part = shared(&whole, &whole[2..5]);
        assert_eq!(part.as_ptr(), whole[2..].as_ptr());
        assert_eq!(&part[..], b"234");
        assert_eq!(&shared(&whole, &[1, 2])[..], [1, 2]);
        assert!(shared(&whole, &whole[10..]).is_empty());
    }
}

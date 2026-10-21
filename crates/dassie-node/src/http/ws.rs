//! The RPC WebSocket at `/rpc`.
//!
//! The handshake is done here, so that it can be refused properly: a
//! cross-origin page gets 403 (RT-6), a client without the contract's
//! subprotocol 400. The socket is anonymous unless the session cookie is
//! valid; an anonymous socket may only ask for the node's basic state.
//!
//! Anyone can open an anonymous socket, since the Origin check binds only
//! browsers. So anonymous sockets have places of their own, fewer than all
//! sockets get and a few per client, and a lifetime: no client can hold
//! them all and lock the owner's browser out.
//!
//! Only binary messages carry the contract; a text message is a protocol
//! error, where the original threw inside its listener (RT-16).

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use dassie_node_core::session;
use futures_util::{SinkExt, StreamExt};
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::api::check_origin;
use super::response::{self, Body, header_has_token, header_str, header_values};
use crate::log::target;
use crate::node::Node;
use crate::rpc::connection::{self, Closed};
use crate::rpc::{Caller, MAX_MESSAGE_BYTES, SUBPROTOCOL};

/// The close code for "your session ended; log in again".
pub const SESSION_ENDED: u16 = 4001;

/// Whether `request` asks to upgrade to a WebSocket.
pub fn is_upgrade<B>(request: &Request<B>) -> bool {
    header_has_token(request.headers(), &header::CONNECTION, "upgrade")
}

/// The client's key, if the request is a WebSocket handshake.
pub(super) fn handshake_key<B>(request: &Request<B>) -> Option<String> {
    let headers = request.headers();
    let is_websocket = header_has_token(headers, &header::UPGRADE, "websocket")
        && header_str(headers, &header::SEC_WEBSOCKET_VERSION) == Some("13");
    header_str(headers, &header::SEC_WEBSOCKET_KEY)
        .filter(|_| is_websocket)
        .map(str::to_string)
}

/// The answer to a request that is not a WebSocket handshake.
pub(super) fn not_a_handshake() -> Response<Body> {
    response::text(StatusCode::BAD_REQUEST, "Expected a WebSocket handshake")
}

/// The 101 that completes the handshake for `key`.
pub(super) fn switching_protocols(key: &str, subprotocol: Option<&'static str>) -> Response<Body> {
    let accept = derive_accept_key(key.as_bytes());
    let mut response = response::with_body(StatusCode::SWITCHING_PROTOCOLS, "text/plain", "");
    let headers = response.headers_mut();
    headers.remove(header::CONTENT_TYPE);
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    if let Some(subprotocol) = subprotocol {
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(subprotocol),
        );
    }
    if let Ok(accept) = HeaderValue::from_str(&accept) {
        headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
    }
    response
}

/// Answers the handshake, and serves the socket once upgraded.
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
    let headers = request.headers();
    if !header_has_token(headers, &header::SEC_WEBSOCKET_PROTOCOL, SUBPROTOCOL) {
        return response::text(
            StatusCode::BAD_REQUEST,
            "Expected the dassie.ui.v1 subprotocol",
        );
    }
    if let Err(refusal) = check_origin(node, &request) {
        return refusal.into_response();
    }
    let token = session::find_session_cookie(header_values(headers, &header::COOKIE));
    let session = match token.map(|token| node.authenticate(&token)).transpose() {
        Ok(session) => session.flatten(),
        Err(error) => {
            tracing::error!(target: target::RPC, %error, "database failure at WebSocket login");
            return response::text(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
        }
    };
    // The socket outlives its HTTP connection, whose place is returned at
    // the upgrade; it holds places of its own until it closes.
    let Some(permit) = node.websocket_permit() else {
        return too_many_sockets();
    };
    let anonymous = match session {
        Some(_) => None,
        None => match node.anonymous_socket_slot(peer.ip()) {
            Ok(slot) => Some(slot),
            Err(refused) => {
                tracing::debug!(target: target::RPC, %peer, ?refused, "too many sockets without a session");
                return too_many_sockets();
            }
        },
    };
    let upgrade = hyper::upgrade::on(&mut request);
    let node = Arc::clone(node);
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let _places = (permit, anonymous);
        match upgrade.await {
            Ok(upgraded) => {
                serve(
                    node,
                    Caller::Browser {
                        session,
                        loopback: peer.ip().to_canonical().is_loopback(),
                    },
                    TokioIo::new(upgraded),
                    shutdown,
                )
                .await
            }
            Err(error) => tracing::debug!(target: target::RPC, %error, "WebSocket upgrade failed"),
        }
    });
    switching_protocols(&key, Some(SUBPROTOCOL))
}

fn too_many_sockets() -> Response<Body> {
    response::text(StatusCode::SERVICE_UNAVAILABLE, "Too many open sockets")
}

async fn serve<S>(node: Arc<Node>, caller: Caller, io: S, shutdown: CancellationToken)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let socket = WebSocketStream::from_raw_socket(io, Role::Server, Some(config)).await;
    let (mut sink, stream) = socket.split();
    // Only binary messages carry the contract. A text message becomes an
    // empty one, which decodes to a message without a kind: a protocol
    // error. Control frames are tungstenite's business.
    let incoming = stream
        .take_while(|message| std::future::ready(!matches!(message, Ok(Message::Close(_)))))
        .filter_map(|message| {
            std::future::ready(match message {
                Ok(Message::Binary(bytes)) => Some(Ok(bytes)),
                Ok(Message::Text(_)) => Some(Ok(Bytes::new())),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
        });
    let outgoing = (&mut sink).with(|bytes: Bytes| {
        std::future::ready(Ok::<_, tungstenite::Error>(Message::Binary(bytes)))
    });
    let closed = connection::serve(
        node,
        caller,
        Box::pin(incoming),
        Box::pin(outgoing),
        shutdown,
    )
    .await;
    let frame = match closed {
        Closed::ByClient => None,
        Closed::ProtocolError => Some((CloseCode::Protocol, "expected dassie.ui.v1 messages")),
        Closed::SessionEnded => Some((CloseCode::from(SESSION_ENDED), "session ended")),
        Closed::Shutdown => Some((CloseCode::Away, "node shutting down")),
        Closed::Idle => Some((CloseCode::Normal, "idle")),
        Closed::Expired => Some((CloseCode::Normal, "lifetime over")),
    };
    if let Some((code, reason)) = frame {
        let close = Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }));
        let _ = sink.send(close).await;
    }
    let _ = sink.close().await;
}

#[cfg(test)]
mod tests {
    use hyper::header::{HeaderMap, HeaderName};
    use proptest::prelude::*;

    use super::super::response::tests::headers;
    use super::*;

    const NAMES: &[HeaderName] = &[
        header::CONNECTION,
        header::UPGRADE,
        header::SEC_WEBSOCKET_VERSION,
        header::SEC_WEBSOCKET_KEY,
    ];

    /// A valid handshake's value of each of [`NAMES`].
    const VALID: [&str; 4] = ["Upgrade", "websocket", "13", "dGhlIHNhbXBsZSBub25jZQ=="];

    /// Headers of a handshake: any, with some of those it lacks filled in
    /// as a valid one has them, so that some are one.
    fn handshake() -> impl Strategy<Value = HeaderMap> {
        (headers(NAMES), any::<[bool; 4]>()).prop_map(|(mut headers, fill)| {
            for ((name, value), fill) in NAMES.iter().zip(VALID).zip(fill) {
                if fill {
                    headers
                        .entry(name)
                        .or_insert(HeaderValue::from_static(value));
                }
            }
            headers
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        // A handshake's headers are anyone's: they make one, or none,
        // without a panic (spec/decisions/tigerstyle.md).
        #[test]
        fn any_handshake_is_read(headers in handshake()) {
            let mut request = Request::new(());
            *request.headers_mut() = headers;
            let _ = is_upgrade(&request);
            if let Some(key) = handshake_key(&request) {
                let headers = request.headers();
                prop_assert_eq!(Some(key.as_str()), header_str(headers, &header::SEC_WEBSOCKET_KEY));
                prop_assert_eq!(header_str(headers, &header::SEC_WEBSOCKET_VERSION), Some("13"));
            }
        }
    }
}

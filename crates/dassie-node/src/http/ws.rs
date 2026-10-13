//! The RPC WebSocket at `/rpc`.
//!
//! The handshake is done here, so that it can be refused properly: a
//! cross-origin page gets 403 (RT-6), a client without the contract's
//! subprotocol 400. The socket is anonymous unless the session cookie is
//! valid; an anonymous socket may only ask for the node's basic state.
//!
//! Only binary messages carry the contract; a text message is a protocol
//! error, where the original threw inside its listener (RT-16).

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
pub fn is_upgrade(request: &Request<Incoming>) -> bool {
    header_has_token(request.headers(), &header::CONNECTION, "upgrade")
}

/// Answers the handshake, and serves the socket once upgraded.
pub fn upgrade(
    node: &Arc<Node>,
    mut request: Request<Incoming>,
    tasks: &TaskTracker,
    shutdown: &CancellationToken,
) -> Response<Body> {
    let headers = request.headers();
    let is_websocket = header_has_token(headers, &header::UPGRADE, "websocket")
        && header_str(headers, &header::SEC_WEBSOCKET_VERSION) == Some("13");
    let Some(key) = header_str(headers, &header::SEC_WEBSOCKET_KEY).filter(|_| is_websocket) else {
        return response::text(StatusCode::BAD_REQUEST, "Expected a WebSocket handshake");
    };
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
    // The socket outlives its HTTP connection, whose permit is returned at
    // the upgrade; it holds one of its own until it closes.
    let Some(permit) = node.websocket_permit() else {
        return response::text(StatusCode::SERVICE_UNAVAILABLE, "Too many open sockets");
    };
    let accept = derive_accept_key(key.as_bytes());
    let upgrade = hyper::upgrade::on(&mut request);
    let node = Arc::clone(node);
    let shutdown = shutdown.clone();
    tasks.spawn(async move {
        let _permit = permit;
        match upgrade.await {
            Ok(upgraded) => {
                serve(
                    node,
                    Caller::Browser(session),
                    TokioIo::new(upgraded),
                    shutdown,
                )
                .await
            }
            Err(error) => tracing::debug!(target: target::RPC, %error, "WebSocket upgrade failed"),
        }
    });
    let mut response = response::with_body(StatusCode::SWITCHING_PROTOCOLS, "text/plain", "");
    let headers = response.headers_mut();
    headers.remove(header::CONTENT_TYPE);
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
    headers.insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(SUBPROTOCOL),
    );
    if let Ok(accept) = HeaderValue::from_str(&accept) {
        headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
    }
    response
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

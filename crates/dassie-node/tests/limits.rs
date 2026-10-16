//! The limits that keep a node's resources bounded against clients that
//! misbehave (D16), and the ends of sessions that are not logouts. Each
//! test guards a defect a review of M2 found.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node_core::session::SessionPolicy;
use futures_util::StreamExt;
use support::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

fn key() -> NodeKey {
    NodeKey::from_bytes(&[7; 32])
}

/// The close code the socket gets next, skipping other messages.
async fn close_code(socket: &mut Socket) -> Option<u16> {
    let wait = async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| u16::from(frame.code)),
                Some(Ok(_)) => {}
                _ => return None,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect("the socket stayed open")
}

// WebSockets outlive the HTTP connection and its permit; they have their
// own cap.
#[tokio::test]
async fn websockets_are_capped() {
    let node = TestNode::start_with(Policies {
        max_websockets: 2,
        ..Policies::DEFAULT
    })
    .await;
    let first = websocket(&node, None, &node.origin()).await.unwrap();
    let _second = websocket(&node, None, &node.origin()).await.unwrap();
    let refused = websocket(&node, None, &node.origin()).await.unwrap_err();
    assert!(refused.contains("503"), "{refused}");
    drop(first);
    // The place comes back once the socket is gone.
    let mut again = Err(String::new());
    for _ in 0..50 {
        again = websocket(&node, None, &node.origin()).await;
        if again.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(again.is_ok(), "{again:?}");
    node.stop().await;
}

#[tokio::test]
async fn idle_sockets_close() {
    let node = TestNode::start_with(Policies {
        socket_idle_ms: 200,
        ..Policies::DEFAULT
    })
    .await;
    let mut socket = websocket(&node, None, &node.origin()).await.unwrap();
    assert_eq!(close_code(&mut socket).await, Some(1000));
    node.stop().await;
}

// A client that sends headers and then trickles the body must not hold its
// connection for ever.
#[tokio::test]
async fn slow_bodies_time_out() {
    let node = TestNode::start_with(Policies {
        body_timeout_ms: 200,
        ..Policies::DEFAULT
    })
    .await;
    let (mut stream, _) = connect_tls(node.https).await;
    let head = format!(
        "POST /api/setup HTTP/1.1\r\nHost: {}\r\nOrigin: {}\r\nContent-Type: application/json\r\n\
         Content-Length: 100\r\n\r\n{{\"a\":",
        node.host(),
        node.origin()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut reply = vec![0; 64];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut reply))
        .await
        .unwrap()
        .unwrap();
    let reply = String::from_utf8_lossy(&reply[..read]);
    assert!(reply.starts_with("HTTP/1.1 408"), "{reply}");
    node.stop().await;
}

// Sessions evicted to make room, like expired ones, close their sockets
// with the session-ended code, as the contract says.
#[tokio::test]
async fn evicted_sessions_close_their_sockets() {
    let sessions = SessionPolicy {
        max_sessions: 1,
        ..SessionPolicy::DEFAULT
    };
    let node = TestNode::start_with(Policies {
        sessions,
        ..Policies::DEFAULT
    })
    .await;
    let cookie = set_up(&node, &key()).await;
    let mut socket = websocket(&node, Some(&cookie), &node.origin())
        .await
        .unwrap();
    let login = send(
        node.https,
        true,
        post_json(&node, "/api/login", &login_body(&key())),
    )
    .await;
    assert_eq!(login.status, hyper::StatusCode::OK);
    assert_eq!(
        close_code(&mut socket).await,
        Some(dassie_node::http::SESSION_ENDED)
    );
    node.stop().await;
}

// Ending your own session answers the call before the socket closes.
#[tokio::test]
async fn revoking_your_own_session_answers_first() {
    let node = TestNode::start().await;
    let cookie = set_up(&node, &key()).await;
    let mut owner = RpcClient::new(
        websocket(&node, Some(&cookie), &node.origin())
            .await
            .unwrap(),
    );
    let revoke = proto::RevokeAllSessionsRequest {
        keep_current: false,
    };
    let reply = owner.call(Method::RevokeAllSessions(revoke)).await;
    let Ok(response::Method::RevokeAllSessions(revoked)) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(revoked.revoked, 1);
    let mut socket = owner.into_inner();
    assert_eq!(
        close_code(&mut socket).await,
        Some(dassie_node::http::SESSION_ENDED)
    );
    node.stop().await;
}

//! Stopping without losing the packets in flight
//! (spec/areas/node.md#shutdown): at SIGTERM the engine takes no new
//! packet and waits for those in flight, while the connections that carry
//! their answers stay up; a second SIGTERM stops it at once. Two BTP
//! clients of one node: Bob pays Alice, who holds the Prepare.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use dassie_identity::IdentityKey;
use dassie_ilp::{Address, ErrorCode, Fulfill, Prepare, Response, Timestamp};
use dassie_node::clock::Clock;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_oer::{Decode, Encode};
use support::btp_client::{BtpClient, Received};
use support::*;

const FULFILLMENT: [u8; 32] = [5; 32];

fn key() -> IdentityKey {
    dassie_identity::TableKeygen::identity_key(&[9; 32])
}

/// A node with the stub, and a BTP token for it.
async fn ready(node: &TestNode) -> String {
    set_up(node, &key()).await;
    node.engine_started().await;
    let mut owner = node.ipc().await;
    let reply = owner
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: "stub".into(),
                config: STUB_UNLIMITED.into(),
            },
        ))
        .await;
    assert!(matches!(
        reply,
        Ok(response::Method::AddSettlementScheme(_))
    ));
    let reply = owner
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "test".into(),
            ledger: "stub+usd".into(),
            cap: None,
        }))
        .await;
    let Ok(response::Method::CreateBtpToken(created)) = reply else {
        panic!("{reply:?}")
    };
    created.secret
}

/// Alice and Bob, and Bob's Prepare of `amount` to Alice, which she holds.
struct Paying {
    alice: BtpClient,
    bob: BtpClient,
    /// Alice's address.
    address: String,
    /// Bob's request ID for the Prepare, and Alice's for the one she holds.
    sent: u32,
    held: u32,
}

async fn paying(node: &TestNode, secret: &str) -> Paying {
    let mut alice = BtpClient::connect(node, secret).await.unwrap();
    let mut bob = BtpClient::connect(node, secret).await.unwrap();
    let now = node.clock.now_ms();
    let (address, ..) = alice.ildcp(now).await;
    bob.ildcp(now).await;
    let sent = bob.prepare(&prepare(&address, now)).await;
    let arrived = alice.receive().await.unwrap();
    assert!(!arrived.response, "Alice holds a Prepare");
    Paying {
        alice,
        bob,
        address,
        sent,
        held: arrived.request_id,
    }
}

/// A Prepare of ten to Alice's `address`, expiring in 20 s.
fn prepare(address: &str, now: u64) -> Vec<u8> {
    let destination = format!("{address}.x");
    Prepare {
        amount: 10,
        expires_at: Timestamp::from_unix_ms(now + 20_000),
        execution_condition: dassie_ilp::condition_for(&FULFILLMENT),
        destination: Address::parse(&destination).unwrap(),
        data: b"",
    }
    .to_vec()
}

/// The response a client received, decoded.
fn answer(received: &Received) -> Response<'_> {
    assert!(received.response, "a response");
    Response::decode_exact(&received.ilp).unwrap()
}

fn rejected_with(received: &Received) -> ErrorCode {
    match answer(received) {
        Response::Reject(reject) => reject.code,
        Response::Fulfill(_) => panic!("fulfilled"),
    }
}

// The node waits for the packet in flight, refusing new packets and new
// clients meanwhile; once Alice fulfills it, Bob gets the Fulfill over
// the connection that stayed up, and the node stops, saying why.
#[tokio::test]
async fn a_stopping_node_waits_for_its_packets_in_flight() {
    let node = TestNode::start().await;
    let secret = ready(&node).await;
    let mut paying = paying(&node, &secret).await;
    node.begin_stop();
    node.draining().await;
    let now = node.clock.now_ms();
    let refused = paying.bob.prepare(&prepare(&paying.address, now)).await;
    let reply = paying.bob.receive().await.unwrap();
    assert_eq!(reply.request_id, refused);
    assert_eq!(rejected_with(&reply), ErrorCode::T03_CONNECTOR_BUSY);
    assert_eq!(BtpClient::connect(&node, &secret).await.err(), Some(None));
    assert!(node.running(), "the daemon stopped with a packet in flight");
    let fulfill = Fulfill {
        fulfillment: FULFILLMENT,
        data: b"",
    }
    .to_vec();
    paying.alice.send(paying.held, &fulfill, true).await;
    let reply = paying.bob.receive().await.unwrap();
    assert_eq!(reply.request_id, paying.sent);
    assert!(matches!(answer(&reply), Response::Fulfill(_)));
    assert_eq!(paying.bob.closed().await, Some(1001));
    assert_eq!(paying.alice.closed().await, Some(1001));
    node.stop().await;
}

// Unanswered, the packet expires: Bob is told, and the node stops then, at
// the latest expiry, on its clock.
#[tokio::test]
async fn a_drain_ends_when_the_packets_in_flight_expire() {
    let node = TestNode::start().await;
    let secret = ready(&node).await;
    let mut paying = paying(&node, &secret).await;
    node.begin_stop();
    node.draining().await;
    node.clock.advance(20_000);
    let reply = paying.bob.receive().await.unwrap();
    assert_eq!(reply.request_id, paying.sent);
    assert_eq!(rejected_with(&reply), ErrorCode::R00_TRANSFER_TIMED_OUT);
    assert_eq!(paying.bob.closed().await, Some(1001));
    node.stop().await;
}

// A second signal stops the node at once, the packet still in flight.
#[tokio::test]
async fn stopping_at_once_cuts_the_drain_short() {
    let node = TestNode::start().await;
    let secret = ready(&node).await;
    let mut paying = paying(&node, &secret).await;
    node.begin_stop();
    node.draining().await;
    node.stop_now();
    assert!(paying.bob.closed().await.is_some());
    node.stop().await;
}

// A drain the owner asked for, as the desktop app's Quit does, can be
// taken back: the node takes packets again, and never stopped.
#[tokio::test]
async fn a_drain_asked_for_can_be_taken_back() {
    let node = TestNode::start().await;
    let secret = ready(&node).await;
    let mut paying = paying(&node, &secret).await;
    let held = node.node.upgrade().unwrap();
    held.request_drain(true);
    drop(held);
    node.draining().await;
    let now = node.clock.now_ms();
    paying.bob.prepare(&prepare(&paying.address, now)).await;
    let reply = paying.bob.receive().await.unwrap();
    assert_eq!(rejected_with(&reply), ErrorCode::T03_CONNECTOR_BUSY);
    let held = node.node.upgrade().unwrap();
    held.request_drain(false);
    let mut stopping = held.stopping();
    drop(held);
    tokio::time::timeout(HANG, stopping.wait_for(Option::is_none))
        .await
        .expect("the drain was not taken back")
        .unwrap();
    paying.bob.prepare(&prepare(&paying.address, now)).await;
    let arrived = paying.alice.receive().await.unwrap();
    assert!(!arrived.response, "the Prepare reached Alice");
    assert!(node.running());
    node.stop_now();
    node.stop().await;
}

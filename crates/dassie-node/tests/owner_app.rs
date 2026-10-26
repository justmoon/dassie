//! Owner apps (spec/areas/rpc.md#paired-devices): the owner starts a
//! pairing on the node's own machine, an app with a device key claims it on
//! a stream with purpose `owner`, through a relay or dialled directly, and
//! then calls the RPC as the owner until it is revoked.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::future::Future;
use std::net::{IpAddr, UdpSocket};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::{NodeId, NodeKey};
use dassie_kit::ClientKey;
use dassie_node::clock::ManualClock;
use dassie_node::front_door::key;
use dassie_node::node::{PAIRING_LIFETIME_MS, Policies};
use dassie_node::outbound::Resolver;
use dassie_node::owner_app::{self, Channel, Route};
use dassie_node::peer_server::Answer;
use dassie_node::quic::relay::RelayError;
use dassie_node::quic::{Body, Handler, Quic};
use dassie_node::rpc::proto::{self, ErrorCode, request::Method, response};
use dassie_node::store::MAX_DEVICES;
use dassie_node_core::connections::ConnectionLimits;
use futures_util::{SinkExt, StreamExt};
use support::op::CA;
use support::{Bound, RpcClient, START_MS, TestNode, Transport, set_up, websocket};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

impl Transport for Channel {
    async fn send_bytes(&mut self, bytes: Bytes) {
        self.send(bytes).await.unwrap();
    }

    async fn receive_bytes(&mut self) -> Option<Bytes> {
        self.next().await?.ok().map(bytes::BytesMut::freeze)
    }
}

/// A QUIC endpoint that only relays, as every node's does.
struct Relay;

impl Handler for Relay {
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
        _body: Body<'a>,
    ) -> Pin<Box<dyn Future<Output = Answer> + Send + 'a>> {
        Box::pin(async {
            Answer {
                status: 404,
                body: Vec::new(),
            }
        })
    }
}

/// An endpoint with key `[seed; 32]`, serving `handler` if given, and its
/// URL.
fn endpoint(
    seed: u8,
    handler: Option<Arc<dyn Handler>>,
    shutdown: &CancellationToken,
) -> (Arc<Quic>, NodeKey, String) {
    let key = NodeKey::from_bytes(&[seed; 32]);
    // The endpoint keeps the last key its sender held.
    let (_identity, receiver) = watch::channel(Some(Arc::new(NodeKey::from_bytes(&[seed; 32]))));
    let quic =
        Arc::new(Quic::new(UdpSocket::bind("127.0.0.1:0").unwrap(), receiver, true).unwrap());
    let url = format!("https://{}", quic.local_addr().unwrap());
    if let Some(handler) = handler {
        tokio::spawn(Arc::clone(&quic).serve(handler, shutdown.clone()));
    }
    (quic, key, url)
}

/// A device: an endpoint whose key is its device key.
fn device(seed: u8, shutdown: &CancellationToken) -> (Arc<Quic>, NodeId) {
    let (quic, key, _) = endpoint(seed, None, shutdown);
    (quic, key.node_id())
}

/// QUIC trusts no roots: each connection pins its node's key, and the
/// resolver only resolves names, private ones included.
fn resolver() -> Resolver {
    Resolver::new(true)
}

/// A node that is set up, and its node ID.
async fn node(env: &[(&str, &str)]) -> (TestNode, NodeId) {
    node_with(Policies::DEV, env).await
}

async fn node_with(policies: Policies, env: &[(&str, &str)]) -> (TestNode, NodeId) {
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 5, policies, env)
        .await;
    let key = NodeKey::from_bytes(&[50; 32]);
    set_up(&node, &key).await;
    (node, key.node_id())
}

fn direct(node: &TestNode) -> String {
    format!("https://{}", node.https)
}

async fn connect_direct(quic: &Arc<Quic>, node: &TestNode, id: NodeId) -> RpcClient<Channel> {
    let url = direct(node);
    let channel = owner_app::connect(quic, id, Route::Direct(&url), &resolver())
        .await
        .unwrap();
    RpcClient::new(channel)
}

/// Starts a pairing over IPC: the code and its secret.
async fn start_pairing(node: &TestNode) -> (String, String) {
    let Ok(response::Method::StartPairing(started)) = node
        .ipc()
        .await
        .call(Method::StartPairing(proto::StartPairingRequest {}))
        .await
    else {
        panic!("no pairing")
    };
    let secret = secret_of(&started.code);
    (started.code, secret)
}

fn secret_of(code: &str) -> String {
    code.split(['?', '&'])
        .find_map(|part| part.strip_prefix("secret="))
        .unwrap()
        .to_string()
}

async fn claim<T: Transport>(
    client: &mut RpcClient<T>,
    secret: &str,
    name: &str,
) -> Result<response::Method, proto::Error> {
    client
        .call(Method::ClaimPairing(proto::ClaimPairingRequest {
            secret: secret.into(),
            name: name.into(),
        }))
        .await
}

async fn devices<T: Transport>(client: &mut RpcClient<T>) -> Vec<proto::Device> {
    match client
        .call(Method::ListDevices(proto::ListDevicesRequest {}))
        .await
    {
        Ok(response::Method::ListDevices(listed)) => listed.devices,
        other => panic!("no devices: {other:?}"),
    }
}

fn code(result: Result<response::Method, proto::Error>) -> ErrorCode {
    result.unwrap_err().code()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_pairs_directly_and_then_calls_as_the_owner() {
    let shutdown = CancellationToken::new();
    let (node, id) = node(&[]).await;
    let (quic, device_id) = device(60, &shutdown);
    let mut app = connect_direct(&quic, &node, id).await;
    // Before it pairs, a device may ask for the basic state and claim a
    // pairing, nothing else.
    assert_eq!(app.state().await, proto::NodeState::Anonymous);
    assert_eq!(
        code(
            app.call(Method::GetStatus(proto::GetStatusRequest {}))
                .await
        ),
        ErrorCode::Unauthenticated
    );
    let (code_text, secret) = start_pairing(&node).await;
    assert!(code_text.starts_with(&format!("dassie-pair:{id}?secret=")));
    claim(&mut app, &secret, "Phone").await.unwrap();
    assert_eq!(app.state().await, proto::NodeState::Authenticated);
    app.call(Method::GetStatus(proto::GetStatusRequest {}))
        .await
        .unwrap();
    let listed = devices(&mut app).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, device_id.as_str());
    assert_eq!(listed[0].name, "Phone");
    // A device cannot start a pairing: its access would outlive its own.
    assert_eq!(
        code(
            app.call(Method::StartPairing(proto::StartPairingRequest {}))
                .await
        ),
        ErrorCode::FailedPrecondition
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_pairs_through_the_relay_the_node_keeps() {
    let shutdown = CancellationToken::new();
    let (_relay, relay_key, relay_url) = endpoint(70, Some(Arc::new(Relay)), &shutdown);
    let entry = key::relay_entry(&relay_key, &relay_url);
    // The test CA: the node's outbound client needs roots, and a build sandbox
    // has no system ones.
    let (node, id) = node(&[
        ("DASSIE_EXTRA_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
        ("DASSIE_RELAY", entry.as_str()),
    ])
    .await;
    let (quic, _) = device(61, &shutdown);
    let route = Route::Relay(relay_key.node_id(), &relay_url);
    let mut channel = None;
    for _ in 0..500 {
        match owner_app::connect(&quic, id, route, &resolver()).await {
            Ok(opened) => {
                channel = Some(opened);
                break;
            }
            // The node has not connected to its relay yet.
            Err(RelayError::UnknownDestination) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => panic!("{error:?}"),
        }
    }
    let mut app = RpcClient::new(channel.expect("the node never reached its relay"));
    let (_, secret) = start_pairing(&node).await;
    claim(&mut app, &secret, "Laptop").await.unwrap();
    assert_eq!(devices(&mut app).await.len(), 1);
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pairing_works_once_and_only_while_it_lasts() {
    let shutdown = CancellationToken::new();
    let (node, id) = node(&[]).await;
    let (first, _) = device(62, &shutdown);
    let (second, _) = device(63, &shutdown);
    let mut first = connect_direct(&first, &node, id).await;
    let mut second = connect_direct(&second, &node, id).await;

    let (_, secret) = start_pairing(&node).await;
    let wrong = "00".repeat(32);
    assert_eq!(
        code(claim(&mut first, &wrong, "A").await),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(claim(&mut first, "zz", "A").await),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(claim(&mut first, &secret, "").await),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        code(claim(&mut first, &secret, &"x".repeat(65)).await),
        ErrorCode::InvalidArgument
    );
    claim(&mut first, &secret, "A").await.unwrap();
    // Used up.
    assert_eq!(
        code(claim(&mut second, &secret, "B").await),
        ErrorCode::NotFound
    );

    // A new pairing replaces one not claimed yet.
    let (_, replaced) = start_pairing(&node).await;
    let (_, current) = start_pairing(&node).await;
    assert_eq!(
        code(claim(&mut second, &replaced, "B").await),
        ErrorCode::NotFound
    );

    // And expires.
    node.clock.advance(PAIRING_LIFETIME_MS);
    assert_eq!(
        code(claim(&mut second, &current, "B").await),
        ErrorCode::NotFound
    );
    let (_, fresh) = start_pairing(&node).await;
    claim(&mut second, &fresh, "B").await.unwrap();
    assert_eq!(devices(&mut node.ipc().await).await.len(), 2);
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_list_refuses_a_new_device() {
    let shutdown = CancellationToken::new();
    let (node, id) = node(&[]).await;
    let seeds = 100..100 + u8::try_from(MAX_DEVICES).unwrap();
    for seed in seeds {
        let (quic, _) = device(seed, &shutdown);
        let mut app = connect_direct(&quic, &node, id).await;
        let (_, secret) = start_pairing(&node).await;
        claim(&mut app, &secret, "D").await.unwrap();
    }
    let (quic, _) = device(99, &shutdown);
    let mut app = connect_direct(&quic, &node, id).await;
    let (_, secret) = start_pairing(&node).await;
    assert_eq!(
        code(claim(&mut app, &secret, "One too many").await),
        ErrorCode::ResourceExhausted
    );
    assert_eq!(
        devices(&mut node.ipc().await).await.len(),
        usize::try_from(MAX_DEVICES).unwrap()
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_a_device_closes_its_connection() {
    let shutdown = CancellationToken::new();
    let (node, id) = node(&[]).await;
    let (quic, device_id) = device(64, &shutdown);
    let mut app = connect_direct(&quic, &node, id).await;
    let (_, secret) = start_pairing(&node).await;
    claim(&mut app, &secret, "Phone").await.unwrap();

    node.ipc()
        .await
        .call(Method::RevokeDevice(proto::RevokeDeviceRequest {
            id: device_id.as_str().into(),
        }))
        .await
        .unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(5), app.receive()).await;
    assert!(
        matches!(closed, Ok(None)),
        "the connection stayed open: {closed:?}"
    );
    assert!(devices(&mut node.ipc().await).await.is_empty());

    // It connects again as a device that is not paired.
    let mut app = connect_direct(&quic, &node, id).await;
    assert_eq!(
        code(
            app.call(Method::GetStatus(proto::GetStatusRequest {}))
                .await
        ),
        ErrorCode::Unauthenticated
    );
    assert_eq!(
        code(
            node.ipc()
                .await
                .call(Method::RevokeDevice(proto::RevokeDeviceRequest {
                    id: device_id.as_str().into(),
                }))
                .await
        ),
        ErrorCode::NotFound
    );
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_on_this_machine_starts_a_pairing() {
    let node = Bound::new()
        .start(ManualClock::new(START_MS), 5, Policies::DEV, &[])
        .await;
    // Not before setup.
    assert_eq!(
        code(
            node.ipc()
                .await
                .call(Method::StartPairing(proto::StartPairingRequest {}))
                .await
        ),
        ErrorCode::FailedPrecondition
    );
    let cookie = set_up(&node, &NodeKey::from_bytes(&[50; 32])).await;
    // A browser on loopback may; the tests have no other address to come
    // from, so the rule for others is the handlers' unit test.
    let socket = websocket(&node, Some(&cookie), &node.origin())
        .await
        .unwrap();
    let mut browser = RpcClient::new(socket);
    let Ok(response::Method::StartPairing(started)) = browser
        .call(Method::StartPairing(proto::StartPairingRequest {}))
        .await
    else {
        panic!("no pairing")
    };
    assert!(started.code.starts_with("dassie-pair:"));
    // A browser cannot claim one: claiming proves a device key.
    assert_eq!(
        code(claim(&mut browser, &secret_of(&started.code), "Tab").await),
        ErrorCode::FailedPrecondition
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_not_paired_cannot_hold_its_stream_for_ever() {
    let shutdown = CancellationToken::new();
    let policies = Policies {
        anonymous_socket_lifetime_ms: 300,
        ..Policies::DEV
    };
    let (node, id) = node_with(policies, &[]).await;
    let (quic, _) = device(65, &shutdown);
    // Not paired: the stream ends after the lifetime, whatever it sends.
    let mut app = connect_direct(&quic, &node, id).await;
    assert_eq!(app.state().await, proto::NodeState::Anonymous);
    let closed = tokio::time::timeout(Duration::from_secs(5), app.receive()).await;
    assert!(
        matches!(closed, Ok(None)),
        "the stream stayed open: {closed:?}"
    );

    // Paired within it, the app connects again, and keeps that stream. The
    // pairing starts first, so that the claim alone races the lifetime.
    let (_, secret) = start_pairing(&node).await;
    let mut app = connect_direct(&quic, &node, id).await;
    claim(&mut app, &secret, "Phone").await.unwrap();
    let mut app = connect_direct(&quic, &node, id).await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert_eq!(app.state().await, proto::NodeState::Authenticated);
    shutdown.cancel();
}

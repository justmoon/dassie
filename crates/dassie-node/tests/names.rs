//! Names between daemons, over real QUIC and HTTPS
//! (spec/areas/payments.md#the-nodes-names): four bootstrap daemons are the
//! registrars, a periphery daemon registers a name as its owner asks, every
//! node takes it as the name's holder, and the node answers Open Payments
//! under `<name>.dassiepay.net`, signed so that another node's client takes
//! the answer only from the holder in its own view
//! (spec/areas/payments.md#signed-responses).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::time::Duration;

use dassie_identity::{NodeId, NodeKey};
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::open_payments::client::{self, ClientError};
use dassie_node::peer_client::WebResponse;
use dassie_node::rpc::proto::{self, request::Method};
use dassie_payments::httpsig;
use dassie_peer::names::Name;
use hyper::StatusCode;
use serde_json::json;
use support::op::{add_stub, body, get_under};
use support::pair::{CA, bootstrap, key, prepare};
use support::{Bound, START_MS, TestNode, set_up};

const NONCE: &str = "names-nonce";

/// `DASSIE_BOOTSTRAP_NODES` naming every one of `nodes` at its port.
fn bootstrap_list(nodes: &[(&NodeKey, u16)]) -> String {
    let entries: Vec<String> = nodes
        .iter()
        .map(|(key, port)| {
            let one = bootstrap(key, *port);
            one[1..one.len() - 1].to_string()
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// Whom `node` takes as `name`'s holder.
async fn holder(node: &TestNode, name: Name) -> Option<NodeId> {
    let engine = node.node.upgrade()?.engine()?;
    engine.name_holder(name).await.ok().flatten()
}

/// A GET of `path` under `name` at `node`, with [`NONCE`].
async fn fetch(node: &TestNode, name: &str, path: &str) -> support::Reply {
    let mut request = get_under(name, path);
    request
        .headers_mut()
        .insert(httpsig::NONCE_FIELD, NONCE.parse().unwrap());
    support::send(node.https, true, request).await
}

/// Checks `reply`, to a GET of `url` with [`NONCE`], as `node`'s client
/// does: signed by the node `node` expects at the URL.
async fn checked_by(node: &TestNode, url: &str, reply: &support::Reply) -> Result<(), ClientError> {
    let own = node.node.upgrade().unwrap();
    let signer = client::signer(&own, url).await?.expect("a front door URL");
    let signature_fields = reply
        .headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_string()))
        .collect();
    let response = WebResponse {
        status: reply.status.as_u16(),
        media_type: None,
        signature_fields,
        body: reply.body.to_vec(),
    };
    let now_s = node.clock.now_ms() / 1_000;
    client::check_signed(signer, (&http::Method::GET, url, NONCE), &response, now_s)
}

/// Moves the shared clock on, a second at a time, giving the daemons real
/// time to talk, until `done` holds: long enough for a commitment's window
/// to close and the lists to reach every node.
async fn until(clock: &ManualClock, mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..1_500 {
        if done().await {
            return;
        }
        clock.advance(1_000);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("not reached in time");
}

/// Four bootstrap daemons, the registrars, set up as core nodes, the keys
/// of `seeds`; and a periphery daemon, with no hostname and the stub
/// ledger, the key of `periphery`. All on one clock.
async fn start(seeds: [u8; 4], periphery: u8) -> (Vec<TestNode>, TestNode, ManualClock) {
    let keys = seeds.map(key);
    let bounds: Vec<Bound> = (0..4).map(|_| Bound::new()).collect();
    let nodes = bootstrap_list(
        &keys
            .iter()
            .zip(&bounds)
            .map(|(key, bound)| (key, bound.https_port()))
            .collect::<Vec<_>>(),
    );
    let env = [
        ("DASSIE_BOOTSTRAP_NODES", nodes.as_str()),
        ("DASSIE_PEER_CA_FILE", CA),
        ("DASSIE_PEER_ALLOW_PRIVATE", "1"),
    ];
    let clock = ManualClock::new(START_MS);
    let mut registrars = Vec::new();
    for ((bound, key), seed) in bounds.into_iter().zip(&keys).zip(seeds) {
        let seed = u64::from(seed);
        let node = bound.start(clock.clone(), seed, Policies::DEV, &env).await;
        prepare(&node, key).await;
        registrars.push(node);
    }
    let p = Bound::new()
        .start(clock.clone(), u64::from(periphery), Policies::DEV, &env)
        .await;
    set_up(&p, &key(periphery)).await;
    add_stub(&p).await;
    (registrars, p, clock)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_answers_under_a_name_it_registered() {
    let (registrars, p, clock) = start([61, 62, 63, 64], 65).await;
    let id_p = key(65).node_id();
    let alice = Name::parse("alice").unwrap();
    const HOST: &str = "alice.dassiepay.net";
    let url = format!("https://{HOST}/.well-known/pay");
    // Not the node's yet: misdirected, and no other node's client asks
    // under it.
    assert_eq!(
        fetch(&p, HOST, "/.well-known/pay").await.status,
        StatusCode::MISDIRECTED_REQUEST
    );
    let a = &registrars[0];
    assert_eq!(
        client::signer(&a.node.upgrade().unwrap(), &url).await,
        Err(ClientError::UnknownName)
    );
    let registered = p
        .ipc()
        .await
        .call(Method::RegisterName(proto::RegisterNameRequest {
            name: alice.to_string(),
        }))
        .await;
    assert!(registered.is_ok(), "{registered:?}");
    until(&clock, async || {
        for node in registrars.iter().chain([&p]) {
            if holder(node, alice).await != Some(id_p) {
                return false;
            }
        }
        // The node answers under it as soon as its view gives it the name.
        fetch(&p, HOST, "/.well-known/pay").await.status == StatusCode::OK
    })
    .await;
    let reply = fetch(&p, HOST, "/.well-known/pay").await;
    let address = body(&reply);
    assert_eq!(address["id"], url);
    assert_eq!(address["authServer"], format!("https://{HOST}/gnap"));
    assert_eq!(address["dassie"], json!({ "nodeId": id_p.as_str() }));
    // Every registrar's client takes the answer as the holder's.
    for node in &registrars {
        assert_eq!(checked_by(node, &url, &reply).await, Ok(()));
    }
    // Not for another name, which no node holds, nor as another node's.
    let bob = "https://bob.dassiepay.net/.well-known/pay";
    assert_eq!(
        checked_by(a, bob, &reply).await,
        Err(ClientError::UnknownName)
    );
    let other_url = format!(
        "https://{}.dassiepay.net/.well-known/pay",
        key(62).node_id().label().unwrap()
    );
    assert!(matches!(
        checked_by(a, &other_url, &reply).await,
        Err(ClientError::Invalid(_))
    ));
    // The default name stays the main one: the node's name to its owner,
    // and as a client of other wallets.
    let status = p
        .ipc()
        .await
        .call(Method::GetStatus(proto::GetStatusRequest {}))
        .await;
    let Ok(proto::response::Method::GetStatus(status)) = status else {
        panic!("{status:?}")
    };
    let label = format!("https://{}.dassiepay.net", id_p.label().unwrap());
    assert_eq!(
        status.open_payments_origins,
        [label, format!("https://{HOST}")]
    );
    for node in registrars {
        node.stop().await;
    }
    p.stop().await;
}

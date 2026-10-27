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

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;

use dassie_identity::{NodeId, NodeKey};
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::open_payments::client::{self, ClientError};
use dassie_node::open_payments::keys::{self, KeyError};
use dassie_node::outbound::{WebError, WebRequest, WebResponse};
use dassie_node::rpc::proto::{self, request::Method};
use dassie_payments::httpsig;
use dassie_peer::names::Name;
use http_body_util::Full;
use hyper::{Request, StatusCode};
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

/// `reply` as the guarded client reads it.
fn web_response(reply: &support::Reply) -> WebResponse {
    let signature_fields = reply
        .headers
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_string()))
        .collect();
    WebResponse {
        status: reply.status.as_u16(),
        media_type: None,
        signature_fields,
        body: reply.body.to_vec(),
    }
}

/// A GET of `url`.
fn get(url: &str) -> WebRequest {
    WebRequest {
        method: http::Method::GET,
        url: url.to_string(),
        headers: Vec::new(),
        body: None,
        timeout_ms: 10_000,
        max_response_bytes: 64 * 1024,
    }
}

/// A GET of `url` by `node`'s Open Payments client, as it sends every
/// request to another wallet, which `answer` answers in place of the
/// guarded client: `dassiepay.net` does not resolve in tests.
async fn fetched_by<R>(
    node: &TestNode,
    url: &str,
    answer: impl FnOnce(WebRequest) -> R,
) -> Result<WebResponse, ClientError>
where
    R: Future<Output = Result<WebResponse, WebError>>,
{
    let own = node.node.upgrade().unwrap();
    client::exchange_through(&own, get(url), answer).await
}

/// Passes `request` on to `to`, as a front door would: to its HTTPS port,
/// under the host of the request's URL.
async fn pass_on(to: SocketAddr, request: WebRequest) -> Result<WebResponse, WebError> {
    let url: http::Uri = request.url.parse().unwrap();
    let mut passed = Request::builder()
        .method(request.method)
        .uri(url.path_and_query().unwrap().as_str())
        .header("host", url.authority().unwrap().as_str());
    for (name, value) in request.headers {
        passed = passed.header(name, value);
    }
    let body = Full::new(Bytes::from(request.body.unwrap_or_default()));
    let reply = support::send(to, true, passed.body(body).unwrap()).await;
    Ok(web_response(&reply))
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
        ("DASSIE_EXTRA_CA_FILE", CA),
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

/// Every registrar's client takes the holder's answer at `url`, passed on
/// by a front door, and no other: `reply` is the holder's answer to an
/// earlier request.
async fn taken_only_from_the_holder(
    registrars: &[TestNode],
    holder: &TestNode,
    url: &str,
    reply: &support::Reply,
) {
    let a = &registrars[0];
    // Every registrar's client takes the holder's answer, passed on by a
    // front door, with a nonce of its own: the request the node signs.
    let https = holder.https;
    for node in registrars {
        let fetched = fetched_by(node, url, |request| pass_on(https, request)).await;
        assert_eq!(fetched.map(|answer| answer.body), Ok(reply.body.to_vec()));
    }
    // An answer the holder gave another request is refused, as is one the
    // front door makes up.
    let replayed = web_response(reply);
    let refused = Err(ClientError::Invalid(
        "the answer is not the node's at that name".into(),
    ));
    assert_eq!(
        fetched_by(a, url, move |_| async move { Ok(replayed) }).await,
        refused
    );
    let forged = WebResponse {
        signature_fields: Vec::new(),
        ..web_response(reply)
    };
    assert_eq!(
        fetched_by(a, url, move |_| async move { Ok(forged) }).await,
        refused
    );
    // Another name, which no node holds, is not asked at all.
    let bob = "https://bob.dassiepay.net/.well-known/pay";
    assert_eq!(
        fetched_by(a, bob, |_| async { panic!("asked under {bob}") }).await,
        Err(ClientError::UnknownName)
    );
    // Nor is the holder's answer taken as another node's.
    let other_url = format!(
        "https://{}.dassiepay.net/.well-known/pay",
        key(62).node_id()
    );
    assert_eq!(
        fetched_by(a, &other_url, |request| pass_on(https, request)).await,
        refused
    );
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
    // Nor does its server fetch the key set of a client that claims the
    // name's wallet address: a front door would vouch for its own key.
    let now_ms = clock.now_ms();
    assert_eq!(
        keys::client_key(&a.node.upgrade().unwrap(), &url, "key", now_ms).await,
        Err(KeyError::Unavailable(ClientError::UnknownName.to_string()))
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
    taken_only_from_the_holder(&registrars, &p, &url, &reply).await;
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
    let label = format!("https://{}.dassiepay.net", id_p);
    assert_eq!(
        status.open_payments_origins,
        [label, format!("https://{HOST}")]
    );
    for node in registrars {
        node.stop().await;
    }
    p.stop().await;
}

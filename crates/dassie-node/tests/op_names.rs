//! Open Payments under a node's names, over real HTTPS
//! (spec/areas/payments.md#the-nodes-names): a node without a hostname
//! answers each request under the name it came through, as front doors
//! pass them on, refuses names not on its list, keeps grants across its
//! names, and signs every answer with its node key
//! (spec/areas/payments.md#signed-responses).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use dassie_identity::NodeId;
use dassie_node::clock::Clock;
use dassie_node::node::Policies;
use dassie_node::open_payments::keys::ClientKey;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_payments::gnap;
use dassie_payments::httpsig::{self, NodeResponseError};
use hyper::StatusCode;
use serde_json::{Value, json};
use support::op::*;
use support::{Reply, TestNode, send};

const OWNERS: &str = "pay.example.com";

/// Checks the node's signature on `reply` to a GET of `path` under `name`
/// with `nonce`, as a Dassie-aware client does.
fn check(
    node: &TestNode,
    reply: &Reply,
    path: &str,
    name: &str,
    nonce: &str,
    id: Option<NodeId>,
) -> Result<NodeId, NodeResponseError> {
    let target = format!("https://{name}{path}");
    let nonce = [(httpsig::NONCE_FIELD, nonce)];
    let request = httpsig::Request {
        method: "GET",
        target_uri: &target,
        fields: &nonce,
    };
    let fields: Vec<(&str, &str)> = reply
        .headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
        .collect();
    let answer = httpsig::Response {
        status: reply.status.as_u16(),
        fields: &fields,
    };
    let now_s = node.clock.now_ms() / 1_000;
    httpsig::check_node_response(&request, &answer, &reply.body, id, now_s)
}

/// A GET of `path` under `name`, with a nonce for the signature.
async fn fetch(node: &TestNode, name: &str, path: &str, nonce: &str) -> Reply {
    let mut request = get_under(name, path);
    request
        .headers_mut()
        .insert(httpsig::NONCE_FIELD, nonce.parse().unwrap());
    send(node.https, true, request).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_node_answers_under_each_of_its_names_and_signs_its_answers() {
    let (node, key) = start_unnamed(41, OWNERS).await;
    let id = key.node_id();
    let default = default_name(&key);
    for name in [default.as_str(), OWNERS] {
        let reply = fetch(&node, name, "/.well-known/pay", "n1").await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
        let address = body(&reply);
        assert_eq!(address["id"], format!("https://{name}/.well-known/pay"));
        assert_eq!(address["authServer"], format!("https://{name}/gnap"));
        assert_eq!(address["resourceServer"], format!("https://{name}"));
        assert_eq!(address["dassie"], json!({ "nodeId": id.as_str() }));
        let exposed = reply.header("access-control-expose-headers").unwrap();
        assert!(exposed.contains("Signature-Input"), "{exposed}");
        // The key checks against the node the name's label names, or the
        // one the client knows for a name of the owner's.
        let known = (name == OWNERS).then_some(id);
        assert_eq!(
            check(&node, &reply, "/.well-known/pay", name, "n1", known),
            Ok(id)
        );
        // Not as an answer to another nonce: an old answer is no answer.
        assert_eq!(
            check(&node, &reply, "/.well-known/pay", name, "n2", known),
            Err(NodeResponseError::Signature(
                httpsig::SignatureError::Invalid
            ))
        );
    }
    // A name the node's owner did not choose is refused, and so is the
    // listener's own address: this node has no hostname.
    for host in ["elsewhere.example".to_string(), node.host()] {
        let reply = fetch(&node, &host, "/.well-known/pay", "n1").await;
        assert_eq!(reply.status, StatusCode::MISDIRECTED_REQUEST, "{host}");
        assert_eq!(body(&reply)["error"]["code"], "misdirected_request");
    }
    // Errors are signed too.
    let missing = fetch(&node, &default, "/pay/xrpl", "n3").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        check(&node, &missing, "/pay/xrpl", &default, "n3", None),
        Ok(id)
    );
    node.stop().await;
}

// A node with a hostname answers under it whatever `Host` says, as
// before, and signs its answers too.
#[tokio::test(flavor = "multi_thread")]
async fn a_node_with_a_hostname_keeps_to_it() {
    let (node, key) = start().await;
    let reply = fetch(&node, "elsewhere.example", "/.well-known/pay", "n1").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(
        body(&reply)["id"],
        format!("{}/.well-known/pay", node.origin())
    );
    let id = key.node_id();
    let target = format!("{}/.well-known/pay", node.origin());
    let nonce = [(httpsig::NONCE_FIELD, "n1")];
    let request = httpsig::Request {
        method: "GET",
        target_uri: &target,
        fields: &nonce,
    };
    let fields: Vec<(&str, &str)> = reply
        .headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
        .collect();
    let answer = httpsig::Response {
        status: 200,
        fields: &fields,
    };
    let now_s = node.clock.now_ms() / 1_000;
    let checked = httpsig::check_node_response(&request, &answer, &reply.body, Some(id), now_s);
    assert_eq!(checked, Ok(id));
    node.stop().await;
}

// D9: a grant made under one name holds under every other, and a
// request's signature is checked against the name it came through.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_holds_under_every_name() {
    let (node, key) = start_unnamed(42, OWNERS).await;
    let default = default_name(&key);
    let client = Client::keyed(7);
    let access = json!([{
        "type": "incoming-payment",
        "actions": ["create", "read", "list"],
        "identifier": format!("https://{default}/.well-known/pay"),
    }]);
    let request = json!({ "access_token": { "access": access }, "client": client.identity });
    let reply = send(
        node.https,
        true,
        signed_under(
            &node,
            &default,
            &client,
            "POST",
            "/gnap",
            None,
            Some(&request),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let grant = body(&reply);
    let token = grant["access_token"]["value"].as_str().unwrap().to_string();
    assert_eq!(
        grant["access_token"]["manage"]
            .as_str()
            .unwrap()
            .split("/gnap/")
            .next(),
        Some(format!("https://{default}").as_str())
    );
    // Under the owner's name, naming the wallet by that name.
    let create = json!({ "walletAddress": format!("https://{OWNERS}/pay/stub") });
    let reply = send(
        node.https,
        true,
        signed_under(
            &node,
            OWNERS,
            &client,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let payment = body(&reply);
    let id = payment["id"].as_str().unwrap();
    let path = id
        .strip_prefix(&format!("https://{OWNERS}"))
        .unwrap()
        .to_string();
    assert!(path.starts_with("/incoming-payments/"), "{id}");
    assert_eq!(
        payment["walletAddress"],
        format!("https://{OWNERS}/pay/stub")
    );
    // Read under the default name: written under that one.
    let reply = send(
        node.https,
        true,
        signed_under(&node, &default, &client, "GET", &path, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let read = body(&reply);
    assert_eq!(read["id"], format!("https://{default}{path}"));
    assert_eq!(read["walletAddress"], format!("https://{default}/pay/stub"));
    // A request signed for one name does not verify under another.
    let mut moved = signed_under(&node, &default, &client, "GET", &path, Some(&token), None);
    moved.headers_mut().insert("host", OWNERS.parse().unwrap());
    let reply = send(node.https, true, moved).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    // Nor does a wallet elsewhere pass for one of the node's.
    let elsewhere = json!({ "walletAddress": "https://elsewhere.example/pay/stub" });
    let reply = send(
        node.https,
        true,
        signed_under(
            &node,
            OWNERS,
            &client,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&elsewhere),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    node.stop().await;
}

/// A node with a hostname, as the client's wallet, and its key: grants
/// that need the owner want a client known by its wallet address.
async fn client_wallet() -> (TestNode, Client) {
    let (wallet, key) = start_keyed(Policies::DEV, 43).await;
    let own = ClientKey::derive(&key).unwrap();
    let client = Client {
        key: own.signing_key().clone(),
        kid: own.kid().to_string(),
        identity: json!({ "walletAddress": format!("{}/.well-known/pay", wallet.origin()) }),
    };
    (wallet, client)
}

/// The grant ID and key the consent page reads, under `name`.
async fn consent_page(node: &TestNode, name: &str, grant: &Value) -> (String, String) {
    let redirect = grant["interact"]["redirect"].as_str().unwrap();
    let path = redirect.strip_prefix(&format!("https://{name}")).unwrap();
    let reply = send(node.https, true, get_under(name, path)).await;
    assert_eq!(reply.status, StatusCode::FOUND, "{}", reply.text());
    let location = reply.header("location").unwrap().to_string();
    let rest = location
        .strip_prefix(&format!("https://{name}/consent/"))
        .unwrap();
    let (id, key) = rest.split_once("?key=").unwrap();
    (id.to_string(), key.to_string())
}

// RFC 9635 §4.2.3: the interaction hash covers the grant endpoint the
// client called. The owner decides outside any request, so a pending
// grant keeps the name it was asked for under.
#[tokio::test(flavor = "multi_thread")]
async fn a_pending_grant_keeps_its_name() {
    let (wallet, client) = client_wallet().await;
    let (node, key) = start_unnamed(44, OWNERS).await;
    let access = json!([{
        "type": "outgoing-payment",
        "actions": ["create", "read"],
        "identifier": format!("https://{OWNERS}/.well-known/pay"),
    }]);
    let request = json!({
        "access_token": { "access": access },
        "client": client.identity,
        "interact": {
            "start": ["redirect"],
            "finish": { "method": "redirect", "uri": "https://client.example/finish", "nonce": "client-nonce" },
        },
    });
    let reply = send(
        node.https,
        true,
        signed_under(
            &node,
            OWNERS,
            &client,
            "POST",
            "/gnap",
            None,
            Some(&request),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let grant = body(&reply);
    assert!(grant["access_token"].is_null());
    let (id, interaction) = consent_page(&node, OWNERS, &grant).await;
    let mut owner = node.ipc().await;
    let Ok(response::Method::DecideGrant(decided)) = owner
        .call(Method::DecideGrant(proto::DecideGrantRequest {
            id,
            key: interaction,
            approve: true,
        }))
        .await
    else {
        panic!("no decision")
    };
    let (hash, reference) = decided
        .finish_url
        .strip_prefix("https://client.example/finish?hash=")
        .unwrap()
        .split_once("&interact_ref=")
        .unwrap();
    let server_nonce = grant["interact"]["finish"].as_str().unwrap();
    let under = |name: &str| {
        let endpoint = format!("https://{name}/gnap");
        gnap::check_interaction_hash(hash, "client-nonce", server_nonce, reference, &endpoint)
    };
    assert!(under(OWNERS));
    assert!(!under(&default_name(&key)));
    // The client continues where it asked, and gets its token, whose
    // access names the wallet under that name.
    node.clock.advance(5_000);
    let continued = json!({ "interact_ref": reference });
    let uri = grant["continue"]["uri"].as_str().unwrap();
    let path = uri.strip_prefix(&format!("https://{OWNERS}")).unwrap();
    let token = grant["continue"]["access_token"]["value"].as_str().unwrap();
    let reply = send(
        node.https,
        true,
        signed_under(
            &node,
            OWNERS,
            &client,
            "POST",
            path,
            Some(token),
            Some(&continued),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let issued = body(&reply);
    assert_eq!(
        issued["access_token"]["access"][0]["identifier"],
        format!("https://{OWNERS}/pay/stub")
    );
    node.stop().await;
    wallet.stop().await;
}

//! Spending through Open Payments over real HTTPS (D9): a grant the owner
//! approves in the wallet, quotes, outgoing payments within the grant's
//! limits and interval, what the grant spent, and receivers on another
//! node.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::time::Duration;

use dassie_node::clock::Clock;
use dassie_node::node::Policies;
use dassie_node::open_payments::keys::ClientKey;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_payments::gnap;
use hyper::StatusCode;
use serde_json::{Value, json};
use support::op::*;
use support::{TestNode, send};

const DAY_MS: u64 = 24 * 60 * 60 * 1_000;

/// The node as its own client, named by its wallet address: grants that
/// need the owner want a client with one.
fn own_client(node: &TestNode, key: &dassie_identity::NodeKey) -> Client {
    let own = ClientKey::derive(key).unwrap();
    Client {
        key: own.signing_key().clone(),
        kid: own.kid().to_string(),
        identity: json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) }),
    }
}

fn host(node: &TestNode) -> String {
    format!("{}/.well-known/pay", node.origin())
}

fn usd(value: u64) -> Value {
    json!({ "value": value.to_string(), "assetCode": "USD", "assetScale": 9 })
}

/// Asks for outgoing payments of up to `limit` a day, and quotes; returns
/// the answer.
async fn ask(node: &TestNode, client: &Client, limit: u64, start: &str) -> Value {
    let request = json!({
        "access_token": { "access": [
            {
                "type": "outgoing-payment",
                "actions": ["create", "read", "list"],
                "identifier": host(node),
                "limits": { "debitAmount": usd(limit), "interval": format!("R/{start}/P1D") },
            },
            { "type": "quote", "actions": ["create", "read"] },
        ]},
        "client": client.identity,
        "interact": {
            "start": ["redirect"],
            "finish": { "method": "redirect", "uri": "https://client.example/finish", "nonce": "client-nonce" },
        },
    });
    let reply = send(
        node.https,
        true,
        signed(node, client, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    body(&reply)
}

/// Continues a grant, with `interact_ref` if given.
async fn continue_grant(
    node: &TestNode,
    client: &Client,
    grant: &Value,
    interact_ref: Option<&str>,
) -> support::Reply {
    let path = path_of(node, &grant["continue"]["uri"]);
    let token = grant["continue"]["access_token"]["value"].as_str().unwrap();
    let body = interact_ref.map(|reference| json!({ "interact_ref": reference }));
    send(
        node.https,
        true,
        signed(node, client, "POST", &path, Some(token), body.as_ref()),
    )
    .await
}

/// The grant ID and interaction key the consent page reads from its URL,
/// after the redirect the client sends the owner to.
async fn consent_page(node: &TestNode, grant: &Value) -> (String, String) {
    let redirect = path_of(node, &grant["interact"]["redirect"]);
    let reply = send(node.https, true, get(node, &redirect)).await;
    assert_eq!(reply.status, StatusCode::FOUND, "{}", reply.text());
    let location = reply.header("location").unwrap().to_string();
    let rest = location
        .strip_prefix(&format!("{}/consent/", node.origin()))
        .unwrap();
    let (id, key) = rest.split_once("?key=").unwrap();
    (id.to_string(), key.to_string())
}

/// The owner's decision; returns where the wallet sends them.
async fn decide(node: &TestNode, id: &str, key: &str, approve: bool) -> String {
    let mut owner = node.ipc().await;
    let Ok(response::Method::DecideGrant(decided)) = owner
        .call(Method::DecideGrant(proto::DecideGrantRequest {
            id: id.into(),
            key: key.into(),
            approve,
        }))
        .await
    else {
        panic!("no decision")
    };
    decided.finish_url
}

/// A grant the owner approved: its access token.
async fn approved(node: &TestNode, client: &Client, limit: u64) -> String {
    let start = dassie_payments::time::format_rfc3339(node.clock.now_ms() - 1_000);
    let grant = ask(node, client, limit, &start).await;
    let (id, key) = consent_page(node, &grant).await;
    let finish = decide(node, &id, &key, true).await;
    let reference = finish.split("interact_ref=").nth(1).unwrap().to_string();
    node.clock.advance(5_000);
    let reply = continue_grant(node, client, &grant, Some(&reference)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    body(&reply)["access_token"]["value"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn post(
    node: &TestNode,
    client: &Client,
    path: &str,
    token: &str,
    request: &Value,
) -> support::Reply {
    send(
        node.https,
        true,
        signed(node, client, "POST", path, Some(token), Some(request)),
    )
    .await
}

/// An incoming payment of the owner's, for `amount`.
async fn receiver(node: &TestNode, amount: u64) -> String {
    node.ipc().await.incoming_payment(Some(amount)).await.url
}

/// Waits until the outgoing payment at `path` has sent what it is to.
async fn sent(node: &TestNode, client: &Client, token: &str, path: &str) -> Value {
    for _ in 0..200 {
        let reply = send(
            node.https,
            true,
            signed(node, client, "GET", path, Some(token), None),
        )
        .await;
        let payment = body(&reply);
        if payment["sentAmount"]["value"] == payment["debitAmount"]["value"]
            || payment["failed"] == true
        {
            return payment;
        }
        node.clock.advance(100);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the payment did not finish");
}

// D9: the owner approves a grant in the wallet; its client quotes and
// pays within the limit, a day at a time, and reads what it spent.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_approves_and_the_client_pays_within_its_limit() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let start = dassie_payments::time::format_rfc3339(node.clock.now_ms() - 1_000);
    let grant = ask(&node, &client, 500, &start).await;
    assert!(grant["access_token"].is_null());
    assert_eq!(grant["continue"]["wait"], 5);
    // Too soon, then still waiting for the owner.
    let reply = continue_grant(&node, &client, &grant, None).await;
    assert_eq!(body(&reply)["error"]["code"], "too_fast");
    node.clock.advance(5_000);
    let reply = continue_grant(&node, &client, &grant, None).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert!(body(&reply)["access_token"].is_null());
    // The owner reads the request, and approves it.
    let (id, key) = consent_page(&node, &grant).await;
    let mut owner = node.ipc().await;
    let Ok(response::Method::GetGrantRequest(asked)) = owner
        .call(Method::GetGrantRequest(proto::GetGrantRequestRequest {
            id: id.clone(),
            key: key.clone(),
        }))
        .await
    else {
        panic!("no request")
    };
    let asked = asked.grant.unwrap();
    assert_eq!(asked.client, host(&node));
    let limits = asked.access[0].limits.as_ref().unwrap();
    assert_eq!(limits.debit_amount.as_ref().unwrap().value, 500);
    let finish = decide(&node, &id, &key, true).await;
    let (hash, reference) = finish
        .strip_prefix("https://client.example/finish?hash=")
        .unwrap()
        .split_once("&interact_ref=")
        .unwrap();
    assert!(gnap::check_interaction_hash(
        hash,
        "client-nonce",
        grant["interact"]["finish"].as_str().unwrap(),
        reference,
        &format!("{}/gnap", node.origin()),
    ));
    // Decided once only; a wrong reference gets nothing.
    assert!(
        owner
            .call(Method::DecideGrant(proto::DecideGrantRequest {
                id,
                key,
                approve: false
            }))
            .await
            .is_err()
    );
    node.clock.advance(5_000);
    let wrong = continue_grant(&node, &client, &grant, Some("nope")).await;
    assert_eq!(body(&wrong)["error"]["code"], "invalid_continuation");
    let reply = continue_grant(&node, &client, &grant, Some(reference)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let issued = body(&reply)["access_token"].clone();
    let token = issued["value"].as_str().unwrap().to_string();
    // A quote for the owner's own receiver, and its payment.
    let target = receiver(&node, 300).await;
    let quote = json!({ "walletAddress": host(&node), "receiver": target, "method": "ilp" });
    let reply = post(&node, &client, "/quotes", &token, &quote).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let quote = body(&reply);
    assert_eq!(quote["debitAmount"], usd(300));
    assert_eq!(quote["receiveAmount"], usd(300));
    let pay = json!({ "walletAddress": host(&node), "quoteId": quote["id"] });
    let reply = post(&node, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let payment = body(&reply);
    assert_eq!(payment["grantSpentDebitAmount"], usd(300));
    let paid = sent(&node, &client, &token, &path_of(&node, &payment["id"])).await;
    assert_eq!(paid["failed"], false);
    assert_eq!(paid["sentAmount"], usd(300));
    // A quote pays once.
    let again = post(&node, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST, "{}", again.text());
    // 300 more would pass the day's 500.
    let target = receiver(&node, 300).await;
    let more =
        json!({ "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300) });
    let refused = post(&node, &client, "/outgoing-payments", &token, &more).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text());
    let spent = body(
        &send(
            node.https,
            true,
            signed(
                &node,
                &client,
                "GET",
                "/outgoing-payment-grant",
                Some(&token),
                None,
            ),
        )
        .await,
    );
    assert_eq!(spent["spentDebitAmount"], usd(300));
    // The next day, the limit is whole again; the token expired, and is
    // rotated, as a grant the owner approved is meant to last.
    node.clock.advance(DAY_MS);
    let manage = path_of(&node, &issued["manage"]);
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", &manage, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let token = body(&reply)["access_token"]["value"]
        .as_str()
        .unwrap()
        .to_string();
    let target = receiver(&node, 300).await;
    let more =
        json!({ "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300) });
    let reply = post(&node, &client, "/outgoing-payments", &token, &more).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let paid = sent(&node, &client, &token, &path_of(&node, &body(&reply)["id"])).await;
    assert_eq!(paid["sentAmount"], usd(300));
    node.stop().await;
}

// A denied grant sends the owner back with the rejection, and gives the
// client nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_grant_gives_nothing() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let start = dassie_payments::time::format_rfc3339(node.clock.now_ms());
    let grant = ask(&node, &client, 500, &start).await;
    let (id, key) = consent_page(&node, &grant).await;
    let finish = decide(&node, &id, &key, false).await;
    assert_eq!(
        finish,
        "https://client.example/finish?result=grant_rejected"
    );
    node.clock.advance(5_000);
    let reply = continue_grant(&node, &client, &grant, None).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(body(&reply)["error"]["code"], "request_denied");
    // Its interaction is gone too.
    let redirect = path_of(&node, &grant["interact"]["redirect"]);
    let reply = send(node.https, true, get(&node, &redirect)).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    node.stop().await;
}

// The owner sees the grants they approved, with what each spent, and
// revokes one: its token stops working.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_revokes_a_grant() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let token = approved(&node, &client, 1_000).await;
    let mut owner = node.ipc().await;
    let Ok(response::Method::ListGrants(listed)) = owner
        .call(Method::ListGrants(proto::ListGrantsRequest {}))
        .await
    else {
        panic!("no grants")
    };
    assert_eq!(listed.grants.len(), 1);
    let grant = &listed.grants[0];
    assert_eq!(grant.state, "approved");
    owner
        .call(Method::RevokeGrant(proto::RevokeGrantRequest {
            id: grant.id.clone(),
        }))
        .await
        .unwrap();
    let target = receiver(&node, 10).await;
    let quote = json!({ "walletAddress": host(&node), "receiver": target, "method": "ilp" });
    let reply = post(&node, &client, "/quotes", &token, &quote).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    node.stop().await;
}

// A receiver on another node is read with a grant from that node's
// authorization server, as this node's own wallet address.
#[tokio::test(flavor = "multi_thread")]
async fn a_quote_reads_a_receiver_on_another_node() {
    let (payer, key) = start().await;
    let (payee, _) = start_keyed(Policies::DEV, 32).await;
    let client = own_client(&payer, &key);
    let token = approved(&payer, &client, 1_000).await;
    let target = receiver(&payee, 700).await;
    let quote = json!({ "walletAddress": host(&payer), "receiver": target, "method": "ilp" });
    let reply = post(&payer, &client, "/quotes", &token, &quote).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let quote = body(&reply);
    assert_eq!(quote["receiver"], target);
    assert_eq!(quote["receiveAmount"], usd(700));
    payer.stop().await;
    payee.stop().await;
}

//! Spending through Open Payments over real HTTPS
//! (spec/decisions/open-payments.md): a grant the owner approves in the
//! wallet, quotes, outgoing payments within the grant's limits and interval,
//! what the grant spent, and receivers on another node.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::time::Duration;

use dassie_node::clock::Clock;
use dassie_node::node::Policies;
use dassie_node::open_payments::keys::ClientKey;
use dassie_node::rpc::proto::{
    self, client_message, request::Method, response, server_message, stream_item,
};
use dassie_payments::gnap;
use hyper::StatusCode;
use serde_json::{Value, json};
use support::op::*;
use support::pair::{Pair, until};
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

/// Milliseconds as the contract's signed times.
fn unix(ms: u64) -> i64 {
    i64::try_from(ms).unwrap()
}

fn usd(value: u64) -> Value {
    json!({ "value": value.to_string(), "assetCode": "USD", "assetScale": 9 })
}

/// Asks for outgoing payments of up to `limit` a day, and quotes; returns
/// the answer.
async fn ask(node: &TestNode, client: &Client, limit: u64, start: &str) -> Value {
    let access = json!([
        {
            "type": "outgoing-payment",
            "actions": ["create", "read", "list"],
            "identifier": host(node),
            "limits": { "debitAmount": usd(limit), "interval": format!("R/{start}/P1D") },
        },
        { "type": "quote", "actions": ["create", "read"] },
    ]);
    ask_for(node, client, &access).await
}

/// Asks for `access`, which needs the owner; returns the answer.
async fn ask_for(node: &TestNode, client: &Client, access: &Value) -> Value {
    let reply = request_grant(node, client, access).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    body(&reply)
}

/// Asks for `access`, which needs the owner; returns the reply as it is.
async fn request_grant(node: &TestNode, client: &Client, access: &Value) -> support::Reply {
    let request = json!({
        "access_token": { "access": access },
        "client": client.identity,
        "interact": {
            "start": ["redirect"],
            "finish": { "method": "redirect", "uri": "https://client.example/finish", "nonce": "client-nonce" },
        },
    });
    send(
        node.https,
        true,
        signed(node, client, "POST", "/gnap", None, Some(&request)),
    )
    .await
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
    approve(node, client, &grant).await
}

/// The owner approves `grant`: its access token.
async fn approve(node: &TestNode, client: &Client, grant: &Value) -> String {
    let (id, key) = consent_page(node, grant).await;
    let finish = decide(node, &id, &key, true).await;
    let reference = finish.split("interact_ref=").nth(1).unwrap().to_string();
    node.clock.advance(5_000);
    let reply = continue_grant(node, client, grant, Some(&reference)).await;
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

// spec/decisions/open-payments.md: the owner approves a grant in the wallet;
// its client quotes and pays within the limit, a day at a time, and reads
// what it spent.
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

// The client revokes a grant through its continuation: the owner's
// connected apps, which follow the grants live, drop it.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_revoking_its_grant_updates_the_owners_view() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let start = dassie_payments::time::format_rfc3339(node.clock.now_ms() - 1_000);
    let asked = ask(&node, &client, 1_000, &start).await;
    let (id, interaction) = consent_page(&node, &asked).await;
    let finish = decide(&node, &id, &interaction, true).await;
    let reference = finish.split("interact_ref=").nth(1).unwrap();
    let reply = continue_grant(&node, &client, &asked, Some(reference)).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let granted = body(&reply);

    let mut owner = node.ipc().await;
    owner
        .send_raw(proto::ClientMessage {
            call_id: 1,
            kind: Some(client_message::Kind::Request(proto::Request {
                method: Some(Method::SubscribeGrants(proto::SubscribeGrantsRequest {})),
            })),
        })
        .await;
    // The node sends each change as it makes it, on no clock: only a hang
    // ends the wait for one.
    let mut listed = async || {
        let message = tokio::time::timeout(support::HANG, owner.receive())
            .await
            .expect("no item")
            .expect("closed");
        let Some(server_message::Kind::Item(item)) = message.kind else {
            panic!("{message:?}")
        };
        let Some(stream_item::Method::SubscribeGrants(listed)) = item.method else {
            panic!("{item:?}")
        };
        listed.grants
    };
    let first = listed().await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, id);

    let continuation = path_of(&node, &granted["continue"]["uri"]);
    let continue_token = granted["continue"]["access_token"]["value"]
        .as_str()
        .unwrap();
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "DELETE",
            &continuation,
            Some(continue_token),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text());
    assert!(listed().await.is_empty());
    node.stop().await;
}

// A receiver on another node is read with a grant from that node's
// authorization server, as this node's own wallet address. The quote then
// measures the rate of the route to it (M7c): these nodes do not peer, so
// there is none, and no quote.
#[tokio::test(flavor = "multi_thread")]
async fn a_quote_reads_a_receiver_on_another_node() {
    let (payer, key) = start().await;
    let (payee, _) = start_keyed(Policies::DEV, 32).await;
    let client = own_client(&payer, &key);
    let token = approved(&payer, &client, 1_000).await;
    let target = receiver(&payee, 700).await;
    let quote = json!({ "walletAddress": host(&payer), "receiver": target, "method": "ilp" });
    let reply = post(&payer, &client, "/quotes", &token, &quote).await;
    assert_eq!(reply.status, StatusCode::BAD_GATEWAY, "{}", reply.text());
    let error = body(&reply);
    assert_eq!(error["error"]["code"], "unreachable");
    assert_eq!(
        error["error"]["description"],
        "there is no route to the receiver"
    );
    payer.stop().await;
    payee.stop().await;
}

// Found in review: two payments of one quote at once. The engine checks the
// quote itself, so one is made and the other refused, and nothing breaks.
#[tokio::test(flavor = "multi_thread")]
async fn a_quote_pays_once_even_twice_at_once() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let token = approved(&node, &client, 1_000).await;
    let target = receiver(&node, 100).await;
    let quote = json!({ "walletAddress": host(&node), "receiver": target, "method": "ilp" });
    let quote = body(&post(&node, &client, "/quotes", &token, &quote).await);
    let pay = json!({ "walletAddress": host(&node), "quoteId": quote["id"] });
    let (first, second) = tokio::join!(
        post(&node, &client, "/outgoing-payments", &token, &pay),
        post(&node, &client, "/outgoing-payments", &token, &pay),
    );
    let mut statuses = [first.status, second.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::BAD_REQUEST]);
    // The engine still answers.
    let target = receiver(&node, 100).await;
    let quote = json!({ "walletAddress": host(&node), "receiver": target, "method": "ilp" });
    let reply = post(&node, &client, "/quotes", &token, &quote).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    node.stop().await;
}

// Found in review, untested: the owner's quotes and a client's are each
// their own, whoever pays them.
#[tokio::test(flavor = "multi_thread")]
async fn the_owners_quotes_and_a_clients_are_apart() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let token = approved(&node, &client, 1_000).await;
    let target = receiver(&node, 100).await;
    let quote = json!({ "walletAddress": host(&node), "receiver": target, "method": "ilp" });
    let theirs = body(&post(&node, &client, "/quotes", &token, &quote).await);
    let id = theirs["id"].as_str().unwrap().rsplit('/').next().unwrap();
    let paid = node
        .ipc()
        .await
        .call(Method::SendPayment(proto::SendPaymentRequest {
            quote_id: id.into(),
        }))
        .await;
    assert_eq!(
        paid.unwrap_err().code(),
        proto::ErrorCode::FailedPrecondition
    );
    let Ok(response::Method::QuoteSend(quoted)) = node
        .ipc()
        .await
        .call(Method::QuoteSend(proto::QuoteSendRequest {
            recipient: format!("${}", node.host()),
            ledger: String::new(),
            amount: Some(proto::quote_send_request::Amount::ReceiveAmount(100)),
            note: String::new(),
            credentials: None,
        }))
        .await
    else {
        panic!("no quote")
    };
    let mine = format!("{}/quotes/{}", node.origin(), quoted.quote.unwrap().id);
    let pay = json!({ "walletAddress": host(&node), "quoteId": mine });
    let reply = post(&node, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    node.stop().await;
}

/// What the token's grant spent in this window.
async fn spent(node: &TestNode, client: &Client, token: &str) -> Value {
    let reply = send(
        node.https,
        true,
        signed(
            node,
            client,
            "GET",
            "/outgoing-payment-grant",
            Some(token),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    body(&reply)
}

/// Waits until the outgoing payment at `path` completed or failed,
/// letting it reconnect as often as it may.
async fn finished(node: &TestNode, client: &Client, token: &str, path: &str) -> Value {
    let mut payment = Value::Null;
    for _ in 0..2_000 {
        let reply = send(
            node.https,
            true,
            signed(node, client, "GET", path, Some(token), None),
        )
        .await;
        payment = body(&reply);
        let complete = payment["sentAmount"]["value"] == payment["debitAmount"]["value"];
        if complete || payment["failed"] == true {
            return payment;
        }
        node.clock.advance(1_000);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the payment did not finish: {payment}");
}

fn value(amount: &Value) -> u64 {
    amount["value"]
        .as_str()
        .map_or(0, |value| value.parse().unwrap())
}

// Overspending at once: six payments of 300 against a limit of 1 000, all
// in flight together. Three fit; the rest are refused, and the grant spent
// exactly what was sent.
#[tokio::test(flavor = "multi_thread")]
async fn payments_at_once_stay_within_the_limit() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let token = approved(&node, &client, 1_000).await;
    let mut requests = Vec::new();
    for _ in 0..6 {
        let target = receiver(&node, 300).await;
        requests.push(json!({
            "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300),
        }));
    }
    let pay = |index: usize| {
        post(
            &node,
            &client,
            "/outgoing-payments",
            &token,
            &requests[index],
        )
    };
    let replies = tokio::join!(pay(0), pay(1), pay(2), pay(3), pay(4), pay(5));
    let replies = [
        replies.0, replies.1, replies.2, replies.3, replies.4, replies.5,
    ];
    let created: Vec<_> = replies
        .iter()
        .filter(|reply| reply.status == StatusCode::CREATED)
        .collect();
    assert_eq!(created.len(), 3);
    assert!(
        replies
            .iter()
            .all(|reply| [StatusCode::CREATED, StatusCode::FORBIDDEN].contains(&reply.status))
    );
    let mut total = 0;
    for reply in created {
        let paid = finished(&node, &client, &token, &path_of(&node, &body(reply)["id"])).await;
        total += value(&paid["sentAmount"]);
    }
    assert_eq!(total, 900);
    assert_eq!(
        spent(&node, &client, &token).await["spentDebitAmount"],
        usd(900)
    );
    node.stop().await;
}

// Overspending with quotes: quotes check no limit, so five of 300 are
// made, then paid at once. Three fit.
#[tokio::test(flavor = "multi_thread")]
async fn quotes_paid_at_once_stay_within_the_limit() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let token = approved(&node, &client, 1_000).await;
    let mut requests = Vec::new();
    for _ in 0..5 {
        let target = receiver(&node, 300).await;
        let quote = json!({ "walletAddress": host(&node), "receiver": target, "method": "ilp" });
        let reply = post(&node, &client, "/quotes", &token, &quote).await;
        assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
        requests.push(json!({ "walletAddress": host(&node), "quoteId": body(&reply)["id"] }));
    }
    let pay = |index: usize| {
        post(
            &node,
            &client,
            "/outgoing-payments",
            &token,
            &requests[index],
        )
    };
    let replies = tokio::join!(pay(0), pay(1), pay(2), pay(3), pay(4));
    let replies = [replies.0, replies.1, replies.2, replies.3, replies.4];
    let created = replies
        .iter()
        .filter(|reply| reply.status == StatusCode::CREATED)
        .count();
    assert_eq!(created, 3);
    assert_eq!(
        spent(&node, &client, &token).await["spentDebitAmount"],
        usd(900)
    );
    node.stop().await;
}

// Double spending one incoming payment: two payments of 300 to one that
// takes 300, at once. Both may be made, as each saw 300 still to pay, each
// over a connection of its own. The receiver takes 300 in all, and each
// payment is told only what it delivered: it sends all it is to, or fails,
// and never completes on what the other delivered. The one that finds the
// incoming payment full fails at once, as the receiver closes its
// connection, rather than offering its money until its deadline. The grant
// is charged only what was sent.
#[tokio::test(flavor = "multi_thread")]
async fn an_incoming_payment_is_not_paid_twice() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let token = approved(&node, &client, 1_000).await;
    let target = receiver(&node, 300).await;
    let pay = json!({
        "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300),
    });
    let started = node.clock.now_ms();
    let (first, second) = tokio::join!(
        post(&node, &client, "/outgoing-payments", &token, &pay),
        post(&node, &client, "/outgoing-payments", &token, &pay),
    );
    let mut total = 0;
    for reply in [first, second] {
        if reply.status != StatusCode::CREATED {
            continue;
        }
        // Done once it sent all it is to, or failed.
        let paid = finished(&node, &client, &token, &path_of(&node, &body(&reply)["id"])).await;
        total += value(&paid["sentAmount"]);
    }
    assert_eq!(total, 300);
    // Well within a connection's idle timeout, and its reconnects.
    let took = node.clock.now_ms() - started;
    assert!(took < 30_000, "the payments took {took} ms");
    assert_eq!(
        spent(&node, &client, &token).await["spentDebitAmount"],
        usd(300)
    );
    node.stop().await;
}

// A new token is the same grant: rotating it does not reset what it
// spent.
#[tokio::test(flavor = "multi_thread")]
async fn rotating_the_token_keeps_what_was_spent() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let start = dassie_payments::time::format_rfc3339(node.clock.now_ms() - 1_000);
    let grant = ask(&node, &client, 500, &start).await;
    let (id, consent) = consent_page(&node, &grant).await;
    let finish = decide(&node, &id, &consent, true).await;
    let reference = finish.split("interact_ref=").nth(1).unwrap().to_string();
    node.clock.advance(5_000);
    let reply = continue_grant(&node, &client, &grant, Some(&reference)).await;
    let issued = body(&reply)["access_token"].clone();
    let token = issued["value"].as_str().unwrap().to_string();
    let target = receiver(&node, 300).await;
    let pay = json!({
        "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300),
    });
    let reply = post(&node, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let manage = path_of(&node, &issued["manage"]);
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", &manage, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let rotated = body(&reply)["access_token"]["value"]
        .as_str()
        .unwrap()
        .to_string();
    let target = receiver(&node, 300).await;
    let pay = json!({
        "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300),
    });
    let reply = post(&node, &client, "/outgoing-payments", &rotated, &pay).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    node.stop().await;
}

/// The owner pays `target` in full from the wallet; returns the path of
/// the outgoing payment.
async fn owner_pays(node: &TestNode, target: &str, amount: u64) -> String {
    let mut owner = node.ipc().await;
    let Ok(response::Method::QuoteSend(quoted)) = owner
        .call(Method::QuoteSend(proto::QuoteSendRequest {
            recipient: target.into(),
            ledger: String::new(),
            amount: Some(proto::quote_send_request::Amount::ReceiveAmount(amount)),
            note: String::new(),
            credentials: None,
        }))
        .await
    else {
        panic!("no quote")
    };
    let Ok(response::Method::SendPayment(sent)) = owner
        .call(Method::SendPayment(proto::SendPaymentRequest {
            quote_id: quoted.quote.unwrap().id,
        }))
        .await
    else {
        panic!("not sent")
    };
    format!("/outgoing-payments/{}", sent.payment.unwrap().id)
}

/// The IDs of the outgoing payments a list shows.
fn listed_ids(node: &TestNode, list: &Value) -> Vec<String> {
    list["result"]
        .as_array()
        .unwrap()
        .iter()
        .map(|payment| path_of(node, &payment["id"]))
        .collect()
}

// Outgoing-payment items for one wallet address, each limited in its own
// way: one reads every payment to one receiver (its amounts limit
// nothing, as it spends nothing), the other creates, within its own limit,
// and reads the client's own. Neither item's limits lift or narrow the
// other's.
#[tokio::test(flavor = "multi_thread")]
async fn each_item_keeps_to_its_own_limits() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let watched = receiver(&node, 100).await;
    let other = receiver(&node, 100).await;
    let to_watched = owner_pays(&node, &watched, 100).await;
    let to_other = owner_pays(&node, &other, 100).await;
    let start = dassie_payments::time::format_rfc3339(node.clock.now_ms() - 1_000);
    let access = json!([
        {
            "type": "outgoing-payment",
            "actions": ["read-all", "list-all"],
            "identifier": host(&node),
            "limits": { "receiver": watched, "debitAmount": usd(1) },
        },
        {
            "type": "outgoing-payment",
            "actions": ["create", "read", "list"],
            "identifier": host(&node),
            "limits": { "debitAmount": usd(10), "interval": format!("R/{start}/P1D") },
        },
    ]);
    let grant = ask_for(&node, &client, &access).await;
    let token = approve(&node, &client, &grant).await;
    let (on, by, with) = (&node, &client, token.as_str());
    // It creates within 10, not the read item's 1, and no more.
    let pay = |amount| async move {
        let target = receiver(on, amount).await;
        let pay = json!({
            "walletAddress": host(on), "incomingPayment": target, "debitAmount": usd(amount),
        });
        post(on, by, "/outgoing-payments", with, &pay).await
    };
    let refused = pay(1_000).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text());
    let created = pay(5).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let own = path_of(&node, &body(&created)["id"]);
    assert_eq!(
        spent(&node, &client, &token).await["spentDebitAmount"],
        usd(5)
    );
    // It reads the owner's payment to the receiver it watches, and its own,
    // but not the owner's to another.
    let read = |path: String| async move {
        send(
            on.https,
            true,
            signed(on, by, "GET", &path, Some(with), None),
        )
        .await
        .status
    };
    assert_eq!(read(to_watched.clone()).await, StatusCode::OK);
    assert_eq!(read(own.clone()).await, StatusCode::OK);
    assert_eq!(read(to_other.clone()).await, StatusCode::FORBIDDEN);
    // Its list shows the same.
    let list = format!(
        "/outgoing-payments?wallet-address={}",
        dassie_payments::uri::percent_encode(&host(&node))
    );
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "GET", &list, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let mut ids = listed_ids(&node, &body(&reply));
    ids.sort();
    let mut expected = vec![to_watched, own];
    expected.sort();
    assert_eq!(ids, expected);
    node.stop().await;
}

// spec/areas/payments.md#paying-stream-credentials: a payment of the owner's to
// STREAM credentials names no incoming payment, so it is no Open Payments
// resource: a client that may read and list all of the owner's payments
// and quotes sees the others, never it or its quote.
#[tokio::test(flavor = "multi_thread")]
async fn payments_to_stream_credentials_are_no_resources() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let to_request = owner_pays(&node, &receiver(&node, 100).await, 100).await;
    let incoming = node.ipc().await.incoming_payment(None).await;
    let mut owner = node.ipc().await;
    let Ok(response::Method::QuoteSend(quoted)) = owner
        .call(Method::QuoteSend(proto::QuoteSendRequest {
            recipient: String::new(),
            ledger: String::new(),
            amount: Some(proto::quote_send_request::Amount::DebitAmount(100)),
            note: String::new(),
            credentials: Some(proto::StreamCredentials {
                destination_account: incoming.destination_account.clone(),
                shared_secret: incoming.shared_secret.clone(),
                asset_code: String::new(),
                asset_scale: 0,
            }),
        }))
        .await
    else {
        panic!("no quote")
    };
    let quote = quoted.quote.unwrap();
    let Ok(response::Method::SendPayment(sent)) = owner
        .call(Method::SendPayment(proto::SendPaymentRequest {
            quote_id: quote.id.clone(),
        }))
        .await
    else {
        panic!("not sent")
    };
    let to_credentials = format!("/outgoing-payments/{}", sent.payment.unwrap().id);
    let access = json!([
        {
            "type": "outgoing-payment",
            "actions": ["read-all", "list-all"],
            "identifier": host(&node),
        },
        { "type": "quote", "actions": ["read-all"] },
    ]);
    let grant = ask_for(&node, &client, &access).await;
    let token = approve(&node, &client, &grant).await;
    let read = |path: String| {
        let request = signed(&node, &client, "GET", &path, Some(&token), None);
        async move { send(node.https, true, request).await.status }
    };
    assert_eq!(read(to_request.clone()).await, StatusCode::OK);
    assert_eq!(read(to_credentials).await, StatusCode::NOT_FOUND);
    assert_eq!(
        read(format!("/quotes/{}", quote.id)).await,
        StatusCode::NOT_FOUND
    );
    let list = format!(
        "/outgoing-payments?wallet-address={}",
        dassie_payments::uri::percent_encode(&host(&node))
    );
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "GET", &list, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(listed_ids(&node, &body(&reply)), vec![to_request]);
    node.stop().await;
}

// Two items that both create from one wallet address, with their own
// limits, leave unclear which applies: the request is refused, however
// the address is spelt.
#[tokio::test(flavor = "multi_thread")]
async fn two_items_may_not_both_create_from_one_wallet_address() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let slug = format!("{}/pay/usd", node.origin());
    let request = json!({
        "access_token": { "access": [
            {
                "type": "outgoing-payment",
                "actions": ["create"],
                "identifier": host(&node),
                "limits": { "debitAmount": usd(10) },
            },
            {
                "type": "outgoing-payment",
                "actions": ["create", "read"],
                "identifier": slug,
                "limits": { "debitAmount": usd(1_000) },
            },
        ]},
        "client": client.identity,
        "interact": {
            "start": ["redirect"],
            "finish": { "method": "redirect", "uri": "https://client.example/finish", "nonce": "n" },
        },
    });
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(body(&reply)["error"]["code"], "invalid_request");
    node.stop().await;
}

// A debit limit in another asset than its wallet address, even one that
// differs only in scale, would make a grant no payment can use: the
// request is refused, naming the asset the limit must be in
// (spec/decisions/unusable-limits.md).
#[tokio::test(flavor = "multi_thread")]
async fn a_debit_limit_in_another_asset_is_refused() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let item = |actions: Value, limit: Value| {
        json!([{
            "type": "outgoing-payment",
            "actions": actions,
            "identifier": host(&node),
            "limits": { "debitAmount": limit },
        }])
    };
    for limit in [
        json!({ "value": "500", "assetCode": "EUR", "assetScale": 9 }),
        json!({ "value": "500", "assetCode": "USD", "assetScale": 2 }),
    ] {
        let reply = request_grant(&node, &client, &item(json!(["create", "read"]), limit)).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
        let error = &body(&reply)["error"];
        assert_eq!(error["code"], "invalid_request");
        let description = error["description"].as_str().unwrap();
        assert!(description.contains("USD at scale 9"), "{description}");
    }
    // An item that only reads creates no payment for its limits to cap.
    let reading = item(
        json!(["read"]),
        json!({ "value": "500", "assetCode": "EUR", "assetScale": 2 }),
    );
    ask_for(&node, &client, &reading).await;
    ask_for(&node, &client, &item(json!(["create", "read"]), usd(500))).await;
    node.stop().await;
}

// A receive limit in another asset than the receiver is paid in fits no
// payment to it: the refusal names the limit and both assets, so the
// client learns why its grant cannot pay.
#[tokio::test(flavor = "multi_thread")]
async fn a_payment_refused_for_its_limits_asset_names_both() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let access = json!([{
        "type": "outgoing-payment",
        "actions": ["create", "read"],
        "identifier": host(&node),
        "limits": {
            "debitAmount": usd(1_000),
            "receiveAmount": { "value": "500", "assetCode": "USD", "assetScale": 2 },
        },
    }]);
    let grant = ask_for(&node, &client, &access).await;
    let token = approve(&node, &client, &grant).await;
    let target = receiver(&node, 300).await;
    let pay =
        json!({ "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300) });
    let refused = post(&node, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text());
    assert_eq!(
        body(&refused)["error"]["description"],
        "the grant's receive limit is in USD at scale 2, and this receiver is paid in USD at scale 9"
    );
    node.stop().await;
}

// Found in testing: an item that only reads, without limits, came first,
// and its limits, none, were taken for the payments of the item that
// creates. A client asking for 10 a day could send any amount.
#[tokio::test(flavor = "multi_thread")]
async fn an_item_that_only_reads_does_not_lift_the_limit() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let start = dassie_payments::time::format_rfc3339(node.clock.now_ms() - 1_000);
    let access = json!([
        { "type": "outgoing-payment", "actions": ["read"], "identifier": host(&node) },
        {
            "type": "outgoing-payment",
            "actions": ["create"],
            "identifier": host(&node),
            "limits": { "debitAmount": usd(10), "interval": format!("R/{start}/P1D") },
        },
    ]);
    let grant = ask_for(&node, &client, &access).await;
    let token = approve(&node, &client, &grant).await;
    let target = receiver(&node, 1_000).await;
    let pay = json!({
        "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(1_000),
    });
    let reply = post(&node, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    node.stop().await;
}

// A grant may limit what its payments deliver as well as what they debit,
// but never what they deliver alone, as Open Payments is changing it. The
// receive limit binds although the debit limit has room.
#[tokio::test(flavor = "multi_thread")]
async fn a_receive_limit_caps_what_is_delivered() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let alone = json!({
        "access_token": { "access": [{
            "type": "outgoing-payment",
            "actions": ["create"],
            "identifier": host(&node),
            "limits": { "receiveAmount": usd(500) },
        }] },
        "client": client.identity,
        "interact": { "start": ["redirect"] },
    });
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", "/gnap", None, Some(&alone)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(body(&reply)["error"]["code"], "invalid_request");
    let access = json!([{
        "type": "outgoing-payment",
        "actions": ["create", "read"],
        "identifier": host(&node),
        "limits": { "debitAmount": usd(1_000), "receiveAmount": usd(500) },
    }]);
    let grant = ask_for(&node, &client, &access).await;
    let token = approve(&node, &client, &grant).await;
    let target = receiver(&node, 300).await;
    let pay =
        json!({ "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300) });
    let reply = post(&node, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let payment = body(&reply);
    assert_eq!(payment["grantSpentReceiveAmount"], usd(300));
    let paid = sent(&node, &client, &token, &path_of(&node, &payment["id"])).await;
    assert_eq!(paid["sentAmount"], usd(300));
    // 300 more would deliver past 500, though 600 fits the debit limit.
    let target = receiver(&node, 300).await;
    let more =
        json!({ "walletAddress": host(&node), "incomingPayment": target, "debitAmount": usd(300) });
    let refused = post(&node, &client, "/outgoing-payments", &token, &more).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.text());
    let spent = spent(&node, &client, &token).await;
    assert_eq!(spent["spentDebitAmount"], usd(300));
    assert_eq!(spent["spentReceiveAmount"], usd(300));
    node.stop().await;
}

// The finish URI and the subject are each up to a URI's bound, but the
// grant's row keeps its interaction only up to its own: a request whose
// interaction could not be kept is refused before it is stored, and one
// that fits is kept through the owner's decision.
#[tokio::test(flavor = "multi_thread")]
async fn an_interaction_too_long_to_keep_is_refused() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let access = json!([{
        "type": "outgoing-payment",
        "actions": ["create", "read"],
        "identifier": host(&node),
    }]);
    let ask = |finish: usize, subject: usize| {
        let request = json!({
            "access_token": { "access": access },
            "client": client.identity,
            "subject": { "sub_ids": [{
                "id": format!("https://wallet.example/{}", "s".repeat(subject)),
                "format": "uri",
            }] },
            "interact": {
                "start": ["redirect"],
                "finish": {
                    "method": "redirect",
                    "uri": format!("https://client.example/{}", "f".repeat(finish)),
                    "nonce": "c".repeat(256),
                },
            },
        });
        send(
            node.https,
            true,
            signed(&node, &client, "POST", "/gnap", None, Some(&request)),
        )
    };
    let reply = ask(2_000, 2_000).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{}", reply.text());
    assert_eq!(body(&reply)["error"]["code"], "invalid_request");
    let reply = ask(1_500, 1_500).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let grant = body(&reply);
    let (id, key) = consent_page(&node, &grant).await;
    let finish = decide(&node, &id, &key, true).await;
    assert!(finish.contains("interact_ref="), "{finish}");
    node.stop().await;
}

/// The request the consent page reads for `grant`, which the owner then
/// denies: a client has only a few requests pending at once.
async fn asked(node: &TestNode, grant: &Value) -> proto::Grant {
    let (id, key) = consent_page(node, grant).await;
    let Ok(response::Method::GetGrantRequest(asked)) = node
        .ipc()
        .await
        .call(Method::GetGrantRequest(proto::GetGrantRequestRequest {
            id: id.clone(),
            key: key.clone(),
        }))
        .await
    else {
        panic!("no request")
    };
    decide(node, &id, &key, false).await;
    asked.grant.unwrap()
}

/// Asks to pay only `receiver`, over `interval` if given; returns the
/// limits as the owner read them.
async fn asked_limits(
    node: &TestNode,
    client: &Client,
    receiver: &str,
    interval: Option<&str>,
) -> proto::GrantLimits {
    let mut limits = json!({ "receiver": receiver, "debitAmount": usd(1_000) });
    if let Some(interval) = interval {
        limits["interval"] = json!(interval);
    }
    limits_read(node, client, limits).await
}

/// Asks to pay within `limits`; returns them as the owner read them.
async fn limits_read(node: &TestNode, client: &Client, limits: Value) -> proto::GrantLimits {
    let access = json!([{
        "type": "outgoing-payment",
        "actions": ["create", "read"],
        "identifier": host(node),
        "limits": limits,
    }]);
    let grant = ask_for(node, client, &access).await;
    asked(node, &grant).await.access[0].limits.clone().unwrap()
}

// The consent page shows when a grant starts and ends, and the receiver it
// may only pay as it reads while the owner is asked: here or on another
// node. One that cannot be read is shown as such, not refused.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_sees_the_span_and_receiver_of_a_request() {
    let (node, key) = start().await;
    let (payee, _) = start_keyed(Policies::DEV, 32).await;
    let client = own_client(&node, &key);
    let start_ms = node.clock.now_ms() + DAY_MS;
    let start = dassie_payments::time::format_rfc3339(start_ms);

    // Here: the first window and six more, from tomorrow.
    let local = receiver(&node, 700).await;
    let limits = asked_limits(&node, &client, &local, Some(&format!("R6/{start}/P1D"))).await;
    assert_eq!(limits.starts_at_unix_ms, Some(unix(start_ms)));
    assert_eq!(limits.ends_at_unix_ms, Some(unix(start_ms + 7 * DAY_MS)));
    let read = limits.receiver_payment.unwrap();
    assert_eq!(read.error, "");
    assert_eq!((read.asset_code.as_str(), read.asset_scale), ("USD", 9));
    assert_eq!(read.remaining, Some(700));
    assert!(!read.closed);
    // A debit limit says nothing of what the receiver is paid in.
    assert!(!read.limit_in_other_asset);

    // On another node, for ever.
    let remote = receiver(&payee, 300).await;
    let limits = asked_limits(&node, &client, &remote, Some(&format!("R/{start}/P1D"))).await;
    assert_eq!(limits.starts_at_unix_ms, Some(unix(start_ms)));
    assert_eq!(limits.ends_at_unix_ms, None);
    let read = limits.receiver_payment.unwrap();
    assert_eq!(read.error, "");
    assert_eq!(read.remaining, Some(300));

    // No such payment, and no interval: all time, no span.
    let (prefix, _) = local.rsplit_once('/').unwrap();
    let missing = format!("{prefix}/{}", "0".repeat(26));
    let limits = asked_limits(&node, &client, &missing, None).await;
    assert_eq!(
        (limits.starts_at_unix_ms, limits.ends_at_unix_ms),
        (None, None)
    );
    let read = limits.receiver_payment.unwrap();
    assert_eq!(read.error, "it is not a payment request that can be read");
    assert_eq!(read.asset_code, "");

    // Found in review: a receiver's node that takes the connection and
    // never answers holds the page only for the lookup's bound, not for
    // each of the client's timeouts in turn.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = silent.local_addr().unwrap().port();
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((socket, _)) = silent.accept().await {
            open.push(socket);
        }
    });
    let (_, id) = local.rsplit_once('/').unwrap();
    let late = format!("https://127.0.0.1:{port}/incoming-payments/{id}");
    // The lookup's bound is the IO's, in real time; what says it ended the
    // read, and not the client's timeouts, is its error. The guard is a
    // hang's.
    let asked = asked_limits(&node, &client, &late, None);
    let limits = tokio::time::timeout(support::HANG, asked)
        .await
        .expect("the lookup is bounded");
    let read = limits.receiver_payment.unwrap();
    assert_eq!(read.error, "its node did not answer in time");
    held.abort();

    // The approved grants read no receiver.
    let access = json!([{
        "type": "outgoing-payment",
        "actions": ["create"],
        "identifier": host(&node),
        "limits": { "receiver": local, "debitAmount": usd(1_000) },
    }]);
    approve(&node, &client, &ask_for(&node, &client, &access).await).await;
    let Ok(response::Method::ListGrants(list)) = node
        .ipc()
        .await
        .call(Method::ListGrants(proto::ListGrantsRequest {}))
        .await
    else {
        panic!("no list")
    };
    let limits = list.grants[0].access[0].limits.as_ref().unwrap();
    assert_eq!(limits.receiver, local);
    assert_eq!(limits.receiver_payment, None);
    node.stop().await;
    payee.stop().await;
}

// A receive limit in another asset than the receiver the grant names fits
// no payment to it: the consent page is told, so the owner sees it before
// approving (spec/decisions/unusable-limits.md). The request itself is not
// refused.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_sees_a_receive_limit_in_another_asset() {
    let (node, key) = start().await;
    let client = own_client(&node, &key);
    let local = receiver(&node, 700).await;
    let flagged = async |receiver: &str, code: &str, scale: u8| {
        let limit = json!({ "value": "500", "assetCode": code, "assetScale": scale });
        let limits =
            json!({ "receiver": receiver, "debitAmount": usd(1_000), "receiveAmount": limit });
        let read = limits_read(&node, &client, limits)
            .await
            .receiver_payment
            .unwrap();
        (read.error.is_empty(), read.limit_in_other_asset)
    };
    // The receiver is paid in USD at scale 9.
    assert_eq!(flagged(&local, "EUR", 9).await, (true, true));
    assert_eq!(flagged(&local, "USD", 2).await, (true, true));
    assert_eq!(flagged(&local, "USD", 9).await, (true, false));
    // One that cannot be read is not judged.
    let (prefix, _) = local.rsplit_once('/').unwrap();
    let missing = format!("{prefix}/{}", "0".repeat(26));
    assert_eq!(flagged(&missing, "EUR", 2).await, (false, false));
    node.stop().await;
}

// Under a receive limit a payment delivers what it reserved of the grant
// and no more, though it fixed its debit and the path pays better than its
// quote, which takes STREAM's slippage off the rate it measured. What it
// did not send goes back to the grant
// (spec/decisions/receive-limit-caps-delivery.md).
#[tokio::test(flavor = "multi_thread")]
async fn a_receive_limit_caps_what_a_better_path_delivers() {
    const AMOUNT: u64 = 1_500_000_000;
    let pair = Pair::start(61, 62).await;
    let (a, b) = (&pair.a, &pair.b);
    let client = own_client(a, &pair.key_a);
    let access = json!([{
        "type": "outgoing-payment",
        "actions": ["create", "read"],
        "identifier": host(a),
        "limits": { "debitAmount": usd(AMOUNT), "receiveAmount": usd(AMOUNT) },
    }]);
    let grant = ask_for(a, &client, &access).await;
    let token = approve(a, &client, &grant).await;
    let target = b.ipc().await.incoming_payment(None).await;
    let pay = json!({
        "walletAddress": host(a), "incomingPayment": target.url, "debitAmount": usd(AMOUNT),
    });
    let reply = post(a, &client, "/outgoing-payments", &token, &pay).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let payment = body(&reply);
    // At the measured one for one, less STREAM's 1% slippage.
    let receive = 1_485_000_000;
    assert_eq!(payment["receiveAmount"], usd(receive));
    let path = path_of(a, &payment["id"]);
    let id = path.rsplit('/').next().unwrap();
    let outgoing = async || {
        let Ok(response::Method::ListOutgoingPayments(list)) = a
            .ipc()
            .await
            .call(Method::ListOutgoingPayments(
                proto::ListOutgoingPaymentsRequest { limit: 0 },
            ))
            .await
        else {
            panic!("no list")
        };
        list.payments.into_iter().find(|p| p.id == id).unwrap()
    };
    until(&pair.clock, async || outgoing().await.state != "sending").await;
    let done = outgoing().await;
    assert_eq!(done.state, "completed", "{}", done.failure);
    assert!(done.sent_amount < AMOUNT, "sent {}", done.sent_amount);
    let Ok(response::Method::ListIncomingPayments(list)) = b
        .ipc()
        .await
        .call(Method::ListIncomingPayments(
            proto::ListIncomingPaymentsRequest { limit: 0 },
        ))
        .await
    else {
        panic!("no list")
    };
    let paid = list.payments.iter().find(|p| target.url.ends_with(&p.id));
    assert_eq!(paid.unwrap().received_amount, receive);
    let spent = spent(a, &client, &token).await;
    assert_eq!(spent["spentDebitAmount"], usd(done.sent_amount));
    assert_eq!(spent["spentReceiveAmount"], usd(receive));
}

//! Open Payments over real HTTPS (spec/decisions/open-payments.md): wallet
//! addresses and keys, grants for clients known by their wallet address or
//! only by their key, signed requests, and incoming payments that take
//! exactly what they ask for. The original had no grants, keys or signatures,
//! and served no incoming payment by its ID.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

mod support;

use bytes::Bytes;
use dassie_node::node::Policies;
use dassie_node::open_payments::keys::ClientKey;
use dassie_node::rpc::proto::{self, request::Method};
use http_body_util::Full;
use hyper::{Request, StatusCode};
use serde_json::json;
use support::op::*;
use support::send;

#[tokio::test(flavor = "multi_thread")]
async fn wallet_addresses_and_their_keys() {
    let (node, key) = start().await;
    let reply = send(node.https, true, get(&node, "/.well-known/pay")).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.header("access-control-allow-origin"), Some("*"));
    let address = body(&reply);
    let origin = node.origin();
    assert_eq!(address["id"], format!("{origin}/.well-known/pay"));
    assert_eq!(address["assetCode"], "USD");
    assert_eq!(address["assetScale"], 9);
    assert_eq!(address["authServer"], format!("{origin}/gnap"));
    assert_eq!(address["resourceServer"], origin);
    let by_ledger = body(&send(node.https, true, get(&node, "/pay/usd")).await);
    assert_eq!(by_ledger["id"], format!("{origin}/pay/usd"));
    let missing = send(node.https, true, get(&node, "/pay/xrp")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(body(&missing)["error"]["code"], "not_found");
    let jwks = body(&send(node.https, true, get(&node, "/pay/usd/jwks.json")).await);
    let own = ClientKey::derive(&key).unwrap();
    assert_eq!(jwks["keys"][0]["kid"], own.kid());
    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/incoming-payments")
        .header("host", node.host())
        .header("origin", "https://elsewhere.example")
        .header("access-control-request-method", "POST")
        .body(Full::new(Bytes::new()))
        .unwrap();
    let reply = send(node.https, true, preflight).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(
        reply
            .header("access-control-allow-headers")
            .unwrap()
            .contains("Signature-Input")
    );
    node.stop().await;
}

// spec/areas/payments.md#wallet-addresses,
// spec/areas/payments.md#where-received-money-is-kept: the owner lists one
// address per asset, chooses the default by its asset's name, and chooses
// where funds are kept only for an asset a settlement hub of the realm
// holds, all of them at once with no name.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_names_the_default_and_where_funds_are_kept() {
    use dassie_node::rpc::proto::response;

    let (node, _) = start().await;
    let mut owner = node.ipc().await;
    let listed = match owner
        .call(Method::ListWalletAddresses(
            proto::ListWalletAddressesRequest {},
        ))
        .await
        .unwrap()
    {
        response::Method::ListWalletAddresses(list) => list.wallet_addresses,
        other => panic!("{other:?}"),
    };
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "usd");
    assert_eq!(listed[0].ledger, "stub+usd");
    assert_eq!(listed[0].deposit_ledger, "stub+usd");
    assert_eq!(listed[0].funds_kept(), proto::FundsKept::Unspecified);
    assert!(listed[0].is_default);

    let default = |name: &str| {
        Method::SetDefaultWalletAddress(proto::SetDefaultWalletAddressRequest { name: name.into() })
    };
    let keep = |name: &str, kept: proto::FundsKept| {
        Method::SetFundsKept(proto::SetFundsKeptRequest {
            name: name.into(),
            funds_kept: kept.into(),
        })
    };
    // No ledger of XRP runs, and a ledger's slug or a hub's name is no
    // wallet address.
    for name in ["xrp", "stub+usd", "hub-xrp"] {
        let refused = owner.call(default(name)).await.unwrap_err();
        assert_eq!(refused.code(), proto::ErrorCode::NotFound, "{name}");
    }
    owner.call(default("usd")).await.unwrap();
    owner.call(default("")).await.unwrap();

    // The stub's dollars are held by no hub; a choice must say where.
    let refused = owner
        .call(keep("usd", proto::FundsKept::OwnAccount))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), proto::ErrorCode::NotFound);
    let refused = owner
        .call(keep("xrp", proto::FundsKept::Unspecified))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), proto::ErrorCode::InvalidArgument);
    owner
        .call(keep("xrp", proto::FundsKept::OwnAccount))
        .await
        .unwrap();
    owner.call(keep("", proto::FundsKept::Hub)).await.unwrap();
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_creates_and_reads_incoming_payments() {
    let (node, _) = start().await;
    let client = Client::keyed(7);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({
        "walletAddress": format!("{}/pay/usd", node.origin()),
        "incomingAmount": { "value": "250000000", "assetCode": "USD", "assetScale": 9 },
        "metadata": { "description": "coffee" },
    });
    let reply = send(
        node.https,
        true,
        signed(
            &node,
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
    let id = payment["id"].as_str().unwrap().to_string();
    let path = id.strip_prefix(&node.origin()).unwrap().to_string();
    assert_eq!(payment["completed"], false);
    assert_eq!(payment["receivedAmount"]["value"], "0");
    assert_eq!(payment["metadata"]["description"], "coffee");
    let method = &payment["methods"][0];
    assert_eq!(method["type"], "ilp");
    // Anyone may see what it received, and where to get a grant.
    let public = body(&send(node.https, true, get(&node, &path)).await);
    assert_eq!(
        public,
        json!({
            "receivedAmount": { "value": "0", "assetCode": "USD", "assetScale": 9 },
            "authServer": format!("{}/gnap", node.origin()),
        })
    );
    // Paying more than it asks for gets it exactly what it asks for.
    let destination = method["ilpAddress"].as_str().unwrap().to_string();
    let secret = base64_url(method["sharedSecret"].as_str().unwrap());
    let received = pay(&node, &destination, &secret, 400_000_000).await;
    assert_eq!(received, 250_000_000);
    let read = body(
        &send(
            node.https,
            true,
            signed(&node, &client, "GET", &path, Some(&token), None),
        )
        .await,
    );
    assert_eq!(read["receivedAmount"]["value"], "250000000");
    assert_eq!(read["completed"], true);
    assert_eq!(read["methods"], json!([]));
    // Completed, it takes nothing more.
    assert_eq!(pay(&node, &destination, &secret, 1).await, 0);
    let list_path = format!(
        "/incoming-payments?wallet-address={}&first=10",
        dassie_payments::uri::percent_encode(&format!("{}/.well-known/pay", node.origin()))
    );
    let list = body(
        &send(
            node.https,
            true,
            signed(&node, &client, "GET", &list_path, Some(&token), None),
        )
        .await,
    );
    assert_eq!(list["result"][0]["id"], id);
    assert_eq!(list["pagination"]["hasNextPage"], false);
    node.stop().await;
}

// Each read of an incoming payment gets credentials of its own: a STREAM
// connection per payment that pays it. Each sender is told what it
// delivered, not what the others did, and together they pay no more than
// it asks for.
#[tokio::test(flavor = "multi_thread")]
async fn each_read_pays_over_a_connection_of_its_own() {
    let (node, _) = start().await;
    let client = Client::keyed(7);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({
        "walletAddress": format!("{}/pay/usd", node.origin()),
        "incomingAmount": { "value": "300", "assetCode": "USD", "assetScale": 9 },
    });
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    let created = body(&reply);
    let path = path_of(&node, &created["id"]);
    let mut methods = vec![created["methods"][0].clone()];
    for _ in 0..2 {
        let read = send(
            node.https,
            true,
            signed(&node, &client, "GET", &path, Some(&token), None),
        )
        .await;
        methods.push(body(&read)["methods"][0].clone());
    }
    let credentials: Vec<(String, Vec<u8>)> = methods
        .iter()
        .map(|method| {
            (
                method["ilpAddress"].as_str().unwrap().to_string(),
                base64_url(method["sharedSecret"].as_str().unwrap()),
            )
        })
        .collect();
    for (index, (address, secret)) in credentials.iter().enumerate() {
        for (other, other_secret) in &credentials[index + 1..] {
            assert_ne!(address, other);
            assert_ne!(secret, other_secret);
        }
    }
    let [(first, first_secret), (second, second_secret), ..] = credentials.as_slice() else {
        panic!("no credentials")
    };
    // Sent, and what the receiver told the sender arrived.
    assert_eq!(
        pay_reported(&node, first, first_secret, 200).await,
        (200, 200)
    );
    assert_eq!(
        pay_reported(&node, second, second_secret, 200).await,
        (100, 100)
    );
    // Complete, it takes no more, and the first sender still hears its own.
    assert_eq!(pay_reported(&node, first, first_secret, 1).await, (0, 200));
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_completes_an_open_payment() {
    let (node, _) = start().await;
    let client = Client::keyed(8);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let payment = body(
        &send(
            node.https,
            true,
            signed(
                &node,
                &client,
                "POST",
                "/incoming-payments",
                Some(&token),
                Some(&create),
            ),
        )
        .await,
    );
    let path = payment["id"]
        .as_str()
        .unwrap()
        .strip_prefix(&node.origin())
        .unwrap()
        .to_string();
    let method = &payment["methods"][0];
    let destination = method["ilpAddress"].as_str().unwrap().to_string();
    let secret = base64_url(method["sharedSecret"].as_str().unwrap());
    assert_eq!(pay(&node, &destination, &secret, 3_000).await, 3_000);
    let complete = format!("{path}/complete");
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", &complete, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(body(&reply)["completed"], true);
    assert_eq!(body(&reply)["receivedAmount"]["value"], "3000");
    assert_eq!(pay(&node, &destination, &secret, 1).await, 0);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_named_by_its_wallet_address() {
    // The node is its own client here: its wallet address publishes the
    // key, which the authorization server fetches over HTTPS.
    let (node, key) = start().await;
    let own = ClientKey::derive(&key).unwrap();
    let client = Client {
        key: own.signing_key().clone(),
        kid: own.kid().to_string(),
        identity: json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) }),
    };
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.text());
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_without_a_good_token_and_signature_are_refused() {
    let (node, _) = start().await;
    let client = Client::keyed(9);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    // No token: 401, and where to get one.
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            None,
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        reply.header("www-authenticate"),
        Some(format!("GNAP as_uri={}/gnap", node.origin()).as_str())
    );
    // Another key with the token.
    let thief = Client::keyed(10);
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &thief,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    // A body the signature does not match.
    let mut tampered = signed(
        &node,
        &client,
        "POST",
        "/incoming-payments",
        Some(&token),
        Some(&create),
    );
    // The same length, so that only the digest tells.
    let changed = serde_json::to_string(&create)
        .unwrap()
        .replace("pay\"", "paz\"");
    *tampered.body_mut() = Full::new(Bytes::from(changed));
    let reply = send(node.https, true, tampered).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    // A signature from long ago.
    let stale = signed(
        &node,
        &client,
        "POST",
        "/incoming-payments",
        Some(&token),
        Some(&create),
    );
    node.clock.advance(10 * 60 * 1_000);
    let reply = send(node.https, true, stale).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.text());
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn grants_reach_only_what_they_name() {
    let (node, _) = start().await;
    let client = Client::keyed(11);
    let other = Client::keyed(12);
    let read_only = grant(
        &node,
        &client,
        json!([{ "type": "incoming-payment", "actions": ["read"] }]),
    )
    .await;
    let create = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&read_only),
            Some(&create),
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    // `read` reaches only what the client created.
    let token = grant(&node, &other, incoming_access(&node)).await;
    let payment = body(
        &send(
            node.https,
            true,
            signed(
                &node,
                &other,
                "POST",
                "/incoming-payments",
                Some(&token),
                Some(&create),
            ),
        )
        .await,
    );
    let path = payment["id"]
        .as_str()
        .unwrap()
        .strip_prefix(&node.origin())
        .unwrap()
        .to_string();
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "GET", &path, Some(&read_only), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    // A wallet address that is not the node's, and grants that need the
    // owner, are not offered.
    let foreign = json!([{ "type": "incoming-payment", "actions": ["create"], "identifier": "https://elsewhere.example/alice" }]);
    let request = json!({ "access_token": { "access": foreign }, "client": client.identity });
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(body(&reply)["error"]["code"], "invalid_request");
    let list_all = json!([{ "type": "incoming-payment", "actions": ["list-all"] }]);
    let request = json!({
        "access_token": { "access": list_all },
        "client": client.identity,
        "interact": { "start": ["redirect"] },
    });
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_reach_their_own_payments_and_read_any() {
    let (node, _) = start().await;
    let (alice, bob) = (Client::keyed(21), Client::keyed(22));
    let alice_token = grant(&node, &alice, incoming_access(&node)).await;
    let bob_token = grant(&node, &bob, incoming_access(&node)).await;
    let payment = create(&node, &alice, &alice_token).await;
    // Stored under the wallet address's canonical URL.
    assert_eq!(
        payment["walletAddress"],
        format!("{}/pay/usd", node.origin())
    );
    let path = path_of(&node, &payment["id"]);
    // Bob may neither complete nor list Alice's payment.
    let complete = format!("{path}/complete");
    let reply = send(
        node.https,
        true,
        signed(&node, &bob, "POST", &complete, Some(&bob_token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.text());
    let list = format!(
        "/incoming-payments?wallet-address={}",
        dassie_payments::uri::percent_encode(&format!("{}/pay/usd", node.origin()))
    );
    let page = body(
        &send(
            node.https,
            true,
            signed(&node, &bob, "GET", &list, Some(&bob_token), None),
        )
        .await,
    );
    assert_eq!(page["result"], json!([]));
    // A sender's wallet reads any payment, without the owner, to pay it.
    let read_all = grant(
        &node,
        &bob,
        json!([{ "type": "incoming-payment", "actions": ["read-all"] }]),
    )
    .await;
    let read = body(
        &send(
            node.https,
            true,
            signed(&node, &bob, "GET", &path, Some(&read_all), None),
        )
        .await,
    );
    assert_eq!(read["methods"][0]["type"], "ilp");
    // An empty grant is refused.
    let request = json!({ "access_token": { "access": [] }, "client": bob.identity });
    let reply = send(
        node.https,
        true,
        signed(&node, &bob, "POST", "/gnap", None, Some(&request)),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_page_newest_first() {
    let (node, _) = start().await;
    let client = Client::keyed(23);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(create(&node, &client, &token).await["id"].clone());
        node.clock.advance(1_000);
    }
    let address =
        dassie_payments::uri::percent_encode(&format!("{}/.well-known/pay", node.origin()));
    let list = |query: String| {
        let path = format!("/incoming-payments?wallet-address={address}{query}");
        send(
            node.https,
            true,
            signed(&node, &client, "GET", &path, Some(&token), None),
        )
    };
    let first = body(&list("&first=2".into()).await);
    assert_eq!(
        first["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].clone())
            .collect::<Vec<_>>(),
        vec![ids[2].clone(), ids[1].clone()]
    );
    assert_eq!(first["pagination"]["hasNextPage"], true);
    assert_eq!(first["pagination"]["hasPreviousPage"], false);
    let cursor = first["pagination"]["endCursor"]
        .as_str()
        .unwrap()
        .to_string();
    let next = body(&list(format!("&first=2&cursor={cursor}")).await);
    assert_eq!(next["result"][0]["id"], ids[0]);
    assert_eq!(next["pagination"]["hasNextPage"], false);
    assert_eq!(next["pagination"]["hasPreviousPage"], true);
    let back = body(
        &list(format!(
            "&last=5&cursor={}",
            next["pagination"]["startCursor"].as_str().unwrap()
        ))
        .await,
    );
    assert_eq!(back["result"].as_array().unwrap().len(), 2);
    assert_eq!(back["result"][0]["id"], ids[2]);
    let unknown = list("&cursor=00000000-0000-4000-8000-000000000000".into()).await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tokens_rotate_and_grants_revoke() {
    let (node, _) = start().await;
    let client = Client::keyed(24);
    let grant = full_grant(&node, &client, incoming_access(&node)).await;
    let token = grant["access_token"]["value"].as_str().unwrap().to_string();
    let manage = path_of(&node, &grant["access_token"]["manage"]);
    let reply = send(
        node.https,
        true,
        signed(&node, &client, "POST", &manage, Some(&token), None),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    let rotated = body(&reply)["access_token"].clone();
    let new_token = rotated["value"].as_str().unwrap().to_string();
    // The old token is gone; the new one works.
    let create_body = json!({ "walletAddress": format!("{}/.well-known/pay", node.origin()) });
    let old = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&token),
            Some(&create_body),
        ),
    )
    .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);
    create(&node, &client, &new_token).await;
    // Revoking the new token ends it.
    let new_manage = path_of(&node, &rotated["manage"]);
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "DELETE",
            &new_manage,
            Some(&new_token),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text());
    let revoked = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&new_token),
            Some(&create_body),
        ),
    )
    .await;
    assert_eq!(revoked.status, StatusCode::UNAUTHORIZED);
    // A second grant, revoked through its continuation, takes its token.
    let second = full_grant(&node, &client, incoming_access(&node)).await;
    let second_token = second["access_token"]["value"]
        .as_str()
        .unwrap()
        .to_string();
    let continuation = path_of(&node, &second["continue"]["uri"]);
    let continue_token = second["continue"]["access_token"]["value"]
        .as_str()
        .unwrap();
    // Another key cannot revoke it.
    let thief = Client::keyed(25);
    let reply = send(
        node.https,
        true,
        signed(
            &node,
            &thief,
            "DELETE",
            &continuation,
            Some(continue_token),
            None,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
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
    let gone = send(
        node.https,
        true,
        signed(
            &node,
            &client,
            "POST",
            "/incoming-payments",
            Some(&second_token),
            Some(&create_body),
        ),
    )
    .await;
    assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
    node.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_never_crowd_out_the_owner() {
    let policies = Policies {
        incoming: dassie_node::engine::IncomingPolicy {
            max_open_owner: 1,
            max_open_clients: 2,
            ..dassie_node::engine::IncomingPolicy::DEFAULT
        },
        ..Policies::DEV
    };
    let (node, _) = start_with(policies).await;
    let client = Client::keyed(26);
    let token = grant(&node, &client, incoming_access(&node)).await;
    let first = create(&node, &client, &token).await;
    node.clock.advance(1_000);
    create(&node, &client, &token).await;
    node.clock.advance(1_000);
    // A third closes the oldest that received nothing.
    create(&node, &client, &token).await;
    let path = path_of(&node, &first["id"]);
    let closed = body(
        &send(
            node.https,
            true,
            signed(&node, &client, "GET", &path, Some(&token), None),
        )
        .await,
    );
    assert_eq!(closed["completed"], true);
    assert_eq!(closed["methods"], json!([]));
    // The owner's share is the owner's: one fits, a second does not.
    let mut owner = node.ipc().await;
    owner.incoming_payment(None).await;
    let full = owner
        .call(Method::CreateIncomingPayment(
            proto::CreateIncomingPaymentRequest::default(),
        ))
        .await;
    assert_eq!(
        full.unwrap_err().code(),
        proto::ErrorCode::ResourceExhausted
    );
    node.stop().await;
}

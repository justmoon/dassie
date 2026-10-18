//! The owner's Send (D9, M6c), over IPC as the wallet calls it: to a
//! payment pointer on a peer, to a payment request, to the node's own
//! wallet address, and what is refused, with fake wallets for what real
//! nodes never do.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

mod support;

use std::convert::Infallible;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use dassie_node::clock::ManualClock;
use dassie_node::rpc::proto::{
    self, client_message, request::Method, response, server_message, stream_item,
};
use http_body_util::Full;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use support::TestNode;
use support::op::{CERT, KEY, start};
use support::pair::{Pair, until};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// 1.5 USD.
const AMOUNT: u64 = 1_500_000_000;
/// What a quote for it debits on a path that delivers one for one: STREAM's
/// 1% slippage on the measured rate, rounded up.
const QUOTED: u64 = 1_515_151_516;

async fn resolve(node: &TestNode, recipient: &str) -> Result<proto::Recipient, proto::Error> {
    match node
        .ipc()
        .await
        .call(Method::ResolveRecipient(proto::ResolveRecipientRequest {
            recipient: recipient.into(),
        }))
        .await?
    {
        response::Method::ResolveRecipient(found) => Ok(found.recipient.unwrap()),
        other => panic!("{other:?}"),
    }
}

async fn quote(
    node: &TestNode,
    recipient: &str,
    amount: Option<u64>,
    note: &str,
) -> Result<proto::SendQuote, proto::Error> {
    match node
        .ipc()
        .await
        .call(Method::QuoteSend(proto::QuoteSendRequest {
            recipient: recipient.into(),
            ledger: String::new(),
            amount: amount.map(proto::quote_send_request::Amount::ReceiveAmount),
            note: note.into(),
        }))
        .await?
    {
        response::Method::QuoteSend(quoted) => Ok(quoted.quote.unwrap()),
        other => panic!("{other:?}"),
    }
}

async fn send(node: &TestNode, quote_id: &str) -> Result<proto::OutgoingPayment, proto::Error> {
    match node
        .ipc()
        .await
        .call(Method::SendPayment(proto::SendPaymentRequest {
            quote_id: quote_id.into(),
        }))
        .await?
    {
        response::Method::SendPayment(sent) => Ok(sent.payment.unwrap()),
        other => panic!("{other:?}"),
    }
}

/// The node's outgoing payment `id`, as the owner lists it.
async fn outgoing(node: &TestNode, id: &str) -> proto::OutgoingPayment {
    let Ok(response::Method::ListOutgoingPayments(list)) = node
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
}

/// The node's incoming payments.
async fn incoming(node: &TestNode) -> Vec<proto::IncomingPayment> {
    let Ok(response::Method::ListIncomingPayments(list)) = node
        .ipc()
        .await
        .call(Method::ListIncomingPayments(
            proto::ListIncomingPaymentsRequest { limit: 0 },
        ))
        .await
    else {
        panic!("no list")
    };
    list.payments
}

/// Moves `clock` on until the payment has finished, and returns it.
async fn finished(clock: &ManualClock, node: &TestNode, id: &str) -> proto::OutgoingPayment {
    until(clock, async || outgoing(node, id).await.state != "sending").await;
    outgoing(node, id).await
}

/// Moves `clock` on until the incoming payment on `node` that `which`
/// picks has received `amount`, and returns it. The receiver counts a
/// packet as it posts it, before its Fulfill goes back, so this returns at
/// once unless the payment is still sending.
async fn paid_in(
    clock: &ManualClock,
    node: &TestNode,
    amount: u64,
    which: impl Fn(&proto::IncomingPayment) -> bool,
) -> proto::IncomingPayment {
    until(clock, async || {
        incoming(node)
            .await
            .iter()
            .any(|payment| which(payment) && payment.received_amount >= amount)
    })
    .await;
    incoming(node).await.into_iter().find(which).unwrap()
}

async fn ledger_balance(node: &TestNode) -> i64 {
    let Ok(response::Method::ListLedgers(ledgers)) = node
        .ipc()
        .await
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await
    else {
        panic!("no ledgers")
    };
    ledgers.ledgers[0].owner_balance
}

fn code(result: Result<impl std::fmt::Debug, proto::Error>) -> proto::ErrorCode {
    result.unwrap_err().code()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_sends_to_a_payment_pointer_on_a_peer() {
    let pair = Pair::start(41, 42).await;
    let (a, b) = (&pair.a, &pair.b);
    let pointer = format!("${}", b.host());
    let recipient = resolve(a, &pointer).await.unwrap();
    assert_eq!(
        recipient.wallet_address,
        format!("{}/.well-known/pay", b.origin())
    );
    assert_eq!(recipient.payment_pointer, pointer);
    assert_eq!(
        (recipient.asset_code.as_str(), recipient.asset_scale),
        ("USD", 9)
    );
    assert!(!recipient.is_own && recipient.request.is_none());

    let before_a = ledger_balance(a).await;
    let before_b = ledger_balance(b).await;
    let quoted = quote(a, &pointer, Some(AMOUNT), "lunch").await.unwrap();
    assert_eq!(quoted.debit_amount, QUOTED);
    assert_eq!(quoted.receive_amount.as_ref().unwrap().value, AMOUNT);
    assert!(
        quoted
            .receiver
            .starts_with(&format!("{}/incoming-payments/", b.origin())),
        "{}",
        quoted.receiver
    );

    // The status page follows it from here.
    let mut stream = a.ipc().await;
    let sent = send(a, &quoted.id).await.unwrap();
    stream
        .send_raw(proto::ClientMessage {
            call_id: 1,
            kind: Some(client_message::Kind::Request(proto::Request {
                method: Some(Method::SubscribeOutgoingPayment(
                    proto::SubscribeOutgoingPaymentRequest {
                        id: sent.id.clone(),
                    },
                )),
            })),
        })
        .await;
    let done = finished(&pair.clock, a, &sent.id).await;
    assert_eq!(done.state, "completed", "{}", done.failure);
    assert_eq!(done.sent_amount, AMOUNT);
    assert_eq!(done.recipient_pointer, pointer);
    assert!(done.client.is_empty());
    let last = loop {
        let message = tokio::time::timeout(Duration::from_secs(5), stream.receive())
            .await
            .expect("no item")
            .expect("closed");
        let Some(server_message::Kind::Item(item)) = message.kind else {
            panic!("{message:?}")
        };
        let Some(stream_item::Method::SubscribeOutgoingPayment(payment)) = item.method else {
            panic!("{item:?}")
        };
        if payment.state != "sending" {
            break payment;
        }
    };
    assert_eq!(last.state, "completed");

    // B has it, with the note, complete; the money moved.
    let received = paid_in(&pair.clock, b, AMOUNT, |payment| {
        payment.url == quoted.receiver
    })
    .await;
    assert_eq!(received.received_amount, AMOUNT);
    assert_eq!(received.metadata_json, r#"{"description":"lunch"}"#);
    assert!(received.client.ends_with("/.well-known/pay"));
    until(&pair.clock, async || {
        ledger_balance(b).await == before_b + i64::try_from(AMOUNT).unwrap()
    })
    .await;
    assert_eq!(
        ledger_balance(a).await,
        before_a - i64::try_from(AMOUNT).unwrap()
    );

    // A quote pays once.
    assert_eq!(
        code(send(a, &quoted.id).await),
        proto::ErrorCode::InvalidArgument
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_pays_a_payment_request_on_a_peer() {
    let pair = Pair::start(43, 44).await;
    let (a, b) = (&pair.a, &pair.b);
    let mut owner_b = b.ipc().await;
    let Ok(response::Method::CreateIncomingPayment(created)) = owner_b
        .call(Method::CreateIncomingPayment(
            proto::CreateIncomingPaymentRequest {
                incoming_amount: Some(AMOUNT),
                description: "invoice 7".into(),
                ..proto::CreateIncomingPaymentRequest::default()
            },
        ))
        .await
    else {
        panic!("no request")
    };
    let request = created.payment.unwrap();
    let recipient = resolve(a, &request.url).await.unwrap();
    let found = recipient.request.unwrap();
    assert_eq!(found.url, request.url);
    assert_eq!(found.remaining, Some(AMOUNT));
    assert_eq!(found.description, "invoice 7");
    // More than it asks, or a note of one's own, is refused.
    assert_eq!(
        code(quote(a, &request.url, Some(AMOUNT + 1), "").await),
        proto::ErrorCode::InvalidArgument
    );
    assert_eq!(
        code(quote(a, &request.url, None, "hello").await),
        proto::ErrorCode::InvalidArgument
    );
    // No amount: what it still accepts.
    let quoted = quote(a, &request.url, None, "").await.unwrap();
    assert_eq!(quoted.debit_amount, QUOTED);
    assert_eq!(quoted.receiver, request.url);
    let sent = send(a, &quoted.id).await.unwrap();
    let done = finished(&pair.clock, a, &sent.id).await;
    assert_eq!(done.state, "completed", "{}", done.failure);
    let paid = paid_in(&pair.clock, b, AMOUNT, |payment| payment.id == request.id).await;
    assert_eq!(paid.received_amount, AMOUNT);
    // Paid, it can be paid no more.
    assert_eq!(
        code(quote(a, &request.url, None, "").await),
        proto::ErrorCode::InvalidArgument
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_sends_to_its_own_wallet_address() {
    let (node, _) = start().await;
    let pointer = format!("${}", node.host());
    let recipient = resolve(&node, &pointer).await.unwrap();
    assert!(recipient.is_own);
    let quoted = quote(&node, &pointer, Some(AMOUNT), "to myself")
        .await
        .unwrap();
    let sent = send(&node, &quoted.id).await.unwrap();
    let done = finished(&node.clock, &node, &sent.id).await;
    assert_eq!(done.state, "completed", "{}", done.failure);
    let received = paid_in(&node.clock, &node, AMOUNT, |payment| {
        payment.url == quoted.receiver
    })
    .await;
    assert_eq!(received.received_amount, AMOUNT);
    assert_eq!(received.metadata_json, r#"{"description":"to myself"}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn sends_that_cannot_be_made_are_refused_and_say_why() {
    let (node, _) = start().await;
    let own = format!("${}", node.host());
    for input in ["", "alice", "http://localhost/.well-known/pay"] {
        let error = resolve(&node, input).await.unwrap_err();
        assert_eq!(error.code(), proto::ErrorCode::InvalidArgument, "{input}");
        assert!(
            error.message.contains("payment pointer"),
            "{}",
            error.message
        );
    }
    // Nobody there.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let error = resolve(&node, &format!("$localhost:{port}"))
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        proto::ErrorCode::Unavailable,
        "{}",
        error.message
    );
    // A wallet address needs an amount, and a note fits in 256 characters.
    assert_eq!(
        code(quote(&node, &own, None, "").await),
        proto::ErrorCode::InvalidArgument
    );
    assert_eq!(
        code(quote(&node, &own, Some(1), &"x".repeat(257)).await),
        proto::ErrorCode::InvalidArgument
    );
    // A quote lapses.
    let quoted = quote(&node, &own, Some(AMOUNT), "").await.unwrap();
    node.clock.advance(6 * 60 * 1_000);
    let error = send(&node, &quoted.id).await.unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::InvalidArgument);
    assert!(error.message.contains("expired"), "{}", error.message);
    // More than the funds.
    let quoted = quote(&node, &own, Some(1_000_000_000_000), "")
        .await
        .unwrap();
    assert_eq!(
        code(send(&node, &quoted.id).await),
        proto::ErrorCode::FailedPrecondition
    );
    // No such payment to follow.
    let mut owner = node.ipc().await;
    owner
        .send_raw(proto::ClientMessage {
            call_id: 9,
            kind: Some(client_message::Kind::Request(proto::Request {
                method: Some(Method::SubscribeOutgoingPayment(
                    proto::SubscribeOutgoingPaymentRequest {
                        id: "00000000-0000-4000-8000-000000000000".into(),
                    },
                )),
            })),
        })
        .await;
    let message = owner.receive().await.unwrap();
    let Some(server_message::Kind::Error(error)) = message.kind else {
        panic!("{message:?}")
    };
    assert_eq!(error.code(), proto::ErrorCode::NotFound);
}

/// A wallet that answers each request with what `answer` says, by method,
/// path, its own port, and whether the request carried a token, over HTTPS
/// at `localhost`; its port.
async fn fake_wallet(
    answer: impl Fn(&str, &str, u16, bool) -> (u16, String) + Send + Sync + 'static,
) -> u16 {
    let certs = CertificateDer::pem_file_iter(CERT)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(KEY).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let answer = Arc::clone(&answer);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(stream).await else {
                    return;
                };
                let service =
                    hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
                        let authorized = request.headers().contains_key("authorization");
                        let (status, body) = answer(
                            request.method().as_str(),
                            request.uri().path(),
                            port,
                            authorized,
                        );
                        async move {
                            Ok::<_, Infallible>(
                                Response::builder()
                                    .status(status)
                                    .header("content-type", "application/json")
                                    .body(Full::new(Bytes::from(body)))
                                    .unwrap(),
                            )
                        }
                    });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            });
        }
    });
    port
}

fn wallet_document(id: &str, port: u16) -> String {
    serde_json::json!({
        "id": id,
        "assetCode": "USD",
        "assetScale": 9,
        "authServer": format!("https://localhost:{port}/gnap"),
        "resourceServer": format!("https://localhost:{port}"),
    })
    .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn wallets_that_misbehave_are_not_paid() {
    let (node, _) = start().await;
    // One that says it is another.
    let port = fake_wallet(|_, path, port, _| match path {
        "/.well-known/pay" => (
            200,
            wallet_document("https://elsewhere.example/.well-known/pay", port),
        ),
        _ => (404, "{}".into()),
    })
    .await;
    let error = resolve(&node, &format!("$localhost:{port}"))
        .await
        .unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::InvalidArgument);
    assert!(error.message.contains("names another"), "{}", error.message);

    // One that grants, then refuses the token, twice: the node asks again
    // once, then gives up.
    let grants = Arc::new(AtomicU32::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (counted, log) = (Arc::clone(&grants), Arc::clone(&seen));
    let port = fake_wallet(move |method, path, port, _| {
        log.lock().unwrap().push(format!("{method} {path}"));
        match (method, path) {
            ("GET", "/.well-known/pay") => (
                200,
                wallet_document(&format!("https://localhost:{port}/.well-known/pay"), port),
            ),
            ("POST", "/gnap") => {
                let n = counted.fetch_add(1, Ordering::SeqCst);
                (
                    200,
                    serde_json::json!({
                        "access_token": {
                            "value": format!("token-{n}"),
                            "manage": format!("https://localhost:{port}/gnap/token/{n}"),
                            "expires_in": 600,
                            "access": [],
                        },
                        "continue": {
                            "access_token": { "value": "c" },
                            "uri": format!("https://localhost:{port}/gnap/continue/{n}"),
                        },
                    })
                    .to_string(),
                )
            }
            _ => (
                401,
                r#"{"error":{"code":"unauthorized","description":"no"}}"#.into(),
            ),
        }
    })
    .await;
    let pointer = format!("$localhost:{port}");
    assert!(resolve(&node, &pointer).await.is_ok());
    let error = quote(&node, &pointer, Some(AMOUNT), "").await.unwrap_err();
    assert_eq!(
        error.code(),
        proto::ErrorCode::InvalidArgument,
        "{}",
        error.message
    );
    assert!(error.message.contains("401"), "{}", error.message);
    assert_eq!(
        grants.load(Ordering::SeqCst),
        2,
        "{:?}",
        seen.lock().unwrap()
    );
}

/// A grant, as an authorization server answers one.
fn granted(port: u16) -> String {
    serde_json::json!({
        "access_token": {
            "value": "token",
            "manage": format!("https://localhost:{port}/gnap/token/1"),
            "expires_in": 600,
            "access": [],
        },
        "continue": {
            "access_token": { "value": "c" },
            "uri": format!("https://localhost:{port}/gnap/continue/1"),
        },
    })
    .to_string()
}

// Found in review: a payment request's wallet address came from the
// request, so a request elsewhere could name any wallet, the owner's too,
// and be shown, and kept, as a payment to it.
#[tokio::test(flavor = "multi_thread")]
async fn payment_requests_speak_only_for_their_own_wallet() {
    let (node, _) = start().await;
    // A wallet that is what it says.
    let shop = fake_wallet(|_, path, port, _| match path {
        "/.well-known/pay" => (
            200,
            wallet_document(&format!("https://localhost:{port}/.well-known/pay"), port),
        ),
        _ => (404, "{}".into()),
    })
    .await;
    // A request elsewhere that names whatever `claims` holds.
    let claims = Arc::new(Mutex::new(String::new()));
    let named = Arc::clone(&claims);
    let evil = fake_wallet(move |method, path, port, authorized| {
        let url = format!("https://localhost:{port}/incoming-payments/1");
        match (method, path, authorized) {
            ("GET", "/incoming-payments/1", false) => (
                200,
                serde_json::json!({
                    "receivedAmount": { "value": "0", "assetCode": "USD", "assetScale": 9 },
                    "authServer": format!("https://localhost:{port}/gnap"),
                })
                .to_string(),
            ),
            ("GET", "/incoming-payments/1", true) => (
                200,
                serde_json::json!({
                    "id": url,
                    "walletAddress": named.lock().unwrap().clone(),
                    "completed": false,
                    "receivedAmount": { "value": "0", "assetCode": "USD", "assetScale": 9 },
                    "createdAt": "2027-01-15T08:00:00.000Z",
                    "methods": [{
                        "type": "ilp",
                        "ilpAddress": "test.evil",
                        "sharedSecret": "A".repeat(43),
                    }],
                })
                .to_string(),
            ),
            ("POST", "/gnap", _) => (200, granted(port)),
            _ => (404, "{}".into()),
        }
    })
    .await;
    let request = format!("https://localhost:{evil}/incoming-payments/1");
    for wallet in [
        format!("https://localhost:{shop}/.well-known/pay"),
        format!("{}/.well-known/pay", node.origin()),
    ] {
        *claims.lock().unwrap() = wallet.clone();
        let error = resolve(&node, &request).await.unwrap_err();
        assert_eq!(error.code(), proto::ErrorCode::InvalidArgument, "{wallet}");
        assert!(error.message.contains("not its own"), "{}", error.message);
        assert_eq!(
            code(quote(&node, &request, Some(1), "").await),
            proto::ErrorCode::InvalidArgument
        );
    }
}

// Found in review: the node's own URLs in another case, or with the
// scheme's port, were taken for another node's.
#[tokio::test(flavor = "multi_thread")]
async fn the_nodes_own_urls_are_its_own_in_any_case() {
    let (node, _) = start().await;
    let shouted = node.origin().replace("localhost", "LOCALHOST");
    let recipient = resolve(&node, &format!("{shouted}/.well-known/pay"))
        .await
        .unwrap();
    assert!(recipient.is_own);
    assert_eq!(recipient.payment_pointer, format!("${}", node.host()));
    let request = node.ipc().await.incoming_payment(Some(AMOUNT)).await;
    let shouted_request = request.url.replace("localhost", "LOCALHOST");
    let found = resolve(&node, &shouted_request).await.unwrap();
    assert!(found.is_own);
    assert_eq!(found.request.unwrap().remaining, Some(AMOUNT));
}

// "You send" (M7c): what is sent is fixed and goes out whole; the incoming
// payment created takes any amount, and the quote says what it buys at
// the measured rate.
#[tokio::test(flavor = "multi_thread")]
async fn the_owner_sends_a_fixed_amount() {
    let pair = Pair::start(47, 48).await;
    let (a, b) = (&pair.a, &pair.b);
    let fixed = |ledger: &str| {
        Method::QuoteSend(proto::QuoteSendRequest {
            recipient: format!("${}", b.host()),
            ledger: ledger.into(),
            amount: Some(proto::quote_send_request::Amount::DebitAmount(AMOUNT)),
            note: String::new(),
        })
    };
    let quoted = match a.ipc().await.call(fixed("")).await {
        Ok(response::Method::QuoteSend(quoted)) => quoted.quote.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(quoted.debit_amount, AMOUNT);
    // At the measured one for one, less STREAM's 1% slippage.
    assert_eq!(quoted.receive_amount.as_ref().unwrap().value, 1_485_000_000);
    let sent = send(a, &quoted.id).await.unwrap();
    let done = finished(&pair.clock, a, &sent.id).await;
    assert_eq!(done.state, "completed", "{}", done.failure);
    assert_eq!(done.sent_amount, AMOUNT);
    let paid = paid_in(&pair.clock, b, AMOUNT, |payment| {
        quoted.receiver.ends_with(&payment.id)
    })
    .await;
    assert_eq!((paid.incoming_amount, paid.received_amount), (None, AMOUNT));
    // Naming a ledger the route does not start on is refused.
    let named = a.ipc().await.call(fixed("xrpl-testnet+xrp")).await;
    assert_eq!(code(named), proto::ErrorCode::InvalidArgument);
}

// Found in review: a fixed amount to send, to a request that takes a
// little less than it buys at the real rate, fills the request and ends
// there, rather than waiting to send the rest until it runs out of time.
#[tokio::test(flavor = "multi_thread")]
async fn a_fixed_amount_to_send_ends_when_the_request_is_full() {
    let pair = Pair::start(49, 50).await;
    let (a, b) = (&pair.a, &pair.b);
    let Ok(response::Method::CreateIncomingPayment(created)) = b
        .ipc()
        .await
        .call(Method::CreateIncomingPayment(
            proto::CreateIncomingPaymentRequest {
                incoming_amount: Some(AMOUNT),
                description: "invoice".into(),
                ..proto::CreateIncomingPaymentRequest::default()
            },
        ))
        .await
    else {
        panic!("no request")
    };
    let request = created.payment.unwrap();
    // 1.51 buys 1.4949 at the quote's rate, which the request takes; at
    // the real one for one it buys more than the 1.50 it asks.
    let quoted = match a
        .ipc()
        .await
        .call(Method::QuoteSend(proto::QuoteSendRequest {
            recipient: request.url.clone(),
            ledger: String::new(),
            amount: Some(proto::quote_send_request::Amount::DebitAmount(
                AMOUNT + 10_000_000,
            )),
            note: String::new(),
        }))
        .await
    {
        Ok(response::Method::QuoteSend(quoted)) => quoted.quote.unwrap(),
        other => panic!("{other:?}"),
    };
    let sent = send(a, &quoted.id).await.unwrap();
    let done = finished(&pair.clock, a, &sent.id).await;
    assert_eq!(done.state, "completed", "{}", done.failure);
    assert_eq!(done.sent_amount, AMOUNT);
    let paid = paid_in(&pair.clock, b, AMOUNT, |payment| payment.id == request.id).await;
    assert_eq!(paid.received_amount, AMOUNT);
}

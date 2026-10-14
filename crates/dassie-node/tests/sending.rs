//! The owner's Send (D9, M6c), over IPC as the wallet calls it: to a
//! payment pointer on a peer, to a payment request, to the node's own
//! wallet address, and what is refused, with fake wallets for what real
//! nodes never do.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

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
use support::op::{CERT, KEY, start};
use support::pair::{Pair, until};
use support::TestNode;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// 1.5 USD.
const AMOUNT: u64 = 1_500_000_000;

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
            amount,
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
    assert_eq!((recipient.asset_code.as_str(), recipient.asset_scale), ("USD", 9));
    assert!(!recipient.is_own && recipient.request.is_none());

    let before_a = ledger_balance(a).await;
    let before_b = ledger_balance(b).await;
    let quoted = quote(a, &pointer, Some(AMOUNT), "lunch").await.unwrap();
    assert_eq!(quoted.debit_amount, AMOUNT);
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
    let received = incoming(b)
        .await
        .into_iter()
        .find(|payment| payment.url == quoted.receiver)
        .unwrap();
    assert_eq!(received.received_amount, AMOUNT);
    assert_eq!(received.metadata_json, r#"{"description":"lunch"}"#);
    assert!(received.client.ends_with("/.well-known/pay"));
    until(&pair.clock, async || {
        ledger_balance(b).await == before_b + i64::try_from(AMOUNT).unwrap()
    })
    .await;
    assert_eq!(ledger_balance(a).await, before_a - i64::try_from(AMOUNT).unwrap());

    // A quote pays once.
    assert_eq!(code(send(a, &quoted.id).await), proto::ErrorCode::InvalidArgument);
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
    assert_eq!(quoted.debit_amount, AMOUNT);
    assert_eq!(quoted.receiver, request.url);
    let sent = send(a, &quoted.id).await.unwrap();
    let done = finished(&pair.clock, a, &sent.id).await;
    assert_eq!(done.state, "completed", "{}", done.failure);
    let paid = incoming(b)
        .await
        .into_iter()
        .find(|payment| payment.id == request.id)
        .unwrap();
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
    let received = incoming(&node)
        .await
        .into_iter()
        .find(|payment| payment.url == quoted.receiver)
        .unwrap();
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
        assert!(error.message.contains("payment pointer"), "{}", error.message);
    }
    // Nobody there.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let error = resolve(&node, &format!("$localhost:{port}")).await.unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::Unavailable, "{}", error.message);
    // A wallet address needs an amount, and a note fits in 256 characters.
    assert_eq!(code(quote(&node, &own, None, "").await), proto::ErrorCode::InvalidArgument);
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
    let quoted = quote(&node, &own, Some(1_000_000_000_000), "").await.unwrap();
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

/// A wallet that answers each request with what `answer` says, by method
/// and path, over HTTPS at `localhost`; its port.
async fn fake_wallet(
    answer: impl Fn(&str, &str, u16) -> (u16, String) + Send + Sync + 'static,
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
                let service = hyper::service::service_fn(move |request: Request<hyper::body::Incoming>| {
                    let (status, body) = answer(request.method().as_str(), request.uri().path(), port);
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
    let port = fake_wallet(|_, path, port| match path {
        "/.well-known/pay" => (200, wallet_document("https://elsewhere.example/.well-known/pay", port)),
        _ => (404, "{}".into()),
    })
    .await;
    let error = resolve(&node, &format!("$localhost:{port}")).await.unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::InvalidArgument);
    assert!(error.message.contains("names another"), "{}", error.message);

    // One that grants, then refuses the token, twice: the node asks again
    // once, then gives up.
    let grants = Arc::new(AtomicU32::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (counted, log) = (Arc::clone(&grants), Arc::clone(&seen));
    let port = fake_wallet(move |method, path, port| {
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
            _ => (401, r#"{"error":{"code":"unauthorized","description":"no"}}"#.into()),
        }
    })
    .await;
    let pointer = format!("$localhost:{port}");
    assert!(resolve(&node, &pointer).await.is_ok());
    let error = quote(&node, &pointer, Some(AMOUNT), "").await.unwrap_err();
    assert_eq!(error.code(), proto::ErrorCode::InvalidArgument, "{}", error.message);
    assert!(error.message.contains("401"), "{}", error.message);
    assert_eq!(grants.load(Ordering::SeqCst), 2, "{:?}", seen.lock().unwrap());
}

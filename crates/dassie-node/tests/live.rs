//! The wallet's live streams and debug methods against a daemon (spec 7
//! §4.6, §5): each stream starts with what is, and follows what changes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node::log::{LogFilter, LogLevel};
use dassie_node::rpc::proto::{
    self, client_message, request::Method, response, server_message, stream_item,
};
use support::*;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

type Owner = RpcClient<Framed<UnixStream, LengthDelimitedCodec>>;

fn key() -> NodeKey {
    NodeKey::from_bytes(&[11; 32])
}

/// Sets the node up, waits for its ledger and activates the stub.
async fn ready(node: &TestNode) -> Owner {
    set_up(node, &key()).await;
    let mut owner = node.ipc().await;
    for _ in 0..200 {
        let reply = owner
            .call(Method::ListLedgers(proto::ListLedgersRequest {}))
            .await;
        if reply.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    owner
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: "stub".into(),
                config: String::new(),
            },
        ))
        .await
        .unwrap();
    owner
}

/// Opens a stream under `call_id`.
async fn subscribe(client: &mut Owner, call_id: u32, method: Method) {
    client
        .send_raw(proto::ClientMessage {
            call_id,
            kind: Some(client_message::Kind::Request(proto::Request {
                method: Some(method),
            })),
        })
        .await;
}

/// The next message on the connection, within 5 s.
async fn next(client: &mut Owner) -> proto::ServerMessage {
    tokio::time::timeout(Duration::from_secs(5), client.receive())
        .await
        .expect("no message within 5 s")
        .expect("the connection closed")
}

/// The next item of the stream `call_id`, skipping others.
async fn item(client: &mut Owner, call_id: u32) -> stream_item::Method {
    loop {
        let message = next(client).await;
        if message.call_id != call_id {
            continue;
        }
        match message.kind.unwrap() {
            server_message::Kind::Item(item) => return item.method.unwrap(),
            other => panic!("stream {call_id} ended: {other:?}"),
        }
    }
}

async fn deposit(owner: &mut Owner, amount: u64) {
    owner
        .call(Method::StubDeposit(proto::StubDepositRequest { amount }))
        .await
        .unwrap();
}

#[tokio::test]
async fn streams_start_with_what_is_and_follow_changes() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let mut streams = node.ipc().await;
    subscribe(
        &mut streams,
        1,
        Method::SubscribeLedgers(proto::SubscribeLedgersRequest {}),
    )
    .await;
    let stream_item::Method::SubscribeLedgers(first) = item(&mut streams, 1).await else {
        panic!("not ledgers");
    };
    assert_eq!(first.ledgers[0].owner_balance, 100_000_000_000);
    assert!(first.ledgers[0].removal_blockers.is_empty());
    deposit(&mut owner, 1_000_000_000).await;
    let stream_item::Method::SubscribeLedgers(next) = item(&mut streams, 1).await else {
        panic!("not ledgers");
    };
    assert_eq!(next.ledgers[0].owner_balance, 101_000_000_000);

    subscribe(
        &mut streams,
        2,
        Method::SubscribeNodes(proto::SubscribeNodesRequest {}),
    )
    .await;
    let stream_item::Method::SubscribeNodes(table) = item(&mut streams, 2).await else {
        panic!("not nodes");
    };
    assert!(table.reset);
    let own = &table.upserted[0];
    assert_eq!(
        (own.id.as_str(), own.distance),
        (key().node_id().as_str(), Some(0))
    );

    // The owner's account: its transfers so far, then the next one.
    let Ok(response::Method::ListAccounts(accounts)) = owner
        .call(Method::ListAccounts(proto::ListAccountsRequest {}))
        .await
    else {
        panic!("no accounts");
    };
    let pool = accounts
        .accounts
        .iter()
        .find(|account| account.path == "stub+usd:equity/owner")
        .unwrap()
        .id;
    subscribe(
        &mut streams,
        3,
        Method::SubscribeTransfers(proto::SubscribeTransfersRequest {
            account: Some(pool),
            history: 10,
        }),
    )
    .await;
    let stream_item::Method::SubscribeTransfers(journal) = item(&mut streams, 3).await else {
        panic!("not transfers");
    };
    assert!(journal.reset);
    assert_eq!(journal.transfers.len(), 2, "the activation and the deposit");
    deposit(&mut owner, 7).await;
    loop {
        let stream_item::Method::SubscribeTransfers(event) = item(&mut streams, 3).await else {
            panic!("not transfers");
        };
        if let Some(transfer) = event.transfers.first() {
            assert_eq!(transfer.amount, 7);
            assert_eq!(event.account.unwrap().credits_posted, 101_000_000_007);
            break;
        }
    }
}

#[tokio::test]
async fn the_log_stream_shows_the_nodes_lines() {
    dassie_node::log::init(LogFilter::new(LogLevel::Info, ""));
    let node = TestNode::start().await;
    let mut streams = node.ipc().await;
    subscribe(
        &mut streams,
        1,
        Method::SubscribeLogs(proto::SubscribeLogsRequest {}),
    )
    .await;
    let stream_item::Method::SubscribeLogs(first) = item(&mut streams, 1).await else {
        panic!("not logs");
    };
    assert!(first.reset);
    tracing::info!(target: "das:node:test", token = "hunter2", "a marker line");
    loop {
        let stream_item::Method::SubscribeLogs(event) = item(&mut streams, 1).await else {
            panic!("not logs");
        };
        let Some(line) = event.lines.iter().find(|l| l.message == "a marker line") else {
            continue;
        };
        assert_eq!(line.fields[0].value, "[redacted]");
        break;
    }
}

#[tokio::test]
async fn the_inspectors_show_the_node_without_its_secrets() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let Ok(response::Method::ListDatabaseRows(rows)) = owner
        .call(Method::ListDatabaseRows(proto::ListDatabaseRowsRequest {
            table: "node_key".into(),
            cursor: 0,
            limit: 0,
        }))
        .await
    else {
        panic!("no rows");
    };
    assert_eq!(
        rows.rows[0].values[1].value,
        Some(proto::database_value::Value::Redacted(true))
    );
    let unknown = owner
        .call(Method::ListDatabaseRows(proto::ListDatabaseRowsRequest {
            table: "sqlite_master".into(),
            cursor: 0,
            limit: 0,
        }))
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), proto::ErrorCode::InvalidArgument);
    let Ok(response::Method::ListServices(services)) = owner
        .call(Method::ListServices(proto::ListServicesRequest {}))
        .await
    else {
        panic!("no services");
    };
    let names: Vec<&str> = services.services.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["acme", "engine", "http", "https", "ipc", "sessions"]
    );
    assert!(
        services
            .services
            .iter()
            .all(|s| s.state() == proto::ServiceState::Running)
    );
    assert!(services.cells.iter().any(|cell| cell.name == "ledger"));
    let stranger = NodeKey::from_bytes(&[12; 32]).node_id().to_string();
    let refused = owner
        .call(Method::RegisterWithBootstrap(
            proto::RegisterWithBootstrapRequest { node: stranger },
        ))
        .await
        .unwrap_err();
    assert_eq!(refused.code(), proto::ErrorCode::NotFound);
}

#[tokio::test]
async fn token_caps_are_kept() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let cap = proto::SpendingCap {
        amount: 1_000_000_000,
        window_ms: 86_400_000,
    };
    let Ok(response::Method::CreateBtpToken(created)) = owner
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "capped".into(),
            ledger: "stub+usd".into(),
            cap: Some(cap),
        }))
        .await
    else {
        panic!("no token");
    };
    let token = created.token.unwrap();
    assert_eq!((token.cap, token.cap_available), (Some(cap), cap.amount));
    let invalid = owner
        .call(Method::SetBtpTokenCap(proto::SetBtpTokenCapRequest {
            id: token.id,
            cap: Some(proto::SpendingCap {
                amount: 1,
                window_ms: 10,
            }),
        }))
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), proto::ErrorCode::InvalidArgument);
    let (dir, clock) = node.stop().await;
    let node = TestNode::start_in(dir, clock, 2).await;
    let mut owner = node.ipc().await;
    let mut tokens = None;
    for _ in 0..200 {
        if let Ok(response::Method::ListBtpTokens(list)) = owner
            .call(Method::ListBtpTokens(proto::ListBtpTokensRequest {}))
            .await
        {
            tokens = Some(list);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(tokens.unwrap().tokens[0].cap, Some(cap));
}

#[tokio::test]
async fn a_connection_holds_at_most_twelve_streams() {
    let node = TestNode::start().await;
    let mut streams = node.ipc().await;
    for call_id in 1..=13 {
        subscribe(
            &mut streams,
            call_id,
            Method::SubscribeLogs(proto::SubscribeLogsRequest {}),
        )
        .await;
    }
    loop {
        let message = next(&mut streams).await;
        if let Some(server_message::Kind::Error(error)) = message.kind {
            assert_eq!(message.call_id, 13);
            assert_eq!(error.code(), proto::ErrorCode::ResourceExhausted);
            break;
        }
    }
}

async fn get_stats(node: &TestNode) -> Reply {
    let request = hyper::Request::get("/stats")
        .header("host", node.host())
        .body(http_body_util::Full::new(bytes::Bytes::new()))
        .unwrap();
    send(node.https, true, request).await
}

// Spec 2 §14: who the node is, for anyone, once it is set up.
#[tokio::test]
async fn stats_name_the_node_once_set_up() {
    let node = TestNode::start().await;
    assert_eq!(get_stats(&node).await.status, hyper::StatusCode::NOT_FOUND);
    let _owner = ready(&node).await;
    let reply = get_stats(&node).await;
    assert_eq!(reply.status, hyper::StatusCode::OK);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let stats: serde_json::Value = serde_json::from_str(&reply.text()).unwrap();
    assert_eq!(stats["node"]["id"], key().node_id().as_str());
    assert_eq!(stats["peers"], serde_json::json!([]));
}

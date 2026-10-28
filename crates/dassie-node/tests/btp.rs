//! BTP clients against an in-process node: authentication, ILDCP, routing,
//! and a STREAM payment to the node's receiver that the journal records
//! packet by packet.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::collections::BTreeMap;

use dassie_identity::IdentityKey;
use dassie_ilp::btp::{self, Body, ProtocolData};
use dassie_ilp::{Address, Prepare, Response, Timestamp};
use dassie_node::clock::{Clock, ManualClock};
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_oer::{Decode, Encode};
use futures_util::SinkExt;
use support::btp_client::{BtpClient, pay_stream};
use support::*;
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

const PAYMENT: u64 = 250_000_000;

fn key() -> IdentityKey {
    dassie_identity::TableKeygen::identity_key(&[9; 32])
}

/// Sets the node up and waits for its ledger, then activates the stub.
type Owner = RpcClient<Framed<UnixStream, LengthDelimitedCodec>>;

async fn ready(node: &TestNode) -> Owner {
    set_up(node, &key()).await;
    node.engine_started().await;
    let mut owner = node.ipc().await;
    let reply = owner
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: "stub".into(),
                config: STUB_UNLIMITED.into(),
            },
        ))
        .await;
    let Ok(response::Method::AddSettlementScheme(added)) = reply else {
        panic!("{reply:?}")
    };
    let ledger = added.ledger.unwrap();
    assert_eq!(
        ledger.owner_balance, 100_000_000_000,
        "the stub's first deposit"
    );
    owner
}

async fn token(owner: &mut Owner) -> (u64, String) {
    let reply = owner
        .call(Method::CreateBtpToken(proto::CreateBtpTokenRequest {
            label: "test".into(),
            ledger: "stub+usd".into(),
            cap: None,
        }))
        .await;
    let Ok(response::Method::CreateBtpToken(created)) = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(created.secret.len(), 22);
    assert!(
        created
            .url
            .starts_with(&format!("btp+wss://:{}@localhost:", created.secret))
    );
    (created.token.unwrap().id, created.secret)
}

async fn ledgers(owner: &mut Owner) -> proto::Ledger {
    let Ok(response::Method::ListLedgers(list)) = owner
        .call(Method::ListLedgers(proto::ListLedgersRequest {}))
        .await
    else {
        panic!("no ledgers")
    };
    list.ledgers.into_iter().next().unwrap()
}

async fn journal(owner: &mut Owner) -> Vec<proto::Transfer> {
    let Ok(response::Method::ListTransfers(list)) = owner
        .call(Method::ListTransfers(proto::ListTransfersRequest {
            after_sequence: None,
            limit: 1000,
            account: None,
        }))
        .await
    else {
        panic!("no journal")
    };
    list.transfers
}

async fn packets(owner: &mut Owner) -> Vec<proto::Packet> {
    let Ok(response::Method::ListPackets(list)) = owner
        .call(Method::ListPackets(proto::ListPacketsRequest {
            after: None,
            limit: 1000,
        }))
        .await
    else {
        panic!("no packets")
    };
    list.packets
}

// A BTP client is told the address that routes to it, and its ledger's
// asset.
#[tokio::test]
async fn ildcp_hands_out_the_routed_address() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let (_, secret) = token(&mut owner).await;
    let mut alice = BtpClient::connect(&node, &secret).await.unwrap();
    let mut bob = BtpClient::connect(&node, &secret).await.unwrap();
    let now = node.clock.now_ms();
    let (alice_address, code, scale) = alice.ildcp(now).await;
    assert_eq!((code.as_str(), scale), ("USD", 9));
    assert!(alice_address.starts_with("test.das."), "{alice_address}");
    let (bob_address, ..) = bob.ildcp(now).await;
    assert_ne!(alice_address, bob_address);
    // Bob pays Alice's address: the packet reaches Alice.
    let fulfillment = [5; 32];
    let destination = format!("{alice_address}.x");
    let packet = Prepare {
        amount: 10,
        expires_at: Timestamp::from_unix_ms(now + 20_000),
        execution_condition: dassie_ilp::condition_for(&fulfillment),
        destination: Address::parse(&destination).unwrap(),
        data: b"",
    };
    let sent = bob.prepare(&packet.to_vec()).await;
    let arrived = alice.receive().await.unwrap();
    assert!(!arrived.response);
    assert_eq!(Prepare::decode_exact(&arrived.ilp).unwrap().amount, 10);
    let fulfill = dassie_ilp::Fulfill {
        fulfillment,
        data: b"",
    }
    .to_vec();
    alice.send(arrived.request_id, &fulfill, true).await;
    let answer = bob.receive().await.unwrap();
    assert_eq!((answer.request_id, answer.response), (sent, true));
    assert_eq!(answer.ilp, fulfill);
    node.stop().await;
}

/// Pays `amount` to the receiver from a Rust STREAM client over BTP.
async fn pay(node: &TestNode, client: &mut BtpClient, owner: &mut Owner, amount: u64) {
    let credentials = owner.incoming_payment(None).await;
    pay_stream(
        &node.clock,
        client,
        &credentials.destination_account,
        &credentials.shared_secret,
        amount,
    )
    .await;
}

/// The done criterion: every fulfilled packet with money has exactly its
/// posted transfer, owner's token to owner's pool, and nothing else moved.
fn check_journal_matches_trace(
    transfers: &[proto::Transfer],
    packets: &[proto::Packet],
    token: u64,
) -> u64 {
    let by_id: BTreeMap<u64, &proto::Transfer> = transfers.iter().map(|t| (t.id, t)).collect();
    let mut total = 0;
    let mut seen = 0;
    for packet in packets {
        match packet.state() {
            proto::PacketState::Fulfilled if packet.amount_in > 0 => {
                assert_eq!(packet.transfers.len(), 1, "{packet:?}");
                let transfer = by_id[&packet.transfers[0]];
                assert_eq!(transfer.state(), proto::TransferState::Posted);
                assert_eq!(transfer.amount, packet.amount_in);
                assert_eq!(
                    transfer.debit_path,
                    format!("stub+usd:equity/owner/btp/{token}")
                );
                assert_eq!(transfer.credit_path, "stub+usd:equity/owner");
                let Some(proto::transfer::Reference::Packet(trace)) = &transfer.reference else {
                    panic!("{transfer:?}")
                };
                assert_eq!(Some(trace), packet.trace.as_ref());
                total += transfer.amount;
                seen += 1;
            }
            proto::PacketState::Rejected => {
                for id in &packet.transfers {
                    assert!(!by_id.contains_key(id), "a rejected packet left a row");
                }
            }
            proto::PacketState::Pending => panic!("a packet in flight: {packet:?}"),
            _ => assert!(packet.transfers.is_empty()),
        }
    }
    let packet_rows = transfers
        .iter()
        .filter(|t| matches!(t.reference, Some(proto::transfer::Reference::Packet(_))))
        .count();
    assert_eq!(
        packet_rows, seen,
        "every packet row belongs to a fulfilled packet"
    );
    total
}

#[tokio::test]
async fn a_stream_client_pays_the_receiver_and_the_journal_matches() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let (token_id, secret) = token(&mut owner).await;
    let mut client = BtpClient::connect(&node, &secret).await.unwrap();
    pay(&node, &mut client, &mut owner, PAYMENT).await;
    let packets = packets(&mut owner).await;
    assert!(
        packets.iter().any(|p| p.reject_code == "F08"),
        "probes above the maximum packet amount get F08"
    );
    // Each record joins its hops by its condition and names its ledgers; a
    // BTP client is no peer, so it names no node.
    for packet in packets
        .iter()
        .filter(|p| p.state() == proto::PacketState::Fulfilled)
    {
        assert_eq!(packet.condition.len(), 32, "{packet:?}");
        // ILDCP is answered here, routed nowhere.
        assert!(!packet.ledger_in.is_empty(), "{packet:?}");
        assert_eq!(
            packet.ledger_out.is_empty(),
            packet.destination.is_empty(),
            "{packet:?}"
        );
        assert!(packet.source_node.is_empty() && packet.destination_node.is_empty());
    }
    let transfers = journal(&mut owner).await;
    assert_eq!(
        check_journal_matches_trace(&transfers, &packets, token_id),
        PAYMENT
    );
    // Money moved within the owner's group: the total is unchanged.
    let ledger = ledgers(&mut owner).await;
    assert_eq!(ledger.owner_balance, 100_000_000_000);
    assert_eq!(ledger.owner_pending, 0);
    let Ok(response::Method::ListBtpTokens(tokens)) = owner
        .call(Method::ListBtpTokens(proto::ListBtpTokensRequest {}))
        .await
    else {
        panic!()
    };
    assert_eq!(
        tokens.tokens[0].net_received,
        -i64::try_from(PAYMENT).unwrap()
    );
    // A restart keeps the books, and the stub deposits nothing again.
    drop(client);
    let (dir, clock) = node.stop().await;
    let node = TestNode::start_in(dir, clock, 2).await;
    node.engine_started().await;
    let mut owner = node.ipc().await;
    let restarted = ledgers(&mut owner).await;
    assert_eq!(restarted, ledger);
    assert_eq!(journal(&mut owner).await, transfers);
    // The token still works.
    BtpClient::connect(&node, &secret).await.unwrap();
    node.stop().await;
}

// spec/areas/node.md#the-event-log: a node told to keep an event log writes
// a line for each record the trace streams, as the RPC has it.
#[tokio::test]
async fn the_event_log_has_a_line_for_each_packet_record() {
    let dir = tempfile::tempdir().unwrap();
    let node = Bound::bind_in(dir)
        .start(
            ManualClock::new(START_MS),
            1,
            Policies::DEFAULT,
            &[("DASSIE_EVENT_LOG", "events.jsonl")],
        )
        .await;
    let mut owner = ready(&node).await;
    let (_, secret) = token(&mut owner).await;
    let mut client = BtpClient::connect(&node, &secret).await.unwrap();
    pay(&node, &mut client, &mut owner, PAYMENT).await;
    let fulfilled: Vec<proto::Packet> = packets(&mut owner)
        .await
        .into_iter()
        .filter(|p| p.state() == proto::PacketState::Fulfilled)
        .collect();
    assert!(!fulfilled.is_empty());
    let path = node.dir.path().join("state").join("events.jsonl");
    // The writer flushes once its queue is empty: wait for the last record.
    let lines = tokio::time::timeout(HANG, async {
        loop {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let lines: Vec<serde_json::Value> = text
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let logged = |packet: &proto::Packet| {
                let trace = packet.trace.unwrap();
                let trace = format!("{}:{}", trace.epoch, trace.sequence);
                lines
                    .iter()
                    .any(|line| line["trace"] == trace.as_str() && line["state"] == "fulfilled")
            };
            if fulfilled.iter().all(logged) {
                return lines;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let start = &lines[0];
    assert_eq!(start["type"], "start");
    assert_eq!(start["version"], dassie_node::VERSION);
    let node_id = start["node"].as_str().unwrap().to_owned();
    for packet in &fulfilled {
        let trace = packet.trace.unwrap();
        let trace = format!("{}:{}", trace.epoch, trace.sequence);
        let line = lines
            .iter()
            .find(|line| line["trace"] == trace.as_str() && line["state"] == "fulfilled")
            .unwrap();
        assert_eq!(line["type"], "packet");
        assert_eq!(line["node"], node_id.as_str());
        assert_eq!(line["amountIn"], packet.amount_in.to_string().as_str());
        let condition: String = packet
            .condition
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(line["condition"], condition.as_str());
        assert_eq!(line["ledgerIn"], packet.ledger_in.as_str());
    }
    node.stop().await;
}

#[tokio::test]
async fn bad_tokens_and_silence_are_refused() {
    let node = TestNode::start_with(Policies {
        btp_auth_timeout_ms: 200,
        ..Policies::DEFAULT
    })
    .await;
    let mut owner = ready(&node).await;
    let (_, secret) = token(&mut owner).await;
    assert_eq!(
        BtpClient::connect(&node, "AAAAAAAAAAAAAAAAAAAAAA")
            .await
            .err(),
        Some(Some(1008))
    );
    // Connected, but never authenticated.
    let (stream, _) = connect_tls(node.https).await;
    let request = format!("wss://{}/btp", node.host())
        .into_client_request()
        .unwrap();
    let (socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .unwrap();
    let mut silent = BtpClient {
        socket,
        next_request: 1,
    };
    assert_eq!(silent.closed().await, Some(1008));
    BtpClient::connect(&node, &secret).await.unwrap();
    // Found by the interop test: ilp-plugin-btp pings before it
    // authenticates, which the first version took for a failed login.
    BtpClient::connect_pinging(&node, &secret, true)
        .await
        .unwrap();
    node.stop().await;
}

#[tokio::test]
async fn revoking_a_token_disconnects_its_clients() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let (id, secret) = token(&mut owner).await;
    let mut client = BtpClient::connect(&node, &secret).await.unwrap();
    owner
        .call(Method::RevokeBtpToken(proto::RevokeBtpTokenRequest { id }))
        .await
        .unwrap();
    assert_eq!(client.closed().await, Some(1008));
    assert!(BtpClient::connect(&node, &secret).await.is_err());
    let reply = owner
        .call(Method::RevokeBtpToken(proto::RevokeBtpTokenRequest { id }))
        .await;
    assert_eq!(reply.unwrap_err().code(), proto::ErrorCode::NotFound);
    node.stop().await;
}

#[tokio::test]
async fn btp_transfers_and_duplicates_get_errors() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let (_, secret) = token(&mut owner).await;
    let mut client = BtpClient::connect(&node, &secret).await.unwrap();
    let transfer = btp::Packet {
        request_id: 5,
        body: Body::Transfer {
            amount: 1_000,
            protocol_data: ProtocolData::EMPTY,
        },
    };
    client
        .socket
        .send(Message::Binary(transfer.to_vec().into()))
        .await
        .unwrap();
    let reply = client.receive().await.unwrap();
    assert!(reply.error && reply.request_id == 5);
    // A packet to a client that never answers, then the same ID again.
    let (address, ..) = client.ildcp(node.clock.now_ms()).await;
    let destination = format!("{address}.self");
    let packet = Prepare {
        amount: 0,
        expires_at: Timestamp::from_unix_ms(node.clock.now_ms() + 20_000),
        execution_condition: [1; 32],
        destination: Address::parse(&destination).unwrap(),
        data: b"",
    }
    .to_vec();
    client.send(40, &packet, false).await;
    let looped = client.receive().await.unwrap();
    assert!(!looped.response, "the packet came back to its sender");
    client.send(40, &packet, false).await;
    let duplicate = client.receive().await.unwrap();
    assert!(duplicate.error && duplicate.request_id == 40);
    node.stop().await;
}

// Found in review: every connection used up an attempt, so a client that
// reconnected often, or several behind one address, got locked out.
#[tokio::test]
async fn only_failed_logins_count_against_an_address() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let (_, secret) = token(&mut owner).await;
    let burst = Policies::DEFAULT.btp_auth_attempts.client_burst;
    for _ in 0..burst * 2 {
        BtpClient::connect(&node, &secret).await.unwrap();
    }
    for _ in 0..burst {
        let refused = BtpClient::connect(&node, "AAAAAAAAAAAAAAAAAAAAAA").await;
        assert_eq!(refused.err(), Some(Some(1008)));
    }
    // Now the address is blocked, the right token included: the handshake
    // gets 429.
    assert_eq!(BtpClient::connect(&node, &secret).await.err(), Some(None));
    node.stop().await;
}

// Found in review: a BTP Error answering our Message was only logged, and
// the packet held its place until it expired.
#[tokio::test]
async fn a_btp_error_rejects_the_packet_it_answers() {
    let node = TestNode::start().await;
    let mut owner = ready(&node).await;
    let (_, secret) = token(&mut owner).await;
    let mut alice = BtpClient::connect(&node, &secret).await.unwrap();
    let mut bob = BtpClient::connect(&node, &secret).await.unwrap();
    let now = node.clock.now_ms();
    let (alice_address, ..) = alice.ildcp(now).await;
    let destination = format!("{alice_address}.x");
    let packet = Prepare {
        amount: 10,
        expires_at: Timestamp::from_unix_ms(now + 20_000),
        execution_condition: [1; 32],
        destination: Address::parse(&destination).unwrap(),
        data: b"",
    };
    let sent = bob.prepare(&packet.to_vec()).await;
    let arrived = alice.receive().await.unwrap();
    let error = btp::Packet {
        request_id: arrived.request_id,
        body: Body::Error(btp::Error {
            code: "F99",
            name: "ApplicationError",
            triggered_at: "",
            data: b"",
            protocol_data: ProtocolData::EMPTY,
        }),
    };
    alice
        .socket
        .send(Message::Binary(error.to_vec().into()))
        .await
        .unwrap();
    let answer = bob.receive().await.unwrap();
    assert_eq!((answer.request_id, answer.response), (sent, true));
    let Response::Reject(reject) = Response::decode_exact(&answer.ilp).unwrap() else {
        panic!("expected a reject");
    };
    assert_eq!(reject.code, dassie_ilp::ErrorCode::F99_APPLICATION_ERROR);
    node.stop().await;
}

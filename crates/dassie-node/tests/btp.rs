//! BTP clients against an in-process node: authentication, ILDCP, routing,
//! and a STREAM payment to the node's receiver that the journal records
//! packet by packet (the M3 done criterion, in Rust).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_ilp::btp::{self, Body, ContentType, ProtocolData, ProtocolDataEntry};
use dassie_ilp::{Address, Prepare, Response, Timestamp, ildcp};
use dassie_node::clock::Clock;
use dassie_node::node::Policies;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_stream::{AssetDetails, BUFFER_LENGTH, Config, Connection, ErrorCode, Policy, Role};
use futures_util::{SinkExt, StreamExt};
use support::*;
use tokio::net::UnixStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

const PAYMENT: u64 = 250_000_000;

fn key() -> NodeKey {
    NodeKey::from_bytes(&[9; 32])
}

/// A BTP client.
struct BtpClient {
    socket: Socket,
    next_request: u32,
}

/// What a BTP packet from the node carried.
#[derive(Debug)]
struct Received {
    request_id: u32,
    response: bool,
    ilp: Vec<u8>,
    error: bool,
}

impl BtpClient {
    async fn connect(node: &TestNode, token: &str) -> Result<Self, Option<u16>> {
        let (stream, _) = connect_tls(node.https).await;
        let request = format!("wss://{}/btp", node.host())
            .into_client_request()
            .unwrap();
        let (mut socket, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .map_err(|_| None)?;
        let entries = [
            entry("auth", ContentType::OCTET_STREAM, b""),
            entry("auth_username", ContentType::TEXT_PLAIN_UTF8, b""),
            entry("auth_token", ContentType::TEXT_PLAIN_UTF8, token.as_bytes()),
        ];
        let auth = btp::Packet {
            request_id: 77,
            body: Body::Message(ProtocolData::Entries(&entries)),
        };
        socket
            .send(Message::Binary(auth.to_vec().into()))
            .await
            .unwrap();
        match socket.next().await {
            Some(Ok(Message::Binary(bytes))) => {
                let reply = btp::Packet::decode_exact(&bytes).unwrap();
                assert_eq!(reply.request_id, 77);
                assert!(matches!(reply.body, Body::Response(data) if data.is_empty()));
                Ok(Self {
                    socket,
                    next_request: 1,
                })
            }
            Some(Ok(Message::Close(frame))) => Err(frame.map(|frame| u16::from(frame.code))),
            _ => Err(None),
        }
    }

    async fn send(&mut self, request_id: u32, packet: &[u8], response: bool) {
        let entries = [entry("ilp", ContentType::OCTET_STREAM, packet)];
        let data = ProtocolData::Entries(&entries);
        let body = if response {
            Body::Response(data)
        } else {
            Body::Message(data)
        };
        let packet = btp::Packet { request_id, body };
        self.socket
            .send(Message::Binary(packet.to_vec().into()))
            .await
            .unwrap();
    }

    async fn prepare(&mut self, packet: &[u8]) -> u32 {
        let request_id = self.next_request;
        self.next_request += 1;
        self.send(request_id, packet, false).await;
        request_id
    }

    async fn receive(&mut self) -> Option<Received> {
        let next = tokio::time::timeout(Duration::from_secs(10), self.socket.next())
            .await
            .expect("the node said nothing");
        let Some(Ok(Message::Binary(bytes))) = next else {
            return None;
        };
        let packet = btp::Packet::decode_exact(&bytes).unwrap();
        let ilp = packet
            .body
            .protocol_data()
            .find("ilp")
            .map(|entry| entry.data.to_vec())
            .unwrap_or_default();
        Some(Received {
            request_id: packet.request_id,
            response: matches!(packet.body, Body::Response(_)),
            error: matches!(packet.body, Body::Error(_)),
            ilp,
        })
    }

    /// The close code, once the node closes.
    async fn closed(&mut self) -> Option<u16> {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), self.socket.next())
                .await
                .expect("the socket stayed open")
            {
                Some(Ok(Message::Close(frame))) => return frame.map(|f| u16::from(f.code)),
                Some(Ok(_)) => {}
                _ => return None,
            }
        }
    }

    /// Asks ILDCP for this client's address and asset.
    async fn ildcp(&mut self, now_ms: u64) -> (String, String, u8) {
        let request = ildcp::request(Timestamp::from_unix_ms(now_ms)).to_vec();
        let id = self.prepare(&request).await;
        let reply = self.receive().await.unwrap();
        assert_eq!(reply.request_id, id);
        let Response::Fulfill(fulfill) = Response::decode_exact(&reply.ilp).unwrap() else {
            panic!("ILDCP was rejected");
        };
        let config = ildcp::Response::decode_exact(fulfill.data).unwrap();
        (
            config.address.as_str().to_string(),
            config.asset_code.to_string(),
            config.asset_scale,
        )
    }
}

fn entry<'a>(name: &'a str, content_type: ContentType, data: &'a [u8]) -> ProtocolDataEntry<'a> {
    ProtocolDataEntry {
        protocol_name: name,
        content_type,
        data,
    }
}

/// Sets the node up and waits for its ledger, then activates the stub.
type Owner = RpcClient<Framed<UnixStream, LengthDelimitedCodec>>;

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
    let reply = owner
        .call(Method::AddSettlementScheme(
            proto::AddSettlementSchemeRequest {
                id: "stub".into(),
                config: String::new(),
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
            after_id: None,
            limit: 1000,
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

// ILP-7 and NET-25: a BTP client was told an address nothing routed to,
// and XRP/9 whatever its ledger.
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
    assert!(alice_address.starts_with("test.das.d"), "{alice_address}");
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
    let Ok(response::Method::CreateStreamCredentials(credentials)) = owner
        .call(Method::CreateStreamCredentials(
            proto::CreateStreamCredentialsRequest { tag: "test".into() },
        ))
        .await
    else {
        panic!("no credentials")
    };
    let (own, ..) = client.ildcp(node.clock.now_ms()).await;
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: &credentials.shared_secret,
        local_address: Some(Address::parse(&own).unwrap()),
        remote_address: Some(Address::parse(&credentials.destination_account).unwrap()),
        local_asset: Some(AssetDetails {
            code: "USD",
            scale: 9,
        }),
        receipt: None,
        policy: Policy::DEFAULT,
    });
    connection.measure_exchange_rate();
    let mut rng = SeededRng::new(4);
    let mut buffer = vec![0u8; BUFFER_LENGTH];
    let mut in_flight: BTreeMap<u32, u64> = BTreeMap::new();
    let mut stream = None;
    for _ in 0..10_000 {
        let now = Timestamp::from_unix_ms(node.clock.now_ms());
        while let Some(transmit) = connection.poll_transmit(now, &mut rng, &mut buffer) {
            let id = client.prepare(&buffer[..transmit.length]).await;
            in_flight.insert(id, transmit.request_id);
        }
        if stream.is_none() && connection.exchange_rate().is_some() {
            let id = connection.open_stream().unwrap();
            connection.set_send_max(id, amount).unwrap();
            stream = Some(id);
        }
        if let Some(id) = stream
            && connection
                .stream(id)
                .is_some_and(|s| s.total_sent() == amount)
        {
            connection.close(now, ErrorCode::NO_ERROR);
            return;
        }
        if in_flight.is_empty() {
            node.clock.advance(100);
            continue;
        }
        let reply = client.receive().await.unwrap();
        if reply.response {
            let request = in_flight.remove(&reply.request_id).unwrap();
            connection.handle_response(now, request, &reply.ilp, &mut buffer);
        } else if let Ok(prepare) = Prepare::decode_exact(&reply.ilp) {
            let length = connection.handle_prepare(now, &mut rng, &prepare, &mut buffer);
            client.send(reply.request_id, &buffer[..length], true).await;
        }
    }
    panic!("the payment did not finish");
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
    // A restart keeps the books, and the stub deposits nothing again (ACC-6).
    drop(client);
    let (dir, clock) = node.stop().await;
    let node = TestNode::start_in(dir, clock, 2).await;
    let mut owner = node.ipc().await;
    let mut restarted = None;
    for _ in 0..200 {
        if let Ok(response::Method::ListLedgers(list)) = owner
            .call(Method::ListLedgers(proto::ListLedgersRequest {}))
            .await
        {
            restarted = list.ledgers.into_iter().next();
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(restarted, Some(ledger));
    assert_eq!(journal(&mut owner).await, transfers);
    // The token still works.
    BtpClient::connect(&node, &secret).await.unwrap();
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

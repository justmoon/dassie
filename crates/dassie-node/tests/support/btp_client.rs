//! A BTP client for tests, speaking to a daemon's `/btp`, and a STREAM
//! payment over it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    dead_code,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_ilp::btp::{self, Body, ContentType, ProtocolData, ProtocolDataEntry};
use dassie_ilp::{Address, Prepare, Response, Timestamp, ildcp};
use dassie_kit::sim::SeededRng;
use dassie_node::clock::{Clock, ManualClock};
use dassie_oer::{Decode, Encode};
use dassie_stream::{
    AssetDetails, BUFFER_LENGTH, Config, Connection, ErrorCode, Event, Policy, Role,
};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::{HANG, Socket, TestNode, connect_tls};

/// A BTP client.
pub struct BtpClient {
    pub socket: Socket,
    pub next_request: u32,
}

/// What a BTP packet from the node carried.
#[derive(Debug)]
pub struct Received {
    pub request_id: u32,
    pub response: bool,
    pub ilp: Vec<u8>,
    pub error: bool,
}

impl BtpClient {
    pub async fn connect(node: &TestNode, token: &str) -> Result<Self, Option<u16>> {
        Self::connect_pinging(node, token, false).await
    }

    /// Connects, pinging first if `ping`, as `ilp-plugin-btp` does.
    pub async fn connect_pinging(
        node: &TestNode,
        token: &str,
        ping: bool,
    ) -> Result<Self, Option<u16>> {
        let (stream, _) = connect_tls(node.https).await;
        let request = format!("wss://{}/btp", node.host())
            .into_client_request()
            .unwrap();
        let (mut socket, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .map_err(|_| None)?;
        if ping {
            socket.send(Message::Ping(Vec::new().into())).await.unwrap();
        }
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
        let mut reply = socket.next().await;
        while let Some(Ok(Message::Pong(_))) = reply {
            reply = socket.next().await;
        }
        match reply {
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

    pub async fn send(&mut self, request_id: u32, packet: &[u8], response: bool) {
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

    pub async fn prepare(&mut self, packet: &[u8]) -> u32 {
        let request_id = self.next_request;
        self.next_request += 1;
        self.send(request_id, packet, false).await;
        request_id
    }

    /// The next message from the node. What the tests wait for here the
    /// node sends without waiting on its clock, so only a hang ends the
    /// wait ([`HANG`]).
    pub async fn receive(&mut self) -> Option<Received> {
        let next = tokio::time::timeout(HANG, self.socket.next())
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
    pub async fn closed(&mut self) -> Option<u16> {
        loop {
            match tokio::time::timeout(HANG, self.socket.next())
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
    pub async fn ildcp(&mut self, now_ms: u64) -> (String, String, u8) {
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

pub fn entry<'a>(
    name: &'a str,
    content_type: ContentType,
    data: &'a [u8],
) -> ProtocolDataEntry<'a> {
    ProtocolDataEntry {
        protocol_name: name,
        content_type,
        data,
    }
}

/// Pays `amount` over STREAM, from `client` to `destination`, until it is
/// all sent.
pub async fn pay_stream(
    clock: &ManualClock,
    client: &mut BtpClient,
    destination: &str,
    shared_secret: &[u8],
    amount: u64,
) {
    let sent = send_stream(clock, client, destination, shared_secret, amount, 10_000).await;
    assert_eq!(sent, amount, "the payment did not finish");
}

/// As [`pay_stream`], waiting `idle` of real time whenever nothing is in
/// flight: for settlements that take real time, on a ledger outside.
pub async fn pay_stream_waiting(
    clock: &ManualClock,
    client: &mut BtpClient,
    destination: &str,
    shared_secret: &[u8],
    amount: u64,
    idle: std::time::Duration,
) {
    let sent = send(
        clock,
        client,
        destination,
        shared_secret,
        amount,
        10_000,
        idle,
    )
    .await
    .0;
    assert_eq!(sent, amount, "the payment did not finish");
}

/// Sends up to `amount` over STREAM for at most `rounds` rounds, and
/// returns what the receiver took.
pub async fn send_stream(
    clock: &ManualClock,
    client: &mut BtpClient,
    destination: &str,
    shared_secret: &[u8],
    amount: u64,
    rounds: usize,
) -> u64 {
    let idle = std::time::Duration::ZERO;
    send(
        clock,
        client,
        destination,
        shared_secret,
        amount,
        rounds,
        idle,
    )
    .await
    .0
}

/// As [`send_stream`], returning also what the receiver says its stream
/// received.
pub async fn send_stream_reported(
    clock: &ManualClock,
    client: &mut BtpClient,
    destination: &str,
    shared_secret: &[u8],
    amount: u64,
    rounds: usize,
) -> (u64, u64) {
    let idle = std::time::Duration::ZERO;
    send(
        clock,
        client,
        destination,
        shared_secret,
        amount,
        rounds,
        idle,
    )
    .await
}

/// Sends, and returns what was sent and what the receiver says its stream
/// received.
async fn send(
    clock: &ManualClock,
    client: &mut BtpClient,
    destination: &str,
    shared_secret: &[u8],
    amount: u64,
    rounds: usize,
    idle: std::time::Duration,
) -> (u64, u64) {
    let (own, ..) = client.ildcp(clock.now_ms()).await;
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret,
        local_address: Some(Address::parse(&own).unwrap()),
        remote_address: Some(Address::parse(destination).unwrap()),
        local_asset: Some(AssetDetails {
            code: "USD",
            scale: 9,
        }),
        policy: Policy::DEFAULT,
    });
    connection.measure_exchange_rate();
    let mut rng = SeededRng::new(4);
    let mut buffer = vec![0u8; BUFFER_LENGTH];
    let mut in_flight: BTreeMap<u32, u64> = BTreeMap::new();
    let mut stream = None;
    let sent = |connection: &Connection, stream: Option<u64>| {
        let stream = stream.and_then(|id| connection.stream(id));
        (
            stream.map_or(0, |s| s.total_sent()),
            stream.map_or(0, |s| s.remote_received()),
        )
    };
    for _ in 0..rounds {
        let now = Timestamp::from_unix_ms(clock.now_ms());
        while let Some(transmit) = connection.poll_transmit(now, &mut rng, &mut buffer) {
            let id = client.prepare(&buffer[..transmit.length]).await;
            in_flight.insert(id, transmit.request_id);
        }
        // A rate that could not be measured is not measured again: no
        // money moves on this connection, so it ends now rather than idle
        // through its rounds while the daemons' clock runs on.
        while let Some(event) = connection.poll_event() {
            if matches!(event, Event::ExchangeRateMeasurementFailed) && stream.is_none() {
                eprintln!("the exchange rate to {destination} could not be measured");
                connection.close(now, ErrorCode::NO_ERROR);
                return sent(&connection, stream);
            }
        }
        if stream.is_none() && connection.exchange_rate().is_some() {
            let id = connection.open_stream().unwrap();
            connection.set_send_max(id, amount).unwrap();
            stream = Some(id);
        }
        if sent(&connection, stream).0 == amount || connection.is_closed() {
            connection.close(now, ErrorCode::NO_ERROR);
            return sent(&connection, stream);
        }
        if in_flight.is_empty() {
            clock.advance(100);
            if !idle.is_zero() {
                tokio::time::sleep(idle).await;
            }
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
    sent(&connection, stream)
}

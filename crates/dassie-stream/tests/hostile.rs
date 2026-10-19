//! Properties: whatever a remote sends never panics a connection or a
//! server, and their invariants hold afterwards (D16, spec 7 §7). The
//! remote knows the shared secret, so arbitrary and damaged plaintext gets
//! through decryption to the parsers and the state behind them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode as IlpErrorCode, Fulfill, Prepare, Reject, Response,
    Timestamp,
};
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_stream::base64url;
use dassie_stream::crypto::{Cipher, Keys};
use dassie_stream::packet::PacketType;
use dassie_stream::receipt::ReceiptSecret;
use dassie_stream::token::{MAX_TOKEN_LENGTH, ServerSecret};
use dassie_stream::{
    AssetDetails, BUFFER_LENGTH, Config, Connection, Policy, Rate, Role, Server, ServerConfig,
};
use proptest::prelude::*;
use support::arbitrary::{self, Mutation, Payload};

const NOW: Timestamp = Timestamp::from_unix_ms(1_700_000_000_000);
const SERVER_SECRET: [u8; 32] = [1; 32];

/// Receiving money needs room on streams the remote opens.
const POLICY: Policy = Policy {
    remote_stream_receive_max: u64::MAX,
    ..Policy::DEFAULT
};

fn amount() -> impl Strategy<Value = u64> {
    prop_oneof![Just(0), 0u64..2000, any::<u64>()]
}

/// A Prepare's data: encrypted under the connection's keys, or not.
fn prepare_data(keys: &Keys, payload: &Payload, sealed: bool) -> Vec<u8> {
    let plaintext = payload.plaintext(0, PacketType::Prepare);
    if sealed {
        arbitrary::seal(&keys.cipher, &plaintext)
    } else {
        plaintext
    }
}

/// A Prepare carrying `data`, fulfillable if `fulfillable`.
fn prepare<'a>(
    keys: &Keys,
    destination: Address<'a>,
    amount: u64,
    fulfillable: bool,
    data: &'a [u8],
) -> Prepare<'a> {
    Prepare {
        amount,
        expires_at: NOW.saturating_add_ms(30_000),
        execution_condition: if fulfillable {
            keys.condition(data)
        } else {
            [0; 32]
        },
        destination,
        data,
    }
}

/// An ILP response to a client's packet.
#[derive(Clone, Debug)]
enum Reply {
    /// Arbitrary bytes.
    Bytes(Vec<u8>),
    /// A Fulfill, with the right fulfillment or not.
    Fulfill { correct: bool, payload: Payload },
    /// A Reject.
    Reject {
        code: IlpErrorCode,
        payload: Payload,
    },
    /// An F08 that says how much the path takes.
    TooLarge { received: u64, maximum: u64 },
}

fn reply() -> impl Strategy<Value = (Reply, Vec<Mutation>)> {
    let codes = vec![
        IlpErrorCode::F99_APPLICATION_ERROR,
        IlpErrorCode::F08_AMOUNT_TOO_LARGE,
        IlpErrorCode::F02_UNREACHABLE,
        IlpErrorCode::T04_INSUFFICIENT_LIQUIDITY,
        IlpErrorCode::R00_TRANSFER_TIMED_OUT,
    ];
    let reply = prop_oneof![
        1 => prop::collection::vec(any::<u8>(), 0..100).prop_map(Reply::Bytes),
        3 => (prop::bool::weighted(0.9), arbitrary::payload())
            .prop_map(|(correct, payload)| Reply::Fulfill { correct, payload }),
        3 => (prop::sample::select(codes), arbitrary::payload())
            .prop_map(|(code, payload)| Reply::Reject { code, payload }),
        1 => (amount(), amount())
            .prop_map(|(received, maximum)| Reply::TooLarge { received, maximum }),
    ];
    // Damage to the ILP packet itself, rather than to what it carries.
    let mutations = prop_oneof![2 => Just(Vec::new()), 1 => arbitrary::mutations()];
    (reply, mutations)
}

impl Reply {
    fn encode(&self, keys: &Keys, request_id: u64, data: &[u8], mutations: &[Mutation]) -> Vec<u8> {
        let mut encoded = match self {
            Self::Bytes(bytes) => bytes.clone(),
            Self::Fulfill { correct, payload } => {
                let plaintext = payload.plaintext(request_id, PacketType::Fulfill);
                Fulfill {
                    fulfillment: if *correct {
                        keys.fulfillment(data)
                    } else {
                        [0; 32]
                    },
                    data: &arbitrary::seal(&keys.cipher, &plaintext),
                }
                .to_vec()
            }
            Self::Reject { code, payload } => {
                let plaintext = payload.plaintext(request_id, PacketType::Reject);
                Reject {
                    code: *code,
                    triggered_by: None,
                    message: "",
                    data: &arbitrary::seal(&keys.cipher, &plaintext),
                }
                .to_vec()
            }
            Self::TooLarge { received, maximum } => Reject {
                code: IlpErrorCode::F08_AMOUNT_TOO_LARGE,
                triggered_by: None,
                message: "",
                data: &AmountTooLarge {
                    received_amount: *received,
                    maximum_amount: *maximum,
                }
                .to_bytes(),
            }
            .to_vec(),
        };
        arbitrary::damage(&mut encoded, mutations);
        encoded
    }
}

/// Where a Prepare to the server goes.
#[derive(Clone, Debug)]
enum Target {
    /// Any segment after the server's address.
    Segment(String),
    /// A token the server did not issue.
    Token(Vec<u8>),
    /// A token plaintext encrypted under the server's key, so that it
    /// reaches the token parser.
    Sealed(Vec<u8>),
}

fn target() -> impl Strategy<Value = Target> {
    prop_oneof![
        1 => "[a-zA-Z0-9_~-]{1,300}".prop_map(Target::Segment),
        1 => prop::collection::vec(any::<u8>(), 0..MAX_TOKEN_LENGTH + 8).prop_map(Target::Token),
        4 => arbitrary::token_plaintext().prop_map(Target::Sealed),
    ]
}

impl Target {
    /// The address segment and the token bytes it stands for.
    fn segment_and_token(&self) -> (String, Vec<u8>) {
        let token = match self {
            Self::Segment(segment) => {
                let mut token = vec![0; segment.len()];
                let length = base64url::decode(segment.as_bytes(), &mut token).unwrap_or(0);
                token.truncate(length);
                return (segment.clone(), token);
            }
            Self::Token(token) => token.clone(),
            Self::Sealed(plaintext) => arbitrary::seal(&Cipher::new(&SERVER_SECRET), plaintext),
        };
        let mut segment = vec![0; base64url::encoded_len(token.len())];
        let length = base64url::encode(&token, &mut segment);
        segment.truncate(length);
        (String::from_utf8(segment).unwrap(), token)
    }
}

fn server_connection() -> Connection {
    Connection::new(&Config {
        role: Role::Server,
        shared_secret: &[3; 32],
        local_address: Address::parse("test.server.abc").ok(),
        remote_address: None,
        local_asset: Some(AssetDetails {
            code: "USD",
            scale: 2,
        }),
        receipt: Some(ReceiptSecret {
            nonce: [4; 16],
            secret: [5; 32],
        }),
        policy: POLICY,
    })
}

fn client_connection() -> Connection {
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: &[3; 32],
        local_address: Address::parse("test.client").ok(),
        remote_address: Address::parse("test.server.abc").ok(),
        local_asset: None,
        receipt: None,
        policy: Policy {
            max_packet_amount: 400,
            ..POLICY
        },
    });
    connection.set_exchange_rate(Rate::ONE);
    // Streams 1 to 7, which replies mostly refer to, each with more to
    // send than the replies take, so that every reply has a packet.
    for _ in 0..4 {
        let stream = connection.open_stream().unwrap();
        connection.set_send_max(stream, 1_000_000).unwrap();
        connection.write(stream, b"hello").unwrap();
    }
    connection
}

proptest! {
    #[test]
    fn hostile_prepares_never_panic_a_connection(
        packets in prop::collection::vec(
            (arbitrary::payload(), prop::bool::weighted(0.9), amount(), prop::bool::weighted(0.8)),
            1..6,
        ),
    ) {
        let keys = Keys::derive(&[3; 32]);
        let destination = Address::parse("test.server.abc").unwrap();
        let mut connection = server_connection();
        let mut rng = SeededRng::new(1);
        let mut buffer = vec![0; BUFFER_LENGTH];
        for (payload, sealed, amount, fulfillable) in packets {
            let data = prepare_data(&keys, &payload, sealed);
            let prepare = prepare(&keys, destination, amount, fulfillable, &data);
            let length = connection.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
            prop_assert!(Response::decode_exact(&buffer[..length]).is_ok());
            connection.check_invariants();
            while connection.poll_event().is_some() {}
        }
    }

    #[test]
    fn hostile_responses_never_panic_a_connection(
        replies in prop::collection::vec(reply(), 1..8),
    ) {
        let keys = Keys::derive(&[3; 32]);
        let mut connection = client_connection();
        let mut rng = SeededRng::new(2);
        let mut buffer = vec![0; BUFFER_LENGTH];
        let mut now = NOW;
        for (reply, mutations) in replies {
            connection.handle_timeout(now);
            connection.check_invariants();
            if let Some(transmit) = connection.poll_transmit(now, &mut rng, &mut buffer) {
                let sent = Prepare::decode_exact(&buffer[..transmit.length]).unwrap();
                let response = reply.encode(&keys, transmit.request_id, sent.data, &mutations);
                connection.handle_response(now, transmit.request_id, &response, &mut buffer);
                connection.check_invariants();
            }
            while connection.poll_event().is_some() {}
            now = now.saturating_add_ms(1000);
        }
    }

    #[test]
    fn hostile_destinations_never_panic_a_server(
        packets in prop::collection::vec(
            (
                target(),
                prop::option::of("\\.[a-z]{1,4}"),
                arbitrary::payload(),
                prop::bool::weighted(0.9),
                amount(),
            ),
            1..5,
        ),
    ) {
        let mut server = Server::new(&ServerConfig {
            secret: SERVER_SECRET,
            address: Address::parse("test.server").unwrap(),
            asset: None,
            policy: POLICY,
            max_connections: 2,
        });
        let secret = ServerSecret::new(SERVER_SECRET);
        let mut rng = SeededRng::new(3);
        let mut buffer = vec![0; BUFFER_LENGTH];
        for (target, rest, payload, sealed, amount) in packets {
            let (segment, token) = target.segment_and_token();
            let text = format!("test.server.{segment}{}", rest.unwrap_or_default());
            let Ok(destination) = Address::parse(&text) else {
                continue;
            };
            let keys = Keys::derive(&secret.shared_secret(&token));
            let data = prepare_data(&keys, &payload, sealed);
            let prepare = prepare(&keys, destination, amount, true, &data);
            let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
            prop_assert!(Response::decode_exact(&buffer[..length]).is_ok());
            server.check_invariants();
            while server.poll_opened().is_some() {}
        }
    }
}

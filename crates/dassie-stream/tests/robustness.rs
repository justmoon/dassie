//! Regression tests for defects a review found: hostile remotes must not
//! crash or wedge a connection, money in flight must be counted, and
//! limits must hold (D16). A hostile peer here knows the shared secret and
//! crafts arbitrary encrypted STREAM packets.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::collections::BTreeMap;

use dassie_ilp::{
    Address, ErrorCode as IlpErrorCode, Fulfill, Prepare, Reject, Response, Timestamp,
};
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode, Writer};
use dassie_stream::crypto::{Keys, OVERHEAD};
use dassie_stream::packet::{Frames, Packet, PacketType};
use dassie_stream::sim::{Path, Simulation};
use dassie_stream::{
    Admission, AssetDetails, BUFFER_LENGTH, Config, Connection, ErrorCode, Event, Frame, Policy,
    Rate, Role, Server, ServerConfig,
};

const NOW: Timestamp = Timestamp::from_unix_ms(1_700_000_000_000);

/// Encrypts a STREAM packet under `keys`.
fn seal(
    keys: &Keys,
    sequence: u64,
    packet_type: PacketType,
    prepare_amount: u64,
    frames: &[Frame<'_>],
) -> Vec<u8> {
    let packet = Packet {
        sequence,
        packet_type,
        prepare_amount,
        frames: Frames::List(frames),
    };
    let length = packet.encoded_len();
    let mut buffer = vec![0; OVERHEAD + length];
    packet.encode_to(&mut Writer::new(&mut buffer[OVERHEAD..]));
    let total = keys.cipher.encrypt_with_iv([7; 12], length, &mut buffer);
    buffer.truncate(total);
    buffer
}

/// A fulfillable Prepare to `destination` carrying `frames`.
fn hostile_prepare(
    keys: &Keys,
    destination: &str,
    sequence: u64,
    amount: u64,
    frames: &[Frame<'_>],
) -> Vec<u8> {
    let data = seal(keys, sequence, PacketType::Prepare, 0, frames);
    Prepare {
        amount,
        expires_at: NOW.saturating_add_ms(30_000),
        execution_condition: keys.condition(&data),
        destination: Address::parse(destination).unwrap(),
        data: &data,
    }
    .to_vec()
}

fn client(secret: &[u8], policy: Policy) -> Connection {
    Connection::new(&Config {
        role: Role::Client,
        shared_secret: secret,
        local_address: Address::parse("test.client").ok(),
        remote_address: Address::parse("test.server.abc").ok(),
        local_asset: None,
        receipt: None,
        policy,
    })
}

fn server(policy: Policy) -> Server {
    Server::new(&ServerConfig {
        secret: [1; 32],
        address: Address::parse("test.server").unwrap(),
        asset: Some(AssetDetails {
            code: "USD",
            scale: 2,
        }),
        policy,
        max_connections: 2,
    })
}

#[test]
fn a_full_server_forgets_its_least_active_connection() {
    // Two connections fill the table. A third still gets in: the one heard
    // from least recently goes, and comes back when its sender returns.
    let mut server = server(Policy::DEFAULT);
    let mut rng = SeededRng::new(9);
    let mut buffer = vec![0; BUFFER_LENGTH];
    let mut send = |server: &mut Server, rng: &mut SeededRng, tag: u8, at_ms: u64| {
        let credentials = server.generate_credentials(rng, &[tag], None);
        let keys = Keys::derive(&credentials.shared_secret);
        let bytes = hostile_prepare(&keys, &credentials.destination, 0, 0, &[]);
        let prepare = Prepare::decode_exact(&bytes).unwrap();
        server.handle_prepare(NOW.saturating_add_ms(at_ms), rng, &prepare, &mut buffer);
    };
    send(&mut server, &mut rng, 1, 0);
    send(&mut server, &mut rng, 2, 10);
    send(&mut server, &mut rng, 3, 20);
    let tags: Vec<Vec<u8>> = server
        .connections()
        .map(|(id, _)| server.connection_tag(id).unwrap().to_vec())
        .collect();
    assert_eq!(tags, vec![vec![2], vec![3]]);
}

#[test]
fn hostile_reply_amounts_do_not_overflow_counters() {
    // Finding 1: replies claiming u64::MAX delivered overflowed the stats.
    let secret = [3u8; 32];
    let keys = Keys::derive(&secret);
    // One unit per packet, so that several replies arrive.
    let policy = Policy {
        max_packet_amount: 1,
        ..Policy::DEFAULT
    };
    let mut connection = client(&secret, policy);
    connection.set_exchange_rate(Rate::ONE);
    let stream = connection.open_stream().unwrap();
    connection.set_send_max(stream, 3).unwrap();
    let mut rng = SeededRng::new(1);
    let mut buffer = vec![0; BUFFER_LENGTH];
    for _ in 0..3 {
        let Some(transmit) = connection.poll_transmit(NOW, &mut rng, &mut buffer) else {
            break;
        };
        let Ok(Prepare { data, .. }) = Prepare::decode_exact(&buffer[..transmit.length]) else {
            panic!("valid prepare");
        };
        let fulfillment = keys.fulfillment(data);
        let reply = seal(
            &keys,
            transmit.request_id,
            PacketType::Fulfill,
            u64::MAX,
            &[],
        );
        let response = Fulfill {
            fulfillment,
            data: &reply,
        }
        .to_vec();
        connection.handle_response(NOW, transmit.request_id, &response, &mut buffer);
        connection.check_invariants();
    }
    assert_eq!(connection.stats().amount_delivered, u64::MAX);
    assert_eq!(connection.stats().packets_fulfilled, 3);
}

#[test]
fn slippage_into_a_fixed_receive_maximum_completes() {
    // Finding 2: the remote's room was converted at the minimum rate, so a
    // packet that should fit arrived too large, forever.
    let mut sim = Simulation::new(41, Path::default(), Policy::DEFAULT);
    sim.server_receive_max = 1000;
    sim.client.set_exchange_rate(Rate::new(99, 100).unwrap());
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 5000).unwrap();
    sim.run_until(10_000, |sim| {
        sim.server_connection()
            .is_some_and(|connection| connection.stats().amount_received == 1000)
    });
    let rejected = sim.client.stats().packets_rejected;
    for _ in 0..200 {
        sim.step();
    }
    assert!(
        sim.client.stats().packets_rejected <= rejected + 1,
        "the sender keeps sending into a full receiver"
    );
}

#[test]
fn money_in_flight_when_the_remote_closes_is_counted() {
    // Finding 3: closing dropped packets in flight, so money the receiver
    // got was never recorded as sent.
    let policy = Policy {
        remote_stream_receive_max: u64::MAX,
        ..Policy::DEFAULT
    };
    let mut server = server(policy);
    let mut rng = SeededRng::new(2);
    let credentials = server.generate_credentials(&mut rng, b"", None);
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: &credentials.shared_secret,
        local_address: Address::parse("test.client").ok(),
        remote_address: Address::parse(&credentials.destination).ok(),
        local_asset: None,
        receipt: None,
        policy,
    });
    connection.set_exchange_rate(Rate::ONE);
    let stream = connection.open_stream().unwrap();
    connection.set_send_max(stream, 500).unwrap();

    let mut buffer = vec![0; BUFFER_LENGTH];
    let transmit = connection
        .poll_transmit(NOW, &mut rng, &mut buffer)
        .unwrap();
    let money = buffer[..transmit.length].to_vec();
    let prepare = Prepare::decode_exact(&money).unwrap();
    let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
    let fulfill = buffer[..length].to_vec();
    assert!(matches!(
        Response::decode_exact(&fulfill),
        Ok(Response::Fulfill(_))
    ));

    // The server closes, and its close overtakes the Fulfill.
    let id = server.connections().next().unwrap().0;
    let remote = server.connection_mut(id).unwrap();
    remote.close(NOW, ErrorCode::NO_ERROR);
    let close = remote.poll_transmit(NOW, &mut rng, &mut buffer).unwrap();
    let close = buffer[..close.length].to_vec();
    let close = Prepare::decode_exact(&close).unwrap();
    connection.handle_prepare(NOW, &mut rng, &close, &mut buffer);
    assert!(connection.is_closed());

    connection.handle_response(NOW, transmit.request_id, &fulfill, &mut buffer);
    connection.check_invariants();
    assert_eq!(connection.stats().amount_sent, 500);
    assert!(connection.is_settled());
}

#[test]
fn a_remote_cannot_overflow_the_event_queue() {
    // Finding 4: a remote opening and closing streams in every packet
    // filled the event queue until an assertion fired.
    let mut server = server(Policy::DEFAULT);
    let mut rng = SeededRng::new(3);
    let credentials = server.generate_credentials(&mut rng, b"", None);
    let keys = Keys::derive(&credentials.shared_secret);
    let mut buffer = vec![0; BUFFER_LENGTH];
    let mut refused = 0;
    let mut next_id = 1;
    for sequence in 0..40 {
        let ids: Vec<u64> = (0..10).map(|i| next_id + 2 * i).collect();
        let mut frames = Vec::new();
        for &stream_id in &ids {
            frames.push(Frame::StreamMaxMoney {
                stream_id,
                receive_max: 0,
                total_received: 0,
            });
            frames.push(Frame::StreamClose {
                stream_id,
                code: ErrorCode::NO_ERROR,
                message: "",
            });
        }
        let bytes = hostile_prepare(&keys, &credentials.destination, sequence, 0, &frames);
        let prepare = Prepare::decode_exact(&bytes).unwrap();
        let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
        match Response::decode_exact(&buffer[..length]).unwrap() {
            Response::Reject(reject) if reject.code == IlpErrorCode::T05_RATE_LIMITED => {
                refused += 1;
            }
            _ => next_id += 20,
        }
        for (_, connection) in server.connections() {
            connection.check_invariants();
        }
    }
    assert!(refused > 0, "the queue never filled");
    // Once the application polls, packets are accepted again.
    let id = server.connections().next().unwrap().0;
    let connection = server.connection_mut(id).unwrap();
    while connection.poll_event().is_some() {}
    let bytes = hostile_prepare(&keys, &credentials.destination, 99, 0, &[]);
    let prepare = Prepare::decode_exact(&bytes).unwrap();
    let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
    assert!(matches!(
        Response::decode_exact(&buffer[..length]),
        Ok(Response::Fulfill(_))
    ));
}

#[test]
#[should_panic(expected = "more streams than one packet can describe")]
fn stream_limits_must_fit_a_packet() {
    // Finding 5: limits the policy accepted produced packets too large to
    // send or parse.
    Policy {
        max_remote_streams: 1000,
        ..Policy::DEFAULT
    }
    .validate();
}

#[test]
fn measuring_twice_does_not_panic() {
    // Finding 6: a second measurement reset the probe count while probes
    // of the first were in flight, and their replies then underflowed it.
    let mut server = server(Policy::DEFAULT);
    let mut rng = SeededRng::new(9);
    let credentials = server.generate_credentials(&mut rng, b"", None);
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: &credentials.shared_secret,
        local_address: Address::parse("test.client").ok(),
        remote_address: Address::parse(&credentials.destination).ok(),
        local_asset: None,
        receipt: None,
        policy: Policy::DEFAULT,
    });
    let mut buffer = vec![0; BUFFER_LENGTH];
    connection.measure_exchange_rate();
    let mut probes = Vec::new();
    while let Some(transmit) = connection.poll_transmit(NOW, &mut rng, &mut buffer) {
        probes.push((transmit.request_id, buffer[..transmit.length].to_vec()));
    }
    // Five probes and the handshake.
    assert_eq!(probes.len(), 6);
    connection.measure_exchange_rate();
    for (request_id, bytes) in probes {
        let prepare = Prepare::decode_exact(&bytes).unwrap();
        let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
        let response = buffer[..length].to_vec();
        connection.handle_response(NOW, request_id, &response, &mut buffer);
        connection.check_invariants();
    }
    let mut measured = 0;
    while let Some(event) = connection.poll_event() {
        if matches!(event, Event::ExchangeRateMeasured { .. }) {
            measured += 1;
        }
    }
    assert_eq!(measured, 1);
}

#[test]
fn f08_for_a_packet_without_money_is_ignored() {
    // Finding 7: an F08 for a data-only packet set the packet limit to zero
    // and silently stopped all payments.
    let secret = [5u8; 32];
    let mut connection = client(&secret, Policy::DEFAULT);
    let stream = connection.open_stream().unwrap();
    connection.write(stream, b"hello").unwrap();
    let mut rng = SeededRng::new(4);
    let mut buffer = vec![0; BUFFER_LENGTH];
    let transmit = connection
        .poll_transmit(NOW, &mut rng, &mut buffer)
        .unwrap();
    let reject = Reject {
        code: IlpErrorCode::F08_AMOUNT_TOO_LARGE,
        triggered_by: None,
        message: "",
        data: &[],
    }
    .to_vec();
    connection.handle_response(NOW, transmit.request_id, &reject, &mut buffer);
    assert_eq!(connection.max_packet_amount(), u64::MAX);
    assert!(!connection.is_closed());
}

#[test]
fn data_keeps_flowing_after_streams_close() {
    // Finding 8: connection-level flow control counted closed streams for
    // ever. Between two Dassie peers that was consistent; against
    // ilp-protocol-stream, which counts open streams only, it stalled (see
    // tests/interop). Here: data keeps flowing across closed streams.
    let mut sim = Simulation::new(47, Path::default(), Policy::DEFAULT);
    let message = vec![9u8; 70_000];
    for round in 0..2 {
        let stream = sim.client.open_stream().unwrap();
        let mut written = 0;
        for _ in 0..100_000 {
            if written < message.len() {
                written += sim.client.write(stream, &message[written..]).unwrap();
            }
            let arrived = sim
                .received_data
                .iter()
                .filter(|((_, id), _)| *id == stream)
                .map(|(_, data)| data.len())
                .sum::<usize>();
            if arrived == message.len() {
                break;
            }
            assert!(sim.step(), "round {round}: data stalled");
        }
        sim.client
            .close_stream(stream, ErrorCode::NO_ERROR)
            .unwrap();
        sim.run_until(100_000, |sim| sim.client.stream(stream).is_none());
    }
}

#[test]
fn idle_connections_close_and_free_their_slot() {
    // Finding 9: connections never timed out, so anyone with credentials
    // could fill a server's table for good.
    let mut server = server(Policy::DEFAULT);
    let mut rng = SeededRng::new(6);
    let credentials = server.generate_credentials(&mut rng, b"", None);
    let keys = Keys::derive(&credentials.shared_secret);
    let mut buffer = vec![0; BUFFER_LENGTH];
    let bytes = hostile_prepare(&keys, &credentials.destination, 0, 0, &[]);
    let prepare = Prepare::decode_exact(&bytes).unwrap();
    server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
    let id = server.connections().next().unwrap().0;
    let connection = server.connection_mut(id).unwrap();
    let deadline = connection.poll_timeout().unwrap();
    assert!(deadline <= NOW.saturating_add_ms(60_000));
    connection.handle_timeout(deadline);
    assert!(connection.is_closed());
    while connection.poll_event().is_some() {}
    server.remove_closed();
    assert_eq!(server.connections().count(), 0);
}

#[test]
fn many_closed_streams_use_fixed_memory() {
    // Finding 10: one bit per stream ever opened, without limit. The
    // window is now a fixed array; opening and closing hundreds of streams
    // must still work, and old streams must stay closed.
    let mut server = server(Policy::DEFAULT);
    let mut rng = SeededRng::new(7);
    let credentials = server.generate_credentials(&mut rng, b"", None);
    let keys = Keys::derive(&credentials.shared_secret);
    let mut buffer = vec![0; BUFFER_LENGTH];
    for round in 0..400u64 {
        let stream_id = 2 * round + 1;
        let frames = [
            Frame::StreamMaxMoney {
                stream_id,
                receive_max: 0,
                total_received: 0,
            },
            Frame::StreamClose {
                stream_id,
                code: ErrorCode::NO_ERROR,
                message: "",
            },
        ];
        let bytes = hostile_prepare(&keys, &credentials.destination, round, 0, &frames);
        let prepare = Prepare::decode_exact(&bytes).unwrap();
        let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
        assert!(
            matches!(
                Response::decode_exact(&buffer[..length]),
                Ok(Response::Fulfill(_))
            ),
            "round {round}"
        );
        let id = server.connections().next().unwrap().0;
        let connection = server.connection_mut(id).unwrap();
        while connection.poll_event().is_some() {}
        connection.check_invariants();
    }
    // A long-closed stream cannot be reopened.
    let frames = [Frame::StreamMoney {
        stream_id: 1,
        shares: 1,
    }];
    let bytes = hostile_prepare(&keys, &credentials.destination, 500, 1, &frames);
    let prepare = Prepare::decode_exact(&bytes).unwrap();
    let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
    assert!(matches!(
        Response::decode_exact(&buffer[..length]),
        Ok(Response::Reject(_))
    ));
}

#[test]
fn empty_and_repeated_data_frames_are_handled() {
    // Low-severity findings: empty data created an empty fragment, and two
    // frames for one stream in one packet were counted twice against the
    // connection window.
    let policy = Policy {
        connection_receive_window_bytes: 20_000,
        ..Policy::DEFAULT
    };
    let mut server = server(policy);
    let mut rng = SeededRng::new(8);
    let credentials = server.generate_credentials(&mut rng, b"", None);
    let keys = Keys::derive(&credentials.shared_secret);
    let mut buffer = vec![0; BUFFER_LENGTH];
    let first = vec![1u8; 8000];
    let second = vec![2u8; 8000];
    let frames = [
        Frame::StreamData {
            stream_id: 1,
            offset: 100,
            data: &[],
        },
        Frame::StreamData {
            stream_id: 1,
            offset: 0,
            data: &first,
        },
        Frame::StreamData {
            stream_id: 1,
            offset: 8000,
            data: &second,
        },
    ];
    let bytes = hostile_prepare(&keys, &credentials.destination, 0, 0, &frames);
    let prepare = Prepare::decode_exact(&bytes).unwrap();
    let length = server.handle_prepare(NOW, &mut rng, &prepare, &mut buffer);
    assert!(matches!(
        Response::decode_exact(&buffer[..length]),
        Ok(Response::Fulfill(_))
    ));
    let id = server.connections().next().unwrap().0;
    let connection = server.connection_mut(id).unwrap();
    connection.check_invariants();
    assert_eq!(connection.stream(1).unwrap().readable(), 16_000);
    assert!(!connection.is_closed());
}

#[test]
fn a_burst_of_rejects_backs_off_once() {
    // Packets in flight together meet the same full credit limit together.
    // Backing off once per rejected packet made eight T04s a 12.8 s wait,
    // long after the peer had settled and there was room again.
    let policy = Policy {
        min_concurrency: 8,
        max_packet_amount: 1000,
        ..Policy::DEFAULT
    };
    let mut connection = client(&[5; 32], policy);
    connection.set_exchange_rate(Rate::ONE);
    let stream = connection.open_stream().unwrap();
    connection.set_send_max(stream, 100_000).unwrap();
    let mut rng = SeededRng::new(5);
    let mut buffer = vec![0; BUFFER_LENGTH];
    let reject = Reject {
        code: IlpErrorCode::T04_INSUFFICIENT_LIQUIDITY,
        triggered_by: None,
        message: "",
        data: &[],
    }
    .to_vec();
    let mut burst = |connection: &mut Connection, now: Timestamp| {
        let mut sent = Vec::new();
        while let Some(transmit) = connection.poll_transmit(now, &mut rng, &mut buffer) {
            sent.push(transmit.request_id);
        }
        for &request_id in &sent {
            connection.handle_response(now, request_id, &reject, &mut buffer);
            connection.check_invariants();
        }
        sent.len()
    };

    assert_eq!(burst(&mut connection, NOW), 8);
    let retry = NOW.saturating_add_ms(100);
    assert_eq!(connection.poll_timeout(), Some(retry));
    // The next burst is refused too: the wait doubles, once.
    assert_eq!(burst(&mut connection, retry), 8);
    assert_eq!(
        connection.poll_timeout(),
        Some(retry.saturating_add_ms(200))
    );
}

/// An incoming payment several connections pay: what it still takes, and
/// what each connection's tag paid.
struct Payment {
    remaining: u64,
    paid: BTreeMap<Vec<u8>, u64>,
}

/// Exchanges what `client` has to send now with `server`, admitting each
/// packet against `payment` as the node's receivers do. Returns how many
/// packets were fulfilled and how many refused.
fn pay(
    now: Timestamp,
    rng: &mut SeededRng,
    server: &mut Server,
    payment: &mut Payment,
    client: &mut Connection,
) -> (u32, u32) {
    let (mut fulfilled, mut refused) = (0, 0);
    let mut out = vec![0; BUFFER_LENGTH];
    let mut answer = vec![0; BUFFER_LENGTH];
    let mut work = vec![0; BUFFER_LENGTH];
    while let Some(transmit) = client.poll_transmit(now, rng, &mut out) {
        let prepare = Prepare::decode_exact(&out[..transmit.length]).unwrap();
        let admit = |tag: &[u8]| Admission::Accept {
            money_limit: payment.remaining,
            received: payment.paid.get(tag).copied().unwrap_or(0),
        };
        let handled = server.handle_prepare_admitted(now, rng, &prepare, admit, &mut answer);
        if let Some(id) = handled.connection {
            let tag = server.connection_tag(id).unwrap().to_vec();
            payment.remaining -= handled.received;
            *payment.paid.entry(tag).or_default() += handled.received;
        }
        match Response::decode_exact(&answer[..handled.length]).unwrap() {
            Response::Fulfill(_) => fulfilled += 1,
            Response::Reject(_) => refused += 1,
        }
        client.handle_response(
            now,
            transmit.request_id,
            &answer[..handled.length],
            &mut work,
        );
        client.check_invariants();
        server.check_invariants();
    }
    (fulfilled, refused)
}

#[test]
fn a_sender_whose_room_another_connection_took_stops_at_once() {
    // Two connections pay one incoming payment of 1 000. Each stream's
    // maximum is what its connection received plus what the payment still
    // takes, so it falls when the other connection pays. Senders ignore a
    // falling maximum, so the loser offered its money again and again,
    // refused until its deadline. Now the receiver closes a connection
    // whose payment takes nothing more, and the sender stops.
    let policy = Policy {
        remote_stream_receive_max: u64::MAX,
        ..Policy::DEFAULT
    };
    let mut server = server(policy);
    let mut rng = SeededRng::new(5);
    let mut payment = Payment {
        remaining: 1_000,
        paid: BTreeMap::new(),
    };
    let connect = |server: &Server, rng: &mut SeededRng, tag: &[u8]| {
        let credentials = server.generate_credentials(rng, tag, None);
        let mut connection = Connection::new(&Config {
            role: Role::Client,
            shared_secret: &credentials.shared_secret,
            local_address: Address::parse("test.client").ok(),
            remote_address: Address::parse(&credentials.destination).ok(),
            local_asset: None,
            receipt: None,
            policy: Policy::DEFAULT,
        });
        connection.set_exchange_rate(Rate::ONE);
        let stream = connection.open_stream().unwrap();
        (connection, stream)
    };
    let (mut a, a_stream) = connect(&server, &mut rng, b"payment/a");
    let (mut b, b_stream) = connect(&server, &mut rng, b"payment/b");

    // A pays 100 and learns that it may deliver 1 000 in all.
    a.set_send_max(a_stream, 100).unwrap();
    assert_eq!(
        pay(NOW, &mut rng, &mut server, &mut payment, &mut a),
        (1, 0)
    );
    assert_eq!(a.stream(a_stream).unwrap().remote_room(), 900);

    // B pays the other 900: the payment is full.
    b.set_send_max(b_stream, 900).unwrap();
    assert_eq!(
        pay(NOW, &mut rng, &mut server, &mut payment, &mut b),
        (1, 0)
    );
    assert_eq!(payment.remaining, 0);

    // A wants to send what it was told it may. Its first packet is refused
    // and closes its connection, and tells it what it delivered.
    a.set_send_max(a_stream, 1_000).unwrap();
    assert_eq!(
        pay(NOW, &mut rng, &mut server, &mut payment, &mut a),
        (0, 1)
    );
    assert!(a.is_settled());
    let mut closed = None;
    while let Some(event) = a.poll_event() {
        if let Event::ConnectionClosed { code, by_remote } = event {
            closed = Some((code, by_remote));
        }
    }
    assert_eq!(closed, Some((ErrorCode::NO_ERROR, true)));
    assert_eq!(a.stream(a_stream).unwrap().remote_received(), 100);
    assert_eq!(a.poll_timeout(), None, "a closed sender waits for nothing");
    let later = NOW.saturating_add_ms(31_000);
    assert_eq!(
        pay(later, &mut rng, &mut server, &mut payment, &mut a),
        (0, 0)
    );
    assert_eq!(a.stats().amount_sent, 100);
    assert_eq!(payment.paid.values().sum::<u64>(), 1_000);
}

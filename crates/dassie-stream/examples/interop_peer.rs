//! Drives the native STREAM implementation over stdio, so that the Deno
//! interop tests (tests/interop/native_interop_test.ts) can connect it to
//! `ilp-protocol-stream`. This is an edge: it reads the system clock and the
//! OS RNG, which the crate itself never does.
//!
//! Frames in both directions are `kind (u8) || id (u64) || length (u32) ||
//! bytes`, big-endian:
//!
//! - 1 `PREPARE` (in): a Prepare for this peer; answered with 2.
//! - 2 `RESPONSE` (out): the answer to a `PREPARE` with the same id.
//! - 3 `REPLY` (in): the response to a `SEND` with the same id.
//! - 4 `SEND` (out): a Prepare this peer sends.
//! - 5 `LOG` (out): a line of text, such as `received 1 3 2500`.
//!
//! Usage:
//!
//! ```text
//! interop_peer client <destination> <shared-secret-hex> <amount> <own-address>
//! interop_peer server <own-address> <receipt-nonce-hex> <receipt-secret-hex>
//! ```

// A process for the interop tests, not a state machine: it runs on the
// real clock, the OS RNG and a reader thread.
#![allow(clippy::disallowed_types, clippy::disallowed_methods, clippy::panic)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines
)]

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dassie_ilp::{Address, Prepare, Timestamp};
use dassie_oer::Decode;
use dassie_stream::receipt::ReceiptSecret;
use dassie_stream::{
    AssetDetails, BUFFER_LENGTH, Config, Connection, ConnectionId, ErrorCode, Event, Policy, Role,
    Server, ServerConfig,
};
use rand_core::UnwrapErr;

type Rng = UnwrapErr<getrandom::SysRng>;

fn now() -> Timestamp {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    Timestamp::from_unix_ms(u64::try_from(elapsed.as_millis()).unwrap())
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Frame {
    kind: u8,
    id: u64,
    bytes: Vec<u8>,
}

fn read_frames() -> Receiver<Frame> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        loop {
            let mut header = [0u8; 13];
            if input.read_exact(&mut header).is_err() {
                return;
            }
            let id = u64::from_be_bytes(header[1..9].try_into().unwrap());
            let length = u32::from_be_bytes(header[9..13].try_into().unwrap());
            let mut bytes = vec![0; length as usize];
            if input.read_exact(&mut bytes).is_err()
                || sender
                    .send(Frame {
                        kind: header[0],
                        id,
                        bytes,
                    })
                    .is_err()
            {
                return;
            }
        }
    });
    receiver
}

fn write_frame(kind: u8, id: u64, bytes: &[u8]) {
    let mut output = std::io::stdout().lock();
    output.write_all(&[kind]).unwrap();
    output.write_all(&id.to_be_bytes()).unwrap();
    output
        .write_all(&u32::try_from(bytes.len()).unwrap().to_be_bytes())
        .unwrap();
    output.write_all(bytes).unwrap();
    output.flush().unwrap();
}

fn log(line: &str) {
    write_frame(5, 0, line.as_bytes());
}

/// Sends everything a connection wants to send, and its events as log lines.
fn pump(connection: &mut Connection, label: &str, rng: &mut Rng, buffer: &mut [u8]) {
    while let Some(transmit) = connection.poll_transmit(now(), rng, buffer) {
        write_frame(4, transmit.request_id, &buffer[..transmit.length]);
    }
    while let Some(event) = connection.poll_event() {
        let line = match event {
            Event::MoneyReceived { stream_id } => {
                let total = connection
                    .stream(stream_id)
                    .map_or(0, |s| s.total_received());
                format!("received {label} {stream_id} {total}")
            }
            Event::MoneySent { stream_id } => {
                let total = connection.stream(stream_id).map_or(0, |s| s.total_sent());
                format!("sent {label} {stream_id} {total}")
            }
            Event::ExchangeRateMeasured { rate } => {
                format!("rate {} {}", rate.numerator(), rate.denominator())
            }
            Event::ConnectionClosed { code, by_remote } => {
                format!("closed {label} {} {by_remote}", code.0)
            }
            other => format!("event {label} {other:?}"),
        };
        log(&line);
    }
}

fn wait(frames: &Receiver<Frame>, deadline: Option<Timestamp>) -> Result<Option<Frame>, ()> {
    let timeout = deadline.map_or(Duration::from_millis(50), |deadline| {
        Duration::from_millis(deadline.unix_ms.saturating_sub(now().unix_ms).min(50))
    });
    match frames.recv_timeout(timeout) {
        Ok(frame) => Ok(Some(frame)),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(()),
    }
}

fn run_client(args: &[String]) {
    let [destination, secret, amount, own] = args else {
        panic!("client <destination> <secret> <amount> <own-address>");
    };
    let secret = hex(secret);
    let amount: u64 = amount.parse().unwrap();
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: &secret,
        local_address: Some(Address::parse(own).unwrap()),
        remote_address: Some(Address::parse(destination).unwrap()),
        local_asset: Some(AssetDetails {
            code: "USD",
            scale: 6,
        }),
        receipt: None,
        policy: Policy::DEFAULT,
    });
    let mut rng = UnwrapErr(getrandom::SysRng);
    let mut buffer = vec![0u8; BUFFER_LENGTH];
    let frames = read_frames();
    connection.measure_exchange_rate();
    let mut stream = None;
    loop {
        pump(&mut connection, "client", &mut rng, &mut buffer);
        if stream.is_none() && connection.exchange_rate().is_some() {
            let id = connection.open_stream().unwrap();
            connection.set_send_max(id, amount).unwrap();
            stream = Some(id);
        }
        if let Some(id) = stream
            && connection
                .stream(id)
                .is_some_and(|s| s.total_sent() == amount)
            && !connection.is_closed()
        {
            connection.close(now(), ErrorCode::NO_ERROR);
        }
        if connection.is_closed() && !connection.has_events() {
            log("done");
            return;
        }
        match wait(&frames, connection.poll_timeout()) {
            Err(()) => return,
            Ok(Some(frame)) if frame.kind == 3 => {
                connection.handle_response(now(), frame.id, &frame.bytes, &mut buffer);
            }
            Ok(Some(frame)) if frame.kind == 1 => {
                let Ok(prepare) = Prepare::decode_exact(&frame.bytes) else {
                    continue;
                };
                let length = connection.handle_prepare(now(), &mut rng, &prepare, &mut buffer);
                write_frame(2, frame.id, &buffer[..length]);
            }
            _ => connection.handle_timeout(now()),
        }
    }
}

fn run_server(args: &[String]) {
    let [own, nonce, receipt_secret] = args else {
        panic!("server <own-address> <receipt-nonce> <receipt-secret>");
    };
    let mut rng = UnwrapErr(getrandom::SysRng);
    let mut secret = [0u8; 32];
    rand_core::Rng::fill_bytes(&mut rng, &mut secret);
    let mut server = Server::new(&ServerConfig {
        secret,
        address: Address::parse(own).unwrap(),
        asset: Some(AssetDetails {
            code: "USD",
            scale: 6,
        }),
        policy: Policy {
            remote_stream_receive_max: 10_000_000,
            ..Policy::DEFAULT
        },
        max_connections: 8,
    });
    let receipt = ReceiptSecret {
        nonce: hex(nonce).try_into().unwrap(),
        secret: hex(receipt_secret).try_into().unwrap(),
    };
    let credentials = server.generate_credentials(&mut rng, b"native", Some(receipt));
    log(&format!(
        "credentials {} {}",
        credentials.destination,
        to_hex(&credentials.shared_secret)
    ));
    let mut buffer = vec![0u8; BUFFER_LENGTH];
    let frames = read_frames();
    let mut connections: Vec<ConnectionId> = Vec::new();
    loop {
        while let Some(id) = server.poll_opened() {
            connections.push(id);
        }
        for &id in &connections {
            if let Some(connection) = server.connection_mut(id) {
                pump(connection, &id.0.to_string(), &mut rng, &mut buffer);
            }
        }
        let deadline = server
            .connections()
            .filter_map(|(_, c)| c.poll_timeout())
            .min();
        match wait(&frames, deadline) {
            Err(()) => return,
            Ok(Some(frame)) if frame.kind == 1 => {
                let Ok(prepare) = Prepare::decode_exact(&frame.bytes) else {
                    continue;
                };
                let length = server.handle_prepare(now(), &mut rng, &prepare, &mut buffer);
                write_frame(2, frame.id, &buffer[..length]);
            }
            _ => {
                let time = now();
                for &id in &connections {
                    if let Some(connection) = server.connection_mut(id) {
                        connection.handle_timeout(time);
                    }
                }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("client") => run_client(&args[1..]),
        Some("server") => run_server(&args[1..]),
        _ => panic!("usage: interop_peer client|server ..."),
    }
}

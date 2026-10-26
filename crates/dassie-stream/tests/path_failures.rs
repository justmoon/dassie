//! A sender whose packets may take different paths: a rate below the
//! minimum on one, or a node on one that fails them, sends the money again
//! rather than stopping, within a bound on failures without progress; only
//! the receiver's own final error closes the connection
//! (spec/areas/stream.md#responses, spec/decisions/path-failures-retried.md).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_ilp::{Address, ErrorCode as IlpErrorCode, Prepare, Reject, Response, Timestamp};
use dassie_kit::sim::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_stream::{
    BUFFER_LENGTH, Config, Connection, Event, MAX_FAILURES_WITHOUT_PROGRESS, Policy, Rate, Role,
    Server, ServerConfig,
};

const NOW: Timestamp = Timestamp::from_unix_ms(1_700_000_000_000);

/// Every packet carries at most this much, so a payment takes many.
const PACKET: u64 = 1_000;

/// A server whose streams take money from the first packet, and a client
/// that pays it at one to one on a stream it opened.
struct Pair {
    server: Server,
    client: Connection,
    stream: u64,
    rng: SeededRng,
    buffer: Vec<u8>,
}

impl Pair {
    fn new() -> Self {
        let policy = Policy {
            remote_stream_receive_max: u64::MAX,
            max_packet_amount: PACKET,
            ..Policy::DEFAULT
        };
        let server = Server::new(&ServerConfig {
            secret: [3; 32],
            address: Address::parse("test.server").unwrap(),
            asset: None,
            policy,
            max_connections: 2,
        });
        let mut rng = SeededRng::new(17);
        let credentials = server.generate_credentials(&mut rng, b"payment");
        let mut client = Connection::new(&Config {
            role: Role::Client,
            shared_secret: &credentials.shared_secret,
            local_address: Address::parse("test.client").ok(),
            remote_address: Address::parse(&credentials.destination).ok(),
            local_asset: None,
            policy,
        });
        client.set_exchange_rate(Rate::ONE);
        let stream = client.open_stream().unwrap();
        client.set_send_max(stream, 1_000_000).unwrap();
        Self {
            server,
            client,
            stream,
            rng,
            buffer: vec![0; BUFFER_LENGTH],
        }
    }

    /// The client's next packet at `now`, if it sends one: its request ID
    /// and the Prepare.
    fn send(&mut self, now: Timestamp) -> Option<(u64, Vec<u8>)> {
        let transmit = self
            .client
            .poll_transmit(now, &mut self.rng, &mut self.buffer)?;
        self.client.check_invariants();
        Some((transmit.request_id, self.buffer[..transmit.length].to_vec()))
    }

    /// Hands a Prepare to the server over a path that delivers
    /// `delivered_permille` of its amount, and returns the answer.
    fn deliver(&mut self, prepare: &[u8], delivered_permille: u64) -> Vec<u8> {
        let prepare = Prepare::decode_exact(prepare).unwrap();
        let forwarded = Prepare {
            amount: prepare.amount * delivered_permille / 1000,
            ..prepare
        };
        let length = self
            .server
            .handle_prepare(NOW, &mut self.rng, &forwarded, &mut self.buffer);
        self.buffer[..length].to_vec()
    }

    fn answer(&mut self, now: Timestamp, request_id: u64, response: &[u8]) {
        self.client
            .handle_response(now, request_id, response, &mut self.buffer);
        self.client.check_invariants();
    }

    /// Sends a packet and answers it with a Reject from a node on the path.
    fn fail_on_the_path(&mut self, code: IlpErrorCode) {
        let (request_id, _) = self.send(NOW).expect("a packet goes at once");
        self.answer(NOW, request_id, &path_reject(code));
    }

    fn events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Some(event) = self.client.poll_event() {
            events.push(event);
        }
        events
    }

    fn sent(&self) -> u64 {
        self.client.stream(self.stream).unwrap().total_sent()
    }
}

/// A Reject from a node on the path: it carries no reply from the
/// receiver.
fn path_reject(code: IlpErrorCode) -> Vec<u8> {
    Reject {
        code,
        triggered_by: Address::parse("test.connector").ok(),
        message: "",
        data: &[],
    }
    .to_vec()
}

fn stopped(events: &[Event]) -> Option<Option<(u64, u64)>> {
    events.iter().find_map(|event| match event {
        Event::MoneyStopped { too_low } => Some(*too_low),
        _ => None,
    })
}

#[test]
fn a_rate_below_the_minimum_goes_again_at_once() {
    let mut pair = Pair::new();
    let (request_id, prepare) = pair.send(NOW).unwrap();
    // A dear path delivers half: the receiver refuses, and says so.
    let answer = pair.deliver(&prepare, 500);
    assert!(matches!(
        Response::decode_exact(&answer),
        Ok(Response::Reject(reject)) if reject.code == IlpErrorCode::F99_APPLICATION_ERROR
    ));
    pair.answer(NOW, request_id, &answer);
    assert!(!pair.client.is_closed());
    assert!(!pair.client.money_stopped());
    assert_eq!(stopped(&pair.events()), None);
    // Its money goes again at once: no back-off, which is for congestion.
    let (request_id, prepare) = pair.send(NOW).expect("no back-off");
    assert_eq!(Prepare::decode_exact(&prepare).unwrap().amount, PACKET);
    // Over a cheaper path, it arrives.
    let answer = pair.deliver(&prepare, 1000);
    pair.answer(NOW, request_id, &answer);
    assert_eq!(pair.sent(), PACKET);
}

#[test]
fn a_final_error_from_a_node_on_the_path_goes_again_at_once() {
    let unknown = IlpErrorCode::new(*b"X01").unwrap();
    // F99 without the receiver's reply, as a receiver that refuses a
    // connection gives, proves no more than a connector's F02.
    for code in [
        IlpErrorCode::F02_UNREACHABLE,
        IlpErrorCode::F99_APPLICATION_ERROR,
        unknown,
    ] {
        let mut pair = Pair::new();
        pair.fail_on_the_path(code);
        assert!(!pair.client.is_closed(), "{code:?}");
        let (request_id, prepare) = pair.send(NOW).expect("no back-off");
        let answer = pair.deliver(&prepare, 1000);
        pair.answer(NOW, request_id, &answer);
        assert_eq!(pair.sent(), PACKET, "{code:?}");
    }
}

#[test]
fn a_final_error_with_the_receivers_reply_closes() {
    let mut pair = Pair::new();
    let (request_id, prepare) = pair.send(NOW).unwrap();
    // The receiver's own refusal, carrying its reply, under another final
    // code: the receiver will not take the connection's packets.
    let answer = pair.deliver(&prepare, 500);
    let Ok(Response::Reject(reject)) = Response::decode_exact(&answer) else {
        panic!("the receiver refuses a packet below the minimum");
    };
    let recoded = Reject {
        code: IlpErrorCode::F02_UNREACHABLE,
        ..reject
    }
    .to_vec();
    pair.answer(NOW, request_id, &recoded);
    assert!(pair.client.is_closed());
}

#[test]
fn failures_without_progress_stop_money_until_a_rate_is_set() {
    let mut pair = Pair::new();
    for _ in 1..MAX_FAILURES_WITHOUT_PROGRESS {
        pair.fail_on_the_path(IlpErrorCode::F02_UNREACHABLE);
    }
    // A Fulfill is progress, and starts the count again.
    let (request_id, prepare) = pair.send(NOW).unwrap();
    let answer = pair.deliver(&prepare, 1000);
    pair.answer(NOW, request_id, &answer);
    assert_eq!(pair.sent(), PACKET);
    for _ in 1..MAX_FAILURES_WITHOUT_PROGRESS {
        pair.fail_on_the_path(IlpErrorCode::F02_UNREACHABLE);
    }
    assert!(!pair.client.money_stopped());
    assert_eq!(stopped(&pair.events()), None);
    // The last of the run is a rate below the minimum, which the event
    // reports.
    let (request_id, prepare) = pair.send(NOW).unwrap();
    let answer = pair.deliver(&prepare, 500);
    pair.answer(NOW, request_id, &answer);
    assert!(pair.client.money_stopped());
    assert!(!pair.client.is_closed());
    assert_eq!(stopped(&pair.events()), Some(Some((PACKET, PACKET / 2))));
    assert!(pair.send(NOW).is_none(), "money stopped");

    // Data still goes, but a path that fails it now backs off, so it
    // cannot loop.
    pair.client.write(pair.stream, b"hello").unwrap();
    let (request_id, prepare) = pair.send(NOW).unwrap();
    assert_eq!(Prepare::decode_exact(&prepare).unwrap().amount, 0);
    pair.answer(NOW, request_id, &path_reject(IlpErrorCode::F02_UNREACHABLE));
    assert!(pair.send(NOW).is_none(), "backed off");
    assert_eq!(stopped(&pair.events()), None, "said once");

    // Setting a rate again resumes money.
    pair.client.set_exchange_rate(Rate::ONE);
    assert!(!pair.client.money_stopped());
    let later = pair.client.poll_timeout().unwrap();
    let (request_id, prepare) = pair.send(later).unwrap();
    assert_eq!(Prepare::decode_exact(&prepare).unwrap().amount, PACKET);
    let answer = pair.deliver(&prepare, 1000);
    pair.answer(later, request_id, &answer);
    assert_eq!(pair.sent(), 2 * PACKET);
}

#[test]
fn a_fulfill_without_money_does_not_end_a_run() {
    // Data alone shows nothing of the rate: if its Fulfills ended a run, a
    // connection that sends data would retry money that no path delivers
    // for as long as the data flows.
    let mut pair = Pair::new();
    pair.client.set_send_max(pair.stream, 0).unwrap();
    pair.client.write(pair.stream, b"hello").unwrap();
    for _ in 1..MAX_FAILURES_WITHOUT_PROGRESS {
        pair.fail_on_the_path(IlpErrorCode::F02_UNREACHABLE);
    }
    let (request_id, prepare) = pair.send(NOW).unwrap();
    assert_eq!(Prepare::decode_exact(&prepare).unwrap().amount, 0);
    let answer = pair.deliver(&prepare, 1000);
    assert!(matches!(
        Response::decode_exact(&answer),
        Ok(Response::Fulfill(_))
    ));
    pair.answer(NOW, request_id, &answer);
    pair.client.write(pair.stream, b"again").unwrap();
    pair.fail_on_the_path(IlpErrorCode::F02_UNREACHABLE);
    assert!(pair.client.money_stopped());
    assert_eq!(stopped(&pair.events()), Some(None));
}

#[test]
fn nodes_on_the_path_alone_stop_money_without_a_rate() {
    let mut pair = Pair::new();
    for _ in 0..MAX_FAILURES_WITHOUT_PROGRESS {
        pair.fail_on_the_path(IlpErrorCode::F02_UNREACHABLE);
    }
    assert!(pair.client.money_stopped());
    assert_eq!(stopped(&pair.events()), Some(None));
}

#[test]
fn temporary_errors_neither_count_nor_reset() {
    let mut pair = Pair::new();
    let mut now = NOW;
    for round in 0..MAX_FAILURES_WITHOUT_PROGRESS {
        let (request_id, _) = pair.send(now).unwrap();
        pair.answer(now, request_id, &path_reject(IlpErrorCode::F02_UNREACHABLE));
        if round == MAX_FAILURES_WITHOUT_PROGRESS / 2 {
            // T04 backs off, as ever, and is no progress.
            let (request_id, _) = pair.send(now).unwrap();
            pair.answer(
                now,
                request_id,
                &path_reject(IlpErrorCode::T04_INSUFFICIENT_LIQUIDITY),
            );
            assert!(pair.send(now).is_none(), "backed off");
            now = pair.client.poll_timeout().unwrap();
        }
    }
    assert!(pair.client.money_stopped());
}

#[test]
fn rate_probes_are_not_counted() {
    // Probes are unfulfillable: a node on the path that fails them all
    // fails the measurement, as before, not money, and nothing closes.
    let mut client = Connection::new(&Config {
        role: Role::Client,
        shared_secret: &[5; 32],
        local_address: Address::parse("test.client").ok(),
        remote_address: Address::parse("test.server.abc").ok(),
        local_asset: None,
        policy: Policy::DEFAULT,
    });
    let mut rng = SeededRng::new(4);
    let mut buffer = vec![0; BUFFER_LENGTH];
    client.measure_exchange_rate();
    // Probes carry money; the packets that announce the client to the
    // receiver alongside them carry none.
    let (mut probes, mut others) = (0, 0);
    while let Some(transmit) = client.poll_transmit(NOW, &mut rng, &mut buffer) {
        let amount = Prepare::decode_exact(&buffer[..transmit.length])
            .unwrap()
            .amount;
        if amount > 0 {
            probes += 1;
        } else {
            others += 1;
        }
        let reject = path_reject(IlpErrorCode::F02_UNREACHABLE);
        client.handle_response(NOW, transmit.request_id, &reject, &mut buffer);
        client.check_invariants();
        assert!(others <= MAX_FAILURES_WITHOUT_PROGRESS, "bounded");
    }
    let mut events = Vec::new();
    while let Some(event) = client.poll_event() {
        events.push(event);
    }
    assert!(probes > 0);
    assert!(
        events.contains(&Event::ExchangeRateMeasurementFailed),
        "{events:?}"
    );
    assert!(!client.is_closed());
    // The other packets alone reached the bound, and then backed off.
    assert_eq!(others, MAX_FAILURES_WITHOUT_PROGRESS);
    assert!(client.money_stopped());
    assert_eq!(stopped(&events), Some(None));
}

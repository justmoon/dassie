//! Deterministic simulations of a client and a server (spec 7 §7, D16).
//! Every step checks both sides' invariants.

mod support;

use dassie_stream::{ErrorCode, Event, Policy, Rate};
use support::sim::{Path, Side, Simulation};

const STEPS: usize = 100_000;

/// For lossy paths: every lost packet costs its 30-second expiry, and a
/// few in a row would otherwise trip the 60-second idle timeout, which
/// these tests are not about.
const LOSSY: Policy = Policy {
    idle_timeout_ms: 600_000,
    ..Policy::DEFAULT
};

fn measured_rate(path: Path) -> Rate {
    let mut sim = Simulation::new(1, path, Policy::DEFAULT);
    sim.client.measure_exchange_rate();
    sim.run_until(STEPS, |sim| sim.trace.contains("ExchangeRate"));
    assert!(!sim.trace.contains("MeasurementFailed"), "{}", sim.trace);
    let line = sim
        .trace
        .lines()
        .find(|line| line.contains("ExchangeRateMeasured"))
        .unwrap();
    // The trace has the event's Debug form: `rate: Rate(n/d)`.
    let fraction = line
        .split("Rate(")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap();
    let (n, d) = fraction.split_once('/').unwrap();
    Rate::new(n.parse().unwrap(), d.parse().unwrap()).unwrap()
}

#[test]
fn measures_a_one_to_one_rate() {
    assert_eq!(measured_rate(Path::default()), Rate::ONE);
}

#[test]
fn measures_a_three_to_two_rate() {
    let path = Path {
        rate: (3, 2),
        ..Path::default()
    };
    assert_eq!(measured_rate(path), Rate::new(3, 2).unwrap());
}

#[test]
fn measures_a_high_precision_rate() {
    // The original's test: floor(1e6 e) / floor(1e6 π), measured with the
    // 10^12 probe.
    let path = Path {
        rate: (2_718_281, 3_141_592),
        ..Path::default()
    };
    assert_eq!(
        measured_rate(path),
        Rate::new(108_156_986_967, 125_000_000_000).unwrap()
    );
}

#[test]
fn measures_a_rate_when_packets_are_limited() {
    // The 10^12 probe gets F08; the 10^9 probe decides, as in the original.
    let path = Path {
        rate: (2_718_281, 3_141_592),
        max_packet_amount: 1_000_000_000,
        ..Path::default()
    };
    assert_eq!(
        measured_rate(path),
        Rate::new(173_051_179, 200_000_000).unwrap()
    );
}

fn pay(path: Path, policy: Policy, amount: u64) -> Simulation {
    let mut sim = Simulation::new(7, path, policy);
    sim.client
        .set_exchange_rate(Rate::new(path.rate.0, path.rate.1).unwrap());
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, amount).unwrap();
    sim.run_until(STEPS, |sim| {
        sim.client
            .stream(stream)
            .is_some_and(|s| s.total_sent() == amount)
    });
    sim
}

fn server_received(sim: &Simulation) -> u64 {
    sim.server_connection()
        .map_or(0, |connection| connection.stats().amount_received)
}

#[test]
fn sends_money() {
    let sim = pay(Path::default(), Policy::DEFAULT, 1_000_000);
    assert_eq!(server_received(&sim), 1_000_000);
    assert_eq!(sim.client.stats().amount_sent, 1_000_000);
}

#[test]
fn sends_money_in_packets_the_path_allows() {
    let path = Path {
        max_packet_amount: 10_000,
        ..Path::default()
    };
    let sim = pay(path, Policy::DEFAULT, 1_000_000);
    assert_eq!(server_received(&sim), 1_000_000);
    assert!(sim.client.max_packet_amount() <= 10_000);
}

#[test]
fn sends_amounts_below_the_de_minimis() {
    // ILP-11: the original never sent payments of 1000 units or less.
    let sim = pay(Path::default(), Policy::DEFAULT, 500);
    assert_eq!(server_received(&sim), 500);
}

#[test]
fn respects_the_receivers_maximum() {
    let mut sim = Simulation::new(3, Path::default(), Policy::DEFAULT);
    sim.server_receive_max = 400;
    sim.client.set_exchange_rate(Rate::ONE);
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 1000).unwrap();
    sim.run_until(STEPS, |sim| server_received(sim) == 400);
    for _ in 0..200 {
        sim.step();
    }
    assert_eq!(server_received(&sim), 400, "{}", sim.trace);
    assert_eq!(sim.client.stream(stream).unwrap().total_sent(), 400);
}

#[test]
fn survives_packet_loss() {
    let path = Path {
        loss_permille: 200,
        max_packet_amount: 50_000,
        ..Path::default()
    };
    let sim = pay(path, LOSSY, 1_000_000);
    assert_eq!(server_received(&sim), 1_000_000);
    // Lost packets come back as R00 when they expire.
    assert!(sim.client.stats().packets_rejected > 0);
}

#[test]
fn moves_data() {
    let mut sim = Simulation::new(11, Path::default(), Policy::DEFAULT);
    let stream = sim.client.open_stream().unwrap();
    // More than the stream and connection windows, so flow control must
    // slide them as the receiver reads.
    let message: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let mut written = 0;
    let delivered = |sim: &Simulation| sim.received_data.values().map(Vec::len).sum::<usize>();
    for _ in 0..STEPS {
        if written < message.len() {
            written += sim.client.write(stream, &message[written..]).unwrap();
        }
        if delivered(&sim) >= message.len() {
            break;
        }
        assert!(sim.step(), "idle before all data arrived");
    }
    let (key, data) = sim.received_data.iter().next().unwrap();
    assert!(matches!(key.0, Side::Server(_)));
    assert_eq!(data, &message);
}

#[test]
fn server_can_send_back_to_the_client() {
    let mut sim = Simulation::new(5, Path::default(), Policy::DEFAULT);
    // The client says hello so the server learns its address.
    sim.run_until(STEPS, |sim| sim.server_connection().is_some());
    sim.run_until(STEPS, |sim| sim.trace.contains("Client Connected"));
    let id = sim.server.connections().next().unwrap().0;
    let connection = sim.server.connection_mut(id).unwrap();
    connection.set_exchange_rate(Rate::ONE);
    let stream = connection.open_stream().unwrap();
    assert_eq!(stream % 2, 0, "server streams are even");
    connection.set_send_max(stream, 5000).unwrap();
    connection.write(stream, b"hello client").unwrap();
    sim.run_until(STEPS, |sim| {
        sim.client
            .stream(stream)
            .is_some_and(|s| s.total_received() == 5000)
            && sim.received_data.contains_key(&(Side::Client, stream))
    });
    assert_eq!(sim.received_data[&(Side::Client, stream)], b"hello client");
}

#[test]
fn issues_and_verifies_receipts() {
    let secret = dassie_stream::receipt::ReceiptSecret {
        nonce: [7; 16],
        secret: [9; 32],
    };
    let mut sim = Simulation::with_receipts(13, Path::default(), Policy::DEFAULT, Some(secret));
    sim.client.set_exchange_rate(Rate::ONE);
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 20_000).unwrap();
    sim.run_until(STEPS, |sim| {
        sim.client
            .stream(stream)
            .is_some_and(|s| s.total_sent() == 20_000)
    });
    let receipt = sim.client.stream(stream).unwrap().last_receipt().unwrap();
    let verified = dassie_stream::receipt::verify(receipt, &secret.secret).unwrap();
    assert_eq!(verified.nonce, secret.nonce);
    assert_eq!(u64::from(verified.stream_id), stream);
    assert_eq!(verified.total_received, 20_000);
}

#[test]
fn closes_and_tells_the_remote() {
    let mut sim = pay(Path::default(), Policy::DEFAULT, 10_000);
    let now = sim.now;
    sim.client.close(now, ErrorCode::NO_ERROR);
    sim.run_to_idle(STEPS);
    assert!(sim.client.is_closed());
    assert!(sim.server_connection().unwrap().is_closed());
    assert!(
        sim.trace
            .contains("ConnectionClosed { code: ErrorCode(1), by_remote: true }")
    );
    assert!(
        sim.trace
            .contains("ConnectionClosed { code: ErrorCode(1), by_remote: false }")
    );
}

#[test]
fn closed_streams_free_their_ids() {
    // More streams than the remote allows at once: closing streams must
    // let new ones open (the original never raised the limit, ILP-11).
    let mut sim = Simulation::new(17, Path::default(), Policy::DEFAULT);
    sim.client.set_exchange_rate(Rate::ONE);
    for round in 0..25u64 {
        let stream = loop {
            match sim.client.open_stream() {
                Ok(stream) => break stream,
                Err(_) => assert!(sim.step(), "stuck opening stream {round}"),
            }
        };
        sim.client.set_send_max(stream, 1000).unwrap();
        sim.run_until(STEPS, |sim| {
            sim.client
                .stream(stream)
                .is_some_and(|s| s.total_sent() == 1000)
        });
        sim.client
            .close_stream(stream, ErrorCode::NO_ERROR)
            .unwrap();
        sim.run_until(STEPS, |sim| sim.client.stream(stream).is_none());
    }
    assert_eq!(server_received(&sim), 25_000);
    let closed = sim.trace.matches("Client StreamClosed").count();
    assert_eq!(closed, 25);
}

#[test]
fn rejects_a_rate_below_the_minimum() {
    let path = Path {
        rate: (1, 2),
        ..Path::default()
    };
    let mut sim = Simulation::new(19, path, Policy::DEFAULT);
    sim.client.set_exchange_rate(Rate::ONE);
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 10_000).unwrap();
    sim.run_until(STEPS, |sim| sim.trace.contains("ExchangeRateTooLow"));
    assert_eq!(server_received(&sim), 0);
    assert_eq!(sim.client.stream(stream).unwrap().total_sent(), 0);
}

fn scenario_trace(seed: u64) -> String {
    let path = Path {
        loss_permille: 100,
        max_packet_amount: 7_000,
        rate: (3, 2),
        ..Path::default()
    };
    let mut sim = Simulation::new(seed, path, LOSSY);
    sim.client.measure_exchange_rate();
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 300_000).unwrap();
    sim.client.write(stream, &[42; 10_000]).unwrap();
    sim.run_until(STEPS, |sim| {
        sim.client
            .stream(stream)
            .is_some_and(|s| s.total_sent() == 300_000)
    });
    let now = sim.now;
    sim.client.close(now, ErrorCode::NO_ERROR);
    sim.run_to_idle(STEPS);
    format!("{}\n{:?}", sim.trace, sim.client.stats())
}

#[test]
fn simulations_are_deterministic() {
    // The same seed must give the same run, event for event (D16).
    assert_eq!(scenario_trace(23), scenario_trace(23));
    assert_ne!(scenario_trace(23), scenario_trace(24));
}

#[test]
fn many_seeds_keep_the_invariants() {
    for seed in 100..120 {
        let trace = scenario_trace(seed);
        assert!(trace.contains("ConnectionClosed"), "seed {seed}");
    }
}

#[test]
fn a_default_receive_max_takes_the_first_packet() {
    let policy = Policy {
        remote_stream_receive_max: u64::MAX,
        ..Policy::DEFAULT
    };
    let sim = pay(Path::default(), policy, 5000);
    assert_eq!(server_received(&sim), 5000);
    assert_eq!(sim.client.stats().packets_rejected, 0, "{}", sim.trace);
}

#[test]
fn events_report_the_remote_opening_streams() {
    let sim = pay(Path::default(), Policy::DEFAULT, 1000);
    let opened = sim
        .trace
        .lines()
        .filter(|line| line.contains("Server(") && line.contains("StreamOpened"))
        .count();
    assert_eq!(opened, 1);
    let _ = Event::Connected;
}

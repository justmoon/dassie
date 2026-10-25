//! Deterministic simulations of a client and a server
//! (spec/areas/testing.md#deterministic-simulation,
//! spec/decisions/tigerstyle.md). Every step checks both sides' invariants.
//! `dassie_stream::sim::payment` is the random run: a failure names its seed,
//! `dassie-sim run stream <seed>` replays it, and `dassie-sim run stream
//! <from>..<to>` runs others (spec/decisions/random-seed-simulations.md).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_node_core::testing::{Outcome, assert_seeds_pass};
use dassie_stream::sim::payment::{self, LOSSY, STEPS};
use dassie_stream::sim::{Detour, Path, Side, Simulation};
use dassie_stream::{ErrorCode, Event, MAX_FAILURES_WITHOUT_PROGRESS, Policy, Rate};

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

/// Pays with a delivery cap of `cap` and money for twice that, at the
/// minimum rate `floor`, until the cap is reached; `then` may change the
/// path once the server has `at` (spec/areas/stream.md#sending). The
/// stream is open on both sides first, so the first packet with money is
/// fulfilled: no refusal shows the rate before it goes.
fn pay_capped(
    path: Path,
    floor: Rate,
    cap: u64,
    at: u64,
    then: impl FnOnce(&mut Simulation),
) -> (Simulation, u64) {
    let mut sim = Simulation::new(11, path, Policy::DEFAULT);
    sim.client.set_exchange_rate(floor);
    let stream = sim.client.open_stream().unwrap();
    sim.client.write(stream, b"hello").unwrap();
    sim.run_until(STEPS, |sim| {
        sim.received_data.values().any(|data| !data.is_empty())
    });
    sim.client.set_send_max(stream, 2 * cap).unwrap();
    sim.client.set_delivery_max(stream, cap).unwrap();
    sim.run_until(STEPS, |sim| server_received(sim) >= at);
    then(&mut sim);
    sim.run_until(STEPS, |sim| sim.client.delivery_cap_reached(stream));
    // Nothing more goes once it is reached, not even packets that would
    // deliver nothing.
    let sent = sim.client.stream(stream).unwrap().total_sent();
    for _ in 0..200 {
        sim.step();
    }
    assert_eq!(sim.client.stream(stream).unwrap().total_sent(), sent);
    (sim, stream)
}

// A path that pays better than the minimum rate does not carry a capped
// stream past its cap, in one packet or in many at once: the first goes
// for half of it, and the rest are sized at the rate replies show.
#[test]
fn a_delivery_cap_holds_on_a_better_path() {
    for max_packet_amount in [u64::MAX, 700] {
        let path = Path {
            rate: (101, 100),
            max_packet_amount,
            ..Path::default()
        };
        let (sim, stream) = pay_capped(path, Rate::ONE, 5_000, 0, |_| {});
        assert_eq!(server_received(&sim), 5_000, "{}", sim.trace);
        let sent = sim.client.stream(stream).unwrap().total_sent();
        assert!(sent < 5_000, "sent {sent}");
        // The rate the first packet with money showed sizes the second to
        // the rest, rather than half of it again and again: the data that
        // opened the stream, then two.
        if max_packet_amount == u64::MAX {
            assert_eq!(sim.client.stats().packets_fulfilled, 3, "{}", sim.trace);
        }
    }
}

// A path that pays worse than the rate replies showed, but no worse than
// the minimum, still delivers the cap, short by less than the least packet
// delivers, and never pays for a packet that delivers nothing.
#[test]
fn a_delivery_cap_is_reached_when_the_path_gets_worse() {
    let path = Path {
        max_packet_amount: 500,
        ..Path::default()
    };
    let floor = Rate::new(95, 100).unwrap();
    let (sim, stream) = pay_capped(path, floor, 5_000, 2_000, |sim| {
        sim.path.rate = (97, 100);
    });
    let received = server_received(&sim);
    assert!(
        (4_999..=5_000).contains(&received),
        "{received}\n{}",
        sim.trace
    );
    // Every packet delivered at least the minimum rate.
    let sent = sim.client.stream(stream).unwrap().total_sent();
    assert!(floor.destination_amount(sent) <= received, "sent {sent}");
}

/// Opens a stream on both sides, as `pay_capped` does, and caps it.
fn capped_stream(path: Path, floor: Rate, send_max: u64, cap: u64) -> (Simulation, u64) {
    let mut sim = Simulation::new(12, path, Policy::DEFAULT);
    sim.client.set_exchange_rate(floor);
    let stream = sim.client.open_stream().unwrap();
    sim.client.write(stream, b"hello").unwrap();
    sim.run_until(STEPS, |sim| {
        sim.received_data.values().any(|data| !data.is_empty())
    });
    sim.client.set_send_max(stream, send_max).unwrap();
    sim.client.set_delivery_max(stream, cap).unwrap();
    (sim, stream)
}

// Found in review: half of a cap can fit no packet that delivers a unit
// while the whole does, as a single source unit at three units each for a
// cap of three. The least such packet goes, or the stream would never send.
#[test]
fn a_delivery_cap_the_least_packet_fills_is_reached() {
    let path = Path {
        rate: (3, 1),
        ..Path::default()
    };
    let (mut sim, stream) = capped_stream(path, Rate::new(3, 1).unwrap(), 1, 3);
    sim.run_until(STEPS, |sim| sim.client.delivery_cap_reached(stream));
    assert_eq!(server_received(&sim), 3, "{}", sim.trace);
}

// A stream told what an earlier connection delivered starts its cap from
// what is left, halved until a reply shows the rate, rather than from all
// of it. The server counts as a Dassie receiver does, over connections with
// the same credentials, so its first reply says 4,000 and more.
#[test]
fn a_delivery_cap_starts_from_what_was_delivered() {
    let path = Path {
        rate: (101, 100),
        ..Path::default()
    };
    let (mut sim, stream) = capped_stream(path, Rate::ONE, 10_000, 5_000);
    sim.money_limit = Some(1_000_000);
    sim.money_received = 4_000;
    sim.client.set_delivered(stream, 4_000).unwrap();
    sim.run_until(STEPS, |sim| sim.client.delivery_cap_reached(stream));
    assert_eq!(sim.money_received, 5_000, "{}", sim.trace);
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
    // Every packet arrives short: each goes again, until the bound on
    // failures without progress stops money and tells the application.
    let path = Path {
        rate: (1, 2),
        ..Path::default()
    };
    let mut sim = Simulation::new(19, path, Policy::DEFAULT);
    sim.client.set_exchange_rate(Rate::ONE);
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 10_000).unwrap();
    sim.run_until(STEPS, |sim| sim.trace.contains("MoneyStopped"));
    assert!(sim.trace.contains("too_low: Some((10000, 5000))"));
    assert_eq!(server_received(&sim), 0);
    assert_eq!(sim.client.stream(stream).unwrap().total_sent(), 0);
    assert_eq!(
        sim.client.stats().packets_rejected,
        u64::from(MAX_FAILURES_WITHOUT_PROGRESS)
    );
    // Nothing more goes until the application sets a rate, and the idle
    // timeout ends a connection it leaves.
    sim.run_to_idle(STEPS);
    assert_eq!(
        sim.client.stats().packets_sent,
        u64::from(MAX_FAILURES_WITHOUT_PROGRESS)
    );
}

/// Pays `amount` at one to one over a path that sends a share of the
/// client's packets over `detour`, in packets of at most 1,000.
fn pay_with_detour(detour: Detour, amount: u64) -> Simulation {
    let path = Path {
        max_packet_amount: 1_000,
        detour_permille: 300,
        detour,
        ..Path::default()
    };
    let sim = pay(path, Policy::DEFAULT, amount);
    assert_eq!(server_received(&sim), amount, "{}", sim.trace);
    assert!(sim.trace.contains("detour"));
    assert!(sim.client.stats().packets_rejected > 0);
    assert!(!sim.trace.contains("MoneyStopped"), "{}", sim.trace);
    sim
}

// A route that delivers less than the minimum takes a share of the
// packets: the receiver refuses those, their money goes again, and the
// payment completes over the route that delivers at the rate. Only
// packets that got the rate are paid for.
#[test]
fn a_dearer_route_does_not_stop_a_payment() {
    let sim = pay_with_detour(Detour::Rate(1, 2), 100_000);
    assert_eq!(sim.client.stats().amount_sent, 100_000);
    assert_eq!(sim.client.stats().amount_delivered, 100_000);
}

// A node on another route cannot reach the receiver: the packets it
// fails go again, and the payment completes over the route that works.
#[test]
fn a_node_that_fails_packets_does_not_stop_a_payment() {
    pay_with_detour(Detour::Unreachable, 100_000);
}

#[test]
fn simulations_are_deterministic() {
    // The same seed must give the same run, event for event
    // (spec/decisions/tigerstyle.md), and runs that differ must still both
    // finish.
    let (first, again, other) = (payment::run(23), payment::run(23), payment::run(24));
    assert_eq!(first.outcome, Outcome::Pass);
    assert_eq!(other.outcome, Outcome::Pass);
    assert_eq!(first.trace, again.trace);
    assert_ne!(first.trace, other.trace);
}

#[test]
fn many_seeds_keep_the_invariants() {
    assert_seeds_pass(100..120, payment::run);
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

#[test]
fn a_connection_receives_no_more_than_its_limit() {
    // As an incoming payment of 600 would let in: the sender wants to send
    // 1000, the receiver takes 600 and tells the sender so.
    let mut sim = Simulation::new(11, Path::default(), Policy::DEFAULT);
    sim.money_limit = Some(600);
    sim.client.set_exchange_rate(Rate::ONE);
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 1_000).unwrap();
    sim.run_until(STEPS, |sim| server_received(sim) == 600);
    for _ in 0..200 {
        if !sim.step() {
            break;
        }
    }
    assert_eq!(server_received(&sim), 600);
    assert_eq!(sim.money_limit, Some(0));
    assert_eq!(sim.client.stats().amount_sent, 600);
    assert_eq!(
        sim.admitted_tags.first().map(Vec::as_slice),
        Some(&b"payment-1"[..])
    );
}

#[test]
fn a_limit_holds_across_a_server_restart() {
    // The server forgets its connection mid-payment, as a node does when it
    // restarts. The application's count of what arrived survives, and the
    // new connection reports it: the sender learns what is left, which is
    // less than a packet at the end, and neither stalls nor overpays.
    let path = Path {
        max_packet_amount: 100,
        ..Path::default()
    };
    let mut sim = Simulation::new(14, path, Policy::DEFAULT);
    sim.money_limit = Some(650);
    sim.client.set_exchange_rate(Rate::ONE);
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 1_000).unwrap();
    sim.run_until(STEPS, |sim| sim.money_received >= 300);
    sim.restart_server();
    sim.run_until(STEPS, |sim| sim.client.stats().amount_sent == 650);
    for _ in 0..200 {
        if !sim.step() {
            break;
        }
    }
    assert_eq!(sim.money_received, 650);
    assert_eq!(sim.money_limit, Some(0));
    assert_eq!(sim.client.stats().amount_sent, 650, "{}", sim.trace);
}

#[test]
fn a_refused_connection_never_opens() {
    // The refusal carries no reply, so the sender cannot tell it from a
    // node on the path: it tries again, until the bound on failures
    // without progress stops its money.
    let mut sim = Simulation::new(12, Path::default(), Policy::DEFAULT);
    sim.refusal = Some(dassie_ilp::ErrorCode::F99_APPLICATION_ERROR);
    sim.client.set_exchange_rate(Rate::ONE);
    let stream = sim.client.open_stream().unwrap();
    sim.client.set_send_max(stream, 1_000).unwrap();
    sim.run_until(STEPS, |sim| sim.client.money_stopped());
    assert!(sim.trace.contains("MoneyStopped { too_low: None }"));
    assert!(sim.server.connections().next().is_none());
    assert_eq!(sim.client.stats().amount_sent, 0);
    assert!(!sim.admitted_tags.is_empty());
}

#[test]
fn a_kept_token_gives_the_same_credentials() {
    // An Open Payments incoming payment shows the same credentials on
    // every read: the application keeps the token, not the secret.
    let mut sim = Simulation::new(13, Path::default(), Policy::DEFAULT);
    let token = sim.server.issue_token(&mut sim.rng, b"payment-2");
    let first = sim.server.credentials_for(&token);
    assert_eq!(sim.server.credentials_for(&token), first);
    let other = sim.server.issue_token(&mut sim.rng, b"payment-2");
    assert_ne!(
        sim.server.credentials_for(&other).shared_secret,
        first.shared_secret
    );
}

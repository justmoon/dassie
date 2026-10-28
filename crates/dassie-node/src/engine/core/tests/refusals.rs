//! Prepares a peer's transport refuses before its engine sees them
//! (spec/areas/peer-protocol.md#statuses,
//! spec/areas/connector.md#responses-and-expiry): the peer did not act on
//! them, so they fail at once, rather than holding what they carry until
//! they expire.

use std::collections::BTreeMap;

use dassie_connector::{EndpointId, PacketState};
use dassie_ilp::ErrorCode;

use crate::sim::{START, Settling, Sim};

// A node over the rate of its failed requests at a peer, or whose body is
// too large or too slow for it, gets the peer transport's 429, 413 or 408:
// each Prepare so refused is rejected as soon as the answer is back, T05 for
// the rate, so that the sender backs off, and T01 otherwise.
#[test]
fn a_prepare_the_peers_transport_refuses_fails_at_once() {
    let mut sim = Sim::with(91, 2, 1, BTreeMap::new(), Settling::Trusting);
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| !sim.peers(0).is_empty() && !sim.peers(1).is_empty();
    assert!(
        sim.run_until_or(START + 60_000, peered),
        "the nodes did not peer"
    );
    // A request and its answer, each as late as the network makes them.
    let round_trip = 2 * sim.latency_ms.1;
    let refusals = [
        (429, ErrorCode::T05_RATE_LIMITED),
        (413, ErrorCode::T01_PEER_UNREACHABLE),
        (408, ErrorCode::T01_PEER_UNREACHABLE),
    ];
    for (status, code) in refusals {
        sim.transport_refusals.insert((0, 1), status);
        let since = sim.core(0).now_ms();
        assert!(sim.pay(0, 1, 500_000_000, None, None));
        sim.run_until(sim.now + 5_000);
        let sent: Vec<_> = sim
            .core(0)
            .packets(None, 100_000)
            .into_iter()
            .filter(|record| matches!(record.destination, Some(EndpointId::Peer(_))))
            .filter(|record| record.received_at_ms >= since)
            .collect();
        assert!(!sent.is_empty(), "{status}: no Prepare went to the peer");
        for record in sent {
            let resolved = record
                .resolved_at_ms
                .unwrap_or_else(|| panic!("{status}: a refused Prepare waits for its expiry"));
            assert_eq!(record.state, PacketState::Rejected(code), "{status}");
            assert!(
                resolved - record.received_at_ms <= round_trip,
                "{status}: failed only after {} ms",
                resolved - record.received_at_ms
            );
        }
    }
}

//! Probes of requesters' URLs (spec/areas/network.md#registration): every
//! probe spends a token of all the node's probes, and failing ones also
//! spend their network's share, so one network cannot starve the others.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::net::IpAddr;
use std::sync::Arc;

use dassie_identity::NodeKey;
use dassie_kit::sim::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::sim::{Network, START, key};
use dassie_peer::wire::{
    Authentication, Content, Entry, NodeInfo, SchemeId, SignedNodeInfo, encode_envelope,
    sign_node_info,
};
use dassie_peer::{Config, Event};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// The one address the failing registrations come from.
const ATTACKER: IpAddr = IpAddr::V4(core::net::Ipv4Addr::new(203, 0, 113, 66));

/// An address in a /24 of its own for each `index`.
fn elsewhere(index: u8) -> IpAddr {
    IpAddr::from([198, 51, index, 10])
}

/// An identity made up for one request.
fn made_up(index: u32) -> Arc<NodeKey> {
    let mut seed = [0xa5u8; 32];
    seed[..4].copy_from_slice(&index.to_be_bytes());
    Arc::new(NodeKey::from_bytes(&seed))
}

/// A registration with node `to` from `from`, whose link state names `url`.
fn registration(network: &Network, from: &Arc<NodeKey>, url: &str, to: usize) -> Vec<u8> {
    let info = NodeInfo {
        node_id: from.node_id(),
        sequence: network.clock(to),
        url,
        alias: "",
        public_key: *from.public_key().as_bytes(),
        entries: vec![Entry::Scheme(SchemeId::parse("stub").unwrap())],
    };
    let signed = sign_node_info(from, &info);
    let content = Content::Registration(SignedNodeInfo::decode_exact(&signed).unwrap()).to_vec();
    let mut auth = Authenticator::new(Arc::clone(from), AuthConfig::DEFAULT, START - 1);
    let target = key(to);
    let (hmac, _) = auth
        .authenticate(
            network.clock(to),
            &mut SeededRng::new(network.clock(to)),
            &target.node_id(),
            &target.public_key(),
            &content,
        )
        .unwrap();
    encode_envelope(auth.own(), &Authentication::Hmac(hmac), &content)
}

/// Registrations from one address, each from a new identity at a URL
/// nothing serves, so each probe fails.
struct Attacker {
    from: IpAddr,
    next: u32,
}

impl Attacker {
    const fn at(from: IpAddr) -> Self {
        Self { from, next: 0 }
    }

    /// Sends `count` registrations to node 0, and their statuses (0 while
    /// one waits on its probe).
    fn send(&mut self, network: &mut Network, count: u32) -> Vec<(u16, String)> {
        (0..count)
            .map(|_| {
                self.next += 1;
                let url = format!("https://nowhere.test:{}", 1000 + self.next);
                let body = registration(network, &made_up(self.next), &url, 0);
                let reply = network.inbound_from(0, &body, Some(self.from));
                (
                    reply.status,
                    String::from_utf8_lossy(&reply.body).into_owned(),
                )
            })
            .collect()
    }
}

// Every probe took a token of one bucket for the whole node, whatever its
// outcome, and failing ones counted against nothing else. Registrations
// whose probes fail, from one address, each from a new identity at a new
// host and port, kept it empty, and every real node was refused for "too
// many probes".
#[test]
fn failing_probes_from_one_network_starve_no_real_registration() {
    let config = Config::DEFAULT;
    let mut network = Network::new(9, 1, 1, config);
    settle(&mut network, 1_000);
    let mut attacker = Attacker::at(ATTACKER);
    attacker.send(&mut network, config.probe_rate.burst);
    // Then one failing registration each time a token comes back.
    let mut real = Vec::new();
    for round in 0..10_u8 {
        settle(&mut network, config.probe_rate.refill_ms);
        attacker.send(&mut network, 1);
        // A real node, each second a new one from its own network.
        let node = Arc::new(NodeKey::from_bytes(&[200 + round; 32]));
        let url = format!("https://real-{round}.test");
        let body = registration(&network, &node, &url, 0);
        let reply = network.inbound_from(0, &body, Some(elsewhere(round)));
        real.push((
            reply.status,
            String::from_utf8_lossy(&reply.body).into_owned(),
        ));
    }
    assert!(
        real.iter().all(|(status, _)| *status == 0),
        "each real registration waits on its probe: {real:?}"
    );
}

// A dual-stack socket reports IPv4 clients as IPv4-mapped IPv6 addresses.
// They are told apart by their IPv4 networks, not taken for one IPv6
// network that every IPv4 client shares.
#[test]
fn mapped_ipv4_requesters_are_told_apart_by_their_network() {
    let mapped = |address: IpAddr| match address {
        IpAddr::V4(v4) => IpAddr::V6(v4.to_ipv6_mapped()),
        IpAddr::V6(_) => address,
    };
    let config = Config::DEFAULT;
    let mut network = Network::new(23, 1, 1, config);
    settle(&mut network, 1_000);
    let mut attacker = Attacker::at(mapped(ATTACKER));
    attacker.send(&mut network, config.probe_rate.burst);
    for round in 0..4_u8 {
        let node = Arc::new(NodeKey::from_bytes(&[210 + round; 32]));
        let url = format!("https://mapped-{round}.test");
        let body = registration(&network, &node, &url, 0);
        let reply = network.inbound_from(0, &body, Some(mapped(elsewhere(round))));
        assert_eq!(reply.status, 0, "{}", String::from_utf8_lossy(&reply.body));
    }
}

// One network's failing registrations take its own share of the probes
// while they are in flight, and keep it once they fail: the rest are
// refused, and a registration from another network still gets its probe.
#[test]
fn failing_registrations_from_one_network_spend_only_its_share() {
    // The network's share of the replay pool for strangers would turn the
    // later registrations away first (503), before their probes: room for
    // every one here, so this sees only the probes' share.
    let config = Config {
        auth: AuthConfig {
            max_senders_per_network: AuthConfig::DEFAULT.max_senders,
            ..AuthConfig::DEFAULT
        },
        ..Config::DEV
    };
    let share = config.probe_rate_per_network;
    let mut network = Network::new(5, 2, 1, config);
    settle(&mut network, 30_000);
    let mut attacker = Attacker::at(ATTACKER);
    let statuses = attacker.send(&mut network, config.probe_rate.burst);
    let (deferred, refused) = statuses.split_at(share.burst as usize);
    assert!(
        deferred.iter().all(|(status, _)| *status == 0),
        "{statuses:?}"
    );
    assert!(
        refused
            .iter()
            .all(|(status, body)| *status == 429 && body == "too many probes from that network"),
        "{statuses:?}"
    );
    // The probes fail; the share stays spent until it refills.
    settle(&mut network, 6_000);
    assert_eq!(attacker.send(&mut network, 1)[0].0, 429);
    let newcomer = Arc::new(NodeKey::from_bytes(&[202; 32]));
    let body = registration(&network, &newcomer, "https://newcomer.test", 0);
    let reply = network.inbound_from(0, &body, Some(elsewhere(1)));
    assert_eq!(reply.status, 0, "the newcomer's URL is probed");
    settle(&mut network, share.refill_ms);
    assert_eq!(attacker.send(&mut network, 1)[0].0, 0, "a token came back");
}

// A real node from another network registers, and is listed, while one
// network keeps sending registrations whose probes fail.
#[test]
fn a_node_registers_while_another_network_probes_in_vain() {
    let config = Config::DEFAULT;
    let mut network = Network::new(21, 3, 1, config);
    network.sources.insert(2, elsewhere(7));
    network.crash(2);
    settle(&mut network, 1_000);
    let mut attacker = Attacker::at(ATTACKER);
    attacker.send(&mut network, config.probe_rate.burst);
    network.start(2);
    let node = key(2).node_id();
    let registered = |network: &Network| {
        network
            .events
            .iter()
            .any(|(at, event)| *at == 0 && *event == Event::Registered(node))
    };
    for _ in 0..300 {
        if registered(&network) {
            break;
        }
        settle(&mut network, 100);
        attacker.send(&mut network, 1);
    }
    assert!(registered(&network), "node 2 registered");
}

// A probe that succeeds costs its network nothing, so nodes that share a
// network, as a dev environment's do on loopback, all register.
#[test]
fn nodes_sharing_a_network_all_register() {
    let config = Config::DEV;
    let count = 2 * config.probe_rate_per_network.burst as usize + 1;
    let mut network = Network::new(22, count, 1, config);
    // On one machine, with one clock.
    for index in 0..count {
        network.crash(index);
        network.nodes[index].skew_ms = 0;
        network.start(index);
        network
            .sources
            .insert(index, IpAddr::V4(core::net::Ipv4Addr::LOCALHOST));
    }
    settle(&mut network, 30_000);
    for index in 1..count {
        let node = key(index).node_id();
        assert!(
            network
                .events
                .iter()
                .any(|(at, event)| *at == 0 && *event == Event::Registered(node)),
            "node {index} registered"
        );
    }
}

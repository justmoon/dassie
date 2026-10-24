//! A registrar's side of names (spec/areas/network.md#names), over the
//! peer protocol, on a small simulated network that checks the nodes'
//! invariants after every step: registrants are keys of their own that
//! send requests to the bootstrap node, node 0, as a registrant node would.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[path = "support/registrant.rs"]
mod registrant;

use std::net::IpAddr;

use dassie_identity::{NodeId, NodeKey};
use dassie_oer::{Decode, Encode};
use dassie_peer::names::Name;
use dassie_peer::sim::Network;
use dassie_peer::wire::{
    Authentication, Content, NameListResponse, NameReceipt, decode_expiry, decode_hash,
    encode_envelope,
};
use dassie_peer::{Config, Event, NameEvent};
use registrant::{Registrant, body, name, receipt};

const CONFIG: Config = Config::DEV;

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// Node 0's name list, as anyone reads it: without authentication.
fn list(network: &mut Network) -> NameListResponse {
    let bytes = Content::NameListRequest.to_vec();
    let stranger = NodeKey::from_bytes(&[99; 32]).node_id();
    let request = encode_envelope(&stranger, &Authentication::None, &bytes);
    let list = NameListResponse::decode_exact(&body(&network.inbound(0, &request))).unwrap();
    let request = encode_envelope(
        &stranger,
        &Authentication::None,
        &Content::NameListHashRequest.to_vec(),
    );
    let hash = decode_hash(&body(&network.inbound(0, &request))).unwrap();
    let digest: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(list.to_vec()).into();
    assert_eq!(hash, digest, "the hash is the list's");
    list
}

fn holder(network: &mut Network, name: &Name) -> Option<NodeId> {
    list(network).0.get(name).map(|holding| holding.holder)
}

/// Crashes node 0 and starts it again, and lets time pass: a node that
/// just started refuses messages dated before its start.
fn restart(network: &mut Network) {
    network.crash(0);
    network.start(0);
    settle(network, 100);
}

/// Runs the network until node 0's clock is at `at_ms`.
fn until(network: &mut Network, at_ms: u64) {
    let wait = at_ms.saturating_sub(network.clock(0));
    settle(network, wait);
}

/// Runs the network past the end of the window `receipt` names.
fn close(network: &mut Network, receipt: &NameReceipt) {
    until(network, receipt.window_end_ms + 1);
}

// spec/designs/names.md#4-the-rule-at-each-registrar: of two registrants
// that commit to one name, the one logged first wins, though it unseals
// last; nothing is listed until its window closes, and a commitment sent
// again gets its receipt again.
#[test]
fn the_commitment_logged_first_wins() {
    let mut network = Network::new(1, 2, 1, CONFIG);
    settle(&mut network, 10_000);
    let (mut alice, mut bob) = (Registrant::new(71), Registrant::new(72));
    let alice_name = name("alice");
    let first = receipt(&alice.commit(&mut network, &alice_name));
    settle(&mut network, 1_000);
    let second = receipt(&bob.commit(&mut network, &alice_name));
    assert!(second.position > first.position);
    assert_eq!(receipt(&alice.commit(&mut network, &alice_name)), first);
    assert!(body(&bob.unseal(&mut network, &alice_name)).is_empty());
    settle(&mut network, 1_000);
    assert!(body(&alice.unseal(&mut network, &alice_name)).is_empty());
    assert!(body(&alice.unseal(&mut network, &alice_name)).is_empty());
    settle(&mut network, CONFIG.names.window_ms / 2);
    assert_eq!(holder(&mut network, &alice_name), None, "nothing shows yet");
    close(&mut network, &first);
    assert_eq!(holder(&mut network, &alice_name), Some(alice.id()));
    close(&mut network, &second);
    assert_eq!(holder(&mut network, &alice_name), Some(alice.id()));
    let events: Vec<&Event> = network
        .events
        .iter()
        .filter(|(at, _)| *at == 0)
        .map(|(_, event)| event)
        .collect();
    // The registrar's timer reports what it decided in counts: the list
    // above says who holds the name.
    let (mut granted, mut lost) = (0, 0);
    for event in events {
        if let Event::Name(NameEvent::RegistrarChanged {
            granted: g,
            lost: l,
            ..
        }) = event
        {
            granted += g;
            lost += l;
        }
    }
    assert_eq!((granted, lost), (1, 1), "alice's granted, bob's lost");
    assert_eq!(network.node(0).stats().names_granted, 1);
    assert_eq!(network.node(0).stats().commitments_logged, 2);
}

// An unsealing that is not valid is refused as not acted upon: another
// node's commitment, a reserved name, a closed window, or a name held when
// the commitment was logged.
#[test]
fn invalid_unsealings_are_refused() {
    let mut network = Network::new(2, 2, 1, CONFIG);
    settle(&mut network, 10_000);
    let (mut alice, mut bob) = (Registrant::new(71), Registrant::new(72));
    let (alice_name, www) = (name("alice"), name("www"));
    let won = receipt(&alice.commit(&mut network, &alice_name));
    receipt(&alice.commit(&mut network, &www));
    assert_eq!(bob.unseal(&mut network, &alice_name).status, 409);
    assert_eq!(alice.unseal(&mut network, &www).status, 409);
    body(&alice.unseal(&mut network, &alice_name));
    close(&mut network, &won);
    // Logged while the name is listed: void.
    receipt(&bob.commit(&mut network, &alice_name));
    let reply = bob.unseal(&mut network, &alice_name);
    assert_eq!(reply.status, 409);
    assert_eq!(
        reply.body,
        b"the name was taken when the commitment was logged"
    );
    // Too late.
    let late = receipt(&alice.commit(&mut network, &name("carol")));
    close(&mut network, &late);
    assert_eq!(alice.unseal(&mut network, &name("carol")).status, 409);
    assert_eq!(holder(&mut network, &alice_name), Some(alice.id()));
}

// spec/designs/names.md#4-the-rule-at-each-registrar: a receipt is given
// only for a commitment on disk, so a crash loses none, and the log's
// positions and times only rise across it.
#[test]
fn a_receipted_commitment_survives_a_crash() {
    let mut network = Network::new(3, 2, 1, CONFIG);
    settle(&mut network, 10_000);
    let mut alice = Registrant::new(71);
    let alice_name = name("alice");
    let first = receipt(&alice.commit(&mut network, &alice_name));
    network.crash(0);
    settle(&mut network, 1_000);
    restart(&mut network);
    assert_eq!(receipt(&alice.commit(&mut network, &alice_name)), first);
    body(&alice.unseal(&mut network, &alice_name));
    let mut bob = Registrant::new(72);
    let later = receipt(&bob.commit(&mut network, &name("bob")));
    assert!(later.position > first.position && later.window_end_ms > first.window_end_ms);
    // The unsealing is on disk too.
    restart(&mut network);
    close(&mut network, &first);
    assert_eq!(holder(&mut network, &alice_name), Some(alice.id()));
    // And the name.
    restart(&mut network);
    assert_eq!(holder(&mut network, &alice_name), Some(alice.id()));
    let head = network.node(0).registrar().unwrap().head();
    assert_eq!(head.next_position, later.position + 1);
}

// spec/designs/names.md#5-holding-a-name: a name leaves the list when it
// expires, its holder alone renews it, in its grace period too, and once
// its grace ends unrenewed it is free.
#[test]
fn a_name_expires_unless_its_holder_renews_it() {
    let mut network = Network::new(4, 2, 1, CONFIG);
    settle(&mut network, 10_000);
    let (mut alice, mut bob) = (Registrant::new(71), Registrant::new(72));
    let alice_name = name("alice");
    let won = receipt(&alice.commit(&mut network, &alice_name));
    body(&alice.unseal(&mut network, &alice_name));
    close(&mut network, &won);
    let expires = list(&mut network).0[&alice_name].expires_at_ms;
    // Early: nothing moves.
    let renewed = decode_expiry(&body(&alice.renew(&mut network, &alice_name))).unwrap();
    assert_eq!(renewed, expires);
    assert_eq!(bob.renew(&mut network, &alice_name).status, 409);
    // In its grace: out of the list, and renewed from now.
    until(&mut network, expires + 1);
    assert_eq!(holder(&mut network, &alice_name), None);
    let renewed = decode_expiry(&body(&alice.renew(&mut network, &alice_name))).unwrap();
    assert_eq!(renewed, network.clock(0) + CONFIG.names.ttl_ms);
    assert_eq!(holder(&mut network, &alice_name), Some(alice.id()));
    // Unrenewed, it is freed after its grace, and someone else may have it.
    let free_at = renewed + CONFIG.names.grace_ms;
    until(&mut network, free_at + 1);
    assert_eq!(alice.renew(&mut network, &alice_name).status, 409);
    let taken = receipt(&bob.commit(&mut network, &alice_name));
    body(&bob.unseal(&mut network, &alice_name));
    close(&mut network, &taken);
    assert_eq!(holder(&mut network, &alice_name), Some(bob.id()));
    assert_eq!(network.node(0).stats().names_freed, 1);
}

// Commitments are free, so throttled (spec/areas/network.md#names): a cap
// per node and per network answered 409, the rate 429, and a commitment in
// the log sent again costs nothing.
#[test]
fn commitments_are_throttled() {
    let mut network = Network::new(5, 2, 1, CONFIG);
    settle(&mut network, 10_000);
    let names = CONFIG.names;
    let mut alice = Registrant::new(71);
    for index in 0..names.max_commitments_per_node {
        let reply = alice.commit(&mut network, &name(&format!("alice{index}")));
        receipt(&reply);
    }
    let reply = alice.commit(&mut network, &name("alice-more"));
    assert_eq!(reply.status, 409);
    let reply = alice.commit(&mut network, &name("alice0"));
    receipt(&reply);
    // One network: its cap.
    let from = Some(IpAddr::from([198, 51, 100, 7]));
    let mut statuses = Vec::new();
    for seed in 0..=names.max_commitments_per_network {
        let mut registrant = Registrant::new(110 + u8::try_from(seed).unwrap());
        let content = Content::NameCommit(registrant.commitment(&name("shared")));
        statuses.push(registrant.send(&mut network, 0, &content, from).status);
    }
    let (taken, refused) = statuses.split_at(names.max_commitments_per_network);
    assert!(taken.iter().all(|status| *status == 200), "{statuses:?}");
    assert_eq!(refused, [409]);
    // Everyone: the rate.
    let logged = names.max_commitments_per_node + names.max_commitments_per_network;
    let left = names.commit_rate.burst as usize - logged;
    for seed in 0..=left {
        let mut registrant = Registrant::new(150 + u8::try_from(seed).unwrap());
        let status = registrant.commit(&mut network, &name("rated")).status;
        assert_eq!(status, if seed < left { 200 } else { 429 }, "{seed}");
    }
}

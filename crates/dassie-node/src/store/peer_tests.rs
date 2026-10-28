#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::IdentityKey;
use dassie_peer::wire::{SchemeId, TerminationReason};
use dassie_peer::{
    Channel, Closed, Method, Outgoing, PeerBatch, Peering, Registration, Sources, StoredChannel,
    StoredClosed, StoredNode, Termination,
};

use super::*;
use crate::store::{BtpTokenRow, LedgerBatch};

fn node(seed: u8) -> StoredNode {
    StoredNode {
        id: IdentityKey::from_bytes(&[seed; 32]).node_id(),
        link_state: Some(vec![seed; 40]),
        sources: Sources::from_bits(Sources::MAJORITY.bits() | Sources::PEER.bits()),
        first_seen_ms: 10,
        registration: Some(Registration {
            registered_ms: 11,
            renewed_ms: 12,
            expires_ms: 13,
            endpoint: dassie_peer::wire::Endpoint::of_url("https://a.example").unwrap(),
            network: Some(dassie_peer::Network([4, 10, 0, 0, 0, 0, 0])),
        }),
        peering: Some(Peering {
            methods: vec![Method {
                scheme: SchemeId::parse("stub").unwrap(),
                state: vec![1, 2],
                credit_out: 900,
                short_for_ms: 17,
                short_reached_ms: 12,
                short_checked_ms: None,
                winding: None,
                unpayable_since_ms: None,
            }],
            since_ms: 14,
            inbound: true,
            last_seen_ms: 15,
            // Every other node is one we are ending, and have not told
            // (spec/decisions/ending-a-peering.md).
            depeering: seed
                .is_multiple_of(2)
                .then(|| dassie_peer::Depeering::new(18, TerminationReason::ShortOfRoom)),
        }),
        termination: None,
    }
}

fn commit(store: &Store, peer: PeerBatch) {
    let batch = LedgerBatch {
        peer,
        ..LedgerBatch::default()
    };
    store.commit_ledger(&batch).unwrap();
}

#[test]
fn the_node_table_round_trips() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.load_peer().unwrap().own_sequence, 0);
    let mut bare = node(2);
    bare.registration = None;
    bare.peering = None;
    bare.link_state = None;
    commit(
        &store,
        PeerBatch {
            nodes: vec![node(1), bare.clone()],
            removed: Vec::new(),
            own_sequence: Some(1_800_000_000_000),
            ..PeerBatch::default()
        },
    );
    let stored = store.load_peer().unwrap();
    assert_eq!(stored.own_sequence, 1_800_000_000_000);
    let mut expected = vec![node(1), bare];
    expected.sort_by_key(|n| n.id);
    assert_eq!(stored.nodes, expected);
}

/// A method on `scheme`, with `credit_out`.
fn method(scheme: &str, credit_out: u64) -> Method {
    Method {
        scheme: SchemeId::parse(scheme).unwrap(),
        state: scheme.as_bytes().to_vec(),
        credit_out,
        short_for_ms: credit_out / 100,
        short_reached_ms: credit_out / 200,
        short_checked_ms: None,
        winding: None,
        unpayable_since_ms: None,
    }
}

// spec/decisions/settlement-methods.md: a peering's methods round-trip in
// the order the pair agreed them, each with what is its own; one dropped
// goes, and the others keep their order.
#[test]
fn a_peerings_methods_round_trip_in_order() {
    let store = Store::open_in_memory().unwrap();
    let mut peered = node(1);
    let peering = peered.peering.as_mut().unwrap();
    peering.methods.push(method("zz-second", 1_100));
    peering.methods.push(method("aa-third", 1_200));
    commit(
        &store,
        PeerBatch {
            nodes: vec![peered.clone()],
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.load_peer().unwrap().nodes, [peered.clone()]);
    peered.peering.as_mut().unwrap().methods.remove(1);
    commit(
        &store,
        PeerBatch {
            nodes: vec![peered.clone()],
            ..PeerBatch::default()
        },
    );
    let loaded = store.load_peer().unwrap();
    let schemes: Vec<&str> = loaded.nodes[0]
        .peering
        .as_ref()
        .unwrap()
        .methods
        .iter()
        .map(|method| method.scheme.as_str())
        .collect();
    assert_eq!(schemes, ["stub", "aa-third"]);
    assert_eq!(loaded.nodes, [peered]);
}

// A method's time short of room while reached is never more than its time
// short: the table refuses one that is.
#[test]
fn the_table_refuses_a_method_reached_longer_than_short() {
    let store = Store::open_in_memory().unwrap();
    let mut peered = node(1);
    let first = &mut peered.peering.as_mut().unwrap().methods[0];
    first.short_reached_ms = first.short_for_ms + 1;
    let batch = LedgerBatch {
        peer: PeerBatch {
            nodes: vec![peered],
            ..PeerBatch::default()
        },
        ..LedgerBatch::default()
    };
    assert!(store.commit_ledger(&batch).is_err());
    assert!(store.load_peer().unwrap().nodes.is_empty());
}

// A peering ended, or a node removed, takes its peering row with it.
#[test]
fn peerings_end_with_their_nodes() {
    let store = Store::open_in_memory().unwrap();
    commit(
        &store,
        PeerBatch {
            nodes: vec![node(1), node(2)],
            ..PeerBatch::default()
        },
    );
    let mut unpeered = node(1);
    unpeered.peering = None;
    commit(
        &store,
        PeerBatch {
            nodes: vec![unpeered],
            removed: vec![node(2).id],
            own_sequence: None,
            ..PeerBatch::default()
        },
    );
    let stored = store.load_peer().unwrap();
    assert_eq!(stored.nodes.len(), 1);
    assert_eq!(stored.nodes[0].peering, None);
    for table in ["peers", "peer_methods"] {
        let rows: i64 = store
            .lock()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
}

// The peer rows commit with the ledger's, or not at all: a peering and
// the accounts it opened land together.
#[test]
fn peer_rows_share_the_ledgers_transaction() {
    let store = Store::open_in_memory().unwrap();
    let batch = LedgerBatch {
        peer: PeerBatch {
            nodes: vec![node(1)],
            ..PeerBatch::default()
        },
        tokens_created: vec![BtpTokenRow {
            id: 0,
            token_hash: [1; 32],
            label: String::new(),
            ledger: dassie_ledger::LedgerId::StubUsd,
            created_at_ms: 1,
            revoked_at_ms: None,
            cap: None,
            cap_usage: dassie_connector::CapUsage::default(),
        }],
        ..LedgerBatch::default()
    };
    assert!(store.commit_ledger(&batch).is_err());
    assert!(store.load_peer().unwrap().nodes.is_empty());
}

// Found in review: a node removed and added again in one batch was
// written, then deleted.
#[test]
fn a_node_removed_and_added_in_one_batch_stays() {
    let store = Store::open_in_memory().unwrap();
    commit(
        &store,
        PeerBatch {
            nodes: vec![node(1)],
            ..PeerBatch::default()
        },
    );
    commit(
        &store,
        PeerBatch {
            nodes: vec![node(1)],
            removed: vec![node(1).id],
            own_sequence: None,
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.load_peer().unwrap().nodes, [node(1)]);
}

// A peering being ended, settlement channels with a message on its way, and
// bans survive a restart (spec/areas/network.md#persistence). A channel stays
// when its node leaves the table.
#[test]
fn terminations_channels_and_bans_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let mut ending = node(1);
    ending.termination = Some(Termination {
        since_ms: 20,
        reason: TerminationReason::Owner,
        by_us: true,
        acknowledged: false,
    });
    let key = IdentityKey::from_bytes(&[3; 32]);
    let channel = StoredChannel {
        node: key.node_id(),
        channel: Channel {
            key: key.public_key(),
            endpoint: dassie_peer::wire::Endpoint::of_url("https://c.example"),
            settled_in: 4,
            settled_out: 7,
            outbox: Some(Outgoing {
                sequence: 7,
                scheme: SchemeId::parse("stub").unwrap(),
                amount: 500,
                data: vec![9],
                attempts: 2,
            }),
            paid_us: true,
        },
    };
    commit(
        &store,
        PeerBatch {
            nodes: vec![ending.clone()],
            channels: vec![channel.clone()],
            banned: vec![node(5).id, node(6).id],
            ..PeerBatch::default()
        },
    );
    let stored = store.load_peer().unwrap();
    assert_eq!(stored.nodes, [ending]);
    assert_eq!(stored.channels, std::slice::from_ref(&channel));
    let mut bans = vec![node(5).id, node(6).id];
    bans.sort();
    assert_eq!(stored.bans, bans);
    // The termination ends, the node leaves, a ban is lifted, and the
    // message arrives: the channel stays, without it.
    let mut delivered = channel;
    delivered.channel.outbox = None;
    commit(
        &store,
        PeerBatch {
            removed: vec![node(1).id],
            channels: vec![delivered.clone()],
            unbanned: vec![node(5).id],
            ..PeerBatch::default()
        },
    );
    let stored = store.load_peer().unwrap();
    assert!(stored.nodes.is_empty());
    assert_eq!(stored.channels, [delivered]);
    assert_eq!(stored.bans, [node(6).id]);
    let terminations: i64 = store
        .lock()
        .query_row("SELECT count(*) FROM peer_terminations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(terminations, 0);
}

// A closed channel stays on disk with its sequences and whether the node
// paid us, and leaves what a restart loads; a late settlement moves its
// sequence, and opening it again writes it open
// (spec/decisions/closing-channels.md).
#[test]
fn a_closed_channel_stays_on_disk() {
    let store = Store::open_in_memory().unwrap();
    let key = IdentityKey::from_bytes(&[4; 32]);
    let open = StoredChannel {
        node: key.node_id(),
        channel: Channel {
            key: key.public_key(),
            endpoint: dassie_peer::wire::Endpoint::of_url("https://d.example"),
            settled_in: 2,
            settled_out: 5,
            outbox: None,
            paid_us: true,
        },
    };
    commit(
        &store,
        PeerBatch {
            channels: vec![open.clone()],
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.closed_channel(&key.node_id()).unwrap(), None);
    commit(
        &store,
        PeerBatch {
            closed: vec![StoredClosed {
                node: key.node_id(),
                closed: open.channel.closed(),
            }],
            ..PeerBatch::default()
        },
    );
    assert!(store.load_peer().unwrap().channels.is_empty());
    assert_eq!(
        store.closed_channel(&key.node_id()).unwrap(),
        Some(open.channel.closed())
    );
    let late = Closed {
        settled_in: 3,
        ..open.channel.closed()
    };
    commit(
        &store,
        PeerBatch {
            closed: vec![StoredClosed {
                node: key.node_id(),
                closed: late,
            }],
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.closed_channel(&key.node_id()).unwrap(), Some(late));
    let reopened = StoredChannel {
        node: key.node_id(),
        channel: Channel::reopen(
            key.public_key(),
            dassie_peer::wire::Endpoint::of_url("https://d.example"),
            late,
        ),
    };
    commit(
        &store,
        PeerBatch {
            channels: vec![reopened.clone()],
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.closed_channel(&key.node_id()).unwrap(), None);
    assert_eq!(store.load_peer().unwrap().channels, [reopened]);
}

// Closing a channel the disk never held is a broken invariant: the batch
// is refused, and nothing is written.
#[test]
fn closing_a_channel_without_its_row_is_refused() {
    let store = Store::open_in_memory().unwrap();
    let batch = LedgerBatch {
        peer: PeerBatch {
            closed: vec![StoredClosed {
                node: IdentityKey::from_bytes(&[5; 32]).node_id(),
                closed: Closed::NEW,
            }],
            ..PeerBatch::default()
        },
        ..LedgerBatch::default()
    };
    assert!(store.commit_ledger(&batch).is_err());
}

fn logged(seed: u8, position: u64, unsealed: bool) -> dassie_peer::names::Logged {
    use dassie_peer::names::{Commitment, Logged, Name, Unsealing};
    let holder = IdentityKey::from_bytes(&[seed; 32]).node_id();
    Logged {
        commitment: Commitment([seed; 32]),
        position,
        logged_ms: 1_000 + position,
        window_end_ms: 2_000 + position,
        committer: holder,
        network: seed
            .is_multiple_of(2)
            .then_some(dassie_peer::Network([6, 32, 1, 13, 184, 0, 1])),
        unsealing: unsealed.then(|| Unsealing {
            name: Name::parse("alice").unwrap(),
            holder,
            contends: seed.is_multiple_of(2),
        }),
    }
}

fn granted(name: &str, freed_ms: Option<u64>) -> dassie_peer::names::Granted {
    dassie_peer::names::Granted {
        name: dassie_peer::names::Name::parse(name).unwrap(),
        holder: IdentityKey::from_bytes(&[9; 32]).node_id(),
        granted_ms: 3_000,
        expires_ms: 4_000,
        freed_ms,
    }
}

// A registrar's log and names come back as written
// (spec/areas/network.md#names), and a batch's removals go first, so a
// commitment that left the log and was logged again in one batch stays.
#[test]
fn a_registrars_log_and_names_round_trip() {
    use dassie_peer::names::LogHead;
    let store = Store::open_in_memory().unwrap();
    let empty = store.load_peer().unwrap();
    assert_eq!(empty.name_log, LogHead::default());
    assert!(empty.commitments.is_empty() && empty.names.is_empty());
    let head = LogHead {
        next_position: 9,
        last_logged_ms: 1_008,
    };
    let (first, second) = (logged(1, 7, false), logged(2, 8, true));
    let (held, freed) = (granted("alice", None), granted("bob", Some(5_000)));
    commit(
        &store,
        PeerBatch {
            name_log: Some(head),
            commitments: vec![first, second],
            names: vec![held, freed],
            ..PeerBatch::default()
        },
    );
    let stored = store.load_peer().unwrap();
    assert_eq!(stored.name_log, head);
    assert_eq!(stored.commitments, [first, second]);
    assert_eq!(stored.names, [held, freed]);
    let again = logged(1, 9, true);
    commit(
        &store,
        PeerBatch {
            commitments_removed: vec![first.commitment, second.commitment],
            commitments: vec![again],
            names_removed: vec![freed.name],
            ..PeerBatch::default()
        },
    );
    let stored = store.load_peer().unwrap();
    assert_eq!(stored.commitments, [again]);
    assert_eq!(stored.names, [held]);
}

// The node's view of who holds each name comes back as written
// (spec/areas/network.md#holders), a batch's removals first.
#[test]
fn the_holders_round_trip() {
    use dassie_peer::names::{HeldName, Name};
    let store = Store::open_in_memory().unwrap();
    assert!(store.load_peer().unwrap().holders.is_empty());
    let held = |name: &str, seed: u8| HeldName {
        name: Name::parse(name).unwrap(),
        holder: IdentityKey::from_bytes(&[seed; 32]).node_id(),
        expires_ms: 4_000 + u64::from(seed),
    };
    let (alice, bob) = (held("alice", 1), held("bob", 2));
    commit(
        &store,
        PeerBatch {
            holders: vec![alice, bob],
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.load_peer().unwrap().holders, [alice, bob]);
    let moved = held("alice", 3);
    commit(
        &store,
        PeerBatch {
            holders_removed: vec![alice.name, bob.name],
            holders: vec![moved],
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.load_peer().unwrap().holders, [moved]);
}

// The node's registrations come back as written, salt and all
// (spec/areas/network.md#the-registrant), a batch's removals first, so a
// registration dropped and started again in one batch stays.
#[test]
fn the_registrations_round_trip() {
    use dassie_peer::names::{Name, NameRegistration, Salt, Stage};
    let store = Store::open_in_memory().unwrap();
    assert!(store.load_peer().unwrap().registrations.is_empty());
    let registration = |name: &str, seed: u8, stage| NameRegistration {
        name: Name::parse(name).unwrap(),
        salt: Salt([seed; 32]),
        started_ms: 5_000 + u64::from(seed),
        stage,
    };
    let alice = registration("alice", 1, Stage::Committing);
    let bob = registration("bob", 2, Stage::Abandoned);
    let carol = registration("carol", 3, Stage::Unsealing);
    commit(
        &store,
        PeerBatch {
            registrations: vec![alice, bob, carol],
            ..PeerBatch::default()
        },
    );
    assert_eq!(
        store.load_peer().unwrap().registrations,
        [alice, bob, carol]
    );
    let again = registration("bob", 4, Stage::Committing);
    commit(
        &store,
        PeerBatch {
            registrations_removed: vec![alice.name, bob.name],
            registrations: vec![again],
            ..PeerBatch::default()
        },
    );
    assert_eq!(store.load_peer().unwrap().registrations, [again, carol]);
}

// spec/decisions/settlement-methods.md: a method being wound down keeps when
// the node decided, and only that: its steps are taken again after a
// restart.
#[test]
fn a_method_wound_down_keeps_its_decision() {
    let store = Store::open_in_memory().unwrap();
    let mut peered = node(1);
    let peering = peered.peering.as_mut().unwrap();
    let mut second = method("zz-second", 1_100);
    second.winding = Some(dassie_peer::Winding {
        since_ms: 30,
        asked_ms: Some(31),
        promised: Some(5),
        booked: 2,
    });
    peering.methods.push(second);
    commit(
        &store,
        PeerBatch {
            nodes: vec![peered.clone()],
            ..PeerBatch::default()
        },
    );
    let loaded = store.load_peer().unwrap();
    let methods = &loaded.nodes[0].peering.as_ref().unwrap().methods;
    assert_eq!(methods[0].winding, None);
    assert_eq!(methods[1].winding, Some(dassie_peer::Winding::new(30)));
}

// spec/decisions/settlement-methods.md: a scheme its owner removes keeps since
// when, so a restart carries the removal on; one removed for good, or added
// again, is not being removed.
#[test]
fn a_scheme_being_removed_keeps_since_when() {
    let store = Store::open_in_memory().unwrap();
    let (stub, two) = (
        dassie_ledger::Name::new("stub").unwrap(),
        dassie_ledger::Name::new("stub-two").unwrap(),
    );
    store
        .commit_ledger(&LedgerBatch {
            schemes_added: vec![(stub, "{}".into()), (two, "{}".into())],
            schemes_removing: vec![(two, 40)],
            ..LedgerBatch::default()
        })
        .unwrap();
    assert_eq!(store.removing_schemes().unwrap(), [(two, 40)]);
    store
        .commit_ledger(&LedgerBatch {
            schemes_removed: vec![two],
            ..LedgerBatch::default()
        })
        .unwrap();
    assert!(store.removing_schemes().unwrap().is_empty());
    store
        .commit_ledger(&LedgerBatch {
            schemes_added: vec![(two, "{}".into())],
            ..LedgerBatch::default()
        })
        .unwrap();
    assert!(store.removing_schemes().unwrap().is_empty());
    assert_eq!(store.settlement_schemes().unwrap().len(), 2);
}

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::NodeKey;
use dassie_peer::wire::{SchemeId, TerminationReason};
use dassie_peer::{
    Channel, Outgoing, PeerBatch, Peering, Registration, Sources, StoredChannel, StoredNode,
    Termination,
};

use super::*;
use crate::store::{BtpTokenRow, LedgerBatch};

fn node(seed: u8) -> StoredNode {
    StoredNode {
        id: NodeKey::from_bytes(&[seed; 32]).node_id(),
        link_state: Some(vec![seed; 40]),
        sources: Sources::from_bits(Sources::MAJORITY.bits() | Sources::PEER.bits()),
        first_seen_ms: 10,
        registration: Some(Registration {
            registered_ms: 11,
            renewed_ms: 12,
            expires_ms: 13,
            url: "https://a.example".into(),
            network: Some(dassie_peer::Network([4, 10, 0, 0, 0, 0, 0])),
        }),
        peering: Some(Peering {
            scheme: SchemeId::parse("stub").unwrap(),
            version: 1,
            state: vec![1, 2],
            credit_in: 1_100_000_000,
            credit_out: 900,
            since_ms: 14,
            inbound: true,
            last_seen_ms: 15,
        }),
        termination: None,
        replay_floor_ms: 16,
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
    let mut expected = vec![
        node(1),
        StoredNode {
            replay_floor_ms: 0,
            ..bare
        },
    ];
    expected.sort_by_key(|n| n.id);
    assert_eq!(stored.nodes, expected);
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
    let peers: i64 = store
        .lock()
        .query_row("SELECT count(*) FROM peers", [], |row| row.get(0))
        .unwrap();
    assert_eq!(peers, 0);
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

// A peering being ended, settlement channels with a message on its way,
// and bans survive a restart (spec 7 §4.4). A channel stays when its node
// leaves the table.
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
    let key = NodeKey::from_bytes(&[3; 32]);
    let channel = StoredChannel {
        node: key.node_id(),
        channel: Channel {
            key: key.public_key(),
            url: "https://c.example".into(),
            settled_in: 4,
            settled_out: 7,
            outbox: Some(Outgoing {
                sequence: 7,
                scheme: SchemeId::parse("stub").unwrap(),
                amount: 500,
                data: vec![9],
                attempts: 2,
            }),
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

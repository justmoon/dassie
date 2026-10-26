#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::NodeKey;
use dassie_peer::wire::{SchemeId, TerminationReason};
use dassie_peer::{
    Channel, Closed, Outgoing, PeerBatch, Peering, Registration, Sources, StoredChannel,
    StoredClosed, StoredNode, Termination,
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
            state: vec![1, 2],
            credit_out: 900,
            since_ms: 14,
            inbound: true,
            last_seen_ms: 15,
            short_for_ms: 17,
            short_reached_ms: 12,
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

// Migration 24 keeps how long a peer has been short of room: all of it
// counted toward ending the peering before, so all of it was reached, and
// an upgrade forgives nothing (spec/decisions/peering-lifecycle.md).
#[test]
fn the_time_short_is_all_reached_after_the_upgrade() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 23).unwrap();
    let id = node(1).id;
    connection
        .execute(
            "INSERT INTO peer_nodes (id, sources, first_seen_ms) VALUES (?1, 1, 10)",
            [id.as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO peers (node, scheme, version, state, credit_out, since_ms, inbound,
               last_seen_ms, replay_floor_ms, short_for_ms)
             VALUES (?1, 'stub', 1, x'', 900, 14, 0, 15, 16, 5000)",
            [id.as_str()],
        )
        .unwrap();
    drop(connection);
    let store = Store::open(temp.path()).unwrap();
    let peering = store.load_peer().unwrap().nodes[0].peering.clone().unwrap();
    assert_eq!(
        (peering.short_for_ms, peering.short_reached_ms),
        (5000, 5000)
    );
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
            ended_for_room: true,
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

// A closed channel stays on disk with its sequences and its mark of an end
// for room, and leaves what a restart loads; a late settlement moves its
// sequence, and opening it again writes it open
// (spec/decisions/closing-channels.md).
#[test]
fn a_closed_channel_stays_on_disk() {
    let store = Store::open_in_memory().unwrap();
    let key = NodeKey::from_bytes(&[4; 32]);
    let open = StoredChannel {
        node: key.node_id(),
        channel: Channel {
            key: key.public_key(),
            url: "https://d.example".into(),
            settled_in: 2,
            settled_out: 5,
            outbox: None,
            ended_for_room: true,
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
        channel: Channel::reopen(key.public_key(), "https://d.example".into(), late),
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
                node: NodeKey::from_bytes(&[5; 32]).node_id(),
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
    let holder = NodeKey::from_bytes(&[seed; 32]).node_id();
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
        holder: NodeKey::from_bytes(&[9; 32]).node_id(),
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

// Migration 37 gives a node that ran before names an empty log at its
// first position: positions and times rise from there.
#[test]
fn migration_37_starts_with_an_empty_log() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 36).unwrap();
    drop(connection);
    let stored = Store::open(temp.path()).unwrap().load_peer().unwrap();
    assert_eq!(stored.name_log, dassie_peer::names::LogHead::default());
    assert!(stored.commitments.is_empty() && stored.names.is_empty());
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
        holder: NodeKey::from_bytes(&[seed; 32]).node_id(),
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

// Migration 38 gives a node that ran before holders an empty view, which
// it takes from the lists once a quorum of them are fresh.
#[test]
fn migration_38_starts_with_no_holders() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 37).unwrap();
    drop(connection);
    let stored = Store::open(temp.path()).unwrap().load_peer().unwrap();
    assert!(stored.holders.is_empty());
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

// Migration 39 gives a node that ran before registrants no registrations.
#[test]
fn migration_39_starts_with_no_registrations() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 38).unwrap();
    drop(connection);
    let stored = Store::open(temp.path()).unwrap().load_peer().unwrap();
    assert!(stored.registrations.is_empty());
}

// Migration 41 finds the accounts a node opened for its downlinks by their
// settlement channels, whose URL is empty for a downlink, on every ledger
// (spec/decisions/account-budgets.md); a core peer's stay the core peers'.
#[test]
fn migration_41_finds_the_downlinks_accounts_by_their_channels() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 40).unwrap();
    let (downlink, core) = (node(1).id, node(2).id);
    for (id, path) in [
        (0, format!("stub+usd:assets/interledger/{downlink}")),
        (1, format!("stub+usd:contra/trust/{downlink}")),
        (2, format!("xrpl-testnet+xrp:assets/interledger/{downlink}")),
        (3, format!("xrpl-testnet+xrp:contra/trust/{downlink}")),
        (4, format!("stub+usd:assets/interledger/{core}")),
        (5, format!("stub+usd:contra/trust/{core}")),
    ] {
        connection
            .execute(
                "INSERT INTO ledger_accounts (id, path) VALUES (?1, ?2)",
                rusqlite::params![id, path],
            )
            .unwrap();
    }
    for (node, url, closed) in [(downlink, "", 1), (core, "https://a.example", 0)] {
        connection
            .execute(
                "INSERT INTO settlement_channels (node, public_key, url, settled_in, settled_out, \
                 closed) VALUES (?1, ?2, ?3, 0, 0, ?4)",
                rusqlite::params![node.as_str(), [0u8; 32].as_slice(), url, closed],
            )
            .unwrap();
    }
    drop(connection);
    let name = dassie_ledger::Name::new(downlink.as_str()).unwrap();
    assert_eq!(
        Store::open(temp.path())
            .unwrap()
            .downlink_accounts()
            .unwrap(),
        [
            (dassie_ledger::LedgerId::StubUsd, name),
            (dassie_ledger::LedgerId::XrplTestnetXrp, name)
        ]
    );
}

// Migration 42 renames the testnet hub's scheme and its ledger
// (spec/areas/hubs.md#the-members-side): a node that peered over the hub
// before loads the same peering, the same balances and the same pending
// payment out of the hub afterwards, under the new names, and its payments
// and grants follow the ledger's wallet address.
#[test]
#[allow(clippy::too_many_lines)]
fn migration_42_renames_the_hub_scheme_and_keeps_its_books() {
    const OLD_LEDGER: &str = "hub-xrp-testnet+xrp";
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 41).unwrap();
    let peer = node(1);
    let id = peer.id;
    let key = NodeKey::from_bytes(&[1; 32]).public_key();
    connection
        .execute(
            "INSERT INTO peer_nodes (id, link_state, sources, first_seen_ms) VALUES (?1, ?2, ?3, 10)",
            rusqlite::params![id.as_str(), peer.link_state, peer.sources.bits()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO peers (node, scheme, version, state, credit_out, since_ms, inbound, \
             last_seen_ms, short_for_ms, short_reached_ms) \
             VALUES (?1, 'hub-xrp-testnet', 1, X'0102', 900, 14, 1, 15, 0, 0)",
            [id.as_str()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO settlement_channels (node, public_key, url, settled_in, settled_out, \
             outbox_sequence, outbox_scheme, outbox_amount, outbox_data, outbox_attempts) \
             VALUES (?1, ?2, 'https://a.example', 0, 1, 1, 'hub-xrp-testnet', 300, X'01', 0)",
            rusqlite::params![id.as_str(), key.as_bytes().as_slice()],
        )
        .unwrap();
    let accounts = [
        (0, "stub+usd:equity/owner".to_string(), 0, 0),
        (1, format!("{OLD_LEDGER}:assets/settlement"), 5_300, 0),
        (2, format!("{OLD_LEDGER}:equity/owner"), 0, 5_000),
        (3, format!("{OLD_LEDGER}:assets/interledger/{id}"), 900, 300),
        (4, format!("{OLD_LEDGER}:contra/trust/{id}"), 0, 900),
    ];
    for (account, path, debits, credits) in &accounts {
        connection
            .execute(
                "INSERT INTO ledger_accounts (id, path, debits_posted, credits_posted) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    account,
                    path,
                    i64::try_from(*debits).unwrap(),
                    i64::try_from(*credits).unwrap()
                ],
            )
            .unwrap();
    }
    connection
        .execute_batch(
            "INSERT INTO ledger_transfers (id, debit, credit, amount, state, created_at_ms, \
               resolved_at_ms, reference, settlement_scheme, settlement_id, sequence) VALUES
             (0, 1, 2, 5000, 'posted', 1, 1, 'deposit', NULL, NULL, 1),
             (1, 3, 4, 900, 'posted', 2, 2, 'trust', NULL, NULL, 2),
             (2, 1, 3, 300, 'posted', 3, 3, 'settlement', 'hub-xrp-testnet', 's1', 3),
             (3, 2, 1, 100, 'pending', 4, NULL, 'owner_payment', 'hub-xrp-testnet', 'w1', 4);
             INSERT INTO settlement_schemes (id, config, activated_at_ms)
               VALUES ('hub-xrp-testnet', '', 1), ('stub', '', 1);
             INSERT INTO settlement_checkpoints (scheme, balance, updated_at_ms, state)
               VALUES ('hub-xrp-testnet', 5000, 4, X'0203');
             INSERT INTO btp_tokens (id, token_hash, label, ledger, created_at_ms)
               VALUES (1, zeroblob(32), 'hub client', 'hub-xrp-testnet+xrp', 1);
             UPDATE payment_settings SET default_ledger = 'hub-xrp-testnet+xrp';
             INSERT INTO incoming_payments (id, ledger, wallet_address, received_amount,
               ilp_address, shared_secret, created_at_ms, expires_at_ms, revision)
               VALUES (zeroblob(16), 'hub-xrp-testnet+xrp', 'https://a.example/pay/hub-xrp-testnet',
                 0, 'test.a.stream-hub-xrp-testnet.x', zeroblob(32), 1, 2, 1),
               (X'01010101010101010101010101010101', 'xrpl-testnet+xrp',
                 'https://a.example/pay/xrpl-testnet', 0, 'test.a.stream-xrpl-testnet.x',
                 zeroblob(32), 1, 2, 5);
             INSERT INTO quotes (id, ledger, wallet_address, receiver, destination, shared_secret,
               debit_amount, receive_amount, receive_asset_code, receive_asset_scale,
               created_at_ms, expires_at_ms)
               VALUES (zeroblob(16), 'hub-xrp-testnet+xrp', 'https://a.example/pay/hub-xrp-testnet',
                 'https://b.example/incoming-payments/1', 'test.b.x', zeroblob(32), 10, 10, 'XRP', 9,
                 1, 2);
             INSERT INTO outgoing_payments (id, ledger, wallet_address, receiver, destination,
               shared_secret, debit_amount, receive_amount, receive_asset_code,
               receive_asset_scale, sent_amount, state, created_at_ms, deadline_ms, revision)
               VALUES (zeroblob(16), 'hub-xrp-testnet+xrp', 'https://a.example/pay/hub-xrp-testnet',
                 'https://b.example/incoming-payments/1', 'test.b.x', zeroblob(32), 10, 10, 'XRP', 9,
                 0, 'sending', 1, 2, 3);
             INSERT INTO gnap_grants (id, state, client, client_key, access, needs_owner,
               continue_token_hash, created_at_ms, decided_at_ms, expires_at_ms, revision)
               VALUES (zeroblob(16), 'approved', 'https://c.example', '{}',
                 '[{\"type\":\"incoming-payment\",\"actions\":[\"read\"],\
                 \"identifier\":\"https://a.example/pay/hub-xrp-testnet\"},\
                 {\"type\":\"incoming-payment\",\"actions\":[\"read\"],\
                 \"identifier\":\"https://a.example/pay/xrpl-testnet\"}]',
                 1, zeroblob(32), 1, 1, 2, 1);
             INSERT INTO gnap_grants (id, state, client, client_key, access, needs_owner,
               continue_token_hash, interaction, created_at_ms, expires_at_ms, revision)
               VALUES (X'02020202020202020202020202020202', 'pending', 'https://c.example', '{}',
                 '[]', 1, X'0202020202020202020202020202020202020202020202020202020202020202',
                 '{\"key\":\"k\",\"server_nonce\":\"n\",\"finish\":null,\
                 \"interact_ref\":null,\"subject\":{\"sub_ids\":[{\"id\":\
                 \"https://a.example/pay/hub-xrp-testnet\",\"format\":\"uri\"}]},\
                 \"public_name\":null,\"continued_at_ms\":1,\"finalized\":false}',
                 1, 2, 2);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO downlink_accounts (ledger, node) VALUES ('hub-xrp-testnet+xrp', ?1)",
            [id.as_str()],
        )
        .unwrap();
    drop(connection);

    let store = Store::open(temp.path()).unwrap();
    let hub = dassie_ledger::LedgerId::HubXrplTestnetXrp;
    let scheme = dassie_ledger::Name::new(dassie_hub::member::SCHEME_ID).unwrap();
    assert_eq!(scheme.as_str(), "hub-xrpl-testnet-xrp");
    assert_eq!(hub.as_str(), "hub-xrpl-testnet-xrp+xrp");

    // The books: every account under the new ledger, with its balance.
    let ledger = store.load_ledger().unwrap();
    let loaded: Vec<_> = ledger
        .accounts
        .iter()
        .map(|a| {
            (
                a.id.0,
                a.path.to_string(),
                a.debits_posted,
                a.credits_posted,
            )
        })
        .collect();
    let renamed: Vec<_> = accounts
        .iter()
        .map(|(account, path, debits, credits)| {
            (
                *account,
                path.replace(OLD_LEDGER, hub.as_str()),
                *debits,
                *credits,
            )
        })
        .collect();
    assert_eq!(loaded, renamed);
    assert!(ledger.accounts[1..].iter().all(|a| a.path.ledger == hub));
    assert_eq!(ledger.pending.len(), 1);
    assert_eq!(
        ledger.pending[0].reference,
        dassie_ledger::Reference::OwnerPayment {
            scheme,
            id: dassie_ledger::name!("w1"),
        }
    );
    let settled = store
        .transfers_last(10, None)
        .unwrap()
        .into_iter()
        .find(|row| row.transfer.amount == 300)
        .unwrap();
    assert_eq!(
        settled.transfer.reference,
        dassie_ledger::Reference::Settlement {
            scheme,
            id: dassie_ledger::name!("s1"),
        }
    );

    // The scheme, its checkpoint, and the hub's ledger elsewhere.
    assert_eq!(
        store.settlement_schemes().unwrap(),
        [
            (scheme, String::new()),
            (dassie_ledger::name!("stub"), String::new())
        ]
    );
    assert_eq!(
        store.checkpoints().unwrap(),
        [(
            scheme,
            Checkpoint {
                balance: 5_000,
                state: vec![2, 3],
            }
        )]
    );
    let name = dassie_ledger::Name::new(id.as_str()).unwrap();
    assert_eq!(store.downlink_accounts().unwrap(), [(hub, name)]);
    assert_eq!(store.btp_tokens().unwrap()[0].ledger, hub);
    assert_eq!(store.default_ledger().unwrap(), Some(hub));

    // The peering, and the settlement on its way over it.
    let stored = store.load_peer().unwrap();
    let peering = stored.nodes[0].peering.as_ref().unwrap();
    assert_eq!(peering.scheme.as_str(), scheme.as_str());
    assert_eq!(
        (peering.credit_out, peering.state.as_slice()),
        (900, [1, 2].as_slice())
    );
    let outbox = stored.channels[0].channel.outbox.as_ref().unwrap();
    assert_eq!(outbox.scheme.as_str(), scheme.as_str());
    assert_eq!(outbox.amount, 300);

    // Payments and grants follow the ledger's wallet address, which
    // migration 46 then names for people, `/pay/hub-xrp`, as it names the
    // XRP ledger's `/pay/xrp`.
    let payment = store.incoming_payment(PaymentId([0; 16])).unwrap().unwrap();
    assert_eq!(payment.ledger, hub);
    assert_eq!(payment.wallet_address, "https://a.example/pay/hub-xrp");
    let other = store.incoming_payment(PaymentId([1; 16])).unwrap().unwrap();
    assert_eq!(
        (other.ledger, other.wallet_address.as_str()),
        (
            dassie_ledger::LedgerId::XrplTestnetXrp,
            "https://a.example/pay/xrp"
        )
    );
    let quote = store.quote(PaymentId([0; 16])).unwrap().unwrap();
    assert_eq!(
        (quote.ledger, quote.wallet_address.as_str()),
        (hub, "https://a.example/pay/hub-xrp")
    );
    let outgoing = store.outgoing_payment(PaymentId([0; 16])).unwrap().unwrap();
    assert_eq!(
        (outgoing.ledger, outgoing.wallet_address.as_str()),
        (hub, "https://a.example/pay/hub-xrp")
    );

    // A grant's identifiers of the hub's wallet address move, and, by
    // migration 46, the XRP ledger's.
    let grant = store.grant(PaymentId([0; 16])).unwrap().unwrap();
    let access: serde_json::Value = serde_json::from_str(&grant.access).unwrap();
    let identifiers: Vec<_> = access
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["identifier"].as_str().unwrap())
        .collect();
    assert_eq!(
        identifiers,
        ["https://a.example/pay/hub-xrp", "https://a.example/pay/xrp"]
    );
    // So does the subject a pending grant asked about, and nothing else
    // in its interaction.
    let pending = store.grant(PaymentId([2; 16])).unwrap().unwrap();
    let interaction: serde_json::Value =
        serde_json::from_str(pending.interaction.as_deref().unwrap()).unwrap();
    assert_eq!(
        interaction,
        serde_json::json!({
            "key": "k",
            "server_nonce": "n",
            "finish": null,
            "interact_ref": null,
            "subject": {"sub_ids": [{
                "id": "https://a.example/pay/hub-xrp",
                "format": "uri",
            }]},
            "public_name": null,
            "continued_at_ms": 1,
            "finalized": false,
        })
    );
    drop(store);

    // The rows that changed take revisions above every other, so live
    // views see them change: migration 42's in their order, then migration
    // 45's above them.
    let connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    let revisions = |table: &str| -> Vec<(Vec<u8>, i64)> {
        let mut statement = connection
            .prepare(&format!("SELECT id, revision FROM {table} ORDER BY id"))
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(
        revisions("incoming_payments"),
        [(vec![0; 16], 28), (vec![1; 16], 22)]
    );
    assert_eq!(revisions("outgoing_payments"), [(vec![0; 16], 8)]);
    assert_eq!(
        revisions("gnap_grants"),
        [(vec![0; 16], 14), (vec![2; 16], 11)]
    );
}

// Migration 46 names the XRP schemes by their ledger, network and asset
// (spec/decisions/scheme-names.md): a node that settled on `xrpl-testnet`
// loads the same scheme, peering, balances and settlement under
// `xrpl-testnet-xrp`; and every ledger's wallet address takes its name for
// people, in payments, quotes and grants, the others' with them.
#[test]
#[allow(clippy::too_many_lines)]
fn migration_46_names_the_xrp_schemes_and_the_wallet_addresses() {
    let temp = tempfile::tempdir().unwrap();
    let mut connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    connection
        .pragma_update(None, "application_id", APPLICATION_ID)
        .unwrap();
    schema::migrate_until(&mut connection, 45).unwrap();
    let peer = node(1);
    let id = peer.id;
    connection
        .execute(
            "INSERT INTO peer_nodes (id, link_state, sources, first_seen_ms) VALUES (?1, ?2, ?3, 10)",
            rusqlite::params![id.as_str(), peer.link_state, peer.sources.bits()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO peers (node, scheme, version, state, credit_out, since_ms, inbound, \
             last_seen_ms, short_for_ms, short_reached_ms) \
             VALUES (?1, 'xrpl-testnet', 1, X'0102', 900, 14, 1, 15, 0, 0)",
            [id.as_str()],
        )
        .unwrap();
    let accounts = [
        (0, "stub+usd:equity/owner".to_string(), 0, 0),
        (
            1,
            "xrpl-testnet+xrp:assets/settlement".to_string(),
            5_300,
            0,
        ),
        (2, "xrpl-testnet+xrp:equity/owner".to_string(), 0, 5_300),
    ];
    for (account, path, debits, credits) in &accounts {
        connection
            .execute(
                "INSERT INTO ledger_accounts (id, path, debits_posted, credits_posted) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![account, path, debits, credits],
            )
            .unwrap();
    }
    connection
        .execute_batch(
            "INSERT INTO ledger_transfers (id, debit, credit, amount, state, created_at_ms, \
               resolved_at_ms, reference, settlement_scheme, settlement_id, sequence) VALUES
             (0, 1, 2, 5300, 'posted', 1, 1, 'deposit', NULL, NULL, 1),
             (1, 2, 1, 100, 'pending', 2, NULL, 'settlement', 'xrpl-testnet', 's1', 2);
             INSERT INTO settlement_schemes (id, config, activated_at_ms)
               VALUES ('xrpl-testnet', '', 1), ('stub', '', 1);
             INSERT INTO settlement_checkpoints (scheme, balance, updated_at_ms, state)
               VALUES ('xrpl-testnet', 5300, 2, NULL);
             UPDATE payment_settings SET default_ledger = 'xrpl-testnet+xrp';
             INSERT INTO incoming_payments (id, ledger, wallet_address, received_amount,
               ilp_address, shared_secret, created_at_ms, expires_at_ms, revision)
               VALUES (zeroblob(16), 'xrpl-testnet+xrp', 'https://a.example/pay/xrpl-testnet',
                 0, 'test.a.stream-xrpl-testnet.x', zeroblob(32), 1, 2, 1),
               (X'01010101010101010101010101010101', 'stub+usd', 'https://a.example/pay/stub',
                 0, 'test.a.stream-stub.x', zeroblob(32), 1, 2, 2),
               (X'02020202020202020202020202020202', 'xrpl-testnet-dsusd+usd',
                 'https://a.example/.well-known/pay', 0, 'test.a.stream-xrpl-testnet-dsusd.x',
                 zeroblob(32), 1, 2, 3);
             INSERT INTO gnap_grants (id, state, client, client_key, access, needs_owner,
               continue_token_hash, created_at_ms, decided_at_ms, expires_at_ms, revision)
               VALUES (zeroblob(16), 'approved', 'https://c.example', '{}',
                 '[{\"type\":\"incoming-payment\",\"actions\":[\"read\"],\
                 \"identifier\":\"https://a.example/pay/xrpl-testnet-dsusd\"},\
                 {\"type\":\"incoming-payment\",\"actions\":[\"read\"],\
                 \"identifier\":\"https://a.example/pay/stub\"}]',
                 1, zeroblob(32), 1, 1, 2, 1);",
        )
        .unwrap();
    drop(connection);

    let store = Store::open(temp.path()).unwrap();
    let xrp = dassie_ledger::LedgerId::XrplTestnetXrp;
    let scheme = dassie_ledger::name!("xrpl-testnet-xrp");
    assert_eq!(xrp.as_str(), "xrpl-testnet-xrp+xrp");
    let ledger = store.load_ledger().unwrap();
    let paths: Vec<_> = ledger
        .accounts
        .iter()
        .map(|a| (a.path.to_string(), a.debits_posted, a.credits_posted))
        .collect();
    assert_eq!(
        paths,
        [
            ("stub+usd:equity/owner".to_string(), 0, 0),
            (
                "xrpl-testnet-xrp+xrp:assets/settlement".to_string(),
                5_300,
                0
            ),
            ("xrpl-testnet-xrp+xrp:equity/owner".to_string(), 0, 5_300),
        ]
    );
    assert_eq!(
        ledger.pending[0].reference,
        dassie_ledger::Reference::Settlement {
            scheme,
            id: dassie_ledger::name!("s1"),
        }
    );
    assert_eq!(
        store.settlement_schemes().unwrap(),
        [
            (dassie_ledger::name!("stub"), String::new()),
            (scheme, String::new())
        ]
    );
    assert_eq!(store.checkpoints().unwrap()[0].0, scheme);
    assert_eq!(store.default_ledger().unwrap(), Some(xrp));
    let stored = store.load_peer().unwrap();
    let peering = stored.nodes[0].peering.as_ref().unwrap();
    assert_eq!(peering.scheme.as_str(), scheme.as_str());

    // Each wallet address its name for people; the default's stays.
    let wallet = |id: u8| {
        let payment = store
            .incoming_payment(PaymentId([id; 16]))
            .unwrap()
            .unwrap();
        (payment.ledger, payment.wallet_address)
    };
    assert_eq!(wallet(0), (xrp, "https://a.example/pay/xrp".to_string()));
    assert_eq!(
        wallet(1),
        (
            dassie_ledger::LedgerId::StubUsd,
            "https://a.example/pay/usd".to_string()
        )
    );
    assert_eq!(
        wallet(2),
        (
            dassie_ledger::LedgerId::XrplTestnetDsusdUsd,
            "https://a.example/.well-known/pay".to_string()
        )
    );
    let grant = store.grant(PaymentId([0; 16])).unwrap().unwrap();
    let access: serde_json::Value = serde_json::from_str(&grant.access).unwrap();
    let identifiers: Vec<_> = access
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["identifier"].as_str().unwrap())
        .collect();
    assert_eq!(
        identifiers,
        ["https://a.example/pay/dsusd", "https://a.example/pay/usd"]
    );
    drop(store);

    // What changed takes revisions above what did not.
    let connection = Connection::open(temp.path().join(DATABASE_FILE)).unwrap();
    let revision = |table: &str, id: u8| -> i64 {
        connection
            .query_row(
                &format!("SELECT revision FROM {table} WHERE id = ?1"),
                [vec![id; 16]],
                |row| row.get(0),
            )
            .unwrap()
    };
    let unchanged = revision("incoming_payments", 2);
    assert_eq!(unchanged, 3);
    assert!(revision("incoming_payments", 0) > unchanged);
    assert!(revision("incoming_payments", 1) > unchanged);
    assert!(revision("gnap_grants", 0) > 1);
}

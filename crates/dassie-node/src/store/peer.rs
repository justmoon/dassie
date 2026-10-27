//! The peer protocol's tables (spec/areas/network.md#persistence): the node
//! table with each node's link state, registration and peering, our own
//! link-state sequence, settlement channels, bans, a registrar's log
//! and names, the node's view of who holds each name, and its own
//! registrations of names. They are
//! written in the same transaction as the ledger, so a peering and its
//! accounts, or a settlement and its message, land together.

use dassie_identity::{NodeId, PublicKey};
use dassie_peer::names::{
    Commitment, Granted, HeldName, LogHead, Logged, Name, NameRegistration, Salt, Stage, Unsealing,
};
use dassie_peer::wire::{Endpoint, SchemeId, TerminationReason};
use dassie_peer::{
    Channel, Closed, Depeering, Method, Network, Outgoing, PeerBatch, Peering, Registration,
    Sources, Stored, StoredChannel, StoredClosed, StoredNode, Termination,
};
use rusqlite::{Connection, Row, Transaction, params};

use super::{Cached, Store, StoreError, from_sql_time, to_sql_time};

/// An amount for SQLite, which holds `i64`. Larger values are refused when
/// a peering is made, so one here is a bug.
fn to_sql_amount(amount: u64) -> Result<i64, StoreError> {
    i64::try_from(amount).map_err(|_| StoreError::Corrupt("amount beyond i64"))
}

fn from_sql_amount(amount: i64) -> Result<u64, StoreError> {
    u64::try_from(amount).map_err(|_| StoreError::Corrupt("negative amount"))
}

impl Store {
    /// The peer protocol's state, as persisted.
    pub fn load_peer(&self) -> Result<Stored, StoreError> {
        let connection = self.lock();
        let own_sequence: i64 = connection.query_row_cached(
            "SELECT sequence FROM own_link_state WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = connection.prepare_cached(
            "SELECT n.id, n.link_state, n.sources, n.first_seen_ms,
                    n.registered_at_ms, n.renewed_at_ms, n.registration_expires_ms,
                    n.registered_url, n.registered_network,
                    p.scheme, p.state, p.credit_out, p.since_ms,
                    p.inbound, p.last_seen_ms,
                    t.since_ms, t.reason, t.by_us, t.acknowledged,
                    p.short_for_ms, p.depeering_since_ms, p.depeering_reason,
                    p.short_reached_ms
             FROM peer_nodes n LEFT JOIN peers p ON p.node = n.id
               LEFT JOIN peer_terminations t ON t.node = n.id
             ORDER BY n.id",
        )?;
        let rows = statement.query_map([], read_node)?;
        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row??);
        }
        Ok(Stored {
            nodes,
            own_sequence: from_sql_time(own_sequence),
            channels: load_channels(&connection)?,
            bans: load_bans(&connection)?,
            name_log: load_name_log(&connection)?,
            commitments: load_commitments(&connection)?,
            names: load_names(&connection)?,
            holders: load_holders(&connection)?,
            registrations: load_registrations(&connection)?,
        })
    }

    /// What stays of the closed settlement channel with `node`, if it was
    /// closed (spec/decisions/closing-channels.md).
    pub fn closed_channel(&self, node: &NodeId) -> Result<Option<Closed>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare_cached(
            "SELECT settled_in, settled_out, ended_for_room, paid_us FROM settlement_channels
             WHERE node = ?1 AND closed = 1",
        )?;
        let mut rows = statement.query([node.as_str()])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(Closed {
            settled_in: from_sql_amount(row.get(0)?)?,
            settled_out: from_sql_amount(row.get(1)?)?,
            ended_for_room: row.get(2)?,
            paid_us: row.get(3)?,
        }))
    }
}

/// The channels open: a closed one stays on disk, and is read only when
/// it is needed.
fn load_channels(connection: &Connection) -> Result<Vec<StoredChannel>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT node, public_key, url, settled_in, settled_out, outbox_sequence,
                outbox_scheme, outbox_amount, outbox_data, outbox_attempts, ended_for_room,
                paid_us
         FROM settlement_channels WHERE closed = 0 ORDER BY node",
    )?;
    let rows = statement.query_map([], |row| Ok(read_channel(row)))?;
    let mut channels = Vec::new();
    for row in rows {
        channels.push(row??);
    }
    Ok(channels)
}

fn read_channel(row: &Row<'_>) -> Result<StoredChannel, StoreError> {
    let node: String = row.get(0)?;
    let node = NodeId::parse(&node).map_err(|_| StoreError::Corrupt("node ID"))?;
    let key: Vec<u8> = row.get(1)?;
    let key = <[u8; 32]>::try_from(key.as_slice())
        .ok()
        .and_then(|key| PublicKey::from_bytes(&key).ok())
        .ok_or(StoreError::Corrupt("public key"))?;
    let outbox = match row.get::<_, Option<String>>(6)? {
        Some(scheme) => Some(Outgoing {
            sequence: from_sql_amount(row.get(5)?)?,
            scheme: SchemeId::parse(&scheme).ok_or(StoreError::Corrupt("scheme ID"))?,
            amount: from_sql_amount(row.get(7)?)?,
            data: row.get(8)?,
            attempts: u32::try_from(row.get::<_, i64>(9)?).unwrap_or(u32::MAX),
        }),
        None => None,
    };
    // A downlink's channel has no endpoint, and its row an empty URL.
    let url: String = row.get(2)?;
    let endpoint = if url.is_empty() {
        None
    } else {
        Some(Endpoint::of_url(&url).ok_or(StoreError::Corrupt("channel URL"))?)
    };
    Ok(StoredChannel {
        node,
        channel: Channel {
            key,
            endpoint,
            settled_in: from_sql_amount(row.get(3)?)?,
            settled_out: from_sql_amount(row.get(4)?)?,
            outbox,
            ended_for_room: row.get(10)?,
            paid_us: row.get(11)?,
        },
    })
}

fn load_bans(connection: &Connection) -> Result<Vec<NodeId>, StoreError> {
    let mut statement = connection.prepare_cached("SELECT node FROM peer_bans ORDER BY node")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut bans = Vec::new();
    for row in rows {
        bans.push(NodeId::parse(&row?).map_err(|_| StoreError::Corrupt("node ID"))?);
    }
    Ok(bans)
}

/// Where a registrar's log stands (spec/areas/network.md#names).
fn load_name_log(connection: &Connection) -> Result<LogHead, StoreError> {
    let (next_position, last_logged_ms): (i64, i64) = connection.query_row_cached(
        "SELECT next_position, last_logged_ms FROM name_log WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(LogHead {
        next_position: from_sql_position(next_position)?,
        last_logged_ms: from_sql_time(last_logged_ms),
    })
}

fn to_sql_position(position: u64) -> Result<i64, StoreError> {
    i64::try_from(position).map_err(|_| StoreError::Corrupt("log position beyond i64"))
}

fn from_sql_position(position: i64) -> Result<u64, StoreError> {
    u64::try_from(position).map_err(|_| StoreError::Corrupt("negative log position"))
}

fn parse_node(text: &str) -> Result<NodeId, StoreError> {
    NodeId::parse(text).map_err(|_| StoreError::Corrupt("node ID"))
}

fn parse_name(text: &str) -> Result<Name, StoreError> {
    Name::parse(text).map_err(|_| StoreError::Corrupt("name"))
}

/// The commitments in a registrar's log, by position.
fn load_commitments(connection: &Connection) -> Result<Vec<Logged>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT commitment, position, logged_ms, window_end_ms, committer, network, name,
                holder, contends
         FROM name_commitments ORDER BY position",
    )?;
    let rows = statement.query_map([], |row| Ok(read_logged(row)))?;
    let mut log = Vec::new();
    for row in rows {
        log.push(row??);
    }
    Ok(log)
}

fn read_logged(row: &Row<'_>) -> Result<Logged, StoreError> {
    let commitment: Vec<u8> = row.get(0)?;
    let commitment = <[u8; 32]>::try_from(commitment.as_slice())
        .map_err(|_| StoreError::Corrupt("commitment"))?;
    let unsealing = match (
        row.get::<_, Option<String>>(6)?,
        row.get::<_, Option<String>>(7)?,
        row.get::<_, Option<bool>>(8)?,
    ) {
        (Some(name), Some(holder), Some(contends)) => Some(Unsealing {
            name: parse_name(&name)?,
            holder: parse_node(&holder)?,
            contends,
        }),
        (None, None, None) => None,
        _ => return Err(StoreError::Corrupt("unsealing")),
    };
    Ok(Logged {
        commitment: Commitment(commitment),
        position: from_sql_position(row.get(1)?)?,
        logged_ms: from_sql_time(row.get(2)?),
        window_end_ms: from_sql_time(row.get(3)?),
        committer: parse_node(&row.get::<_, String>(4)?)?,
        network: row
            .get::<_, Option<Vec<u8>>>(5)?
            .and_then(|bytes| <[u8; 7]>::try_from(bytes).ok())
            .map(Network),
        unsealing,
    })
}

/// The names a registrar granted, by name.
fn load_names(connection: &Connection) -> Result<Vec<Granted>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT name, holder, granted_ms, expires_ms, freed_ms FROM granted_names ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<i64>>(4)?,
        ))
    })?;
    let mut names = Vec::new();
    for row in rows {
        let (name, holder, granted_ms, expires_ms, freed_ms) = row?;
        names.push(Granted {
            name: parse_name(&name)?,
            holder: parse_node(&holder)?,
            granted_ms: from_sql_time(granted_ms),
            expires_ms: from_sql_time(expires_ms),
            freed_ms: freed_ms.map(from_sql_time),
        });
    }
    Ok(names)
}

/// Who holds each name, in the node's view, by name
/// (spec/areas/network.md#holders).
fn load_holders(connection: &Connection) -> Result<Vec<HeldName>, StoreError> {
    let mut statement = connection
        .prepare_cached("SELECT name, holder, expires_ms FROM name_holders ORDER BY name")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut holders = Vec::new();
    for row in rows {
        let (name, holder, expires_ms) = row?;
        holders.push(HeldName {
            name: parse_name(&name)?,
            holder: parse_node(&holder)?,
            expires_ms: from_sql_time(expires_ms),
        });
    }
    Ok(holders)
}

/// The node's registrations of names, by name
/// (spec/areas/network.md#the-registrant).
fn load_registrations(connection: &Connection) -> Result<Vec<NameRegistration>, StoreError> {
    let mut statement = connection.prepare_cached(
        "SELECT name, salt, started_ms, stage FROM name_registrations ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut registrations = Vec::new();
    for row in rows {
        let (name, salt, started_ms, stage) = row?;
        let salt = <[u8; 32]>::try_from(salt.as_slice())
            .map_err(|_| StoreError::Corrupt("registration salt"))?;
        let stage = match stage.as_str() {
            "committing" => Stage::Committing,
            "unsealing" => Stage::Unsealing,
            "abandoned" => Stage::Abandoned,
            _ => return Err(StoreError::Corrupt("registration stage")),
        };
        registrations.push(NameRegistration {
            name: parse_name(&name)?,
            salt: Salt(salt),
            started_ms: from_sql_time(started_ms),
            stage,
        });
    }
    Ok(registrations)
}

/// A row of the node table, joined with its peering.
fn read_node(row: &Row<'_>) -> rusqlite::Result<Result<StoredNode, StoreError>> {
    let id: String = row.get(0)?;
    let Ok(id) = NodeId::parse(&id) else {
        return Ok(Err(StoreError::Corrupt("node ID")));
    };
    let sources: i64 = row.get(2)?;
    let registration = match (
        row.get::<_, Option<i64>>(4)?,
        row.get::<_, Option<i64>>(5)?,
        row.get::<_, Option<i64>>(6)?,
        row.get::<_, Option<String>>(7)?,
    ) {
        (Some(registered), Some(renewed), Some(expires), Some(url)) => Some(Registration {
            registered_ms: from_sql_time(registered),
            renewed_ms: from_sql_time(renewed),
            expires_ms: from_sql_time(expires),
            endpoint: match Endpoint::of_url(&url) {
                Some(endpoint) => endpoint,
                None => return Ok(Err(StoreError::Corrupt("registered URL"))),
            },
            network: row
                .get::<_, Option<Vec<u8>>>(8)?
                .and_then(|bytes| <[u8; 7]>::try_from(bytes).ok())
                .map(Network),
        }),
        _ => None,
    };
    let peering = match row.get::<_, Option<String>>(9)? {
        Some(scheme) => match read_peering(row, &scheme) {
            Ok(peering) => Some(peering),
            Err(error) => return Ok(Err(error)),
        },
        None => None,
    };
    let termination = match row.get::<_, Option<i64>>(15)? {
        Some(since) if peering.is_some() => Some(Termination {
            since_ms: from_sql_time(since),
            reason: TerminationReason::from_code(u8::try_from(row.get::<_, i64>(16)?).unwrap_or(0)),
            by_us: row.get::<_, i64>(17)? != 0,
            acknowledged: row.get::<_, i64>(18)? != 0,
        }),
        _ => None,
    };
    Ok(Ok(StoredNode {
        id,
        link_state: row.get(1)?,
        sources: Sources::from_bits(u8::try_from(sources).unwrap_or(0)),
        first_seen_ms: from_sql_time(row.get(3)?),
        registration,
        peering,
        termination,
    }))
}

/// A stored peering: the store keeps one settlement method per peering,
/// the one it was made on, until a table of methods holds more
/// (spec/roadmap.md#settlement-methods), so a row is a peering with that
/// one method.
fn read_peering(row: &Row<'_>, scheme: &str) -> Result<Peering, StoreError> {
    let scheme = SchemeId::parse(scheme).ok_or(StoreError::Corrupt("scheme ID"))?;
    let short_for_ms = u64::try_from(row.get::<_, i64>(19)?)
        .map_err(|_| StoreError::Corrupt("time short of room"))?;
    let short_reached_ms = u64::try_from(row.get::<_, i64>(22)?)
        .ok()
        .filter(|reached| *reached <= short_for_ms)
        .ok_or(StoreError::Corrupt("time short of room while reached"))?;
    let method = Method {
        scheme,
        state: row.get(10)?,
        credit_out: from_sql_amount(row.get(11)?)?,
        short_for_ms,
        short_reached_ms,
        short_checked_ms: None,
    };
    Ok(Peering {
        methods: vec![method],
        since_ms: from_sql_time(row.get(12)?),
        inbound: row.get::<_, i64>(13)? != 0,
        last_seen_ms: from_sql_time(row.get(14)?),
        depeering: match row.get::<_, Option<i64>>(20)? {
            Some(since) => {
                let reason = u8::try_from(row.get::<_, i64>(21)?)
                    .map_err(|_| StoreError::Corrupt("depeering reason"))?;
                Some(Depeering::new(
                    from_sql_time(since),
                    TerminationReason::from_code(reason),
                ))
            }
            None => None,
        },
    })
}

/// Writes what the peer node changed.
pub(super) fn write_peer(
    transaction: &Transaction<'_>,
    batch: &PeerBatch,
) -> Result<(), StoreError> {
    // Removals first: a node removed and added again in one batch stays.
    for removed in &batch.removed {
        // The peering goes with it (ON DELETE CASCADE).
        transaction.execute_cached("DELETE FROM peer_nodes WHERE id = ?1", [removed.as_str()])?;
    }
    for node in &batch.nodes {
        write_node(transaction, node)?;
    }
    if let Some(sequence) = batch.own_sequence {
        transaction.execute_cached(
            "UPDATE own_link_state SET sequence = ?1 WHERE id = 1",
            [to_sql_time(sequence)],
        )?;
    }
    for channel in &batch.channels {
        write_channel(transaction, channel)?;
    }
    for closed in &batch.closed {
        write_closed(transaction, closed)?;
    }
    for banned in &batch.banned {
        transaction.execute_cached(
            "INSERT INTO peer_bans (node) VALUES (?1) ON CONFLICT DO NOTHING",
            [banned.as_str()],
        )?;
    }
    for unbanned in &batch.unbanned {
        transaction.execute_cached("DELETE FROM peer_bans WHERE node = ?1", [unbanned.as_str()])?;
    }
    write_registrar(transaction, batch)?;
    write_holders(transaction, batch)?;
    write_registrations(transaction, batch)
}

/// Writes what changed in the node's registrations of names
/// (spec/areas/network.md#the-registrant).
fn write_registrations(transaction: &Transaction<'_>, batch: &PeerBatch) -> Result<(), StoreError> {
    // Removals first, as for nodes: a registration dropped and started
    // again in one batch stays.
    for removed in &batch.registrations_removed {
        transaction.execute_cached(
            "DELETE FROM name_registrations WHERE name = ?1",
            [removed.as_str()],
        )?;
    }
    for registration in &batch.registrations {
        let stage = match registration.stage {
            Stage::Committing => "committing",
            Stage::Unsealing => "unsealing",
            Stage::Abandoned => "abandoned",
        };
        transaction.execute_cached(
            "INSERT INTO name_registrations (name, salt, started_ms, stage)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (name) DO UPDATE SET salt = ?2, started_ms = ?3, stage = ?4",
            params![
                registration.name.as_str(),
                registration.salt.0.as_slice(),
                to_sql_time(registration.started_ms),
                stage,
            ],
        )?;
    }
    Ok(())
}

/// Writes what changed in the node's view of who holds each name
/// (spec/areas/network.md#holders).
fn write_holders(transaction: &Transaction<'_>, batch: &PeerBatch) -> Result<(), StoreError> {
    // Removals first, as for nodes.
    for removed in &batch.holders_removed {
        transaction.execute_cached(
            "DELETE FROM name_holders WHERE name = ?1",
            [removed.as_str()],
        )?;
    }
    for held in &batch.holders {
        transaction.execute_cached(
            "INSERT INTO name_holders (name, holder, expires_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT (name) DO UPDATE SET holder = ?2, expires_ms = ?3",
            params![
                held.name.as_str(),
                held.holder.as_str(),
                to_sql_time(held.expires_ms)
            ],
        )?;
    }
    Ok(())
}

/// Writes what a registrar changed: its log's head, the commitments in the
/// log and the names granted (spec/areas/network.md#names).
fn write_registrar(transaction: &Transaction<'_>, batch: &PeerBatch) -> Result<(), StoreError> {
    if let Some(head) = batch.name_log {
        transaction.execute_cached(
            "UPDATE name_log SET next_position = ?1, last_logged_ms = ?2 WHERE id = 1",
            params![
                to_sql_position(head.next_position)?,
                to_sql_time(head.last_logged_ms)
            ],
        )?;
    }
    // Removals first, as for nodes.
    for removed in &batch.commitments_removed {
        transaction.execute_cached(
            "DELETE FROM name_commitments WHERE commitment = ?1",
            [removed.0.as_slice()],
        )?;
    }
    for logged in &batch.commitments {
        write_logged(transaction, logged)?;
    }
    for removed in &batch.names_removed {
        transaction.execute_cached(
            "DELETE FROM granted_names WHERE name = ?1",
            [removed.as_str()],
        )?;
    }
    for granted in &batch.names {
        transaction.execute_cached(
            "INSERT INTO granted_names (name, holder, granted_ms, expires_ms, freed_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (name) DO UPDATE SET holder = ?2, granted_ms = ?3, expires_ms = ?4,
               freed_ms = ?5",
            params![
                granted.name.as_str(),
                granted.holder.as_str(),
                to_sql_time(granted.granted_ms),
                to_sql_time(granted.expires_ms),
                granted.freed_ms.map(to_sql_time),
            ],
        )?;
    }
    Ok(())
}

fn write_logged(transaction: &Transaction<'_>, logged: &Logged) -> Result<(), StoreError> {
    let unsealing = logged.unsealing.as_ref();
    transaction.execute_cached(
        "INSERT INTO name_commitments (commitment, position, logged_ms, window_end_ms,
           committer, network, name, holder, contends)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (commitment) DO UPDATE SET position = ?2, logged_ms = ?3,
           window_end_ms = ?4, committer = ?5, network = ?6, name = ?7, holder = ?8,
           contends = ?9",
        params![
            logged.commitment.0.as_slice(),
            to_sql_position(logged.position)?,
            to_sql_time(logged.logged_ms),
            to_sql_time(logged.window_end_ms),
            logged.committer.as_str(),
            logged.network.map(|network| network.0.to_vec()),
            unsealing.map(|u| u.name.as_str()),
            unsealing.map(|u| u.holder.as_str()),
            unsealing.map(|u| u.contends),
        ],
    )?;
    Ok(())
}

fn write_channel(transaction: &Transaction<'_>, stored: &StoredChannel) -> Result<(), StoreError> {
    let channel = &stored.channel;
    let outbox = channel.outbox.as_ref();
    transaction.execute_cached(
        "INSERT INTO settlement_channels (node, public_key, url, settled_in, settled_out,
           outbox_sequence, outbox_scheme, outbox_amount, outbox_data, outbox_attempts,
           ended_for_room, paid_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (node) DO UPDATE SET public_key = ?2, url = ?3, settled_in = ?4,
           settled_out = ?5, outbox_sequence = ?6, outbox_scheme = ?7, outbox_amount = ?8,
           outbox_data = ?9, outbox_attempts = ?10, ended_for_room = ?11, paid_us = ?12,
           closed = 0",
        params![
            stored.node.as_str(),
            channel.key.as_bytes().to_vec(),
            channel
                .endpoint
                .as_ref()
                .map(Endpoint::url)
                .unwrap_or_default(),
            to_sql_amount(channel.settled_in)?,
            to_sql_amount(channel.settled_out)?,
            outbox.map(|o| to_sql_amount(o.sequence)).transpose()?,
            outbox.map(|o| o.scheme.as_str()),
            outbox.map(|o| to_sql_amount(o.amount)).transpose()?,
            outbox.map(|o| o.data.as_slice()),
            outbox.map(|o| i64::from(o.attempts)),
            channel.ended_for_room,
            channel.paid_us,
        ],
    )?;
    Ok(())
}

/// Closes a channel, or moves a closed one's sequence. Its row was written
/// when it opened, and stays.
fn write_closed(transaction: &Transaction<'_>, stored: &StoredClosed) -> Result<(), StoreError> {
    let closed = &stored.closed;
    let changed = transaction.execute_cached(
        "UPDATE settlement_channels SET closed = 1, settled_in = ?2, settled_out = ?3,
           ended_for_room = ?4, paid_us = ?5, outbox_sequence = NULL, outbox_scheme = NULL,
           outbox_amount = NULL, outbox_data = NULL, outbox_attempts = NULL
         WHERE node = ?1",
        params![
            stored.node.as_str(),
            to_sql_amount(closed.settled_in)?,
            to_sql_amount(closed.settled_out)?,
            closed.ended_for_room,
            closed.paid_us,
        ],
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StoreError::Corrupt("a closed channel without its row"))
    }
}

fn write_node(transaction: &Transaction<'_>, node: &StoredNode) -> Result<(), StoreError> {
    let registration = node.registration.as_ref();
    let registered_url = registration.map(|r| r.endpoint.url());
    transaction.execute_cached(
        "INSERT INTO peer_nodes (id, link_state, sources, first_seen_ms,
           registered_at_ms, renewed_at_ms, registration_expires_ms, registered_url,
           registered_network)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET link_state = ?2, sources = ?3,
           registered_at_ms = ?5, renewed_at_ms = ?6, registration_expires_ms = ?7,
           registered_url = ?8, registered_network = ?9",
        params![
            node.id.as_str(),
            node.link_state,
            i64::from(node.sources.bits()),
            to_sql_time(node.first_seen_ms),
            registration.map(|r| to_sql_time(r.registered_ms)),
            registration.map(|r| to_sql_time(r.renewed_ms)),
            registration.map(|r| to_sql_time(r.expires_ms)),
            registered_url.as_deref(),
            registration.and_then(|r| r.network).map(|n| n.0.to_vec()),
        ],
    )?;
    let Some(peering) = &node.peering else {
        transaction.execute_cached("DELETE FROM peers WHERE node = ?1", [node.id.as_str()])?;
        return Ok(());
    };
    // The node keeps one method per peering, which a row holds.
    assert_eq!(peering.methods.len(), 1, "a stored peering has one method");
    let method = peering.first();
    transaction.execute_cached(
        "INSERT INTO peers (node, scheme, state, credit_out, since_ms,
           inbound, last_seen_ms, short_for_ms, depeering_since_ms, depeering_reason,
           short_reached_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (node) DO UPDATE SET scheme = ?2, state = ?3,
           credit_out = ?4, since_ms = ?5, inbound = ?6,
           last_seen_ms = ?7, short_for_ms = ?8,
           depeering_since_ms = ?9, depeering_reason = ?10, short_reached_ms = ?11",
        params![
            node.id.as_str(),
            method.scheme.as_str(),
            method.state,
            to_sql_amount(method.credit_out)?,
            to_sql_time(peering.since_ms),
            i64::from(peering.inbound),
            to_sql_time(peering.last_seen_ms),
            i64::try_from(method.short_for_ms)
                .map_err(|_| StoreError::Corrupt("time short of room"))?,
            peering.depeering.map(|d| to_sql_time(d.since_ms)),
            peering.depeering.map(|d| i64::from(d.reason.code())),
            i64::try_from(method.short_reached_ms)
                .map_err(|_| StoreError::Corrupt("time short of room while reached"))?,
        ],
    )?;
    write_termination(transaction, node)
}

fn write_termination(transaction: &Transaction<'_>, node: &StoredNode) -> Result<(), StoreError> {
    let Some(termination) = &node.termination else {
        transaction.execute_cached(
            "DELETE FROM peer_terminations WHERE node = ?1",
            [node.id.as_str()],
        )?;
        return Ok(());
    };
    transaction.execute_cached(
        "INSERT INTO peer_terminations (node, since_ms, reason, by_us, acknowledged)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (node) DO UPDATE SET since_ms = ?2, reason = ?3, by_us = ?4,
           acknowledged = ?5",
        params![
            node.id.as_str(),
            to_sql_time(termination.since_ms),
            i64::from(termination.reason.code()),
            i64::from(termination.by_us),
            i64::from(termination.acknowledged),
        ],
    )?;
    Ok(())
}

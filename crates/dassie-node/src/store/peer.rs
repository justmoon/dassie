//! The peer protocol's tables (spec 7 §4.4): the node table with each
//! node's link state, registration and peering, and our own link-state
//! sequence (NET-1, NET-2, NET-7, ACC-18). They are written in the same
//! transaction as the ledger, so a peering and its accounts land together.

use dassie_identity::NodeId;
use dassie_peer::wire::SchemeId;
use dassie_peer::{PeerBatch, Peering, Registration, Sources, Stored, StoredNode};
use rusqlite::{Row, Transaction, params};

use super::{Store, StoreError, from_sql_time, to_sql_time};

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
        let own_sequence: i64 = connection.query_row(
            "SELECT sequence FROM own_link_state WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        let mut statement = connection.prepare(
            "SELECT n.id, n.link_state, n.sources, n.first_seen_ms,
                    n.registered_at_ms, n.renewed_at_ms, n.registration_expires_ms,
                    p.scheme, p.version, p.state, p.credit_in, p.credit_out, p.since_ms,
                    p.inbound, p.last_seen_ms, p.replay_floor_ms
             FROM peer_nodes n LEFT JOIN peers p ON p.node = n.id
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
        })
    }
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
    ) {
        (Some(registered), Some(renewed), Some(expires)) => Some(Registration {
            registered_ms: from_sql_time(registered),
            renewed_ms: from_sql_time(renewed),
            expires_ms: from_sql_time(expires),
        }),
        _ => None,
    };
    let peering = match row.get::<_, Option<String>>(7)? {
        Some(scheme) => match read_peering(row, &scheme) {
            Ok(peering) => Some(peering),
            Err(error) => return Ok(Err(error)),
        },
        None => None,
    };
    let replay_floor: Option<i64> = row.get(15)?;
    Ok(Ok(StoredNode {
        id,
        link_state: row.get(1)?,
        sources: Sources::from_bits(u8::try_from(sources).unwrap_or(0)),
        first_seen_ms: from_sql_time(row.get(3)?),
        registration,
        peering,
        replay_floor_ms: replay_floor.map_or(0, from_sql_time),
    }))
}

fn read_peering(row: &Row<'_>, scheme: &str) -> Result<Peering, StoreError> {
    let scheme = SchemeId::parse(scheme).ok_or(StoreError::Corrupt("scheme ID"))?;
    let version: i64 = row.get(8)?;
    Ok(Peering {
        scheme,
        version: u16::try_from(version).map_err(|_| StoreError::Corrupt("scheme version"))?,
        state: row.get(9)?,
        credit_in: from_sql_amount(row.get(10)?)?,
        credit_out: from_sql_amount(row.get(11)?)?,
        since_ms: from_sql_time(row.get(12)?),
        inbound: row.get::<_, i64>(13)? != 0,
        last_seen_ms: from_sql_time(row.get(14)?),
    })
}

/// Writes what the peer node changed.
pub(super) fn write_peer(
    transaction: &Transaction<'_>,
    batch: &PeerBatch,
) -> Result<(), StoreError> {
    for node in &batch.nodes {
        write_node(transaction, node)?;
    }
    for removed in &batch.removed {
        // The peering goes with it (ON DELETE CASCADE).
        transaction.execute("DELETE FROM peer_nodes WHERE id = ?1", [removed.as_str()])?;
    }
    if let Some(sequence) = batch.own_sequence {
        transaction.execute(
            "UPDATE own_link_state SET sequence = ?1 WHERE id = 1",
            [to_sql_time(sequence)],
        )?;
    }
    Ok(())
}

fn write_node(transaction: &Transaction<'_>, node: &StoredNode) -> Result<(), StoreError> {
    let registration = node.registration.as_ref();
    transaction.execute(
        "INSERT INTO peer_nodes (id, link_state, sources, first_seen_ms,
           registered_at_ms, renewed_at_ms, registration_expires_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (id) DO UPDATE SET link_state = ?2, sources = ?3,
           registered_at_ms = ?5, renewed_at_ms = ?6, registration_expires_ms = ?7",
        params![
            node.id.as_str(),
            node.link_state,
            i64::from(node.sources.bits()),
            to_sql_time(node.first_seen_ms),
            registration.map(|r| to_sql_time(r.registered_ms)),
            registration.map(|r| to_sql_time(r.renewed_ms)),
            registration.map(|r| to_sql_time(r.expires_ms)),
        ],
    )?;
    let Some(peering) = &node.peering else {
        transaction.execute("DELETE FROM peers WHERE node = ?1", [node.id.as_str()])?;
        return Ok(());
    };
    transaction.execute(
        "INSERT INTO peers (node, scheme, version, state, credit_in, credit_out, since_ms,
           inbound, last_seen_ms, replay_floor_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (node) DO UPDATE SET scheme = ?2, version = ?3, state = ?4,
           credit_in = ?5, credit_out = ?6, since_ms = ?7, inbound = ?8,
           last_seen_ms = ?9, replay_floor_ms = ?10",
        params![
            node.id.as_str(),
            peering.scheme.as_str(),
            i64::from(peering.version),
            peering.state,
            to_sql_amount(peering.credit_in)?,
            to_sql_amount(peering.credit_out)?,
            to_sql_time(peering.since_ms),
            i64::from(peering.inbound),
            to_sql_time(peering.last_seen_ms),
            to_sql_time(node.replay_floor_ms),
        ],
    )?;
    Ok(())
}

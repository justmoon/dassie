//! Paired devices (spec/areas/rpc.md#paired-devices): a bounded list the
//! owner revokes one by one. A full list refuses a new device rather than
//! dropping one the owner paired.

use dassie_identity::NodeId;
use rusqlite::{OptionalExtension, Row, params};

use super::{Store, StoreError, from_sql_time, to_sql_time};

/// Most devices a node keeps.
pub const MAX_DEVICES: u32 = 32;

/// Longest name a device gives itself, in bytes.
pub const MAX_DEVICE_NAME_BYTES: usize = 64;

/// How long a device's last use may go unrecorded: a device's every call
/// would otherwise write.
pub const DEVICE_SEEN_GRANULARITY_MS: u64 = 60 * 60 * 1_000;

/// A paired device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    /// Its device key's node ID.
    pub id: NodeId,
    /// As it named itself.
    pub name: String,
    /// When it paired.
    pub paired_at_ms: u64,
    /// When it was last seen, to [`DEVICE_SEEN_GRANULARITY_MS`].
    pub last_seen_at_ms: u64,
}

/// Why a device was not added.
#[derive(Debug)]
pub enum AddDevice {
    /// The node keeps [`MAX_DEVICES`] already.
    Full,
    /// The store failed.
    Failed(StoreError),
}

impl From<rusqlite::Error> for AddDevice {
    fn from(error: rusqlite::Error) -> Self {
        Self::Failed(error.into())
    }
}

impl Store {
    /// Adds `device`, or pairs it anew if it was paired already, unless the
    /// node keeps [`MAX_DEVICES`] others.
    pub fn add_device(&self, device: &DeviceRow) -> Result<(), AddDevice> {
        assert!(device.name.len() <= MAX_DEVICE_NAME_BYTES);
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let others: u32 = transaction.query_row(
            "SELECT count(*) FROM devices WHERE id != ?1",
            [device.id.as_str()],
            |row| row.get(0),
        )?;
        if others >= MAX_DEVICES {
            return Err(AddDevice::Full);
        }
        transaction.execute(
            "INSERT OR REPLACE INTO devices (id, name, paired_at_ms, last_seen_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                device.id.as_str(),
                device.name,
                to_sql_time(device.paired_at_ms),
                to_sql_time(device.last_seen_at_ms),
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// The device `id`, if it is paired.
    pub fn device(&self, id: &NodeId) -> Result<Option<DeviceRow>, StoreError> {
        self.lock()
            .query_row(
                "SELECT id, name, paired_at_ms, last_seen_at_ms FROM devices WHERE id = ?1",
                [id.as_str()],
                |row| Ok(device_row(row)),
            )
            .optional()?
            .transpose()
    }

    /// Every paired device, most recently paired first.
    pub fn devices(&self) -> Result<Vec<DeviceRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, name, paired_at_ms, last_seen_at_ms FROM devices \
             ORDER BY paired_at_ms DESC, id LIMIT ?1",
        )?;
        let rows = statement.query_map([MAX_DEVICES], |row| Ok(device_row(row)))?;
        let mut devices = Vec::new();
        for row in rows {
            devices.push(row??);
        }
        Ok(devices)
    }

    /// Records that the device was seen at `now_ms`.
    pub fn touch_device(&self, id: &NodeId, now_ms: u64) -> Result<(), StoreError> {
        self.lock().execute(
            "UPDATE devices SET last_seen_at_ms = max(last_seen_at_ms, ?2) WHERE id = ?1",
            params![id.as_str(), to_sql_time(now_ms)],
        )?;
        Ok(())
    }

    /// Deletes a device, and says whether there was one.
    pub fn delete_device(&self, id: &NodeId) -> Result<bool, StoreError> {
        Ok(self
            .lock()
            .execute("DELETE FROM devices WHERE id = ?1", [id.as_str()])?
            == 1)
    }
}

fn device_row(row: &Row<'_>) -> Result<DeviceRow, StoreError> {
    let id: String = row.get(0)?;
    let id = NodeId::parse(&id).map_err(|_| StoreError::Corrupt("node ID"))?;
    Ok(DeviceRow {
        id,
        name: row.get(1)?,
        paired_at_ms: from_sql_time(row.get(2)?),
        last_seen_at_ms: from_sql_time(row.get(3)?),
    })
}

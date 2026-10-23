//! The hubs the node validates (spec/areas/node.md#persistence,
//! spec/areas/hubs.md#the-validator-in-the-node): for each, its consensus
//! state and the state after its last final block, written together before
//! a step's messages go out, so a restart never signs a vote that conflicts
//! with one sent before; the states after a few recent final blocks and
//! after the last blocks of recent epochs, which validators far behind
//! catch up from; how far it followed the escrow on the XRP Ledger; and the
//! payout bundles it signed.
//!
//! The rows hold bytes the hub's crate encodes and reads; the store knows
//! only their bounds.

use rusqlite::{OptionalExtension, params};

use super::{Store, StoreError, from_sql_time, to_sql_time};

/// The largest consensus state stored: what the engine can hold above its
/// last final block, its blocks' payloads included.
pub const MAX_HUB_CONSENSUS_BYTES: usize = 512 * 1024 * 1024;

/// The largest ledger stored: the hub's terms bound its tables.
pub const MAX_HUB_LEDGER_BYTES: usize = 256 * 1024 * 1024;

/// The largest payout bundle stored.
pub const MAX_HUB_BUNDLE_BYTES: usize = 16 * 1024 * 1024;

/// The payout bundles kept per hub, the newest: a validator signs one per
/// final block it pays out from, and only the highest matters.
pub const KEPT_BUNDLES: u32 = 8;

/// The states after recent final blocks kept per hub, the newest first:
/// a peer names its tip to a validator far behind, which asks for the
/// state after it a moment later, when a few more blocks are final.
pub const KEPT_TIPS: u32 = 8;

/// The states after the last blocks of epochs kept per hub: as many as the
/// engine keeps those blocks, which it names to a validator still in an
/// epoch that ended.
pub const KEPT_BOUNDARIES: u32 = 8;

const _: () = assert!(KEPT_BOUNDARIES as usize == dassie_hub::consensus::MAX_BOUNDARIES);

/// What a validator resumes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredHub {
    /// Its consensus state.
    pub consensus: Vec<u8>,
    /// The height of its last final block.
    pub height: u64,
    /// The state after it.
    pub ledger: Vec<u8>,
}

/// How far a validator followed its hub's escrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EscrowCursor {
    /// The newest validated ledger it saw, on the chain it follows.
    pub newest_ledger: u32,
    /// When that ledger closed, in seconds since the XRP Ledger's epoch.
    pub newest_close_time: u64,
    /// The ledger up to which it attested the escrow's transactions.
    pub scanned_to: u32,
}

/// What one step of a validator changed.
#[derive(Clone, Copy, Debug, Default)]
pub struct HubStep<'a> {
    /// Its consensus state, if the step changed it.
    pub consensus: Option<&'a [u8]>,
    /// The height and state after a block that became final, if one did.
    pub ledger: Option<(u64, &'a [u8])>,
    /// The heights of blocks the step learned were the last of their
    /// epochs, whose states are kept longer.
    pub boundaries: &'a [u64],
}

impl Store {
    /// What the validator of `hub` resumes from, if it ran before: its
    /// consensus state and its newest state.
    pub fn hub(&self, hub: &[u8; 32]) -> Result<Option<StoredHub>, StoreError> {
        let row = self
            .lock()
            .query_row(
                "SELECT c.state, s.height, s.state FROM hub_consensus c \
                 JOIN hub_states s ON s.hub = c.hub WHERE c.hub = ?1 \
                 ORDER BY s.height DESC LIMIT 1",
                [&hub[..]],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.map(|(consensus, height, ledger)| StoredHub {
            consensus,
            height: from_sql_time(height),
            ledger,
        }))
    }

    /// The state of `hub` after its block at `height`, if it is kept.
    pub fn hub_state(&self, hub: &[u8; 32], height: u64) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT state FROM hub_states WHERE hub = ?1 AND height = ?2",
                params![&hub[..], to_sql_time(height)],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Writes what a step of `hub`'s validator changed, in one transaction:
    /// a validator that runs for the first time writes both. States beyond
    /// [`KEPT_TIPS`] and [`KEPT_BOUNDARIES`] go; the newest always stays.
    pub fn save_hub_step(
        &self,
        hub: &[u8; 32],
        now_ms: u64,
        step: HubStep<'_>,
    ) -> Result<(), StoreError> {
        if step.consensus.is_none() && step.ledger.is_none() && step.boundaries.is_empty() {
            return Ok(());
        }
        let too_large = step
            .consensus
            .is_some_and(|bytes| bytes.len() > MAX_HUB_CONSENSUS_BYTES)
            || step
                .ledger
                .is_some_and(|(_, bytes)| bytes.len() > MAX_HUB_LEDGER_BYTES);
        if too_large {
            return Err(StoreError::Corrupt("hub state over its bound"));
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if let Some(state) = step.consensus {
            transaction.execute(
                "INSERT INTO hub_consensus (hub, updated_at_ms, state) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (hub) DO UPDATE SET updated_at_ms = ?2, state = ?3",
                params![&hub[..], to_sql_time(now_ms), state],
            )?;
        }
        if let Some((height, state)) = step.ledger {
            transaction.execute(
                "INSERT INTO hub_states (hub, height, boundary, state) VALUES (?1, ?2, 0, ?3) \
                 ON CONFLICT (hub, height) DO UPDATE SET state = ?3",
                params![&hub[..], to_sql_time(height), state],
            )?;
        }
        for height in step.boundaries {
            transaction.execute(
                "UPDATE hub_states SET boundary = 1 WHERE hub = ?1 AND height = ?2",
                params![&hub[..], to_sql_time(*height)],
            )?;
        }
        transaction.execute(
            "DELETE FROM hub_states WHERE hub = ?1 \
             AND height < (SELECT max(height) FROM hub_states WHERE hub = ?1) \
             AND height NOT IN (SELECT height FROM hub_states WHERE hub = ?1 \
               ORDER BY height DESC LIMIT ?2) \
             AND height NOT IN (SELECT height FROM hub_states WHERE hub = ?1 AND boundary = 1 \
               ORDER BY height DESC LIMIT ?3)",
            params![&hub[..], KEPT_TIPS, KEPT_BOUNDARIES],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// How far the validator of `hub` followed its escrow, if it did.
    pub fn hub_escrow(&self, hub: &[u8; 32]) -> Result<Option<EscrowCursor>, StoreError> {
        let row = self
            .lock()
            .query_row(
                "SELECT newest_ledger, newest_close_time, scanned_to FROM hub_escrow WHERE hub = ?1",
                [&hub[..]],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, i64>(1)?, row.get(2)?)),
            )
            .optional()?;
        Ok(
            row.map(|(newest_ledger, close_time, scanned_to)| EscrowCursor {
                newest_ledger,
                newest_close_time: from_sql_time(close_time),
                scanned_to,
            }),
        )
    }

    /// Records how far the validator of `hub` followed its escrow.
    pub fn set_hub_escrow(&self, hub: &[u8; 32], cursor: &EscrowCursor) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT INTO hub_escrow (hub, newest_ledger, newest_close_time, scanned_to) \
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT (hub) DO UPDATE SET \
             newest_ledger = ?2, newest_close_time = ?3, scanned_to = ?4",
            params![
                &hub[..],
                cursor.newest_ledger,
                to_sql_time(cursor.newest_close_time),
                cursor.scanned_to,
            ],
        )?;
        Ok(())
    }

    /// Keeps the payout bundle the validator of `hub` signed for the block
    /// at `height`, and only the [`KEPT_BUNDLES`] newest of the hub's.
    pub fn add_hub_bundle(
        &self,
        hub: &[u8; 32],
        height: u64,
        now_ms: u64,
        bundle: &[u8],
    ) -> Result<(), StoreError> {
        if bundle.len() > MAX_HUB_BUNDLE_BYTES {
            return Err(StoreError::Corrupt("payout bundle over its bound"));
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO hub_bundles (hub, height, signed_at_ms, bundle) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (hub, height) DO UPDATE SET signed_at_ms = ?3, bundle = ?4",
            params![&hub[..], to_sql_time(height), to_sql_time(now_ms), bundle],
        )?;
        transaction.execute(
            "DELETE FROM hub_bundles WHERE hub = ?1 AND height NOT IN \
             (SELECT height FROM hub_bundles WHERE hub = ?1 ORDER BY height DESC LIMIT ?2)",
            params![&hub[..], KEPT_BUNDLES],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// The payout bundles the validator of `hub` signed, the newest first:
    /// the height each pays out from, and the bundle.
    pub fn hub_bundles(&self, hub: &[u8; 32]) -> Result<Vec<(u64, Vec<u8>)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT height, bundle FROM hub_bundles WHERE hub = ?1 ORDER BY height DESC",
        )?;
        let rows = statement
            .query_map([&hub[..]], |row| {
                Ok((from_sql_time(row.get(0)?), row.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_written_whole_and_read_back() {
        let store = Store::open_in_memory().unwrap();
        let hub = [7; 32];
        assert_eq!(store.hub(&hub).unwrap(), None);
        let step = |consensus, ledger| HubStep {
            consensus,
            ledger,
            boundaries: &[],
        };
        store
            .save_hub_step(&hub, 10, step(Some(&[1, 2]), Some((0, &[3]))))
            .unwrap();
        store
            .save_hub_step(&hub, 11, step(Some(&[4]), None))
            .unwrap();
        let read = store.hub(&hub).unwrap().unwrap();
        assert_eq!(read.consensus, vec![4]);
        assert_eq!((read.height, read.ledger), (0, vec![3]));
        store
            .save_hub_step(&hub, 12, step(None, Some((5, &[6]))))
            .unwrap();
        assert_eq!(store.hub(&hub).unwrap().unwrap().height, 5);
        assert_eq!(store.hub_state(&hub, 0).unwrap(), Some(vec![3]));
        assert_eq!(store.hub(&[8; 32]).unwrap(), None);
    }

    // Recent states and those of the last blocks of epochs are kept for
    // validators far behind, each within its bound; the newest always.
    #[test]
    fn states_are_kept_within_their_bounds() {
        let store = Store::open_in_memory().unwrap();
        let hub = [7; 32];
        let first = HubStep {
            consensus: Some(&[1]),
            ledger: Some((1, &[1])),
            boundaries: &[],
        };
        store.save_hub_step(&hub, 1, first).unwrap();
        for height in 2..100_u64 {
            let boundaries: &[u64] = if height % 10 == 0 { &[height] } else { &[] };
            let step = HubStep {
                consensus: None,
                ledger: Some((height, &[2])),
                boundaries,
            };
            store.save_hub_step(&hub, height, step).unwrap();
        }
        let kept = |height| store.hub_state(&hub, height).unwrap().is_some();
        let tips = (1..100).filter(|h| kept(*h) && h % 10 != 0).count();
        assert_eq!(tips, usize::try_from(KEPT_TIPS).unwrap());
        assert!(kept(99), "the newest stays");
        let boundaries: Vec<u64> = (1..100).filter(|h| kept(*h) && h % 10 == 0).collect();
        assert_eq!(boundaries.len(), usize::try_from(KEPT_BOUNDARIES).unwrap());
        assert_eq!(boundaries.last(), Some(&90));
        assert!(!kept(10), "the oldest epoch's goes");
    }

    #[test]
    fn the_escrow_cursor_and_bundles_are_kept() {
        let store = Store::open_in_memory().unwrap();
        let hub = [7; 32];
        assert_eq!(store.hub_escrow(&hub).unwrap(), None);
        let cursor = EscrowCursor {
            newest_ledger: 9,
            newest_close_time: 700,
            scanned_to: 8,
        };
        store.set_hub_escrow(&hub, &cursor).unwrap();
        assert_eq!(store.hub_escrow(&hub).unwrap(), Some(cursor));
        for height in 0..u64::from(KEPT_BUNDLES) + 3 {
            store.add_hub_bundle(&hub, height, 1, &[1]).unwrap();
        }
        let bundles = store.hub_bundles(&hub).unwrap();
        assert_eq!(bundles.len(), KEPT_BUNDLES as usize);
        assert_eq!(
            bundles[0].0,
            u64::from(KEPT_BUNDLES) + 2,
            "the newest first"
        );
    }
}

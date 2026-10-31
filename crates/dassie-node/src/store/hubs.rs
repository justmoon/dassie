//! The hubs the node validates (spec/areas/node.md#persistence,
//! spec/areas/hubs.md#the-validator-in-the-node): for each, its consensus
//! state and the state after its last final block, written together before
//! a step's messages go out, so a restart never signs a vote that conflicts
//! with one sent before; the states after a few recent final blocks and
//! after the last blocks of recent epochs, which validators far behind
//! catch up from; how far it followed the escrow on the XRP Ledger, with
//! what its verifier keeps across a restart; and the payout bundles it
//! signed.
//!
//! The rows hold bytes the hub's crate encodes and reads; the store knows
//! only their bounds.

use rusqlite::{OptionalExtension, Transaction, params};

use super::{Cached, Store, StoreError, from_sql_time, to_sql_time};

/// The largest consensus state stored: what the engine can hold above its
/// last final block, its blocks' payloads included.
pub const MAX_HUB_CONSENSUS_BYTES: usize = 512 * 1024 * 1024;

/// The largest ledger stored: the hub's terms bound its tables.
pub const MAX_HUB_LEDGER_BYTES: usize = 256 * 1024 * 1024;

/// The largest state of a validator's verifier stored, with the hub's
/// network and its publishers' digest before it: as large as an XRPL
/// account's state may be, which keeps the same lists, manifests and
/// anchor.
pub const MAX_HUB_VERIFIER_BYTES: usize = 256 * 1024;

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

impl HubStep<'_> {
    /// Whether the step changed nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.consensus.is_none() && self.ledger.is_none() && self.boundaries.is_empty()
    }
}

/// What one step of a hub's validator writes: what the step changed, and
/// the payout bundles it signed, each with the height it pays out from.
#[derive(Clone, Copy, Debug)]
pub struct HubWrite<'a> {
    /// The hub.
    pub hub: &'a [u8; 32],
    /// When the step ran.
    pub now_ms: u64,
    /// What it changed.
    pub step: HubStep<'a>,
    /// The payout bundles it signed.
    pub bundles: &'a [(u64, &'a [u8])],
}

impl HubWrite<'_> {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.step.is_empty() && self.bundles.is_empty()
    }

    fn check(&self) -> Result<(), StoreError> {
        let too_large = self
            .step
            .consensus
            .is_some_and(|bytes| bytes.len() > MAX_HUB_CONSENSUS_BYTES)
            || self
                .step
                .ledger
                .is_some_and(|(_, bytes)| bytes.len() > MAX_HUB_LEDGER_BYTES);
        if too_large {
            return Err(StoreError::Corrupt("hub state over its bound"));
        }
        if self
            .bundles
            .iter()
            .any(|(_, bundle)| bundle.len() > MAX_HUB_BUNDLE_BYTES)
        {
            return Err(StoreError::Corrupt("payout bundle over its bound"));
        }
        Ok(())
    }
}

/// Writes what a step of `hub`'s validator changed: states beyond
/// [`KEPT_TIPS`] and [`KEPT_BOUNDARIES`] go; the newest always stays.
fn write_hub_step(
    transaction: &Transaction<'_>,
    hub: &[u8; 32],
    now_ms: u64,
    step: HubStep<'_>,
) -> Result<(), StoreError> {
    if step.is_empty() {
        return Ok(());
    }
    if let Some(state) = step.consensus {
        transaction.execute_cached(
            "INSERT INTO hub_consensus (hub, updated_at_ms, state) VALUES (?1, ?2, ?3) \
             ON CONFLICT (hub) DO UPDATE SET updated_at_ms = ?2, state = ?3",
            params![&hub[..], to_sql_time(now_ms), state],
        )?;
    }
    if let Some((height, state)) = step.ledger {
        transaction.execute_cached(
            "INSERT INTO hub_states (hub, height, boundary, state) VALUES (?1, ?2, 0, ?3) \
             ON CONFLICT (hub, height) DO UPDATE SET state = ?3",
            params![&hub[..], to_sql_time(height), state],
        )?;
    }
    for height in step.boundaries {
        transaction.execute_cached(
            "UPDATE hub_states SET boundary = 1 WHERE hub = ?1 AND height = ?2",
            params![&hub[..], to_sql_time(*height)],
        )?;
    }
    transaction.execute_cached(
        "DELETE FROM hub_states WHERE hub = ?1 \
         AND height < (SELECT max(height) FROM hub_states WHERE hub = ?1) \
         AND height NOT IN (SELECT height FROM hub_states WHERE hub = ?1 \
           ORDER BY height DESC LIMIT ?2) \
         AND height NOT IN (SELECT height FROM hub_states WHERE hub = ?1 AND boundary = 1 \
           ORDER BY height DESC LIMIT ?3)",
        params![&hub[..], KEPT_TIPS, KEPT_BOUNDARIES],
    )?;
    Ok(())
}

/// Keeps the payout bundle the validator of `hub` signed for the block at
/// `height`, and only the [`KEPT_BUNDLES`] newest of the hub's.
fn write_hub_bundle(
    transaction: &Transaction<'_>,
    hub: &[u8; 32],
    height: u64,
    now_ms: u64,
    bundle: &[u8],
) -> Result<(), StoreError> {
    transaction.execute_cached(
        "INSERT INTO hub_bundles (hub, height, signed_at_ms, bundle) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT (hub, height) DO UPDATE SET signed_at_ms = ?3, bundle = ?4",
        params![&hub[..], to_sql_time(height), to_sql_time(now_ms), bundle],
    )?;
    transaction.execute_cached(
        "DELETE FROM hub_bundles WHERE hub = ?1 AND height NOT IN \
         (SELECT height FROM hub_bundles WHERE hub = ?1 ORDER BY height DESC LIMIT ?2)",
        params![&hub[..], KEPT_BUNDLES],
    )?;
    Ok(())
}

impl Store {
    /// What the validator of `hub` resumes from, if it ran before: its
    /// consensus state and its newest state.
    pub fn hub(&self, hub: &[u8; 32]) -> Result<Option<StoredHub>, StoreError> {
        let row = self
            .lock()
            .query_row_cached(
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
            .query_row_cached(
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
        self.commit_hub_writes(&[HubWrite {
            hub,
            now_ms,
            step,
            bundles: &[],
        }])
    }

    /// Writes the steps of several hubs' validators, each with the payout
    /// bundles it signed, in one transaction: all of them or, on error, none
    /// (spec/areas/hubs.md#the-validator-in-the-node). Every bound is checked
    /// before anything is written.
    pub fn commit_hub_writes(&self, writes: &[HubWrite<'_>]) -> Result<(), StoreError> {
        for write in writes {
            write.check()?;
        }
        if writes.iter().all(HubWrite::is_empty) {
            return Ok(());
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        for write in writes {
            write_hub_step(&transaction, write.hub, write.now_ms, write.step)?;
            for (height, bundle) in write.bundles {
                write_hub_bundle(&transaction, write.hub, *height, write.now_ms, bundle)?;
            }
        }
        #[cfg(feature = "testing")]
        self.fail_if_aimed_at_hubs(&transaction);
        transaction.commit()?;
        Ok(())
    }

    /// How far the validator of `hub` followed its escrow, if it did.
    pub fn hub_escrow(&self, hub: &[u8; 32]) -> Result<Option<EscrowCursor>, StoreError> {
        let row = self
            .lock()
            .query_row_cached(
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

    /// What the validator of `hub` kept of its verifier with its cursor, if
    /// it kept anything: bytes its watcher writes and reads.
    pub fn hub_verifier(&self, hub: &[u8; 32]) -> Result<Option<Vec<u8>>, StoreError> {
        let row = self
            .lock()
            .query_row_cached(
                "SELECT verifier FROM hub_escrow WHERE hub = ?1",
                [&hub[..]],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?;
        Ok(row.flatten())
    }

    /// Records how far the validator of `hub` followed its escrow; what it
    /// kept of its verifier stays.
    pub fn set_hub_escrow(&self, hub: &[u8; 32], cursor: &EscrowCursor) -> Result<(), StoreError> {
        self.lock().execute_cached(
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

    /// Records how far the validator of `hub` followed its escrow, and with
    /// it its verifier's state, in one write, so a restart reads the two as
    /// they were together. Without a state, what was kept of the verifier
    /// is dropped, and a restart starts it afresh.
    pub fn keep_hub_escrow(
        &self,
        hub: &[u8; 32],
        cursor: &EscrowCursor,
        verifier: Option<&[u8]>,
    ) -> Result<(), StoreError> {
        if verifier.is_some_and(|bytes| bytes.len() > MAX_HUB_VERIFIER_BYTES) {
            return Err(StoreError::Corrupt("verifier state over its bound"));
        }
        self.lock().execute_cached(
            "INSERT INTO hub_escrow (hub, newest_ledger, newest_close_time, scanned_to, verifier) \
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (hub) DO UPDATE SET \
             newest_ledger = ?2, newest_close_time = ?3, scanned_to = ?4, \
             verifier = ?5",
            params![
                &hub[..],
                cursor.newest_ledger,
                to_sql_time(cursor.newest_close_time),
                cursor.scanned_to,
                verifier,
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
        self.commit_hub_writes(&[HubWrite {
            hub,
            now_ms,
            step: HubStep::default(),
            bundles: &[(height, bundle)],
        }])
    }

    /// The payout bundles the validator of `hub` signed, the newest first:
    /// the height each pays out from, and the bundle.
    pub fn hub_bundles(&self, hub: &[u8; 32]) -> Result<Vec<(u64, Vec<u8>)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare_cached(
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

    // The steps of several hubs, each with its bundles, go in one commit,
    // and a commit that fails writes none of them: a commit of the
    // engine's is not one it aims at.
    #[test]
    fn steps_of_several_hubs_commit_together_or_not_at_all() {
        let store = Store::open_in_memory().unwrap();
        let (a, b) = ([1; 32], [2; 32]);
        let write = |hub, consensus: &'static [u8], bundles| HubWrite {
            hub,
            now_ms: 10,
            step: HubStep {
                consensus: Some(consensus),
                ledger: Some((3, &[9])),
                boundaries: &[],
            },
            bundles,
        };
        let bundle: &[(u64, &[u8])] = &[(3, &[5, 5])];

        let failed = store.fail_commit(crate::store::Aim::Hub, 0);
        let refused = store.commit_hub_writes(&[write(&a, &[1], bundle), write(&b, &[2], &[])]);
        assert!(refused.is_err());
        assert!(failed.held().is_some_and(|held| held.hub));
        assert_eq!(
            (store.hub(&a).unwrap(), store.hub(&b).unwrap()),
            (None, None)
        );
        assert!(store.hub_bundles(&a).unwrap().is_empty());

        let engine = store.fail_commit(crate::store::Aim::Any, 0);
        store
            .commit_hub_writes(&[write(&a, &[1], bundle), write(&b, &[2], &[])])
            .unwrap();
        assert_eq!(
            engine.held(),
            None,
            "a hub's commit is none of the engine's"
        );
        store.let_commits_through();
        assert_eq!(store.hub(&a).unwrap().unwrap().consensus, vec![1]);
        assert_eq!(store.hub(&b).unwrap().unwrap().consensus, vec![2]);
        assert_eq!(store.hub_bundles(&a).unwrap(), vec![(3, vec![5, 5])]);

        let too_large = vec![0; MAX_HUB_BUNDLE_BYTES + 1];
        let over: &[(u64, &[u8])] = &[(4, &too_large)];
        assert!(
            store
                .commit_hub_writes(&[write(&a, &[7], &[]), write(&b, &[8], over)])
                .is_err()
        );
        assert_eq!(
            store.hub(&a).unwrap().unwrap().consensus,
            vec![1],
            "nothing of it"
        );
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
        assert_eq!(store.hub_verifier(&hub).unwrap(), None);
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

    // spec/areas/hubs.md#the-validator-in-the-node: the verifier's state is
    // written with the cursor, stays while only the cursor moves, is
    // dropped when written without one, and is bounded.
    #[test]
    fn the_verifier_is_kept_with_the_cursor() {
        let store = Store::open_in_memory().unwrap();
        let hub = [7; 32];
        let cursor = |newest_ledger| EscrowCursor {
            newest_ledger,
            newest_close_time: 700,
            scanned_to: 8,
        };
        store
            .keep_hub_escrow(&hub, &cursor(9), Some(&[1, 2, 3]))
            .unwrap();
        assert_eq!(store.hub_escrow(&hub).unwrap(), Some(cursor(9)));
        assert_eq!(store.hub_verifier(&hub).unwrap(), Some(vec![1, 2, 3]));
        store.set_hub_escrow(&hub, &cursor(10)).unwrap();
        assert_eq!(store.hub_escrow(&hub).unwrap(), Some(cursor(10)));
        assert_eq!(store.hub_verifier(&hub).unwrap(), Some(vec![1, 2, 3]));
        store
            .keep_hub_escrow(&hub, &cursor(11), Some(&[4]))
            .unwrap();
        assert_eq!(store.hub_verifier(&hub).unwrap(), Some(vec![4]));
        let over = vec![0; MAX_HUB_VERIFIER_BYTES + 1];
        assert!(
            store
                .keep_hub_escrow(&hub, &cursor(12), Some(&over))
                .is_err()
        );
        assert_eq!(store.hub_escrow(&hub).unwrap(), Some(cursor(11)));
        let largest = vec![5; MAX_HUB_VERIFIER_BYTES];
        store
            .keep_hub_escrow(&hub, &cursor(12), Some(&largest))
            .unwrap();
        assert_eq!(store.hub_verifier(&hub).unwrap(), Some(largest));
        store.keep_hub_escrow(&hub, &cursor(13), None).unwrap();
        assert_eq!(store.hub_escrow(&hub).unwrap(), Some(cursor(13)));
        assert_eq!(store.hub_verifier(&hub).unwrap(), None);
        // The column's own bound is the same.
        let refused = store.lock().execute(
            "UPDATE hub_escrow SET verifier = ?1 WHERE hub = ?2",
            params![over, &hub[..]],
        );
        assert!(refused.is_err());
    }
}

//! Founding settlement hubs (spec/areas/hubs.md#founding-a-hub): what a
//! validator keeps of each founding it took part in, by the terms' nonce;
//! a sponsor's foundings, with their escrow's funding; and the payout
//! bundles a member read from its hub's validators.
//!
//! The rows hold bytes the hub's crate encodes and reads; the store knows
//! only their bounds.

use rusqlite::{OptionalExtension, params};

use super::{Store, StoreError, from_sql_time, to_sql_time};

/// The most foundings a validator keeps: those that hold one of its places
/// (at most its bound on hubs) and those lapsed since it last looked.
pub const MAX_FOUNDING_RECORDS: u32 = 64;

/// The most foundings a sponsor has at once, finished ones included until
/// the owner drops them.
pub const MAX_FOUNDINGS: u32 = 8;

/// The most payout bundles a member keeps of one hub: one per validator.
pub const MAX_PAYOUT_ROWS: u32 = 16;

/// The largest rules or founding state stored.
const MAX_STATE_BYTES: usize = 65_536;

/// What a validator keeps of one founding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundingRecordRow {
    /// The terms' nonce.
    pub nonce: [u8; 32],
    /// The sponsor's node key.
    pub sponsor: [u8; 32],
    /// The digest of the terms it accepted.
    pub terms: [u8; 32],
    /// When it accepted them.
    pub accepted_at_ms: u64,
    /// The rules whose genesis it signed, encoded, and the genesis
    /// timestamp.
    pub genesis: Option<(Vec<u8>, u64)>,
    /// Whether it runs the hub.
    pub running: bool,
}

/// A sponsor's founding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundingRow {
    /// The terms' nonce.
    pub nonce: [u8; 32],
    /// Where it stands, encoded.
    pub state: Vec<u8>,
    /// The XRPL server its escrow is set up on.
    pub xrpl: String,
    /// What funds the escrow, in drops.
    pub funding_drops: u64,
    /// The payment that funds it, once made: its ID with the XRP scheme.
    pub escrow_payment: Option<String>,
    /// What went wrong last, for the owner.
    pub problem: Option<String>,
    /// When the owner asked for it.
    pub created_at_ms: u64,
    /// When it last changed.
    pub updated_at_ms: u64,
}

type RecordColumns = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    Option<Vec<u8>>,
    Option<i64>,
    bool,
);

fn array(bytes: &[u8]) -> Result<[u8; 32], StoreError> {
    bytes
        .try_into()
        .map_err(|_| StoreError::Corrupt("a hub founding's key or digest"))
}

fn record_of(columns: RecordColumns) -> Result<FoundingRecordRow, StoreError> {
    let (nonce, sponsor, terms, accepted, rules, genesis_ms, running) = columns;
    Ok(FoundingRecordRow {
        nonce: array(&nonce)?,
        sponsor: array(&sponsor)?,
        terms: array(&terms)?,
        accepted_at_ms: from_sql_time(accepted),
        genesis: rules.zip(genesis_ms.map(from_sql_time)),
        running,
    })
}

impl Store {
    /// What the validator keeps of the foundings it took part in, by
    /// nonce, at most [`MAX_FOUNDING_RECORDS`].
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn hub_founding_records(&self) -> Result<Vec<FoundingRecordRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT nonce, sponsor, terms, accepted_at_ms, rules, genesis_ms, running \
             FROM hub_founding_records ORDER BY nonce LIMIT ?1",
        )?;
        let rows = statement
            .query_map([MAX_FOUNDING_RECORDS], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })?
            .collect::<Result<Vec<RecordColumns>, _>>()?;
        rows.into_iter().map(record_of).collect()
    }

    /// Keeps `record`, in place of the one of its nonce, and drops those of
    /// `lapsed`, in one transaction.
    ///
    /// # Errors
    ///
    /// The database failed, the rules are over their bound, or the table
    /// is full.
    pub fn put_hub_founding_record(
        &self,
        record: &FoundingRecordRow,
        lapsed: &[[u8; 32]],
    ) -> Result<(), StoreError> {
        if record
            .genesis
            .as_ref()
            .is_some_and(|(rules, _)| rules.is_empty() || rules.len() > MAX_STATE_BYTES)
        {
            return Err(StoreError::Corrupt("hub rules over their bound"));
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        for nonce in lapsed {
            transaction.execute(
                "DELETE FROM hub_founding_records WHERE nonce = ?1 AND running = 0",
                [&nonce[..]],
            )?;
        }
        let (rules, genesis_ms) = record.genesis.as_ref().map_or((None, None), |(rules, at)| {
            (Some(rules.as_slice()), Some(to_sql_time(*at)))
        });
        transaction.execute(
            "INSERT INTO hub_founding_records \
             (nonce, sponsor, terms, accepted_at_ms, rules, genesis_ms, running) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (nonce) DO UPDATE SET \
             sponsor = ?2, terms = ?3, accepted_at_ms = ?4, rules = ?5, genesis_ms = ?6, \
             running = ?7",
            params![
                &record.nonce[..],
                &record.sponsor[..],
                &record.terms[..],
                to_sql_time(record.accepted_at_ms),
                rules,
                genesis_ms,
                record.running,
            ],
        )?;
        let count: u32 =
            transaction.query_row("SELECT count(*) FROM hub_founding_records", [], |row| {
                row.get(0)
            })?;
        if count > MAX_FOUNDING_RECORDS {
            return Err(StoreError::Corrupt("too many hub foundings kept"));
        }
        transaction.commit()?;
        Ok(())
    }

    /// The sponsor's foundings, the oldest first, at most
    /// [`MAX_FOUNDINGS`].
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub fn hub_foundings(&self) -> Result<Vec<FoundingRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT nonce, state, xrpl, funding_drops, escrow_payment, problem, \
             created_at_ms, updated_at_ms FROM hub_foundings \
             ORDER BY created_at_ms, nonce LIMIT ?1",
        )?;
        let rows = statement
            .query_map([MAX_FOUNDINGS], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(
                |(nonce, state, xrpl, drops, payment, problem, created, updated)| {
                    Ok(FoundingRow {
                        nonce: array(&nonce)?,
                        state,
                        xrpl,
                        funding_drops: from_sql_time(drops),
                        escrow_payment: payment,
                        problem,
                        created_at_ms: from_sql_time(created),
                        updated_at_ms: from_sql_time(updated),
                    })
                },
            )
            .collect()
    }

    /// Keeps `founding`, in place of the one of its nonce: a new one only
    /// while the sponsor has fewer than [`MAX_FOUNDINGS`]. Whether it kept
    /// it.
    ///
    /// # Errors
    ///
    /// The database failed, or the state is over its bound.
    pub fn put_hub_founding(&self, founding: &FoundingRow) -> Result<bool, StoreError> {
        if founding.state.is_empty() || founding.state.len() > MAX_STATE_BYTES {
            return Err(StoreError::Corrupt("a hub founding over its bound"));
        }
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let known = transaction
            .query_row(
                "SELECT 1 FROM hub_foundings WHERE nonce = ?1",
                [&founding.nonce[..]],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let count: u32 =
            transaction.query_row("SELECT count(*) FROM hub_foundings", [], |row| row.get(0))?;
        if !known && count >= MAX_FOUNDINGS {
            return Ok(false);
        }
        transaction.execute(
            "INSERT INTO hub_foundings (nonce, state, xrpl, funding_drops, escrow_payment, \
             problem, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT (nonce) DO UPDATE SET state = ?2, xrpl = ?3, funding_drops = ?4, \
             escrow_payment = ?5, problem = ?6, updated_at_ms = ?8",
            params![
                &founding.nonce[..],
                founding.state,
                founding.xrpl,
                to_sql_time(founding.funding_drops),
                founding.escrow_payment,
                founding.problem,
                to_sql_time(founding.created_at_ms),
                to_sql_time(founding.updated_at_ms),
            ],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// Writes `founding` over the one of its nonce, if the owner has not
    /// dropped it: whether it was there.
    ///
    /// # Errors
    ///
    /// The database failed, or the state is over its bound.
    pub fn update_hub_founding(&self, founding: &FoundingRow) -> Result<bool, StoreError> {
        if founding.state.is_empty() || founding.state.len() > MAX_STATE_BYTES {
            return Err(StoreError::Corrupt("a hub founding over its bound"));
        }
        let updated = self.lock().execute(
            "UPDATE hub_foundings SET state = ?2, escrow_payment = ?3, problem = ?4, \
             updated_at_ms = ?5 WHERE nonce = ?1",
            params![
                &founding.nonce[..],
                founding.state,
                founding.escrow_payment,
                founding.problem,
                to_sql_time(founding.updated_at_ms),
            ],
        )?;
        Ok(updated > 0)
    }

    /// Drops the founding of `nonce`: whether there was one.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn remove_hub_founding(&self, nonce: &[u8; 32]) -> Result<bool, StoreError> {
        let removed = self
            .lock()
            .execute("DELETE FROM hub_foundings WHERE nonce = ?1", [&nonce[..]])?;
        Ok(removed > 0)
    }

    /// The payout bundles kept of `hub`, by validator: its place, the
    /// height the bundle pays out, and the bundle.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn hub_payouts(&self, hub: &[u8; 32]) -> Result<Vec<(u8, u64, Vec<u8>)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT validator, height, bundle FROM hub_payouts WHERE hub = ?1 \
             ORDER BY validator LIMIT ?2",
        )?;
        let rows = statement
            .query_map(params![&hub[..], MAX_PAYOUT_ROWS], |row| {
                Ok((
                    row.get::<_, u8>(0)?,
                    from_sql_time(row.get(1)?),
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Keeps `bundle`, validator `validator`'s of `hub`, paying out the
    /// block at `height`, in place of the one kept before.
    ///
    /// # Errors
    ///
    /// The database failed, or the bundle is over its bound.
    pub fn put_hub_payout(
        &self,
        hub: &[u8; 32],
        validator: u8,
        height: u64,
        now_ms: u64,
        bundle: &[u8],
    ) -> Result<(), StoreError> {
        if bundle.is_empty() || bundle.len() > super::MAX_HUB_BUNDLE_BYTES {
            return Err(StoreError::Corrupt("payout bundle over its bound"));
        }
        self.lock().execute(
            "INSERT INTO hub_payouts (hub, validator, height, received_at_ms, bundle) \
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (hub, validator) DO UPDATE SET \
             height = ?3, received_at_ms = ?4, bundle = ?5",
            params![
                &hub[..],
                validator,
                to_sql_time(height),
                to_sql_time(now_ms),
                bundle
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_foundings_and_payouts_read_back() {
        let store = Store::open_in_memory().unwrap();
        let record = FoundingRecordRow {
            nonce: [1; 32],
            sponsor: [2; 32],
            terms: [3; 32],
            accepted_at_ms: 5,
            genesis: None,
            running: false,
        };
        store.put_hub_founding_record(&record, &[]).unwrap();
        let signed = FoundingRecordRow {
            genesis: Some((vec![9; 10], 7)),
            running: true,
            ..record.clone()
        };
        let other = FoundingRecordRow {
            nonce: [4; 32],
            ..record.clone()
        };
        store.put_hub_founding_record(&other, &[]).unwrap();
        store.put_hub_founding_record(&signed, &[[4; 32]]).unwrap();
        assert_eq!(store.hub_founding_records().unwrap(), vec![signed]);

        let founding = |nonce| FoundingRow {
            nonce: [nonce; 32],
            state: vec![1, 2],
            xrpl: "wss://x".into(),
            funding_drops: 2_000_000,
            escrow_payment: None,
            problem: None,
            created_at_ms: u64::from(nonce),
            updated_at_ms: 1,
        };
        for nonce in 0..u8::try_from(MAX_FOUNDINGS).unwrap() {
            assert!(store.put_hub_founding(&founding(nonce)).unwrap());
        }
        assert!(!store.put_hub_founding(&founding(100)).unwrap(), "full");
        let mut changed = founding(0);
        changed.escrow_payment = Some("ab".into());
        assert!(store.put_hub_founding(&changed).unwrap());
        assert_eq!(store.hub_foundings().unwrap()[0], changed);
        assert!(store.remove_hub_founding(&[0; 32]).unwrap());
        assert!(!store.remove_hub_founding(&[0; 32]).unwrap());

        store.put_hub_payout(&[7; 32], 2, 10, 1, &[1, 2]).unwrap();
        store.put_hub_payout(&[7; 32], 2, 11, 1, &[3]).unwrap();
        assert_eq!(store.hub_payouts(&[7; 32]).unwrap(), vec![(2, 11, vec![3])]);
    }
}

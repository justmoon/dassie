//! Open Payments' tables: incoming payments, which the engine writes with
//! the journal rows that pay into them, and which ledger `$host` names.

use std::fmt;

use dassie_ledger::LedgerId;
use rusqlite::{OptionalExtension, Row, Transaction, params};

use super::{Store, StoreError, from_sql_time, to_sql_time};

/// Most open incoming payments read back at a start.
pub const MAX_OPEN_LOADED: u32 = 65_536;

/// A resource's ID: 16 random bytes, written as a UUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaymentId(pub [u8; 16]);

impl PaymentId {
    /// Reads the UUID form, in either case.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex: Vec<u8> = text.bytes().filter(|&byte| byte != b'-').collect();
        let dashes_valid = text.len() == 36
            && [8, 13, 18, 23]
                .iter()
                .all(|&at| text.as_bytes().get(at) == Some(&b'-'));
        if !dashes_valid || hex.len() != 32 {
            return None;
        }
        let mut bytes = [0u8; 16];
        let (pairs, _) = hex.as_chunks::<2>();
        for (slot, pair) in bytes.iter_mut().zip(pairs) {
            let text = std::str::from_utf8(pair).ok()?;
            *slot = u8::from_str_radix(text, 16).ok()?;
        }
        Some(Self(bytes))
    }

    /// An ID from random bytes, marked as a version 4 UUID.
    #[must_use]
    pub fn from_random(mut bytes: [u8; 16]) -> Self {
        if let Some(version) = bytes.get_mut(6) {
            *version = (*version & 0x0f) | 0x40;
        }
        if let Some(variant) = bytes.get_mut(8) {
            *variant = (*variant & 0x3f) | 0x80;
        }
        Self(bytes)
    }
}

impl fmt::Display for PaymentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, byte) in self.0.iter().enumerate() {
            if matches!(index, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// An incoming payment as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingRow {
    /// Its ID.
    pub id: PaymentId,
    /// The ledger it pays into.
    pub ledger: LedgerId,
    /// The wallet address it was created at.
    pub wallet_address: String,
    /// The client that created it, as grants name clients; none for the
    /// owner.
    pub client: Option<String>,
    /// The most it accepts, if limited.
    pub incoming_amount: Option<u64>,
    /// What it has received.
    pub received_amount: u64,
    /// Its STREAM destination.
    pub ilp_address: String,
    /// Its STREAM shared secret.
    pub shared_secret: [u8; 32],
    /// The creator's metadata, as JSON.
    pub metadata: Option<String>,
    /// When it was created.
    pub created_at_ms: u64,
    /// When it stops accepting money.
    pub expires_at_ms: u64,
    /// When it was completed, if it was.
    pub completed_at_ms: Option<u64>,
}

impl IncomingRow {
    /// Whether it still accepts money at `now_ms`.
    #[must_use]
    pub fn is_open(&self, now_ms: u64) -> bool {
        self.completed_at_ms.is_none() && now_ms < self.expires_at_ms
    }
}

/// What a packet changed of an incoming payment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncomingProgress {
    /// What it has received now.
    pub received_amount: u64,
    /// When it was completed, if it now is.
    pub completed_at_ms: Option<u64>,
}

/// A page of a list of incoming payments, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingQuery<'a> {
    /// The ledger whose payments to list.
    pub ledger: LedgerId,
    /// Only those this client created, if given.
    pub client: Option<&'a str>,
    /// The item to start after (forward) or before (backward).
    pub cursor: Option<PaymentId>,
    /// Toward older items (`first`) or newer ones (`last`).
    pub forward: bool,
    /// How many.
    pub count: u32,
}

/// A page, with whether items lie beyond either end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingPage {
    /// The items, newest first.
    pub rows: Vec<IncomingRow>,
    /// Whether newer items exist.
    pub has_newer: bool,
    /// Whether older items exist.
    pub has_older: bool,
}

const COLUMNS: &str = "id, ledger, wallet_address, client, incoming_amount, received_amount, \
     ilp_address, shared_secret, metadata, created_at_ms, expires_at_ms, completed_at_ms";

impl Store {
    /// The incoming payments still open at `now_ms`, oldest first.
    pub fn open_incoming_payments(&self, now_ms: u64) -> Result<Vec<IncomingRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM incoming_payments \
             WHERE completed_at_ms IS NULL AND expires_at_ms > ?1 \
             ORDER BY created_at_ms, id LIMIT ?2"
        ))?;
        statement
            .query_map(params![to_sql_time(now_ms), MAX_OPEN_LOADED], read_incoming)?
            .map(|row| row?)
            .collect()
    }

    /// One incoming payment.
    pub fn incoming_payment(&self, id: PaymentId) -> Result<Option<IncomingRow>, StoreError> {
        let connection = self.lock();
        let row = connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM incoming_payments WHERE id = ?1"),
                [id.0.as_slice()],
                read_incoming,
            )
            .optional()?;
        row.transpose()
    }

    /// A page of incoming payments (Open Payments pagination).
    pub fn incoming_page(&self, query: &IncomingQuery<'_>) -> Result<IncomingPage, StoreError> {
        let connection = self.lock();
        let anchor: Option<(i64, Vec<u8>)> = match query.cursor {
            Some(cursor) => connection
                .query_row(
                    "SELECT created_at_ms, id FROM incoming_payments WHERE id = ?1 AND ledger = ?2",
                    params![cursor.0.as_slice(), query.ledger.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?,
            None => None,
        };
        if query.cursor.is_some() && anchor.is_none() {
            return Ok(IncomingPage {
                rows: Vec::new(),
                has_newer: false,
                has_older: false,
            });
        }
        let (created, id) = anchor.unzip();
        let (comparison, order) = if query.forward {
            ("<", "DESC")
        } else {
            (">", "ASC")
        };
        let filter = format!(
            "ledger = ?1 AND (?2 IS NULL OR client = ?2) \
             AND (?3 IS NULL OR (created_at_ms, id) {comparison} (?3, ?4))"
        );
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM incoming_payments WHERE {filter} \
             ORDER BY created_at_ms {order}, id {order} LIMIT ?5"
        ))?;
        let limit = query.count.saturating_add(1);
        let ledger = query.ledger.as_str();
        let mut rows = statement
            .query_map(
                params![ledger, query.client, created, id, limit],
                read_incoming,
            )?
            .map(|row| row?)
            .collect::<Result<Vec<_>, StoreError>>()?;
        let beyond = rows.len() > usize::try_from(query.count).unwrap_or(usize::MAX);
        rows.truncate(usize::try_from(query.count).unwrap_or(usize::MAX));
        let (has_newer, has_older) = if query.forward {
            (query.cursor.is_some(), beyond)
        } else {
            rows.reverse();
            (beyond, query.cursor.is_some())
        };
        Ok(IncomingPage {
            rows,
            has_newer,
            has_older,
        })
    }

    /// Up to `limit` incoming payments written or changed after revision
    /// `after`, in that order, with their revisions: a reader that pages
    /// with the last revision it saw sees every change once.
    pub fn incoming_after(
        &self,
        after: u64,
        limit: u32,
    ) -> Result<Vec<(u64, IncomingRow)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS}, revision FROM incoming_payments WHERE revision > ?1 \
             ORDER BY revision LIMIT ?2"
        ))?;
        statement
            .query_map(params![to_sql_time(after), limit], |row| {
                Ok((row.get::<_, i64>(12)?, read_incoming(row)?))
            })?
            .map(|row| {
                let (revision, payment) = row?;
                Ok((from_sql_time(revision), payment?))
            })
            .collect()
    }

    /// The newest `limit` incoming payments, newest first, and the highest
    /// revision: where a live view starts.
    pub fn incoming_latest(&self, limit: u32) -> Result<(u64, Vec<IncomingRow>), StoreError> {
        let connection = self.lock();
        let revision: Option<i64> =
            connection.query_row("SELECT max(revision) FROM incoming_payments", [], |row| {
                row.get(0)
            })?;
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM incoming_payments ORDER BY created_at_ms DESC, id DESC LIMIT ?1"
        ))?;
        let rows = statement
            .query_map([limit], read_incoming)?
            .map(|row| row?)
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok((revision.map_or(0, from_sql_time), rows))
    }

    /// The ledger `$host` names, if one was chosen.
    pub fn default_ledger(&self) -> Result<Option<LedgerId>, StoreError> {
        let stored: Option<String> = self.lock().query_row(
            "SELECT default_ledger FROM payment_settings WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        stored
            .map(|text| LedgerId::parse(&text).ok_or(StoreError::Corrupt("default ledger")))
            .transpose()
    }

    /// Chooses the ledger `$host` names, or goes back to the lowest active.
    pub fn set_default_ledger(&self, ledger: Option<LedgerId>) -> Result<(), StoreError> {
        self.lock().execute(
            "UPDATE payment_settings SET default_ledger = ?1 WHERE id = 1",
            [ledger.map(|ledger| ledger.as_str())],
        )?;
        Ok(())
    }
}

type IncomingResult = Result<IncomingRow, StoreError>;

fn read_incoming(row: &Row<'_>) -> rusqlite::Result<IncomingResult> {
    let id: Vec<u8> = row.get(0)?;
    let ledger: String = row.get(1)?;
    let wallet_address: String = row.get(2)?;
    let client: Option<String> = row.get(3)?;
    let incoming: Option<i64> = row.get(4)?;
    let received: i64 = row.get(5)?;
    let ilp_address: String = row.get(6)?;
    let secret: Vec<u8> = row.get(7)?;
    let metadata: Option<String> = row.get(8)?;
    let created: i64 = row.get(9)?;
    let expires: i64 = row.get(10)?;
    let completed: Option<i64> = row.get(11)?;
    Ok((|| {
        let amount = |value: i64| u64::try_from(value).map_err(|_| StoreError::Corrupt("amount"));
        Ok(IncomingRow {
            id: PaymentId(
                id.try_into()
                    .map_err(|_| StoreError::Corrupt("payment ID"))?,
            ),
            ledger: LedgerId::parse(&ledger).ok_or(StoreError::Corrupt("payment ledger"))?,
            wallet_address,
            client,
            incoming_amount: incoming.map(amount).transpose()?,
            received_amount: amount(received)?,
            ilp_address,
            shared_secret: secret
                .try_into()
                .map_err(|_| StoreError::Corrupt("shared secret"))?,
            metadata,
            created_at_ms: from_sql_time(created),
            expires_at_ms: from_sql_time(expires),
            completed_at_ms: completed.map(from_sql_time),
        })
    })())
}

fn sql_amount(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt("amount beyond i64"))
}

const NEXT_REVISION: &str = "(SELECT coalesce(max(revision), 0) + 1 FROM incoming_payments)";

/// Writes the incoming payments a batch created and what it paid into
/// them.
pub(super) fn write_incoming(
    transaction: &Transaction<'_>,
    created: &[IncomingRow],
    progress: &std::collections::BTreeMap<PaymentId, IncomingProgress>,
) -> Result<(), StoreError> {
    for row in created {
        transaction.execute(
            &format!(
                "INSERT INTO incoming_payments ({COLUMNS}, revision) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, {NEXT_REVISION})"
            ),
            params![
                row.id.0.as_slice(),
                row.ledger.as_str(),
                row.wallet_address,
                row.client,
                row.incoming_amount.map(sql_amount).transpose()?,
                sql_amount(row.received_amount)?,
                row.ilp_address,
                row.shared_secret.as_slice(),
                row.metadata,
                to_sql_time(row.created_at_ms),
                to_sql_time(row.expires_at_ms),
                row.completed_at_ms.map(to_sql_time),
            ],
        )?;
    }
    for (id, change) in progress {
        let updated = transaction.execute(
            &format!(
                "UPDATE incoming_payments SET received_amount = ?2, completed_at_ms = ?3, \
                 revision = {NEXT_REVISION} WHERE id = ?1"
            ),
            params![
                id.0.as_slice(),
                sql_amount(change.received_amount)?,
                change.completed_at_ms.map(to_sql_time),
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::Corrupt("incoming payment"));
        }
    }
    Ok(())
}

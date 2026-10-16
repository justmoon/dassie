//! Open Payments' tables: incoming payments, which the engine writes with
//! the journal rows that pay into them, and which ledger `$host` names.

use std::fmt;

use dassie_ledger::LedgerId;
use rusqlite::{OptionalExtension, Row, ToSql, Transaction, params, params_from_iter};

use super::{Store, StoreError, from_sql_time, to_sql_time};

/// Most open incoming payments read back at a start.
pub const MAX_OPEN_LOADED: u32 = 65_536;

/// How long an incoming payment that received nothing is kept after it
/// closed or expired.
pub const UNPAID_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

/// Most rows one pruning deletes.
const PRUNE_BATCH: u32 = 1_000;

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
        // Hex digits only: `from_str_radix` also takes a sign, so `+a`
        // would name the same ID as `0a`.
        if !dashes_valid || hex.len() != 32 || !hex.iter().all(u8::is_ascii_hexdigit) {
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

/// Which credentials of an incoming payment a STREAM connection uses: each
/// read of the payment gets its own, so each payment that pays it has a
/// connection of its own. [`ConnectionKey::LEGACY`] for those issued with
/// the payment before connections had keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConnectionKey(pub [u8; 16]);

impl ConnectionKey {
    /// The key of credentials issued before connections had keys.
    pub const LEGACY: Self = Self([0; 16]);

    /// A key from random bytes: never [`Self::LEGACY`].
    #[must_use]
    pub fn from_random(mut bytes: [u8; 16]) -> Self {
        if bytes == [0; 16] {
            bytes[0] = 1;
        }
        Self(bytes)
    }
}

/// What our receiver took into an incoming payment from one packet, over
/// one of its connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncomingPaid {
    /// The payment.
    pub payment: PaymentId,
    /// The connection.
    pub connection: ConnectionKey,
    /// How much.
    pub amount: u64,
}

/// What a packet changed of an incoming payment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncomingProgress {
    /// What it has received now.
    pub received_amount: u64,
    /// When it was completed, if it now is.
    pub completed_at_ms: Option<u64>,
}

/// A page of a list of payments, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingQuery<'a> {
    /// The ledger whose payments to list.
    pub ledger: LedgerId,
    /// Only those one of these reaches: at most [`MAX_LISTED`], one per
    /// access item of a grant.
    pub reach: &'a [Listed<'a>],
    /// The item to start after (forward) or before (backward).
    pub cursor: Option<PaymentId>,
    /// Toward older items (`first`) or newer ones (`last`).
    pub forward: bool,
    /// How many.
    pub count: u32,
}

/// Payments a list reaches: those of a client, to a receiver, both, or
/// all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Listed<'a> {
    /// Only those this client created, if given.
    pub client: Option<&'a str>,
    /// Only those paying this incoming payment, if given: outgoing
    /// payments only.
    pub receiver: Option<&'a str>,
}

/// Most alternatives one list query takes: more than a grant has access
/// items.
pub const MAX_LISTED: usize = 4;

/// The SQL that keeps the rows one of `reach` reaches, with its
/// parameters numbered from `first`: none reach nothing.
fn listed_filter<'a>(reach: &[Listed<'a>], first: usize) -> (String, Vec<&'a str>) {
    assert!(reach.len() <= MAX_LISTED, "too many list alternatives");
    let mut values = Vec::new();
    let mut alternatives = Vec::with_capacity(reach.len());
    for listed in reach {
        let mut parts = Vec::new();
        for (column, value) in [("client", listed.client), ("receiver", listed.receiver)] {
            if let Some(value) = value {
                values.push(value);
                parts.push(format!("{column} = ?{}", first + values.len() - 1));
            }
        }
        alternatives.push(if parts.is_empty() {
            "1".to_string()
        } else {
            parts.join(" AND ")
        });
    }
    if alternatives.is_empty() {
        alternatives.push("0".to_string());
    }
    (format!("({})", alternatives.join(" OR ")), values)
}

/// A page, with whether items lie beyond either end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    /// The items, newest first.
    pub rows: Vec<T>,
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

    /// What each connection of the open incoming payments received: at
    /// most `limit` rows.
    pub fn open_incoming_connections(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<(PaymentId, ConnectionKey, u64)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT c.incoming_payment, c.connection, c.received \
             FROM incoming_connections AS c JOIN incoming_payments AS p \
             ON p.id = c.incoming_payment \
             WHERE p.completed_at_ms IS NULL AND p.expires_at_ms > ?1 LIMIT ?2",
        )?;
        statement
            .query_map(params![to_sql_time(now_ms), limit], |row| {
                let payment: Vec<u8> = row.get(0)?;
                let key: Vec<u8> = row.get(1)?;
                let received: i64 = row.get(2)?;
                Ok((|| {
                    let corrupt = || StoreError::Corrupt("incoming connection");
                    Ok((
                        PaymentId(<[u8; 16]>::try_from(payment.as_slice()).map_err(|_| corrupt())?),
                        ConnectionKey(<[u8; 16]>::try_from(key.as_slice()).map_err(|_| corrupt())?),
                        u64::try_from(received).map_err(|_| corrupt())?,
                    ))
                })())
            })?
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

    /// A page of incoming payments (Open Payments pagination), or `None`
    /// when the cursor is not an item the query could list.
    pub fn incoming_page(
        &self,
        query: &IncomingQuery<'_>,
    ) -> Result<Option<IncomingPage>, StoreError> {
        page(
            &self.lock(),
            "incoming_payments",
            COLUMNS,
            read_incoming,
            query,
        )
    }

    /// Forgets incoming payments that received nothing, a while after they
    /// closed or expired: anyone may create them, so they must not pile
    /// up. Returns how many went.
    pub fn prune_incoming(&self, now_ms: u64) -> Result<u32, StoreError> {
        let before = to_sql_time(now_ms.saturating_sub(UNPAID_RETENTION_MS));
        let connection = self.lock();
        // What a closed payment's connections received is not read again.
        connection.execute(
            "DELETE FROM incoming_connections WHERE (incoming_payment, connection) IN \
             (SELECT c.incoming_payment, c.connection FROM incoming_connections AS c \
             JOIN incoming_payments AS p ON p.id = c.incoming_payment \
             WHERE p.completed_at_ms IS NOT NULL OR p.expires_at_ms <= ?1 LIMIT ?2)",
            params![to_sql_time(now_ms), PRUNE_BATCH],
        )?;
        let removed = connection.execute(
            "DELETE FROM incoming_payments WHERE id IN (SELECT id FROM incoming_payments \
             WHERE received_amount = 0 AND coalesce(completed_at_ms, expires_at_ms) <= ?1 \
             LIMIT ?2)",
            params![before, PRUNE_BATCH],
        )?;
        u32::try_from(removed).map_err(|_| StoreError::Corrupt("row count"))
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

/// A page of incoming payments.
pub type IncomingPage = Page<IncomingRow>;

/// A page of `table`'s rows by `(created_at_ms, id)`, newest first, or
/// `None` when the cursor is not an item the query could list.
pub(super) fn page<T>(
    connection: &rusqlite::Connection,
    table: &str,
    columns: &str,
    read: fn(&Row<'_>) -> rusqlite::Result<Result<T, StoreError>>,
    query: &IncomingQuery<'_>,
) -> Result<Option<Page<T>>, StoreError> {
    let ledger = query.ledger.as_str();
    let anchor: Option<(i64, Vec<u8>)> = match query.cursor {
        Some(cursor) => {
            let (reached, values) = listed_filter(query.reach, 3);
            let mut bound: Vec<&dyn ToSql> = vec![&cursor.0, &ledger];
            bound.extend(values.iter().map(|value| value as &dyn ToSql));
            connection
                .query_row(
                    &format!(
                        "SELECT created_at_ms, id FROM {table} \
                         WHERE id = ?1 AND ledger = ?2 AND {reached}"
                    ),
                    params_from_iter(bound),
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
        }
        None => None,
    };
    if query.cursor.is_some() && anchor.is_none() {
        return Ok(None);
    }
    let (created, id) = anchor.unzip();
    let (comparison, order) = if query.forward {
        ("<", "DESC")
    } else {
        (">", "ASC")
    };
    let (reached, values) = listed_filter(query.reach, 5);
    let filter = format!(
        "ledger = ?1 AND {reached} \
         AND (?2 IS NULL OR (created_at_ms, id) {comparison} (?2, ?3))"
    );
    let mut statement = connection.prepare(&format!(
        "SELECT {columns} FROM {table} WHERE {filter} \
         ORDER BY created_at_ms {order}, id {order} LIMIT ?4"
    ))?;
    let limit = query.count.saturating_add(1);
    let mut bound: Vec<&dyn ToSql> = vec![&ledger, &created, &id, &limit];
    bound.extend(values.iter().map(|value| value as &dyn ToSql));
    let mut rows = statement
        .query_map(params_from_iter(bound), read)?
        .map(|row| row?)
        .collect::<Result<Vec<_>, StoreError>>()?;
    let count = usize::try_from(query.count).unwrap_or(usize::MAX);
    let beyond = rows.len() > count;
    rows.truncate(count);
    let (has_newer, has_older) = if query.forward {
        (query.cursor.is_some(), beyond)
    } else {
        rows.reverse();
        (beyond, query.cursor.is_some())
    };
    Ok(Some(Page {
        rows,
        has_newer,
        has_older,
    }))
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
    connections: &std::collections::BTreeMap<(PaymentId, ConnectionKey), u64>,
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
    for ((id, connection), received) in connections {
        transaction.execute(
            "INSERT INTO incoming_connections (incoming_payment, connection, received) \
             VALUES (?1, ?2, ?3) ON CONFLICT (incoming_payment, connection) \
             DO UPDATE SET received = ?3",
            params![
                id.0.as_slice(),
                connection.0.as_slice(),
                sql_amount(*received)?
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn ids_have_one_spelling() {
        let id = "0a1b2c3d-4e5f-4a7b-8c9d-0e1f2a3b4c5d";
        assert_eq!(PaymentId::parse(id).unwrap().to_string(), id);
        assert_eq!(
            PaymentId::parse(&id.to_uppercase()),
            PaymentId::parse(id),
            "either case"
        );
        // `from_str_radix` reads `+a` as `0a`.
        assert_eq!(
            PaymentId::parse("+a1b2c3d-4e5f-4a7b-8c9d-0e1f2a3b4c5d"),
            None
        );
        for bad in [
            "",
            "0a1b2c3d4e5f4a7b8c9d0e1f2a3b4c5d",
            "0a1b2c3d-4e5f-4a7b-8c9d-0e1f2a3b4c5",
            "0a1b2c3d-4e5f-4a7b-8c9d-0e1f2a3b4c5g",
            "0a1b2c3d-4e5f-4a7b-8c9d--e1f2a3b4c5d",
            "0a1b2c3d-4e5f-4a7b-8c9d-0e1f2a3b4cé",
        ] {
            assert_eq!(PaymentId::parse(bad), None, "{bad}");
        }
    }

    /// IDs with one character changed, and any text.
    fn text() -> impl Strategy<Value = String> {
        let by = prop_oneof![
            prop::sample::select(vec!['+', '-', ' ', 'A', 'G']),
            any::<char>()
        ];
        let changed = (any::<[u8; 16]>(), 0..36usize, by).prop_map(|(bytes, at, by)| {
            let mut chars: Vec<char> = PaymentId(bytes).to_string().chars().collect();
            chars[at] = by;
            chars.into_iter().collect()
        });
        prop_oneof![changed, "\\PC{0,40}"]
    }

    proptest! {
        // Path segments and cursors come from the network (D16).
        #[test]
        fn parsing_never_panics_and_is_canonical(text in text()) {
            if let Some(id) = PaymentId::parse(&text) {
                prop_assert_eq!(id.to_string(), text.to_ascii_lowercase());
            }
        }
    }
}

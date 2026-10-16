//! Spending through Open Payments: quotes, outgoing payments, and what
//! each grant spent per window of its interval.
//!
//! Quotes move no money: the resource server writes them as it makes
//! them. Outgoing payments and grant spend are the engine's, written with
//! the journal rows that move the money, so a payment never shows what the
//! books lack.

use std::collections::BTreeMap;

use dassie_ledger::LedgerId;
use rusqlite::{OptionalExtension, Row, Transaction, params};

use super::payments::{IncomingQuery, Page, page};
use super::{PaymentId, Store, StoreError, from_sql_time, to_sql_time};

/// Most clients' quotes not yet expired.
pub const MAX_LIVE_QUOTES: u32 = 4_096;
/// Most of the owner's quotes not yet expired: apart from clients', so
/// they cannot crowd the owner out.
pub const MAX_LIVE_OWNER_QUOTES: u32 = 256;

/// Most outgoing payments still sending that are loaded at a start.
pub const MAX_SENDING_LOADED: u32 = 4_096;

/// An amount in an asset that may not be the ledger's: what a receiver
/// gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForeignAmount {
    /// The value.
    pub value: u64,
    /// Its asset.
    pub asset_code: String,
    /// Its scale.
    pub asset_scale: u8,
}

/// A quote as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteRow {
    /// Its ID.
    pub id: PaymentId,
    /// The ledger that pays.
    pub ledger: LedgerId,
    /// The wallet address that pays.
    pub wallet_address: String,
    /// The client that made it; none for the owner.
    pub client: Option<String>,
    /// The incoming payment it pays.
    pub receiver: String,
    /// The receiver's STREAM destination.
    pub destination: String,
    /// The receiver's STREAM shared secret.
    pub shared_secret: [u8; 32],
    /// What it debits.
    pub debit_amount: u64,
    /// What the receiver gets.
    pub receive_amount: ForeignAmount,
    /// When it was made.
    pub created_at_ms: u64,
    /// When it can no longer be paid.
    pub expires_at_ms: u64,
    /// The wallet address it pays, when the owner named one.
    pub recipient: Option<String>,
    /// Whether it fixes what is sent, rather than what is delivered.
    pub fixed_debit: bool,
}

/// Where an outgoing payment is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutgoingState {
    /// It sends what is left.
    Sending,
    /// It sent its debit amount.
    Completed,
    /// It stopped short.
    Failed,
}

impl OutgoingState {
    /// As stored.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sending => "sending",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "sending" => Ok(Self::Sending),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(StoreError::Corrupt("outgoing payment state")),
        }
    }
}

/// An outgoing payment as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingRow {
    /// Its ID.
    pub id: PaymentId,
    /// The ledger that pays.
    pub ledger: LedgerId,
    /// The wallet address that pays.
    pub wallet_address: String,
    /// The client that made it; none for the owner.
    pub client: Option<String>,
    /// The grant it spends, and the window it was reserved in.
    pub grant: Option<(PaymentId, u64)>,
    /// The quote it pays, if any.
    pub quote_id: Option<PaymentId>,
    /// The incoming payment it pays.
    pub receiver: String,
    /// The receiver's STREAM destination.
    pub destination: String,
    /// The receiver's STREAM shared secret.
    pub shared_secret: [u8; 32],
    /// What it debits at most.
    pub debit_amount: u64,
    /// What the receiver is to get.
    pub receive_amount: ForeignAmount,
    /// What it debited so far: what was posted.
    pub sent_amount: u64,
    /// Where it is.
    pub state: OutgoingState,
    /// Why it failed.
    pub failure: Option<String>,
    /// The creator's metadata, as JSON.
    pub metadata: Option<String>,
    /// When it was made.
    pub created_at_ms: u64,
    /// When it fails if it has not sent all.
    pub deadline_ms: u64,
    /// When it completed or failed.
    pub finished_at_ms: Option<u64>,
    /// The wallet address it pays, when the owner named one.
    pub recipient: Option<String>,
    /// Whether it sends all its debit amount, rather than stopping once
    /// the receiver has its receive amount.
    pub fixed_debit: bool,
}

/// What a step changed of an outgoing payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingProgress {
    /// What it debited now.
    pub sent_amount: u64,
    /// Where it is now.
    pub state: OutgoingState,
    /// Why it failed.
    pub failure: Option<String>,
    /// When it finished.
    pub finished_at_ms: Option<u64>,
}

/// What a batch changes of outgoing payments and grant spend.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutgoingBatch {
    /// Payments made.
    pub created: Vec<OutgoingRow>,
    /// What payments sent, and how they ended.
    pub progress: BTreeMap<PaymentId, OutgoingProgress>,
    /// Grant spend to add, or take back, per grant and window: debit and
    /// receive.
    pub spend: BTreeMap<(PaymentId, u64), (i128, i128)>,
}

impl OutgoingBatch {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.progress.is_empty() && self.spend.is_empty()
    }
}

const QUOTE_COLUMNS: &str = "id, ledger, wallet_address, client, receiver, destination, \
     shared_secret, debit_amount, receive_amount, receive_asset_code, receive_asset_scale, \
     created_at_ms, expires_at_ms, recipient, fixed_debit";

const OUTGOING_COLUMNS: &str = "id, ledger, wallet_address, client, grant_id, grant_window_ms, \
     quote_id, receiver, destination, shared_secret, debit_amount, receive_amount, \
     receive_asset_code, receive_asset_scale, sent_amount, state, failure, metadata, \
     created_at_ms, deadline_ms, finished_at_ms, recipient, fixed_debit";

const NEXT_REVISION: &str = "(SELECT coalesce(max(revision), 0) + 1 FROM outgoing_payments)";

impl Store {
    /// Stores a quote, after forgetting expired ones; false when too many
    /// of its kind, the owner's or clients', are live.
    pub fn insert_quote(&self, quote: &QuoteRow, now_ms: u64) -> Result<bool, StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM quotes WHERE expires_at_ms <= ?1 \
             AND id NOT IN (SELECT quote_id FROM outgoing_payments WHERE quote_id IS NOT NULL)",
            [to_sql_time(now_ms)],
        )?;
        let owner = quote.client.is_none();
        let live: i64 = transaction.query_row(
            "SELECT count(*) FROM quotes WHERE expires_at_ms > ?1 AND (client IS NULL) = ?2",
            params![to_sql_time(now_ms), owner],
            |row| row.get(0),
        )?;
        let most = if owner {
            MAX_LIVE_OWNER_QUOTES
        } else {
            MAX_LIVE_QUOTES
        };
        if live >= i64::from(most) {
            return Ok(false);
        }
        transaction.execute(
            &format!(
                "INSERT INTO quotes ({QUOTE_COLUMNS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
            ),
            params![
                quote.id.0.as_slice(),
                quote.ledger.as_str(),
                quote.wallet_address,
                quote.client,
                quote.receiver,
                quote.destination,
                quote.shared_secret.as_slice(),
                sql(quote.debit_amount)?,
                sql(quote.receive_amount.value)?,
                quote.receive_amount.asset_code,
                quote.receive_amount.asset_scale,
                to_sql_time(quote.created_at_ms),
                to_sql_time(quote.expires_at_ms),
                quote.recipient,
                quote.fixed_debit,
            ],
        )?;
        transaction.commit()?;
        Ok(true)
    }

    /// One quote.
    pub fn quote(&self, id: PaymentId) -> Result<Option<QuoteRow>, StoreError> {
        self.lock()
            .query_row(
                &format!("SELECT {QUOTE_COLUMNS} FROM quotes WHERE id = ?1"),
                [id.0.as_slice()],
                read_quote,
            )
            .optional()?
            .transpose()
    }

    /// Whether an outgoing payment pays the quote already.
    pub fn quote_used(&self, id: PaymentId) -> Result<bool, StoreError> {
        let used: Option<i64> = self
            .lock()
            .query_row(
                "SELECT 1 FROM outgoing_payments WHERE quote_id = ?1",
                [id.0.as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(used.is_some())
    }

    /// Whether an outgoing payment pays to `destination` already.
    pub fn destination_used(&self, destination: &str) -> Result<bool, StoreError> {
        let used: Option<i64> = self
            .lock()
            .query_row(
                "SELECT 1 FROM outgoing_payments WHERE destination = ?1 LIMIT 1",
                [destination],
                |row| row.get(0),
            )
            .optional()?;
        Ok(used.is_some())
    }

    /// One outgoing payment.
    pub fn outgoing_payment(&self, id: PaymentId) -> Result<Option<OutgoingRow>, StoreError> {
        self.lock()
            .query_row(
                &format!("SELECT {OUTGOING_COLUMNS} FROM outgoing_payments WHERE id = ?1"),
                [id.0.as_slice()],
                read_outgoing,
            )
            .optional()?
            .transpose()
    }

    /// The outgoing payments still sending, oldest first.
    pub fn sending_payments(&self) -> Result<Vec<OutgoingRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {OUTGOING_COLUMNS} FROM outgoing_payments WHERE state = 'sending' \
             ORDER BY created_at_ms, id LIMIT ?1"
        ))?;
        statement
            .query_map([MAX_SENDING_LOADED], read_outgoing)?
            .map(|row| row?)
            .collect()
    }

    /// A page of outgoing payments (Open Payments pagination), or `None`
    /// when the cursor is not an item the query could list.
    pub fn outgoing_page(
        &self,
        query: &IncomingQuery<'_>,
    ) -> Result<Option<Page<OutgoingRow>>, StoreError> {
        page(
            &self.lock(),
            "outgoing_payments",
            OUTGOING_COLUMNS,
            read_outgoing,
            query,
        )
    }

    /// The newest `limit` outgoing payments, newest first, and the highest
    /// revision: where a live view starts.
    pub fn outgoing_latest(&self, limit: u32) -> Result<(u64, Vec<OutgoingRow>), StoreError> {
        let connection = self.lock();
        let revision: Option<i64> =
            connection.query_row("SELECT max(revision) FROM outgoing_payments", [], |row| {
                row.get(0)
            })?;
        let mut statement = connection.prepare(&format!(
            "SELECT {OUTGOING_COLUMNS} FROM outgoing_payments \
             ORDER BY created_at_ms DESC, id DESC LIMIT ?1"
        ))?;
        let rows = statement
            .query_map([limit], read_outgoing)?
            .map(|row| row?)
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok((revision.map_or(0, from_sql_time), rows))
    }

    /// What a grant spent in the window starting at `window_start_ms`:
    /// debited, and delivered.
    pub fn grant_spend(
        &self,
        grant: PaymentId,
        window_start_ms: u64,
    ) -> Result<(u64, u64), StoreError> {
        let spent: Option<(i64, i64)> = self
            .lock()
            .query_row(
                "SELECT debit_amount, receive_amount FROM grant_spend \
                 WHERE grant_id = ?1 AND window_start_ms = ?2",
                params![grant.0.as_slice(), to_sql_time(window_start_ms)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (debit, receive) = spent.unwrap_or((0, 0));
        Ok((unsigned(debit)?, unsigned(receive)?))
    }
}

pub(super) fn write_outgoing(
    transaction: &Transaction<'_>,
    batch: &OutgoingBatch,
) -> Result<(), StoreError> {
    for row in &batch.created {
        insert_outgoing(transaction, row)?;
    }
    for (id, progress) in &batch.progress {
        let updated = transaction.execute(
            &format!(
                "UPDATE outgoing_payments SET sent_amount = ?2, state = ?3, failure = ?4, \
                 finished_at_ms = ?5, revision = {NEXT_REVISION} WHERE id = ?1"
            ),
            params![
                id.0.as_slice(),
                sql(progress.sent_amount)?,
                progress.state.as_str(),
                progress.failure,
                progress.finished_at_ms.map(to_sql_time),
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::Corrupt("outgoing payment"));
        }
    }
    for ((grant, window), (debit, receive)) in &batch.spend {
        let (debit, receive) = (signed(*debit)?, signed(*receive)?);
        transaction.execute(
            "INSERT INTO grant_spend (grant_id, window_start_ms, debit_amount, receive_amount) \
             VALUES (?1, ?2, max(?3, 0), max(?4, 0)) ON CONFLICT (grant_id, window_start_ms) \
             DO UPDATE SET debit_amount = max(debit_amount + ?3, 0), \
             receive_amount = max(receive_amount + ?4, 0)",
            params![grant.0.as_slice(), to_sql_time(*window), debit, receive],
        )?;
    }
    Ok(())
}

fn insert_outgoing(transaction: &Transaction<'_>, row: &OutgoingRow) -> Result<(), StoreError> {
    let (grant, window) = row.grant.unzip();
    transaction.execute(
        &format!(
            "INSERT INTO outgoing_payments ({OUTGOING_COLUMNS}, revision) VALUES \
             (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
             ?18, ?19, ?20, ?21, ?22, ?23, {NEXT_REVISION})"
        ),
        params![
            row.id.0.as_slice(),
            row.ledger.as_str(),
            row.wallet_address,
            row.client,
            grant.map(|grant| grant.0.to_vec()),
            window.map(to_sql_time),
            row.quote_id.map(|quote| quote.0.to_vec()),
            row.receiver,
            row.destination,
            row.shared_secret.as_slice(),
            sql(row.debit_amount)?,
            sql(row.receive_amount.value)?,
            row.receive_amount.asset_code,
            row.receive_amount.asset_scale,
            sql(row.sent_amount)?,
            row.state.as_str(),
            row.failure,
            row.metadata,
            to_sql_time(row.created_at_ms),
            to_sql_time(row.deadline_ms),
            row.finished_at_ms.map(to_sql_time),
            row.recipient,
            row.fixed_debit,
        ],
    )?;
    Ok(())
}

fn read_quote(row: &Row<'_>) -> rusqlite::Result<Result<QuoteRow, StoreError>> {
    let id: Vec<u8> = row.get(0)?;
    let ledger: String = row.get(1)?;
    let wallet_address: String = row.get(2)?;
    let client: Option<String> = row.get(3)?;
    let receiver: String = row.get(4)?;
    let destination: String = row.get(5)?;
    let secret: Vec<u8> = row.get(6)?;
    let debit: i64 = row.get(7)?;
    let receive: i64 = row.get(8)?;
    let code: String = row.get(9)?;
    let scale: i64 = row.get(10)?;
    let created: i64 = row.get(11)?;
    let expires: i64 = row.get(12)?;
    let recipient: Option<String> = row.get(13)?;
    let fixed_debit: bool = row.get(14)?;
    Ok((|| {
        Ok(QuoteRow {
            id: payment_id(id)?,
            ledger: LedgerId::parse(&ledger).ok_or(StoreError::Corrupt("quote ledger"))?,
            wallet_address,
            client,
            receiver,
            destination,
            shared_secret: secret_of(secret)?,
            debit_amount: unsigned(debit)?,
            receive_amount: foreign(receive, code, scale)?,
            created_at_ms: from_sql_time(created),
            expires_at_ms: from_sql_time(expires),
            recipient,
            fixed_debit,
        })
    })())
}

fn read_outgoing(row: &Row<'_>) -> rusqlite::Result<Result<OutgoingRow, StoreError>> {
    let id: Vec<u8> = row.get(0)?;
    let ledger: String = row.get(1)?;
    let grant: Option<Vec<u8>> = row.get(4)?;
    let window: Option<i64> = row.get(5)?;
    let quote: Option<Vec<u8>> = row.get(6)?;
    let secret: Vec<u8> = row.get(9)?;
    let (debit, receive, sent): (i64, i64, i64) = (row.get(10)?, row.get(11)?, row.get(14)?);
    let (code, scale): (String, i64) = (row.get(12)?, row.get(13)?);
    let state: String = row.get(15)?;
    let (created, deadline): (i64, i64) = (row.get(18)?, row.get(19)?);
    let finished: Option<i64> = row.get(20)?;
    let (wallet_address, client, receiver, destination) =
        (row.get(2)?, row.get(3)?, row.get(7)?, row.get(8)?);
    let (failure, metadata) = (row.get(16)?, row.get(17)?);
    let recipient: Option<String> = row.get(21)?;
    let fixed_debit: bool = row.get(22)?;
    Ok((|| {
        let grant = match (grant, window) {
            (Some(grant), Some(window)) => Some((payment_id(grant)?, from_sql_time(window))),
            (None, None) => None,
            _ => return Err(StoreError::Corrupt("outgoing payment grant")),
        };
        Ok(OutgoingRow {
            id: payment_id(id)?,
            ledger: LedgerId::parse(&ledger).ok_or(StoreError::Corrupt("outgoing ledger"))?,
            wallet_address,
            client,
            grant,
            quote_id: quote.map(payment_id).transpose()?,
            receiver,
            destination,
            shared_secret: secret_of(secret)?,
            debit_amount: unsigned(debit)?,
            receive_amount: foreign(receive, code, scale)?,
            sent_amount: unsigned(sent)?,
            state: OutgoingState::parse(&state)?,
            failure,
            metadata,
            created_at_ms: from_sql_time(created),
            deadline_ms: from_sql_time(deadline),
            finished_at_ms: finished.map(from_sql_time),
            recipient,
            fixed_debit,
        })
    })())
}

fn payment_id(bytes: Vec<u8>) -> Result<PaymentId, StoreError> {
    <[u8; 16]>::try_from(bytes.as_slice())
        .map(PaymentId)
        .map_err(|_| StoreError::Corrupt("payment ID"))
}

fn secret_of(bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| StoreError::Corrupt("shared secret"))
}

fn foreign(value: i64, asset_code: String, scale: i64) -> Result<ForeignAmount, StoreError> {
    Ok(ForeignAmount {
        value: unsigned(value)?,
        asset_code,
        asset_scale: u8::try_from(scale).map_err(|_| StoreError::Corrupt("asset scale"))?,
    })
}

fn sql(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt("amount beyond i64"))
}

fn signed(value: i128) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt("amount beyond i64"))
}

fn unsigned(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt("amount"))
}

//! Verdicts on Fulfills between peers (D17): the Fulfills we sent and wait
//! for a verdict on, and the Fulfills we took, kept to answer a resend.

use dassie_identity::NodeId;
use dassie_ledger::{AccountId, TraceId};
use rusqlite::{OptionalExtension, Row, Transaction, params};

use super::{PaymentId, Store, StoreError, from_sql_time, to_sql_time};

/// What a refused Fulfill books.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compensation {
    /// Our own receiver took the packet: its transfers are reversed.
    Reverse,
    /// Someone who keeps what they were paid took it: the loss is ours.
    Loss,
}

/// A posted transfer of the packet: debit, credit, amount.
pub type PostedTransfer = (AccountId, AccountId, u64);

/// A Fulfill we sent a peer, waiting for its verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwedFulfill {
    /// The packet.
    pub trace: TraceId,
    /// The upstream peer.
    pub peer: NodeId,
    /// The request ID its Prepare came with.
    pub request_id: u32,
    /// The Fulfill, as sent.
    pub fulfill: Vec<u8>,
    /// What a refusal books.
    pub compensation: Compensation,
    /// The packet's transfers as posted; the first debits the peer's
    /// account.
    pub transfers: [Option<PostedTransfer>; 2],
    /// The incoming payment our receiver paid, and how much.
    pub incoming: Option<(PaymentId, u64)>,
    /// The expiry of the Prepare we sent on.
    pub expires_at_ms: u64,
    /// When the packet was posted.
    pub created_at_ms: u64,
}

/// A Fulfill we took from a peer: the peer, the request ID we sent the
/// Prepare with, and the condition.
pub type TakenFulfill = (NodeId, u32, [u8; 32]);

/// What a batch changes in the verdicts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerdictBatch {
    /// Fulfills sent, now waiting.
    pub owed: Vec<OwedFulfill>,
    /// Fulfills whose verdict came, by packet.
    pub settled: Vec<TraceId>,
    /// Fulfills taken.
    pub taken: Vec<TakenFulfill>,
    /// Forget Fulfills taken before this time, the oldest first, at most
    /// [`FORGET_AT_MOST`] at once.
    pub forget_before_ms: Option<u64>,
}

impl VerdictBatch {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.owed.is_empty()
            && self.settled.is_empty()
            && self.taken.is_empty()
            && self.forget_before_ms.is_none()
    }
}

/// Most Fulfills taken that one batch forgets, so that forgetting never
/// holds up the engine for long; it forgets again every minute.
pub const FORGET_AT_MOST: u32 = 10_000;

const OWED_COLUMNS: &str = "packet_epoch, packet_sequence, peer, request_id, fulfill, \
     compensation, debit_1, credit_1, amount_1, debit_2, credit_2, amount_2, \
     incoming_payment, incoming_amount, expires_at_ms, created_at_ms";

impl Store {
    /// Every Fulfill waiting for its verdict, oldest first.
    ///
    /// # Errors
    ///
    /// The database failed, or holds an invalid row.
    pub fn owed_fulfills(&self) -> Result<Vec<OwedFulfill>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {OWED_COLUMNS} FROM owed_fulfills ORDER BY created_at_ms, packet_epoch, packet_sequence"
        ))?;
        let rows = statement
            .query_map([], read_owed)?
            .map(|row| row?)
            .collect::<Result<Vec<_>, StoreError>>()?;
        Ok(rows)
    }

    /// The bytes of a Fulfill owed a verdict.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn owed_fulfill_bytes(&self, trace: TraceId) -> Result<Option<Vec<u8>>, StoreError> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT fulfill FROM owed_fulfills WHERE packet_epoch = ?1 AND packet_sequence = ?2",
                params![sql(trace.epoch)?, sql(trace.sequence)?],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Whether we took the Fulfill of `condition` that `peer` sent for our
    /// Prepare `request_id`, in the last 30 days.
    ///
    /// # Errors
    ///
    /// The database failed.
    pub fn fulfill_taken(
        &self,
        peer: &NodeId,
        request_id: u32,
        condition: &[u8; 32],
    ) -> Result<bool, StoreError> {
        let connection = self.lock();
        let found: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM peer_fulfills WHERE peer = ?1 AND request_id = ?2 AND condition = ?3",
                params![peer.as_str(), i64::from(request_id), condition.as_slice()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }
}

fn read_owed(row: &Row<'_>) -> rusqlite::Result<Result<OwedFulfill, StoreError>> {
    let epoch: i64 = row.get(0)?;
    let sequence: i64 = row.get(1)?;
    let peer: String = row.get(2)?;
    let request_id: i64 = row.get(3)?;
    let fulfill: Vec<u8> = row.get(4)?;
    let compensation: String = row.get(5)?;
    let mut raw = [(None, None, None); 2];
    for (index, slot) in raw.iter_mut().enumerate() {
        let base = 6 + index * 3;
        *slot = (row.get(base)?, row.get(base + 1)?, row.get(base + 2)?);
    }
    let payment: Option<Vec<u8>> = row.get(12)?;
    let incoming_amount: Option<i64> = row.get(13)?;
    let expires: i64 = row.get(14)?;
    let created: i64 = row.get(15)?;
    Ok((|| {
        let mut transfers = [None; 2];
        for (slot, parts) in transfers.iter_mut().zip(raw) {
            *slot = match parts {
                (Some(debit), Some(credit), Some(amount)) => {
                    Some((account(debit)?, account(credit)?, unsigned(amount)?))
                }
                (None, None, None) => None,
                _ => return Err(StoreError::Corrupt("owed transfer")),
            };
        }
        let incoming = match (payment, incoming_amount) {
            (Some(id), Some(amount)) => Some((
                PaymentId(
                    <[u8; 16]>::try_from(id.as_slice())
                        .map_err(|_| StoreError::Corrupt("payment ID"))?,
                ),
                unsigned(amount)?,
            )),
            (None, None) => None,
            _ => return Err(StoreError::Corrupt("owed incoming payment")),
        };
        if transfers[0].is_none() {
            return Err(StoreError::Corrupt("owed transfers"));
        }
        Ok(OwedFulfill {
            trace: TraceId {
                epoch: unsigned(epoch)?,
                sequence: unsigned(sequence)?,
            },
            peer: NodeId::parse(&peer).map_err(|_| StoreError::Corrupt("owed peer"))?,
            request_id: u32::try_from(request_id)
                .map_err(|_| StoreError::Corrupt("owed request ID"))?,
            fulfill,
            compensation: match compensation.as_str() {
                "reverse" => Compensation::Reverse,
                "loss" => Compensation::Loss,
                _ => return Err(StoreError::Corrupt("owed compensation")),
            },
            transfers,
            incoming,
            expires_at_ms: from_sql_time(expires),
            created_at_ms: from_sql_time(created),
        })
    })())
}

pub(super) fn write_verdicts(
    transaction: &Transaction<'_>,
    batch: &VerdictBatch,
    at_ms: u64,
) -> Result<(), StoreError> {
    for owed in &batch.owed {
        insert_owed(transaction, owed)?;
    }
    for trace in &batch.settled {
        transaction.execute(
            "DELETE FROM owed_fulfills WHERE packet_epoch = ?1 AND packet_sequence = ?2",
            params![sql(trace.epoch)?, sql(trace.sequence)?],
        )?;
    }
    if let Some(before) = batch.forget_before_ms {
        transaction.execute(
            "DELETE FROM peer_fulfills WHERE (peer, request_id, condition) IN \
             (SELECT peer, request_id, condition FROM peer_fulfills \
              WHERE taken_at_ms < ?1 ORDER BY taken_at_ms LIMIT ?2)",
            params![to_sql_time(before), FORGET_AT_MOST],
        )?;
    }
    for (peer, request_id, condition) in &batch.taken {
        transaction.execute(
            "INSERT OR IGNORE INTO peer_fulfills (peer, request_id, condition, taken_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                peer.as_str(),
                i64::from(*request_id),
                condition.as_slice(),
                to_sql_time(at_ms),
            ],
        )?;
    }
    Ok(())
}

fn insert_owed(transaction: &Transaction<'_>, owed: &OwedFulfill) -> Result<(), StoreError> {
    let [first, second] = owed.transfers;
    let first = first.ok_or(StoreError::Corrupt("owed transfers"))?;
    let part = |transfer: Option<PostedTransfer>| -> Result<[Option<i64>; 3], StoreError> {
        Ok(match transfer {
            Some((debit, credit, amount)) => [
                Some(i64::from(debit.0)),
                Some(i64::from(credit.0)),
                Some(sql(amount)?),
            ],
            None => [None; 3],
        })
    };
    let second = part(second)?;
    let (payment, incoming_amount) = match owed.incoming {
        Some((id, amount)) => (Some(id.0.to_vec()), Some(sql(amount)?)),
        None => (None, None),
    };
    transaction.execute(
        &format!(
            "INSERT INTO owed_fulfills ({OWED_COLUMNS}) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
        ),
        params![
            sql(owed.trace.epoch)?,
            sql(owed.trace.sequence)?,
            owed.peer.as_str(),
            i64::from(owed.request_id),
            owed.fulfill,
            match owed.compensation {
                Compensation::Reverse => "reverse",
                Compensation::Loss => "loss",
            },
            i64::from(first.0.0),
            i64::from(first.1.0),
            sql(first.2)?,
            second[0],
            second[1],
            second[2],
            payment,
            incoming_amount,
            to_sql_time(owed.expires_at_ms),
            to_sql_time(owed.created_at_ms),
        ],
    )?;
    Ok(())
}

fn sql(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt("amount beyond i64"))
}

fn unsigned(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt("amount"))
}

fn account(value: i64) -> Result<AccountId, StoreError> {
    u32::try_from(value)
        .map(AccountId)
        .map_err(|_| StoreError::Corrupt("account ID"))
}

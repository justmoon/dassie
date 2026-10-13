//! The ledger's tables: accounts, the transfer journal, settlement schemes
//! and their checkpoints, and BTP tokens.
//!
//! The ledger's engine is the only writer. It hands a [`LedgerBatch`] to
//! [`Store::commit_ledger`] after every batch of work, and acts on the work's
//! outcome only once that returns: what the journal says is what happened.

use dassie_ledger::{
    AccountId, AccountPath, JournalEntry, LedgerId, Name, Reference, StoredAccount, TraceId,
    Transfer, TransferId, TransferState,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};

use super::{Store, StoreError, from_sql_time, to_sql_time};

/// Everything one batch of the engine's work changed, persisted in one
/// transaction.
#[derive(Clone, Debug, Default)]
pub struct LedgerBatch {
    /// When the batch ran.
    pub at_ms: u64,
    /// The ledger's journal entries, in order.
    pub journal: Vec<JournalEntry>,
    /// Settlement schemes activated, with their config.
    pub schemes_added: Vec<(Name, String)>,
    /// Settlement schemes deactivated. Their checkpoints stay.
    pub schemes_removed: Vec<Name>,
    /// Each scheme's balance as the books now record it.
    pub checkpoints: Vec<(Name, u64)>,
    /// BTP tokens created.
    pub tokens_created: Vec<BtpTokenRow>,
    /// BTP tokens revoked, by ID.
    pub tokens_revoked: Vec<u64>,
}

impl LedgerBatch {
    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.journal.is_empty()
            && self.schemes_added.is_empty()
            && self.schemes_removed.is_empty()
            && self.checkpoints.is_empty()
            && self.tokens_created.is_empty()
            && self.tokens_revoked.is_empty()
    }
}

/// A BTP token as stored: its hash, never the token (D10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtpTokenRow {
    /// Never reused.
    pub id: u64,
    /// SHA-256 of the token.
    pub token_hash: [u8; 32],
    /// The owner's name for it.
    pub label: String,
    /// The ledger its client uses.
    pub ledger: LedgerId,
    /// When it was made.
    pub created_at_ms: u64,
    /// When it was revoked, if it was.
    pub revoked_at_ms: Option<u64>,
}

/// What the ledger restores from.
#[derive(Clone, Debug)]
pub struct StoredLedger {
    /// Every account, in ID order.
    pub accounts: Vec<StoredAccount>,
    /// The transfers still pending: settlements only.
    pub pending: Vec<Transfer>,
    /// Past every stored transfer.
    pub next_transfer: TransferId,
}

/// A transfer in the journal, with its accounts' paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalRow {
    /// The transfer.
    pub transfer: Transfer,
    /// Where it is now.
    pub state: TransferState,
    /// When it was posted or voided.
    pub resolved_at_ms: Option<u64>,
    /// The debited account's path.
    pub debit_path: String,
    /// The credited account's path.
    pub credit_path: String,
}

impl Store {
    /// Counts this start and returns its number, the boot epoch that keeps
    /// packet references unique across restarts.
    pub fn next_boot_epoch(&self) -> Result<u64, StoreError> {
        let epoch: i64 = self.lock().query_row(
            "UPDATE boot SET epoch = epoch + 1 RETURNING epoch",
            [],
            |row| row.get(0),
        )?;
        u64::try_from(epoch).map_err(|_| StoreError::Corrupt("boot epoch"))
    }

    /// The ledger as the journal left it, after checking that every
    /// account's cached totals are the sums of its posted transfers.
    pub fn load_ledger(&self) -> Result<StoredLedger, StoreError> {
        let connection = self.lock();
        check_totals(&connection)?;
        let mut statement = connection.prepare(
            "SELECT id, path, debits_posted, credits_posted FROM ledger_accounts ORDER BY id",
        )?;
        let accounts = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .map(|row| {
                let (id, path, debits, credits) = row?;
                Ok(StoredAccount {
                    id: AccountId(
                        u32::try_from(id).map_err(|_| StoreError::Corrupt("account ID"))?,
                    ),
                    path: AccountPath::parse(&path)
                        .map_err(|_| StoreError::Corrupt("account path"))?,
                    debits_posted: amount(debits)?,
                    credits_posted: amount(credits)?,
                })
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let mut statement = connection.prepare(&format!(
            "SELECT {TRANSFER_COLUMNS} FROM ledger_transfers t WHERE t.state = 'pending' ORDER BY t.id"
        ))?;
        let pending = statement
            .query_map([], read_transfer)?
            .map(|row| row?.map(|(transfer, _, _)| transfer))
            .collect::<Result<Vec<_>, StoreError>>()?;
        let last: Option<i64> =
            connection.query_row("SELECT max(id) FROM ledger_transfers", [], |row| row.get(0))?;
        let next_transfer = match last {
            Some(last) => amount(last)? + 1,
            None => 0,
        };
        Ok(StoredLedger {
            accounts,
            pending,
            next_transfer: TransferId(next_transfer),
        })
    }

    /// The active settlement schemes and their config, by ID.
    pub fn settlement_schemes(&self) -> Result<Vec<(Name, String)>, StoreError> {
        let connection = self.lock();
        let mut statement =
            connection.prepare("SELECT id, config FROM settlement_schemes ORDER BY id")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (id, config) = row?;
                Ok((name(&id, "scheme ID")?, config))
            })
            .collect()
    }

    /// The balance each scheme's books last recorded.
    pub fn checkpoints(&self) -> Result<Vec<(Name, u64)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT scheme, balance FROM settlement_checkpoints ORDER BY scheme")?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .map(|row| {
                let (scheme, balance) = row?;
                Ok((name(&scheme, "scheme ID")?, amount(balance)?))
            })
            .collect()
    }

    /// Every BTP token, revoked ones too, by ID.
    pub fn btp_tokens(&self) -> Result<Vec<BtpTokenRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, token_hash, label, ledger, created_at_ms, revoked_at_ms \
             FROM btp_tokens ORDER BY id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            })?
            .map(|row| {
                let (id, hash, label, ledger, created, revoked) = row?;
                Ok(BtpTokenRow {
                    id: amount(id)?,
                    token_hash: hash
                        .try_into()
                        .map_err(|_| StoreError::Corrupt("token hash"))?,
                    label,
                    ledger: LedgerId::parse(&ledger).ok_or(StoreError::Corrupt("token ledger"))?,
                    created_at_ms: from_sql_time(created),
                    revoked_at_ms: revoked.map(from_sql_time),
                })
            })
            .collect()
    }

    /// Writes a batch in one transaction: all of it or, on error, none.
    pub fn commit_ledger(&self, batch: &LedgerBatch) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        for entry in &batch.journal {
            write_entry(&transaction, entry)?;
        }
        write_settlement(&transaction, batch)?;
        write_tokens(&transaction, batch)?;
        transaction.commit()?;
        Ok(())
    }

    /// Up to `limit` journal rows after `after`, in ID order.
    pub fn transfers_after(
        &self,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<JournalRow>, StoreError> {
        let connection = self.lock();
        let after = after.map_or(-1, |after| i64::try_from(after).unwrap_or(i64::MAX));
        let mut statement = connection.prepare(&format!(
            "SELECT {TRANSFER_COLUMNS}, d.path, c.path FROM ledger_transfers t \
             JOIN ledger_accounts d ON d.id = t.debit JOIN ledger_accounts c ON c.id = t.credit \
             WHERE t.id > ?1 ORDER BY t.id LIMIT ?2"
        ))?;
        statement
            .query_map(params![after, limit], |row| {
                Ok((
                    read_transfer(row)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            })?
            .map(|row| {
                let (transfer, debit_path, credit_path) = row?;
                let (transfer, state, resolved_at_ms) = transfer?;
                Ok(JournalRow {
                    transfer,
                    state,
                    resolved_at_ms,
                    debit_path,
                    credit_path,
                })
            })
            .collect()
    }
}

/// The columns [`read_transfer`] reads, in its order.
const TRANSFER_COLUMNS: &str = "t.id, t.debit, t.credit, t.amount, t.state, t.created_at_ms, \
     t.resolved_at_ms, t.reference, t.packet_epoch, t.packet_sequence, t.settlement_scheme, \
     t.settlement_id";

type TransferRow = Result<(Transfer, TransferState, Option<u64>), StoreError>;

fn read_transfer(row: &Row<'_>) -> rusqlite::Result<TransferRow> {
    let id: i64 = row.get(0)?;
    let debit: i64 = row.get(1)?;
    let credit: i64 = row.get(2)?;
    let value: i64 = row.get(3)?;
    let state: String = row.get(4)?;
    let created: i64 = row.get(5)?;
    let resolved: Option<i64> = row.get(6)?;
    let kind: String = row.get(7)?;
    let epoch: Option<i64> = row.get(8)?;
    let sequence: Option<i64> = row.get(9)?;
    let scheme: Option<String> = row.get(10)?;
    let settlement: Option<String> = row.get(11)?;
    Ok((|| {
        let reference = match (kind.as_str(), epoch, sequence, scheme, settlement) {
            ("packet", Some(epoch), Some(sequence), None, None) => Reference::Packet(TraceId {
                epoch: amount(epoch)?,
                sequence: amount(sequence)?,
            }),
            ("settlement", None, None, Some(scheme), Some(id)) => Reference::Settlement {
                scheme: name(&scheme, "settlement scheme")?,
                id: name(&id, "settlement ID")?,
            },
            ("trust", ..) => Reference::Trust,
            ("deposit", ..) => Reference::Deposit,
            ("withdrawal", ..) => Reference::Withdrawal,
            ("reconciliation", ..) => Reference::Reconciliation,
            _ => return Err(StoreError::Corrupt("transfer reference")),
        };
        let state = match state.as_str() {
            "pending" => TransferState::Pending,
            "posted" => TransferState::Posted,
            "voided" => TransferState::Voided,
            _ => return Err(StoreError::Corrupt("transfer state")),
        };
        let account = |id: i64| {
            u32::try_from(id)
                .map(AccountId)
                .map_err(|_| StoreError::Corrupt("account ID"))
        };
        let transfer = Transfer {
            id: TransferId(amount(id)?),
            debit: account(debit)?,
            credit: account(credit)?,
            amount: amount(value)?,
            reference,
            created_at_ms: from_sql_time(created),
        };
        Ok((transfer, state, resolved.map(from_sql_time)))
    })())
}

fn write_entry(transaction: &Transaction<'_>, entry: &JournalEntry) -> Result<(), StoreError> {
    match *entry {
        JournalEntry::AccountOpened { id, path } => {
            transaction.execute(
                "INSERT INTO ledger_accounts (id, path) VALUES (?1, ?2)",
                params![id.0, path.to_string()],
            )?;
        }
        JournalEntry::Transfer {
            transfer,
            state: TransferState::Pending,
            ..
        } => insert_transfer(transaction, &transfer, "pending", None)?,
        JournalEntry::Transfer {
            transfer,
            state: TransferState::Posted,
            at_ms,
        } => {
            let updated = transaction.execute(
                "UPDATE ledger_transfers SET state = 'posted', resolved_at_ms = ?2 \
                 WHERE id = ?1 AND state = 'pending'",
                params![sql_amount(transfer.id.0)?, to_sql_time(at_ms)],
            )?;
            if updated == 0 {
                insert_transfer(transaction, &transfer, "posted", Some(at_ms))?;
            }
            let value = sql_amount(transfer.amount)?;
            transaction.execute(
                "UPDATE ledger_accounts SET debits_posted = debits_posted + ?2 WHERE id = ?1",
                params![transfer.debit.0, value],
            )?;
            transaction.execute(
                "UPDATE ledger_accounts SET credits_posted = credits_posted + ?2 WHERE id = ?1",
                params![transfer.credit.0, value],
            )?;
        }
        JournalEntry::Transfer {
            transfer,
            state: TransferState::Voided,
            at_ms,
        } => {
            let updated = transaction.execute(
                "UPDATE ledger_transfers SET state = 'voided', resolved_at_ms = ?2 \
                 WHERE id = ?1 AND state = 'pending'",
                params![sql_amount(transfer.id.0)?, to_sql_time(at_ms)],
            )?;
            if updated != 1 {
                return Err(StoreError::Corrupt("voided transfer"));
            }
        }
    }
    Ok(())
}

fn insert_transfer(
    transaction: &Transaction<'_>,
    transfer: &Transfer,
    state: &str,
    resolved_at_ms: Option<u64>,
) -> Result<(), StoreError> {
    let (kind, epoch, sequence, scheme, settlement) = match transfer.reference {
        Reference::Packet(trace) => (
            "packet",
            Some(sql_amount(trace.epoch)?),
            Some(sql_amount(trace.sequence)?),
            None,
            None,
        ),
        Reference::Settlement { scheme, id } => (
            "settlement",
            None,
            None,
            Some(scheme.as_str().to_string()),
            Some(id.as_str().to_string()),
        ),
        Reference::Trust => ("trust", None, None, None, None),
        Reference::Deposit => ("deposit", None, None, None, None),
        Reference::Withdrawal => ("withdrawal", None, None, None, None),
        Reference::Reconciliation => ("reconciliation", None, None, None, None),
    };
    transaction.execute(
        "INSERT INTO ledger_transfers (id, debit, credit, amount, state, created_at_ms, \
         resolved_at_ms, reference, packet_epoch, packet_sequence, settlement_scheme, settlement_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            sql_amount(transfer.id.0)?,
            transfer.debit.0,
            transfer.credit.0,
            sql_amount(transfer.amount)?,
            state,
            to_sql_time(transfer.created_at_ms),
            resolved_at_ms.map(to_sql_time),
            kind,
            epoch,
            sequence,
            scheme,
            settlement,
        ],
    )?;
    Ok(())
}

fn write_settlement(transaction: &Transaction<'_>, batch: &LedgerBatch) -> Result<(), StoreError> {
    let at = to_sql_time(batch.at_ms);
    for (id, config) in &batch.schemes_added {
        transaction.execute(
            "INSERT INTO settlement_schemes (id, config, activated_at_ms) VALUES (?1, ?2, ?3)",
            params![id.as_str(), config, at],
        )?;
    }
    for id in &batch.schemes_removed {
        transaction.execute(
            "DELETE FROM settlement_schemes WHERE id = ?1",
            [id.as_str()],
        )?;
    }
    for (scheme, balance) in &batch.checkpoints {
        transaction.execute(
            "INSERT INTO settlement_checkpoints (scheme, balance, updated_at_ms) VALUES (?1, ?2, ?3) \
             ON CONFLICT (scheme) DO UPDATE SET balance = ?2, updated_at_ms = ?3",
            params![scheme.as_str(), sql_amount(*balance)?, at],
        )?;
    }
    Ok(())
}

fn write_tokens(transaction: &Transaction<'_>, batch: &LedgerBatch) -> Result<(), StoreError> {
    for token in &batch.tokens_created {
        transaction.execute(
            "INSERT INTO btp_tokens (id, token_hash, label, ledger, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                sql_amount(token.id)?,
                token.token_hash.as_slice(),
                token.label,
                token.ledger.as_str(),
                to_sql_time(token.created_at_ms),
            ],
        )?;
    }
    for id in &batch.tokens_revoked {
        transaction.execute(
            "UPDATE btp_tokens SET revoked_at_ms = ?2 WHERE id = ?1 AND revoked_at_ms IS NULL",
            params![sql_amount(*id)?, to_sql_time(batch.at_ms)],
        )?;
    }
    Ok(())
}

/// Fails unless every account's cached totals are the sums of its posted
/// transfers. The indexes on (debit, state) and (credit, state) make each
/// sum a range scan.
fn check_totals(connection: &Connection) -> Result<(), StoreError> {
    let wrong: Option<String> = connection
        .query_row(
            "SELECT a.path FROM ledger_accounts a WHERE \
             a.debits_posted != (SELECT coalesce(sum(amount), 0) FROM ledger_transfers \
                                 WHERE debit = a.id AND state = 'posted') \
             OR a.credits_posted != (SELECT coalesce(sum(amount), 0) FROM ledger_transfers \
                                     WHERE credit = a.id AND state = 'posted') \
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match wrong {
        Some(path) => {
            tracing::error!(target: crate::log::target::DATABASE, %path, "account totals do not match the journal");
            Err(StoreError::Corrupt("account total"))
        }
        None => Ok(()),
    }
}

fn amount(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Corrupt("amount"))
}

fn sql_amount(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Corrupt("amount beyond i64"))
}

fn name(text: &str, what: &'static str) -> Result<Name, StoreError> {
    Name::new(text).map_err(|_| StoreError::Corrupt(what))
}

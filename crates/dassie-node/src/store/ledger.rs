//! The ledger's tables: accounts, the transfer journal, settlement schemes
//! and their checkpoints, and BTP tokens.
//!
//! The ledger's engine is the only writer. It hands a [`LedgerBatch`] to
//! [`Store::commit_ledger`] after every batch of work, and acts on the work's
//! outcome only once that returns: what the journal says is what happened.

use dassie_connector::{CapUsage, SpendingCap};
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
    /// Settlement schemes deactivated. Their checkpoints stay. Written
    /// before the additions; a scheme added and then removed within the
    /// batch is left out of `schemes_added`.
    pub schemes_removed: Vec<Name>,
    /// Each scheme's balance as the books now record it, and its state.
    pub checkpoints: Vec<(Name, Checkpoint)>,
    /// BTP tokens created.
    pub tokens_created: Vec<BtpTokenRow>,
    /// BTP tokens revoked, by ID.
    pub tokens_revoked: Vec<u64>,
    /// BTP tokens whose cap, or what is used of it, changed.
    pub token_caps: Vec<TokenCap>,
    /// What the peer protocol changed.
    pub peer: dassie_peer::PeerBatch,
    /// Incoming payments created.
    pub incoming_created: Vec<super::IncomingRow>,
    /// What packets paid into incoming payments, and completions.
    pub incoming_progress: std::collections::BTreeMap<super::PaymentId, super::IncomingProgress>,
    /// What each connection of an incoming payment received now.
    pub incoming_connections:
        std::collections::BTreeMap<(super::PaymentId, super::ConnectionKey), u64>,
    /// Connections of incoming payments that gave their place to a new
    /// payer: forgotten.
    pub incoming_connections_gone:
        std::collections::BTreeSet<(super::PaymentId, super::ConnectionKey)>,
    /// Outgoing payments made, what they sent, and grant spend.
    pub outgoing: super::OutgoingBatch,
    /// The owner's peer settings, if they changed.
    pub peer_settings: Option<super::PeerSettings>,
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
            && self.token_caps.is_empty()
            && self.peer.is_empty()
            && self.incoming_created.is_empty()
            && self.incoming_progress.is_empty()
            && self.incoming_connections.is_empty()
            && self.incoming_connections_gone.is_empty()
            && self.outgoing.is_empty()
            && self.peer_settings.is_none()
    }

    /// Whether it changes Open Payments resources.
    #[must_use]
    pub fn changes_payments(&self) -> bool {
        !(self.incoming_created.is_empty()
            && self.incoming_progress.is_empty()
            && self.outgoing.is_empty())
    }
}

/// What the books keep for a settlement scheme: the balance they record
/// for it, and whatever else the scheme must find again after a restart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checkpoint {
    /// The balance on the underlying ledger the books record.
    pub balance: u64,
    /// The scheme's state; empty for none.
    pub state: Vec<u8>,
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
    /// Its spending cap, if it has one.
    pub cap: Option<SpendingCap>,
    /// What is used of the cap.
    pub cap_usage: CapUsage,
}

/// A token's cap, and what is used of it, as of a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenCap {
    /// The token.
    pub id: u64,
    /// Its cap; none lifts it.
    pub cap: Option<SpendingCap>,
    /// What is used of it.
    pub usage: CapUsage,
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
    /// Where the row is in the order rows were written or last changed.
    pub sequence: u64,
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

    /// What the books last kept for each scheme.
    pub fn checkpoints(&self) -> Result<Vec<(Name, Checkpoint)>, StoreError> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT scheme, balance, state FROM settlement_checkpoints ORDER BY scheme")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            })?
            .map(|row| {
                let (scheme, balance, state) = row?;
                Ok((
                    name(&scheme, "scheme ID")?,
                    Checkpoint {
                        balance: amount(balance)?,
                        state: state.unwrap_or_default(),
                    },
                ))
            })
            .collect()
    }

    /// Every BTP token, revoked ones too, by ID.
    pub fn btp_tokens(&self) -> Result<Vec<BtpTokenRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, token_hash, label, ledger, created_at_ms, revoked_at_ms, cap_amount, \
             cap_window_ms, cap_used_amount, cap_used_remainder, cap_as_of_ms \
             FROM btp_tokens ORDER BY id",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    (
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ),
                    read_cap(row, 6)?,
                ))
            })?
            .map(|row| {
                let ((id, hash, label, ledger, created, revoked), cap) = row?;
                let (cap, cap_usage) = cap?;
                Ok(BtpTokenRow {
                    id: amount(id)?,
                    token_hash: hash
                        .try_into()
                        .map_err(|_| StoreError::Corrupt("token hash"))?,
                    label,
                    ledger: LedgerId::parse(&ledger).ok_or(StoreError::Corrupt("token ledger"))?,
                    created_at_ms: from_sql_time(created),
                    revoked_at_ms: revoked.map(from_sql_time),
                    cap,
                    cap_usage,
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
        super::peer::write_peer(&transaction, &batch.peer)?;
        super::payments::write_incoming(
            &transaction,
            &batch.incoming_created,
            &batch.incoming_progress,
            &batch.incoming_connections,
            &batch.incoming_connections_gone,
        )?;
        super::outgoing::write_outgoing(&transaction, &batch.outgoing)?;
        super::peer_settings::write_peer_settings(&transaction, batch.peer_settings)?;
        #[cfg(feature = "testing")]
        self.fail_if_aimed(&transaction, batch);
        transaction.commit()?;
        Ok(())
    }

    /// Up to `limit` journal rows written or changed after `after`, a
    /// sequence number, in that order; only those of `account` if given. A
    /// reader that pages with the last sequence it saw sees every row, and
    /// every change of a row, once.
    pub fn transfers_after(
        &self,
        after: Option<u64>,
        limit: u32,
        account: Option<AccountId>,
    ) -> Result<Vec<JournalRow>, StoreError> {
        let after = after.map_or(0, |after| i64::try_from(after).unwrap_or(i64::MAX));
        let (filter, order) = match account {
            // Each half is a range scan of an index on (account, sequence).
            Some(_) => (
                "t.id IN (SELECT id FROM (SELECT id FROM ledger_transfers \
                   WHERE debit = ?3 AND sequence > ?1 ORDER BY sequence LIMIT ?2) \
                 UNION ALL SELECT id FROM (SELECT id FROM ledger_transfers \
                   WHERE credit = ?3 AND sequence > ?1 ORDER BY sequence LIMIT ?2))",
                "ASC",
            ),
            None => ("t.sequence > ?1 AND ?3 IS NULL", "ASC"),
        };
        self.journal_rows(filter, order, after, limit, account)
    }

    /// Up to `limit` fees posted to `fees` after the sequence `after`, in
    /// sequence order (D20). Each comes with the account its packet came
    /// from, which the fee debits, and the account the packet's other
    /// transfer from it credits: on the same ledger, the account it went
    /// to; across ledgers, the FX account.
    pub fn fees_after(
        &self,
        fees: AccountId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<FeeRow>, StoreError> {
        let after = i64::try_from(after).unwrap_or(i64::MAX);
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT f.sequence, f.debit, o.credit, f.amount FROM ledger_transfers f \
             LEFT JOIN ledger_transfers o ON o.packet_epoch = f.packet_epoch \
               AND o.packet_sequence = f.packet_sequence AND o.debit = f.debit AND o.id != f.id \
             WHERE f.credit = ?1 AND f.sequence > ?2 AND f.reference = 'packet' \
               AND f.state = 'posted' \
             ORDER BY f.sequence LIMIT ?3",
        )?;
        statement
            .query_map(params![fees.0, after, limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .map(|row| {
                let (sequence, from, onward, value) = row?;
                let account = |id: i64| {
                    u32::try_from(id)
                        .map(AccountId)
                        .map_err(|_| StoreError::Corrupt("account ID"))
                };
                Ok(FeeRow {
                    sequence: amount(sequence)?,
                    from: account(from)?,
                    onward: onward.map(account).transpose()?,
                    amount: amount(value)?,
                })
            })
            .collect()
    }

    /// The sequence of the last trust transfer: credit extended, or raised.
    pub fn last_trust_sequence(&self) -> Result<Option<u64>, StoreError> {
        let connection = self.lock();
        let last: Option<i64> = connection.query_row(
            "SELECT max(sequence) FROM ledger_transfers WHERE reference = 'trust'",
            [],
            |row| row.get(0),
        )?;
        last.map(amount).transpose()
    }

    /// The last `limit` journal rows, of `account` if given, in sequence
    /// order: where a live view of the journal starts.
    pub fn transfers_last(
        &self,
        limit: u32,
        account: Option<AccountId>,
    ) -> Result<Vec<JournalRow>, StoreError> {
        let filter = match account {
            Some(_) => {
                "t.id IN (SELECT id FROM (SELECT id FROM ledger_transfers \
                   WHERE debit = ?3 AND sequence > ?1 ORDER BY sequence DESC LIMIT ?2) \
                 UNION ALL SELECT id FROM (SELECT id FROM ledger_transfers \
                   WHERE credit = ?3 AND sequence > ?1 ORDER BY sequence DESC LIMIT ?2))"
            }
            None => "t.sequence > ?1 AND ?3 IS NULL",
        };
        let mut rows = self.journal_rows(filter, "DESC", 0, limit, account)?;
        rows.reverse();
        Ok(rows)
    }

    fn journal_rows(
        &self,
        filter: &str,
        order: &str,
        after: i64,
        limit: u32,
        account: Option<AccountId>,
    ) -> Result<Vec<JournalRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "SELECT {TRANSFER_COLUMNS}, d.path, c.path FROM ledger_transfers t \
             JOIN ledger_accounts d ON d.id = t.debit JOIN ledger_accounts c ON c.id = t.credit \
             WHERE {filter} ORDER BY t.sequence {order} LIMIT ?2"
        ))?;
        let account = account.map(|id| id.0);
        statement
            .query_map(params![after, limit, account], |row| {
                Ok((
                    read_transfer(row)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, String>(14)?,
                ))
            })?
            .map(|row| {
                let (transfer, sequence, debit_path, credit_path) = row?;
                let (transfer, state, resolved_at_ms) = transfer?;
                Ok(JournalRow {
                    sequence: amount(sequence)?,
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

/// A fee our connector earned on a packet from one peer to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeRow {
    /// Its place in the journal.
    pub sequence: u64,
    /// The account the packet came from.
    pub from: AccountId,
    /// The account the packet's other transfer from `from` credits.
    pub onward: Option<AccountId>,
    /// The fee.
    pub amount: u64,
}

/// The columns [`read_transfer`] reads, in its order.
const TRANSFER_COLUMNS: &str = "t.id, t.debit, t.credit, t.amount, t.state, t.created_at_ms, \
     t.resolved_at_ms, t.reference, t.packet_epoch, t.packet_sequence, t.settlement_scheme, \
     t.settlement_id, t.sequence";

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
            ("refusal", Some(epoch), Some(sequence), None, None) => Reference::Refusal(TraceId {
                epoch: amount(epoch)?,
                sequence: amount(sequence)?,
            }),
            ("fee", None, None, Some(scheme), Some(id)) => Reference::Fee {
                scheme: name(&scheme, "settlement scheme")?,
                id: name(&id, "fee ID")?,
            },
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
                "UPDATE ledger_transfers SET state = 'posted', resolved_at_ms = ?2, \
                 sequence = (SELECT max(sequence) + 1 FROM ledger_transfers) \
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
                "UPDATE ledger_transfers SET state = 'voided', resolved_at_ms = ?2, \
                 sequence = (SELECT max(sequence) + 1 FROM ledger_transfers) \
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
        Reference::Refusal(trace) => (
            "refusal",
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
        Reference::Fee { scheme, id } => (
            "fee",
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
         resolved_at_ms, reference, packet_epoch, packet_sequence, settlement_scheme, settlement_id, \
         sequence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, \
         (SELECT coalesce(max(sequence), 0) + 1 FROM ledger_transfers))",
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
    // Removals first: a scheme removed and added again in one batch ends up
    // added. (One added and then removed is not in `schemes_added`.)
    for id in &batch.schemes_removed {
        transaction.execute(
            "DELETE FROM settlement_schemes WHERE id = ?1",
            [id.as_str()],
        )?;
    }
    for (id, config) in &batch.schemes_added {
        transaction.execute(
            "INSERT INTO settlement_schemes (id, config, activated_at_ms) VALUES (?1, ?2, ?3)",
            params![id.as_str(), config, at],
        )?;
    }
    for (scheme, checkpoint) in &batch.checkpoints {
        let state = (!checkpoint.state.is_empty()).then_some(checkpoint.state.as_slice());
        transaction.execute(
            "INSERT INTO settlement_checkpoints (scheme, balance, updated_at_ms, state) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (scheme) DO UPDATE SET balance = ?2, updated_at_ms = ?3, state = ?4",
            params![scheme.as_str(), sql_amount(checkpoint.balance)?, at, state],
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
        write_cap(
            transaction,
            &TokenCap {
                id: token.id,
                cap: token.cap,
                usage: token.cap_usage,
            },
        )?;
    }
    for cap in &batch.token_caps {
        write_cap(transaction, cap)?;
    }
    for id in &batch.tokens_revoked {
        transaction.execute(
            "UPDATE btp_tokens SET revoked_at_ms = ?2 WHERE id = ?1 AND revoked_at_ms IS NULL",
            params![sql_amount(*id)?, to_sql_time(batch.at_ms)],
        )?;
    }
    Ok(())
}

/// Writes a token's cap and what is used of it. The usage is split into
/// whole amounts and a remainder, which together keep it exact.
fn write_cap(transaction: &Transaction<'_>, cap: &TokenCap) -> Result<(), StoreError> {
    let (amount, window, used, remainder) = match cap.cap {
        Some(limit) => {
            let window = u128::from(limit.window_ms);
            (
                Some(sql_amount(limit.amount)?),
                Some(sql_amount(limit.window_ms)?),
                // Above i64::MAX only through absurd amounts in flight;
                // rounding up keeps it on the safe side.
                i64::try_from(cap.usage.used / window).unwrap_or(i64::MAX),
                i64::try_from(cap.usage.used % window).unwrap_or(0),
            )
        }
        None => (None, None, 0, 0),
    };
    transaction.execute(
        "UPDATE btp_tokens SET cap_amount = ?2, cap_window_ms = ?3, cap_used_amount = ?4, \
         cap_used_remainder = ?5, cap_as_of_ms = ?6 WHERE id = ?1",
        params![
            sql_amount(cap.id)?,
            amount,
            window,
            used,
            remainder,
            to_sql_time(cap.usage.as_of_ms),
        ],
    )?;
    Ok(())
}

type CapColumns = Result<(Option<SpendingCap>, CapUsage), StoreError>;

/// Reads a token's cap from the five columns at `at`.
fn read_cap(row: &Row<'_>, at: usize) -> rusqlite::Result<CapColumns> {
    let amount_column: Option<i64> = row.get(at)?;
    let window: Option<i64> = row.get(at + 1)?;
    let used: i64 = row.get(at + 2)?;
    let remainder: i64 = row.get(at + 3)?;
    let as_of: i64 = row.get(at + 4)?;
    Ok((|| {
        let usage = |window_ms: u64| -> Result<CapUsage, StoreError> {
            Ok(CapUsage {
                used: u128::from(amount(used)?) * u128::from(window_ms)
                    + u128::from(amount(remainder)?),
                as_of_ms: from_sql_time(as_of),
            })
        };
        match (amount_column, window) {
            (Some(limit), Some(window_ms)) => {
                let cap = SpendingCap {
                    amount: amount(limit)?,
                    window_ms: amount(window_ms)?,
                };
                if !cap.is_valid() {
                    return Err(StoreError::Corrupt("token cap"));
                }
                Ok((Some(cap), usage(cap.window_ms)?))
            }
            (None, None) => Ok((None, CapUsage::new(from_sql_time(as_of)))),
            _ => Err(StoreError::Corrupt("token cap")),
        }
    })())
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

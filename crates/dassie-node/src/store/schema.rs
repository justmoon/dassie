//! Schema v1, its migrations, and the check that the file matches it.
//!
//! The original's database is not read (D3). The new one keeps its ideas: an
//! application ID that marks the file as Dassie's, `user_version`
//! migrations each in a transaction, STRICT tables, and a check at every
//! open that the tables are what the code expects (spec 2 §5.1). The check
//! is stricter than the original's: an extra column is an error too.

use rusqlite::{Connection, OptionalExtension, Transaction};

use super::StoreError;

/// Marks a file as a Dassie database. Like the original's, a random
/// positive int32 with nibbles 2–4 set to `DA5`.
pub const APPLICATION_ID: i32 = 0x7DA5_E2C9;

/// The original's ID. A file with it is a v0 database, which this version
/// does not read (D3).
pub const V0_APPLICATION_ID: i32 = 0x1DA5_3B81;

/// The migrations, in order. Migration `n` (1-based) brings `user_version`
/// to `n`. Never edit one that has shipped; add another.
const MIGRATIONS: &[&str] = &[MIGRATION_1, MIGRATION_2];

const MIGRATION_1: &str = "
CREATE TABLE node_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  realm TEXT NOT NULL CHECK (realm IN ('test', 'live')),
  hostname TEXT CHECK (hostname IS NULL OR (length(hostname) BETWEEN 1 AND 253 AND hostname = lower(hostname))),
  http_port INTEGER NOT NULL CHECK (http_port BETWEEN 1 AND 65535),
  https_port INTEGER NOT NULL CHECK (https_port BETWEEN 1 AND 65535 AND https_port != http_port),
  alias TEXT NOT NULL CHECK (length(CAST(alias AS BLOB)) <= 64),
  acme_directory_url TEXT NOT NULL CHECK (acme_directory_url LIKE 'https://%')
) STRICT;

INSERT INTO node_config VALUES
  (1, 'test', NULL, 80, 443, '', 'https://acme-v02.api.letsencrypt.org/directory');

CREATE TABLE node_key (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  private_key BLOB NOT NULL CHECK (length(private_key) = 32),
  created_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE tls_certificate (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  source TEXT NOT NULL CHECK (source IN ('self-signed', 'acme', 'manual')),
  chain_pem TEXT NOT NULL,
  key_pem TEXT NOT NULL,
  installed_at_ms INTEGER NOT NULL
) STRICT;

CREATE TABLE acme_account (
  directory_url TEXT PRIMARY KEY,
  credentials TEXT NOT NULL,
  contact TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE sessions (
  id BLOB PRIMARY KEY CHECK (length(id) = 32),
  created_at_ms INTEGER NOT NULL,
  last_used_at_ms INTEGER NOT NULL CHECK (last_used_at_ms >= created_at_ms),
  user_agent TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE INDEX sessions_by_last_use ON sessions (last_used_at_ms);
";

/// The ledger (spec 7 §4.4): its accounts with their posted totals cached,
/// the journal of transfers, the settlement schemes and their checkpoints,
/// and the BTP tokens, by hash (D10). Amounts are integers in the ledger's
/// units; the ledger keeps every total below `i64::MAX`.
const MIGRATION_2: &str = "
CREATE TABLE boot (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  epoch INTEGER NOT NULL CHECK (epoch >= 0)
) STRICT;

INSERT INTO boot VALUES (1, 0);

CREATE TABLE ledger_accounts (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  path TEXT NOT NULL UNIQUE,
  debits_posted INTEGER NOT NULL DEFAULT 0 CHECK (debits_posted >= 0),
  credits_posted INTEGER NOT NULL DEFAULT 0 CHECK (credits_posted >= 0)
) STRICT;

CREATE TABLE ledger_transfers (
  id INTEGER PRIMARY KEY CHECK (id >= 0),
  debit INTEGER NOT NULL REFERENCES ledger_accounts (id),
  credit INTEGER NOT NULL REFERENCES ledger_accounts (id) CHECK (credit != debit),
  amount INTEGER NOT NULL CHECK (amount > 0),
  state TEXT NOT NULL CHECK (state IN ('pending', 'posted', 'voided')),
  created_at_ms INTEGER NOT NULL,
  resolved_at_ms INTEGER CHECK ((state = 'pending') = (resolved_at_ms IS NULL)),
  reference TEXT NOT NULL CHECK (reference IN
    ('packet', 'settlement', 'trust', 'deposit', 'withdrawal', 'reconciliation')),
  packet_epoch INTEGER CHECK ((reference = 'packet') = (packet_epoch IS NOT NULL)),
  packet_sequence INTEGER CHECK ((reference = 'packet') = (packet_sequence IS NOT NULL)),
  settlement_scheme TEXT CHECK ((reference = 'settlement') = (settlement_scheme IS NOT NULL)),
  settlement_id TEXT CHECK ((reference = 'settlement') = (settlement_id IS NOT NULL))
) STRICT;

CREATE INDEX ledger_transfers_by_debit ON ledger_transfers (debit, state);
CREATE INDEX ledger_transfers_by_credit ON ledger_transfers (credit, state);
CREATE INDEX ledger_transfers_pending ON ledger_transfers (state) WHERE state = 'pending';

CREATE TABLE settlement_schemes (
  id TEXT PRIMARY KEY,
  config TEXT NOT NULL,
  activated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE settlement_checkpoints (
  scheme TEXT PRIMARY KEY,
  balance INTEGER NOT NULL CHECK (balance >= 0),
  updated_at_ms INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE btp_tokens (
  id INTEGER PRIMARY KEY CHECK (id >= 1),
  token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
  label TEXT NOT NULL CHECK (length(CAST(label AS BLOB)) <= 64),
  ledger TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  revoked_at_ms INTEGER
) STRICT;
";

/// A column as the code expects it: name, type, NOT NULL, part of the
/// primary key.
type Column = (&'static str, &'static str, bool, bool);

/// A table as the code expects it.
struct Table {
    name: &'static str,
    without_rowid: bool,
    columns: &'static [Column],
}

const TABLES: &[Table] = &[
    Table {
        name: "node_config",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("realm", "TEXT", true, false),
            ("hostname", "TEXT", false, false),
            ("http_port", "INTEGER", true, false),
            ("https_port", "INTEGER", true, false),
            ("alias", "TEXT", true, false),
            ("acme_directory_url", "TEXT", true, false),
        ],
    },
    Table {
        name: "node_key",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("private_key", "BLOB", true, false),
            ("created_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "tls_certificate",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("source", "TEXT", true, false),
            ("chain_pem", "TEXT", true, false),
            ("key_pem", "TEXT", true, false),
            ("installed_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "acme_account",
        without_rowid: true,
        columns: &[
            ("directory_url", "TEXT", true, true),
            ("credentials", "TEXT", true, false),
            ("contact", "TEXT", true, false),
            ("created_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "sessions",
        without_rowid: true,
        columns: &[
            ("id", "BLOB", true, true),
            ("created_at_ms", "INTEGER", true, false),
            ("last_used_at_ms", "INTEGER", true, false),
            ("user_agent", "TEXT", true, false),
        ],
    },
    Table {
        name: "boot",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("epoch", "INTEGER", true, false),
        ],
    },
    Table {
        name: "ledger_accounts",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("path", "TEXT", true, false),
            ("debits_posted", "INTEGER", true, false),
            ("credits_posted", "INTEGER", true, false),
        ],
    },
    Table {
        name: "ledger_transfers",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("debit", "INTEGER", true, false),
            ("credit", "INTEGER", true, false),
            ("amount", "INTEGER", true, false),
            ("state", "TEXT", true, false),
            ("created_at_ms", "INTEGER", true, false),
            ("resolved_at_ms", "INTEGER", false, false),
            ("reference", "TEXT", true, false),
            ("packet_epoch", "INTEGER", false, false),
            ("packet_sequence", "INTEGER", false, false),
            ("settlement_scheme", "TEXT", false, false),
            ("settlement_id", "TEXT", false, false),
        ],
    },
    Table {
        name: "settlement_schemes",
        without_rowid: true,
        columns: &[
            ("id", "TEXT", true, true),
            ("config", "TEXT", true, false),
            ("activated_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "settlement_checkpoints",
        without_rowid: true,
        columns: &[
            ("scheme", "TEXT", true, true),
            ("balance", "INTEGER", true, false),
            ("updated_at_ms", "INTEGER", true, false),
        ],
    },
    Table {
        name: "btp_tokens",
        without_rowid: false,
        columns: &[
            ("id", "INTEGER", false, true),
            ("token_hash", "BLOB", true, false),
            ("label", "TEXT", true, false),
            ("ledger", "TEXT", true, false),
            ("created_at_ms", "INTEGER", true, false),
            ("revoked_at_ms", "INTEGER", false, false),
        ],
    },
];

/// Claims a new file, refuses foreign or v0 ones, and migrates.
pub fn prepare(connection: &mut Connection) -> Result<(), StoreError> {
    let application_id: i32 =
        connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let schema_version: i32 =
        connection.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    match application_id {
        APPLICATION_ID => {}
        0 if schema_version == 0 => {
            tracing::info!(target: crate::log::target::DATABASE, "initializing a new database");
            connection.pragma_update(None, "application_id", APPLICATION_ID)?;
        }
        V0_APPLICATION_ID => return Err(StoreError::V0Database),
        other => return Err(StoreError::ForeignDatabase(other)),
    }
    migrate(connection)
}

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let current: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let known = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if current > known {
        return Err(StoreError::TooNew {
            found: current,
            known,
        });
    }
    for (index, sql) in MIGRATIONS
        .iter()
        .enumerate()
        .skip(usize::try_from(current).unwrap_or(0))
    {
        let version = i64::try_from(index).unwrap_or(i64::MAX).saturating_add(1);
        let transaction = connection.transaction()?;
        transaction.execute_batch(sql)?;
        transaction.pragma_update(None, "user_version", version)?;
        transaction.commit()?;
        tracing::info!(target: crate::log::target::DATABASE, version, "migrated");
    }
    Ok(())
}

/// Checks every table against the code: present, STRICT, the right ROWID
/// setting, and exactly the expected columns.
pub fn check(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction()?;
    for table in TABLES {
        check_table(&transaction, table)?;
    }
    Ok(())
}

fn check_table(transaction: &Transaction<'_>, table: &Table) -> Result<(), StoreError> {
    let mismatch = |problem: &str| StoreError::SchemaMismatch(format!("{}: {problem}", table.name));
    let flags: Option<(i64, i64)> = transaction
        .query_row(
            "SELECT strict, wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
            [table.name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((strict, without_rowid)) = flags else {
        return Err(mismatch("missing"));
    };
    if strict != 1 {
        return Err(mismatch("not STRICT"));
    }
    if (without_rowid == 1) != table.without_rowid {
        return Err(mismatch("wrong WITHOUT ROWID setting"));
    }
    let mut statement = transaction
        .prepare("SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid")?;
    let columns = statement
        .query_map([table.name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, i64>(3)? > 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let expected = table
        .columns
        .iter()
        .map(|&(name, kind, not_null, pk)| (name.to_string(), kind.to_string(), not_null, pk));
    if !columns.iter().cloned().eq(expected) {
        return Err(mismatch(&format!("columns are {columns:?}")));
    }
    Ok(())
}

//! The database: what must survive a restart (spec 7 §4.4).
//!
//! One SQLite file, `dassie.sqlite3`, in a data directory that only the
//! daemon's user may enter (0700), itself readable only by that user (0600),
//! because it holds the node key (D10, RT-25). Modes found looser at start
//! are tightened, with a warning.
//!
//! Every statement is short, and the connection is used synchronously behind
//! a mutex: a statement takes microseconds and a thread pool would cost more
//! than it saves. Commits wait for the disk (`synchronous = FULL`), because
//! a fulfilled packet is passed on only once its transfer is durable, power
//! loss included; the ledger's engine commits a whole batch of packets at
//! once to keep that affordable.

mod gnap;
mod inspect;
mod ledger;
#[cfg(test)]
mod ledger_tests;
mod outgoing;
#[cfg(test)]
mod owed_tests;
mod payments;
mod peer;
mod peer_settings;
#[cfg(test)]
mod peer_settings_tests;
#[cfg(test)]
mod peer_tests;
mod rates;
mod schema;
mod snapshot;

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use dassie_identity::NodeKey;
use dassie_node_core::certificate::CertificateSource;
use dassie_node_core::config::{Alias, DirectoryUrl, Hostname, NodeConfig, Ports, Realm};
use dassie_node_core::session::{SessionId, SessionPolicy, SessionTimes};
use rusqlite::{Connection, OptionalExtension, params};

pub use gnap::{GrantRow, GrantState, MAX_GRANTS, TokenRow};
pub use inspect::{BLOB_CUT, Cell, ColumnInfo, MAX_ROWS, RowsPage, TEXT_CUT, TableInfo, Value};
pub use ledger::{
    BtpTokenRow, Checkpoint, FeeRow, JournalRow, LedgerBatch, StoredLedger, TokenCap,
};
pub use outgoing::{
    ForeignAmount, MAX_LIVE_OWNER_QUOTES, MAX_LIVE_QUOTES, OutgoingBatch, OutgoingProgress,
    OutgoingRow, OutgoingState, QuoteRow,
};
pub use payments::{
    ConnectionKey, IncomingPage, IncomingPaid, IncomingProgress, IncomingQuery, IncomingRow,
    Listed, MAX_LISTED, MAX_OPEN_LOADED, Page, PaymentId, UNPAID_RETENTION_MS,
};
pub use peer_settings::PeerSettings;
pub use rates::StoredRate;
pub use schema::{APPLICATION_ID, SCHEMA_VERSION, V0_APPLICATION_ID};
pub use snapshot::{PRE_MIGRATION_FILE, ROLLED_BACK_FILE, Restored, restore_pre_migration};

use crate::log::target;

/// The database file's name.
pub const DATABASE_FILE: &str = "dassie.sqlite3";

/// Something went wrong with the database.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite reported an error.
    Sqlite(rusqlite::Error),
    /// Creating or securing the data directory failed.
    Io(std::io::Error),
    /// The file is a database of the original Dassie.
    V0Database,
    /// The file belongs to another application.
    ForeignDatabase(i32),
    /// The file was written by a newer version.
    TooNew {
        /// Its `user_version`.
        found: i64,
        /// The newest this version knows.
        known: i64,
    },
    /// A table does not match the code.
    SchemaMismatch(String),
    /// A stored value is invalid, which only a bug or a hand edit can cause.
    Corrupt(&'static str),
    /// The database is too new for this version, and the copy from before
    /// its migration cannot be put back, for this reason.
    NoUsableCopy(&'static str),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "database error: {error}"),
            Self::Io(error) => write!(f, "data directory: {error}"),
            Self::V0Database => f.write_str(
                "this is a database of Dassie v0, which this version cannot read; \
                 move it away and set the node up again with your passphrase: \
                 the node ID and ledger accounts follow from it",
            ),
            Self::ForeignDatabase(id) => {
                write!(
                    f,
                    "the database file belongs to another application (application ID {id:#x})"
                )
            }
            Self::TooNew { found, known } => write!(
                f,
                "the database is from a newer version of Dassie (schema {found}, this version knows {known})"
            ),
            Self::SchemaMismatch(problem) => {
                write!(f, "the database does not match this version: {problem}")
            }
            Self::Corrupt(what) => write!(f, "the database holds an invalid {what}"),
            Self::NoUsableCopy(why) => write!(
                f,
                "the database is from a newer version of Dassie, and the copy from before \
                 its migration cannot be put back: {why}"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The certificate as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCertificate {
    /// Where it came from.
    pub source: CertificateSource,
    /// PEM, leaf first.
    pub chain_pem: String,
    /// PEM.
    pub key_pem: String,
}

/// A session as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    /// The token's hash.
    pub id: SessionId,
    /// When it was made and last used.
    pub times: SessionTimes,
    /// The browser that logged in.
    pub user_agent: String,
}

/// The database.
#[derive(Debug)]
pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    /// Opens or creates the database in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self, StoreError> {
        secure_directory(data_dir)?;
        let path = data_dir.join(DATABASE_FILE);
        secure_file(&path)?;
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        // Before migrating, so that a rollback to the version before can
        // have the database back as that version left it.
        if let Some(from) = schema::pending_migration(&connection)? {
            snapshot::keep_copy(&connection, data_dir, from)?;
        }
        Self::prepare(connection)
    }

    /// A database in memory, for `verify-install` and tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut connection: Connection) -> Result<Self, StoreError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        schema::prepare(&mut connection)?;
        schema::check(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock left no transaction open: rusqlite
        // rolls back on drop. So the connection is still consistent.
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `SELECT 1 + 1`, for `verify-install`.
    pub fn self_test(&self) -> Result<i64, StoreError> {
        Ok(self
            .lock()
            .query_row("SELECT 1 + 1", [], |row| row.get(0))?)
    }

    /// The configuration.
    pub fn config(&self) -> Result<NodeConfig, StoreError> {
        let row = self.lock().query_row(
            "SELECT realm, hostname, http_port, https_port, alias, acme_directory_url FROM node_config",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )?;
        let (realm, hostname, http_port, https_port, alias, directory) = row;
        Ok(NodeConfig {
            realm: Realm::parse(&realm).map_err(|_| StoreError::Corrupt("realm"))?,
            hostname: hostname
                .map(|name| Hostname::parse(&name).map_err(|_| StoreError::Corrupt("hostname")))
                .transpose()?,
            ports: Ports::new(http_port, https_port).map_err(|_| StoreError::Corrupt("ports"))?,
            alias: Alias::parse(&alias).map_err(|_| StoreError::Corrupt("alias"))?,
            acme_directory: DirectoryUrl::parse(&directory)
                .map_err(|_| StoreError::Corrupt("ACME directory"))?,
        })
    }

    /// Stores the configuration.
    pub fn set_config(&self, config: &NodeConfig) -> Result<(), StoreError> {
        self.lock().execute(
            "UPDATE node_config SET realm = ?1, hostname = ?2, http_port = ?3, https_port = ?4, \
             alias = ?5, acme_directory_url = ?6",
            params![
                config.realm.as_str(),
                config.hostname.as_ref().map(Hostname::as_str),
                config.ports.http(),
                config.ports.https(),
                config.alias.as_str(),
                config.acme_directory.as_str(),
            ],
        )?;
        Ok(())
    }

    /// The node key, once set up.
    pub fn node_key(&self) -> Result<Option<NodeKey>, StoreError> {
        let bytes: Option<Vec<u8>> = self
            .lock()
            .query_row("SELECT private_key FROM node_key", [], |row| row.get(0))
            .optional()?;
        bytes
            .map(|bytes| {
                let seed: [u8; 32] = bytes
                    .try_into()
                    .map_err(|_| StoreError::Corrupt("node key"))?;
                Ok(NodeKey::from_bytes(&seed))
            })
            .transpose()
    }

    /// Stores the node key unless one is stored already, and says whether it
    /// did. Two setups racing each other cannot both win. The setup token
    /// goes with it: setup is done.
    pub fn set_node_key_once(&self, key: &NodeKey, now_ms: u64) -> Result<bool, StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let changed = transaction.execute(
            "INSERT INTO node_key (id, private_key, created_at_ms) VALUES (1, ?1, ?2) ON CONFLICT DO NOTHING",
            params![key.as_bytes().as_slice(), to_sql_time(now_ms)],
        )?;
        transaction.execute("DELETE FROM setup_token", [])?;
        transaction.commit()?;
        Ok(changed == 1)
    }

    /// The setup token, if one is stored.
    pub fn setup_token(&self) -> Result<Option<[u8; 32]>, StoreError> {
        let bytes: Option<Vec<u8>> = self
            .lock()
            .query_row("SELECT token FROM setup_token", [], |row| row.get(0))
            .optional()?;
        bytes
            .map(|bytes| {
                bytes
                    .try_into()
                    .map_err(|_| StoreError::Corrupt("setup token"))
            })
            .transpose()
    }

    /// Stores the setup token, replacing any other.
    pub fn set_setup_token(&self, token: &[u8; 32]) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT OR REPLACE INTO setup_token (id, token) VALUES (1, ?1)",
            [token.as_slice()],
        )?;
        Ok(())
    }

    /// The certificate, if one is stored.
    pub fn certificate(&self) -> Result<Option<StoredCertificate>, StoreError> {
        let row: Option<(String, String, String)> = self
            .lock()
            .query_row(
                "SELECT source, chain_pem, key_pem FROM tls_certificate",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        row.map(|(source, chain_pem, key_pem)| {
            let source = CertificateSource::parse(&source)
                .ok_or(StoreError::Corrupt("certificate source"))?;
            Ok(StoredCertificate {
                source,
                chain_pem,
                key_pem,
            })
        })
        .transpose()
    }

    /// Stores the certificate, replacing any other.
    pub fn set_certificate(
        &self,
        certificate: &StoredCertificate,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT OR REPLACE INTO tls_certificate (id, source, chain_pem, key_pem, installed_at_ms) \
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![certificate.source.as_str(), certificate.chain_pem, certificate.key_pem, to_sql_time(now_ms)],
        )?;
        Ok(())
    }

    /// The ACME account credentials (instant-acme's JSON) for `directory`.
    pub fn acme_account(&self, directory: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT credentials FROM acme_account WHERE directory_url = ?1",
                [directory],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Stores the ACME account for `directory`.
    pub fn set_acme_account(
        &self,
        directory: &str,
        credentials: &str,
        contact: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        self.lock().execute(
            "INSERT OR REPLACE INTO acme_account (directory_url, credentials, contact, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4)",
            params![directory, credentials, contact, to_sql_time(now_ms)],
        )?;
        Ok(())
    }

    /// Stores a new session, and deletes the least recently used ones beyond
    /// the policy's limit, in one transaction. Says how many it deleted.
    pub fn insert_session(
        &self,
        record: &SessionRecord,
        policy: &SessionPolicy,
    ) -> Result<u32, StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO sessions (id, created_at_ms, last_used_at_ms, user_agent) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.id.0.as_slice(),
                to_sql_time(record.times.created_at_ms),
                to_sql_time(record.times.last_used_at_ms),
                record.user_agent,
            ],
        )?;
        // The new session always stays; of the others, the most recently
        // used fill the remaining places. Ties on time go by creation, then
        // by ID, so the order is total and the outcome deterministic.
        let evicted = transaction.execute(
            "DELETE FROM sessions WHERE id != ?1 AND id NOT IN \
             (SELECT id FROM sessions WHERE id != ?1 \
              ORDER BY last_used_at_ms DESC, created_at_ms DESC, id LIMIT ?2)",
            params![
                record.id.0.as_slice(),
                policy.max_sessions.saturating_sub(1)
            ],
        )?;
        transaction.commit()?;
        Ok(u32::try_from(evicted).unwrap_or(u32::MAX))
    }

    /// The session with `id`.
    pub fn session(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError> {
        Ok(self
            .lock()
            .query_row(
                "SELECT id, created_at_ms, last_used_at_ms, user_agent FROM sessions WHERE id = ?1",
                [id.0.as_slice()],
                session_row,
            )
            .optional()?)
    }

    /// Every session, most recently used first.
    pub fn sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, created_at_ms, last_used_at_ms, user_agent FROM sessions \
             ORDER BY last_used_at_ms DESC, id",
        )?;
        Ok(statement
            .query_map([], session_row)?
            .collect::<Result<_, _>>()?)
    }

    /// Records a use of the session.
    pub fn touch_session(&self, id: &SessionId, now_ms: u64) -> Result<(), StoreError> {
        self.lock().execute(
            "UPDATE sessions SET last_used_at_ms = max(last_used_at_ms, ?2) WHERE id = ?1",
            params![id.0.as_slice(), to_sql_time(now_ms)],
        )?;
        Ok(())
    }

    /// Deletes a session, and says whether there was one.
    pub fn delete_session(&self, id: &SessionId) -> Result<bool, StoreError> {
        Ok(self
            .lock()
            .execute("DELETE FROM sessions WHERE id = ?1", [id.0.as_slice()])?
            == 1)
    }

    /// Deletes every session but `keep`, and says how many.
    pub fn delete_sessions_except(&self, keep: Option<&SessionId>) -> Result<u32, StoreError> {
        let keep = keep.map(|id| id.0.to_vec()).unwrap_or_default();
        let deleted = self
            .lock()
            .execute("DELETE FROM sessions WHERE id != ?1", [keep])?;
        Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
    }

    /// Deletes the sessions that have expired at `now_ms`.
    pub fn prune_sessions(&self, policy: &SessionPolicy, now_ms: u64) -> Result<u32, StoreError> {
        let deleted = self.lock().execute(
            "DELETE FROM sessions WHERE last_used_at_ms + ?1 <= ?3 OR created_at_ms + ?2 <= ?3",
            params![
                to_sql_time(policy.idle_timeout_ms),
                to_sql_time(policy.max_lifetime_ms),
                to_sql_time(now_ms)
            ],
        )?;
        Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
    }
}

fn session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let id: [u8; 32] = row.get(0)?;
    Ok(SessionRecord {
        id: SessionId(id),
        times: SessionTimes {
            created_at_ms: from_sql_time(row.get(1)?),
            last_used_at_ms: from_sql_time(row.get(2)?),
        },
        user_agent: row.get(3)?,
    })
}

/// Milliseconds as SQLite's signed integer. Times past the year 292 million
/// are clamped.
fn to_sql_time(ms: u64) -> i64 {
    i64::try_from(ms).unwrap_or(i64::MAX)
}

fn from_sql_time(ms: i64) -> u64 {
    u64::try_from(ms).unwrap_or(0)
}

/// Creates `path` with mode 0700, or tightens an existing directory to it.
fn secure_directory(path: &Path) -> Result<(), StoreError> {
    if !path.exists() {
        DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    }
    tighten(path, 0o700)
}

/// Creates `path` with mode 0600 before SQLite opens it, so there is no
/// moment at which it is readable by others; tightens an existing file.
fn secure_file(path: &Path) -> Result<(), StoreError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    tighten(path, 0o600)
}

fn tighten(path: &Path, mode: u32) -> Result<(), StoreError> {
    let current = fs::metadata(path)?.permissions().mode() & 0o777;
    if current & !mode != 0 {
        tracing::warn!(
            target: target::DATABASE,
            path = %path.display(),
            from = format_args!("{current:o}"),
            to = format_args!("{mode:o}"),
            "tightening permissions"
        );
        fs::set_permissions(path, Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

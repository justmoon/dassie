//! The database: what must survive a restart (spec/areas/node.md#persistence).
//!
//! One SQLite file, `dassie.sqlite3`, in a data directory that only the
//! daemon's user may enter (0700), itself readable only by that user (0600),
//! because it holds the node key (spec/decisions/secrets-at-rest.md).
//! Modes found looser at start are tightened, with a warning.
//!
//! Every statement is short, and the connection is used synchronously behind
//! a mutex: a statement takes microseconds and a thread pool would cost more
//! than it saves. Commits wait for the disk (`synchronous = FULL`), because
//! a fulfilled packet is passed on only once its transfer is durable, power
//! loss included; the ledger's engine commits a whole batch of packets at
//! once to keep that affordable.

mod devices;
#[cfg(feature = "testing")]
mod faults;
mod gnap;
mod hub_founding;
mod hub_funding;
mod hubs;
mod inspect;
mod ledger;
#[cfg(test)]
mod ledger_tests;
mod outgoing;
mod payments;
mod peer;
mod peer_settings;
#[cfg(test)]
mod peer_settings_tests;
#[cfg(test)]
mod peer_tests;
mod rates;
mod schema;
#[cfg(test)]
mod schema_tests;
mod snapshot;

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use dassie_identity::NodeKey;
use dassie_node_core::certificate::CertificateSource;
use dassie_node_core::config::{Alias, DirectoryUrl, Hostname, NodeConfig, Realm};
use dassie_node_core::session::{SessionId, SessionPolicy, SessionTimes};
use rusqlite::{Connection, OptionalExtension, Params, Row, params};

pub use devices::{
    AddDevice, DEVICE_SEEN_GRANULARITY_MS, DeviceRow, MAX_DEVICE_NAME_BYTES, MAX_DEVICES,
};
#[cfg(feature = "testing")]
pub use faults::{Aim, FailedCommit, Holds};
pub use gnap::{GrantCounts, GrantRow, GrantState, MAX_GRANTS, TokenRow};
pub use hub_founding::{
    FoundingRecordRow, FoundingRow, MAX_FOUNDING_RECORDS, MAX_FOUNDINGS, MAX_PAYOUT_ROWS,
};
pub use hub_funding::{HubFunding, HubFundingRow, HubMoveRow};
pub use hubs::{
    EscrowCursor, HubStep, KEPT_BOUNDARIES, KEPT_BUNDLES, KEPT_TIPS, MAX_HUB_BUNDLE_BYTES,
    MAX_HUB_CONSENSUS_BYTES, MAX_HUB_LEDGER_BYTES, MAX_HUB_VERIFIER_BYTES, StoredHub,
};
pub use inspect::{BLOB_CUT, Cell, ColumnInfo, MAX_ROWS, RowsPage, TEXT_CUT, TableInfo, Value};
pub use ledger::{BtpTokenRow, Checkpoint, JournalRow, LedgerBatch, StoredLedger, TokenCap};
pub use outgoing::{
    ForeignAmount, MAX_LIVE_OWNER_QUOTES, MAX_LIVE_QUOTES, OutgoingBatch, OutgoingProgress,
    OutgoingRow, OutgoingState, QuoteRow,
};
pub use payments::{
    ConnectionKey, FINISHED_RETENTION_MS, FinishedIncoming, IncomingPage, IncomingPaid,
    IncomingProgress, IncomingQuery, IncomingRow, Listed, MAX_LISTED, MAX_OPEN_LOADED, Page,
    PaymentId, UNPAID_RETENTION_MS,
};
pub use peer_settings::PeerSettings;
pub use rates::StoredRate;
pub use schema::{APPLICATION_ID, EARLY_TESTNET_APPLICATION_ID, SCHEMA_VERSION};
pub use snapshot::{
    PRE_MIGRATION_FILE, ROLLED_BACK_FILE, ROLLING_BACK_FILE, Restored, restore_pre_migration,
};

use crate::log::target;

/// The database file's name.
pub const DATABASE_FILE: &str = "dassie.sqlite3";

/// The file a store holds an exclusive lock on while it is open, so that
/// only one node runs on a data directory.
pub const LOCK_FILE: &str = "dassie.lock";

/// How many prepared statements the connection keeps: every statement the
/// store runs, with room to spare. Past that, the one used longest ago is
/// prepared again when it is next needed.
const STATEMENT_CACHE: usize = 256;

/// Something went wrong with the database.
#[derive(Debug)]
pub enum StoreError {
    /// SQLite reported an error.
    Sqlite(rusqlite::Error),
    /// Creating or securing the data directory failed.
    Io(std::io::Error),
    /// The file is a database of the early testnet.
    EarlyTestnetDatabase,
    /// The file belongs to another application.
    ForeignDatabase(i32),
    /// The file was written by a newer version.
    TooNew {
        /// Its `user_version`.
        found: i64,
        /// The newest this version knows.
        known: i64,
    },
    /// The file has a schema older than the baseline's, which no release
    /// wrote: a build from before the first release on this schema
    /// (spec/decisions/database-continuity.md).
    TooOld {
        /// Its `user_version`.
        found: i64,
    },
    /// A table does not match the code.
    SchemaMismatch(String),
    /// A stored value is invalid, which only a bug or a hand edit can cause.
    Corrupt(&'static str),
    /// The database is too new for this version, and the copy from before
    /// its migration cannot be put back, for this reason.
    NoUsableCopy(&'static str),
    /// There is no database, but a rollback moved one aside
    /// ([`ROLLING_BACK_FILE`]): it stopped before it put back the copy from
    /// before the migration.
    RollbackUnfinished,
    /// Another process has the data directory open: a node runs on it.
    InUse,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "database error: {error}"),
            Self::Io(error) => write!(f, "data directory: {error}"),
            Self::EarlyTestnetDatabase => f.write_str(
                "this database was made by a release of Dassie for the early testnet, \
                 before the testnet was reset, and this version cannot read it; move it \
                 away and set the node up again with a new phrase, since this version \
                 reads no phrase made before the reset",
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
            Self::TooOld { found } => write!(
                f,
                "the database has schema {found}, older than any release of Dassie wrote, which \
                 this version does not migrate: set the node up again"
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
            Self::RollbackUnfinished => write!(
                f,
                "a rollback stopped half way: there is no {DATABASE_FILE}, and \
                 {ROLLING_BACK_FILE} is the database the update left, with its SQLite side \
                 files ({ROLLING_BACK_FILE}-wal, -shm; some may still be {DATABASE_FILE}-wal \
                 and -shm, which belong with it). To go back to the database from before \
                 the update, move those side files aside and put {PRE_MIGRATION_FILE} in place \
                 as {DATABASE_FILE}. To start afresh instead, with a new identity and no \
                 books, remove {ROLLING_BACK_FILE} and its side files"
            ),
            Self::InUse => write!(
                f,
                "another Dassie node runs on this data directory (it holds {LOCK_FILE})"
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

/// Statements run through the connection's cache of prepared statements,
/// as [`Connection::execute`] and [`Connection::query_row`] run one: a
/// statement run again is not parsed and planned again, which took most of
/// the time a busy node spent in the store.
trait Cached {
    /// [`Connection::execute`], cached.
    fn execute_cached<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize>;

    /// [`Connection::query_row`], cached.
    fn query_row_cached<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>;
}

impl Cached for Connection {
    fn execute_cached<P: Params>(&self, sql: &str, params: P) -> rusqlite::Result<usize> {
        self.prepare_cached(sql)?.execute(params)
    }

    fn query_row_cached<T, P, F>(&self, sql: &str, params: P, f: F) -> rusqlite::Result<T>
    where
        P: Params,
        F: FnOnce(&Row<'_>) -> rusqlite::Result<T>,
    {
        self.prepare_cached(sql)?.query_row(params, f)
    }
}

/// The database.
#[derive(Debug)]
pub struct Store {
    connection: Mutex<Connection>,
    /// The data directory's lock, held while the store is open; none in
    /// memory.
    _lock: Option<fs::File>,
    /// The commit a simulation makes fail, while it has one to.
    #[cfg(feature = "testing")]
    failing: Mutex<Option<faults::Failing>>,
}

impl Store {
    /// Opens or creates the database in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self, StoreError> {
        Self::open_with(data_dir, schema::MIGRATIONS)
    }

    /// [`Store::open`] with `migrations` for this code's, so that a test can
    /// open a database as a version with more of them would.
    fn open_with(data_dir: &Path, migrations: &[&str]) -> Result<Self, StoreError> {
        secure_directory(data_dir)?;
        let lock = lock_directory(data_dir)?;
        let path = data_dir.join(DATABASE_FILE);
        // A rollback that stopped between moving the database aside and
        // putting the copy in left none, and a new one would be a node
        // with another identity and no books.
        if !path.exists() && data_dir.join(ROLLING_BACK_FILE).exists() {
            return Err(StoreError::RollbackUnfinished);
        }
        secure_file(&path)?;
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        // Before migrating, so that a rollback to the version before can
        // have the database back as that version left it.
        if let Some(from) = schema::pending_migration(&connection, migrations)? {
            snapshot::keep_copy(&connection, data_dir, from)?;
        }
        Self::prepare(connection, Some(lock), migrations)
    }

    /// A database in memory, for `verify-install` and tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::prepare(migrated_in_memory()?, None, schema::MIGRATIONS)
    }

    fn prepare(
        mut connection: Connection,
        lock: Option<fs::File>,
        migrations: &[&str],
    ) -> Result<Self, StoreError> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.set_prepared_statement_cache_capacity(STATEMENT_CACHE);
        connection.pragma_update(None, "foreign_keys", "ON")?;
        schema::prepare(&mut connection, migrations)?;
        schema::check(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            _lock: lock,
            #[cfg(feature = "testing")]
            failing: Mutex::new(None),
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
            .query_row_cached("SELECT 1 + 1", [], |row| row.get(0))?)
    }

    /// The configuration.
    pub fn config(&self) -> Result<NodeConfig, StoreError> {
        let row = self.lock().query_row_cached(
            "SELECT realm, hostname, alias, acme_directory_url FROM node_config",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        let (realm, hostname, alias, directory) = row;
        Ok(NodeConfig {
            realm: Realm::parse(&realm).map_err(|_| StoreError::Corrupt("realm"))?,
            hostname: hostname
                .map(|name| Hostname::parse(&name).map_err(|_| StoreError::Corrupt("hostname")))
                .transpose()?,
            alias: Alias::parse(&alias).map_err(|_| StoreError::Corrupt("alias"))?,
            acme_directory: DirectoryUrl::parse(&directory)
                .map_err(|_| StoreError::Corrupt("ACME directory"))?,
        })
    }

    /// Stores the configuration.
    pub fn set_config(&self, config: &NodeConfig) -> Result<(), StoreError> {
        self.lock().execute_cached(
            "UPDATE node_config SET realm = ?1, hostname = ?2, alias = ?3, acme_directory_url = ?4",
            params![
                config.realm.as_str(),
                config.hostname.as_ref().map(Hostname::as_str),
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
            .query_row_cached("SELECT private_key FROM node_key", [], |row| row.get(0))
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
    /// did, with the node's realm in the same write. Two setups racing each
    /// other cannot both win. The setup token goes with it: setup is done.
    pub fn set_node_key_once(
        &self,
        key: &NodeKey,
        realm: Realm,
        now_ms: u64,
    ) -> Result<bool, StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let changed = transaction.execute_cached(
            "INSERT INTO node_key (id, private_key, created_at_ms) VALUES (1, ?1, ?2) ON CONFLICT DO NOTHING",
            params![key.as_bytes().as_slice(), to_sql_time(now_ms)],
        )?;
        // The realm is fixed with the key, and never changes after
        // (spec/decisions/mainnet-schemes.md).
        if changed == 1 {
            transaction
                .execute_cached("UPDATE node_config SET realm = ?1", params![realm.as_str()])?;
        }
        transaction.execute_cached("DELETE FROM setup_token", [])?;
        transaction.commit()?;
        Ok(changed == 1)
    }

    /// The setup token, if one is stored.
    pub fn setup_token(&self) -> Result<Option<[u8; 32]>, StoreError> {
        let bytes: Option<Vec<u8>> = self
            .lock()
            .query_row_cached("SELECT token FROM setup_token", [], |row| row.get(0))
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
        self.lock().execute_cached(
            "INSERT OR REPLACE INTO setup_token (id, token) VALUES (1, ?1)",
            [token.as_slice()],
        )?;
        Ok(())
    }

    /// The certificate, if one is stored.
    pub fn certificate(&self) -> Result<Option<StoredCertificate>, StoreError> {
        let row: Option<(String, String, String)> = self
            .lock()
            .query_row_cached(
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
        self.lock().execute_cached(
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
            .query_row_cached(
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
        self.lock().execute_cached(
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
        transaction.execute_cached(
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
        let evicted = transaction.execute_cached(
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
            .query_row_cached(
                "SELECT id, created_at_ms, last_used_at_ms, user_agent FROM sessions WHERE id = ?1",
                [id.0.as_slice()],
                session_row,
            )
            .optional()?)
    }

    /// Every session, most recently used first.
    pub fn sessions(&self) -> Result<Vec<SessionRecord>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare_cached(
            "SELECT id, created_at_ms, last_used_at_ms, user_agent FROM sessions \
             ORDER BY last_used_at_ms DESC, id",
        )?;
        Ok(statement
            .query_map([], session_row)?
            .collect::<Result<_, _>>()?)
    }

    /// Records a use of the session.
    pub fn touch_session(&self, id: &SessionId, now_ms: u64) -> Result<(), StoreError> {
        self.lock().execute_cached(
            "UPDATE sessions SET last_used_at_ms = max(last_used_at_ms, ?2) WHERE id = ?1",
            params![id.0.as_slice(), to_sql_time(now_ms)],
        )?;
        Ok(())
    }

    /// Deletes a session, and says whether there was one.
    pub fn delete_session(&self, id: &SessionId) -> Result<bool, StoreError> {
        Ok(self
            .lock()
            .execute_cached("DELETE FROM sessions WHERE id = ?1", [id.0.as_slice()])?
            == 1)
    }

    /// Deletes every session but `keep`, and says how many.
    pub fn delete_sessions_except(&self, keep: Option<&SessionId>) -> Result<u32, StoreError> {
        let keep = keep.map(|id| id.0.to_vec()).unwrap_or_default();
        let deleted = self
            .lock()
            .execute_cached("DELETE FROM sessions WHERE id != ?1", [keep])?;
        Ok(u32::try_from(deleted).unwrap_or(u32::MAX))
    }

    /// Deletes the sessions that have expired at `now_ms`.
    pub fn prune_sessions(&self, policy: &SessionPolicy, now_ms: u64) -> Result<u32, StoreError> {
        let deleted = self.lock().execute_cached(
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

/// A database in memory at the current schema: a copy of one the process
/// migrated once, so that a simulation or a test that opens many stores
/// migrates only once; a copy starts from the same bytes, which
/// [`Store::prepare`] still checks.
fn migrated_in_memory() -> Result<Connection, StoreError> {
    static MIGRATED: Mutex<Option<Connection>> = Mutex::new(None);
    let mut migrated = MIGRATED.lock().unwrap_or_else(PoisonError::into_inner);
    let source = match migrated.take() {
        Some(source) => source,
        None => {
            let mut source = Connection::open_in_memory()?;
            schema::prepare(&mut source, schema::MIGRATIONS)?;
            source
        }
    };
    let mut copy = Connection::open_in_memory()?;
    rusqlite::backup::Backup::new(&source, &mut copy)?.run_to_completion(
        i32::MAX,
        Duration::ZERO,
        None,
    )?;
    *migrated = Some(source);
    Ok(copy)
}

/// Takes the exclusive lock on the data directory's [`LOCK_FILE`], which
/// lasts until the file is closed, when the process exits at the latest.
/// Two nodes on one database would each sign, settle and book as if the
/// other did not exist; SQLite's own locks keep only their transactions
/// apart.
fn lock_directory(data_dir: &Path) -> Result<fs::File, StoreError> {
    let path = data_dir.join(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(fs::TryLockError::WouldBlock) => Err(StoreError::InUse),
        Err(fs::TryLockError::Error(error)) => Err(StoreError::Io(error)),
    }
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

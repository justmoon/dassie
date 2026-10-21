//! The copy of the database from before a migration, and putting it back
//! when an update is rolled back (spec/decisions/release-integrity.md).
//!
//! A new version migrates the schema as it opens the database, before it
//! serves anything. The version before it refuses a database it does not
//! know ([`StoreError::TooNew`]), so if the new version then fails to start,
//! going back to the old one needs the database as it was. The daemon keeps
//! that copy just before it migrates, and `dassie update` puts it back when
//! it rolls back. The database it replaces is kept beside it, untouched, for
//! the owner to look at.

use std::fs::{self, File, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OpenFlags};

use super::schema::{APPLICATION_ID, SCHEMA_VERSION};
use super::{DATABASE_FILE, StoreError};
use crate::log::target;

/// The copy of the database from before its last migration.
pub const PRE_MIGRATION_FILE: &str = "dassie.sqlite3.pre-migration";
/// Where a rollback moves the database it replaces.
pub const ROLLED_BACK_FILE: &str = "dassie.sqlite3.rolled-back";
/// Where a rollback moves the database it replaces until the copy is in.
/// Without a database beside it, a rollback stopped half way.
pub const ROLLING_BACK_FILE: &str = "dassie.sqlite3.rolling-back";
/// SQLite's files beside a database in WAL mode, by suffix.
const COMPANIONS: [&str; 3] = ["", "-wal", "-shm"];
/// How much older than the update the copy's time may look.
const MTIME_SLACK: Duration = Duration::from_secs(1);

/// Writes a copy of the database, at schema version `from`, to
/// [`PRE_MIGRATION_FILE`], replacing the copy of an earlier migration.
pub(super) fn keep_copy(
    connection: &Connection,
    data_dir: &Path,
    from: i64,
) -> Result<(), StoreError> {
    let staged = data_dir.join(format!("{PRE_MIGRATION_FILE}.partial"));
    let target = data_dir.join(PRE_MIGRATION_FILE);
    let name = staged
        .to_str()
        .ok_or_else(|| std::io::Error::other("the data directory's path is not UTF-8"))?;
    match fs::remove_file(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // A consistent copy, written without disturbing the database.
    connection.execute("VACUUM INTO ?1", [name])?;
    let file = File::open(&staged)?;
    // It holds the node key, like the database.
    file.set_permissions(Permissions::from_mode(0o600))?;
    file.sync_all()?;
    fs::rename(&staged, &target)?;
    File::open(data_dir)?.sync_all()?;
    tracing::info!(
        target: target::DATABASE,
        from,
        to = SCHEMA_VERSION,
        copy = %target.display(),
        "kept a copy of the database from before the migration"
    );
    Ok(())
}

/// What [`restore_pre_migration`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restored {
    /// Nothing: this version can open the database as it is.
    Nothing,
    /// It put back the copy from before the migration.
    Copy {
        /// The schema version of the database it moved aside.
        from: i64,
        /// The schema version it put back.
        to: i64,
    },
}

/// Puts back the database from before the last migration, if the database
/// is too new for this version and the copy was made at `since` or later,
/// by the start that failed. The node must not be running.
///
/// The database it replaces goes to [`ROLLED_BACK_FILE`], with its WAL, by
/// way of [`ROLLING_BACK_FILE`] while the copy is not in yet.
///
/// # Errors
///
/// [`StoreError::NoUsableCopy`] if the database is too new and there is no
/// copy from that start that this version can open, besides IO and SQLite
/// errors.
pub fn restore_pre_migration(data_dir: &Path, since: SystemTime) -> Result<Restored, StoreError> {
    let database = data_dir.join(DATABASE_FILE);
    if !database.exists() {
        return Ok(Restored::Nothing);
    }
    let (application_id, found) = versions(&database)?;
    if application_id != APPLICATION_ID || found <= SCHEMA_VERSION {
        return Ok(Restored::Nothing);
    }
    let copy = data_dir.join(PRE_MIGRATION_FILE);
    let made = match fs::metadata(&copy) {
        Ok(metadata) => metadata.modified()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::NoUsableCopy("there is none"));
        }
        Err(error) => return Err(error.into()),
    };
    // The kernel stamps files with a clock that may lag the system clock
    // by a tick.
    if made + MTIME_SLACK < since {
        return Err(StoreError::NoUsableCopy(
            "the one there is older than this update",
        ));
    }
    let (copy_id, to) = versions(&copy)?;
    if copy_id != APPLICATION_ID || to > SCHEMA_VERSION {
        return Err(StoreError::NoUsableCopy(
            "the one there is not one this version can open",
        ));
    }
    // What earlier rollbacks moved aside goes, finished or not: there is a
    // database again.
    for name in [ROLLED_BACK_FILE, ROLLING_BACK_FILE] {
        for suffix in COMPANIONS {
            match fs::remove_file(data_dir.join(format!("{name}{suffix}"))) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    // The database first, so that its WAL is never left with the copy
    // alone; until the copy is in, `ROLLING_BACK_FILE` without a database
    // stops a node from starting afresh (`Store::open`).
    for suffix in COMPANIONS {
        let file = data_dir.join(format!("{DATABASE_FILE}{suffix}"));
        if file.exists() {
            fs::rename(&file, data_dir.join(format!("{ROLLING_BACK_FILE}{suffix}")))?;
        }
    }
    fs::rename(&copy, &database)?;
    for suffix in COMPANIONS {
        let aside = data_dir.join(format!("{ROLLING_BACK_FILE}{suffix}"));
        if aside.exists() {
            fs::rename(&aside, data_dir.join(format!("{ROLLED_BACK_FILE}{suffix}")))?;
        }
    }
    File::open(data_dir)?.sync_all()?;
    Ok(Restored::Copy { from: found, to })
}

/// The application ID and schema version of the database at `path`, read
/// without changing it.
fn versions(path: &Path) -> Result<(i32, i64), StoreError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let application_id = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let version = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok((application_id, version))
}

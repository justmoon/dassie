//! The database as the debug pages show it (spec 7 §5): every table and its
//! rows, a page at a time, never a secret.
//!
//! The SQL is built only from the schema's own table definitions, never
//! from a name a caller passed. Secret columns are selected as `NULL`, so
//! their values are never read. Long values are cut.

use rusqlite::types::ValueRef;
use rusqlite::{Row, params};

use super::schema::{SECRET_COLUMNS, TABLES, Table};
use super::{Store, StoreError};

/// The most rows in a page.
pub const MAX_ROWS: u32 = 200;
/// Text longer than this many characters is cut.
pub const TEXT_CUT: usize = 1_024;
/// Blobs longer than this many bytes are cut.
pub const BLOB_CUT: usize = 256;

/// A column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    /// Its name.
    pub name: &'static str,
    /// Its SQLite type.
    pub kind: &'static str,
    /// Whether it is part of the primary key.
    pub primary_key: bool,
    /// Whether its values are secret, and never shown.
    pub redacted: bool,
}

/// A table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableInfo {
    /// Its name.
    pub name: &'static str,
    /// Its columns.
    pub columns: Vec<ColumnInfo>,
    /// How many rows it has.
    pub rows: u64,
    /// Whether `rows` is exact. The journal reports its highest row ID
    /// instead, since counting it would hold up the node.
    pub rows_exact: bool,
}

/// A value in a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `NULL`.
    Null,
    /// An integer.
    Integer(i64),
    /// Text, perhaps cut.
    Text(String),
    /// Bytes, perhaps cut.
    Blob(Vec<u8>),
    /// A secret.
    Redacted,
}

/// A value and, if it was cut, its full length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The value.
    pub value: Value,
    /// Its length before it was cut, if it was.
    pub full_length: Option<u64>,
}

/// A page of rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowsPage {
    /// The rows, each a cell per column.
    pub rows: Vec<Vec<Cell>>,
    /// Where the next page starts, if there may be one.
    pub next_cursor: Option<u64>,
}

/// Whether `table.column` holds a secret.
fn is_secret(table: &str, column: &str) -> bool {
    SECRET_COLUMNS
        .iter()
        .any(|(secret_table, secret_column)| *secret_table == table && *secret_column == column)
}

impl Store {
    /// Every table, in schema order.
    pub fn inspect_tables(&self) -> Result<Vec<TableInfo>, StoreError> {
        let connection = self.lock();
        TABLES
            .iter()
            .map(|table| {
                let (count, exact) = if table.name == "ledger_transfers" {
                    ("coalesce(max(rowid), 0)", false)
                } else {
                    ("count(*)", true)
                };
                let rows: i64 = connection.query_row(
                    &format!("SELECT {count} FROM \"{}\"", table.name),
                    [],
                    |row| row.get(0),
                )?;
                Ok(TableInfo {
                    name: table.name,
                    columns: columns(table),
                    rows: u64::try_from(rows).unwrap_or(0),
                    rows_exact: exact,
                })
            })
            .collect()
    }

    /// Up to `limit` rows of `table`, from `cursor` on (0 for the first
    /// page), with secrets redacted; `None` if there is no such table.
    pub fn inspect_rows(
        &self,
        table: &str,
        cursor: u64,
        limit: u32,
    ) -> Result<Option<RowsPage>, StoreError> {
        let Some(table) = TABLES.iter().find(|known| known.name == table) else {
            return Ok(None);
        };
        let limit = limit.clamp(1, MAX_ROWS);
        let cursor_sql = i64::try_from(cursor).unwrap_or(i64::MAX);
        let selected = selection(table);
        let sql = if table.without_rowid {
            // Every WITHOUT ROWID table is bounded by the node's limits, so
            // an offset stays cheap.
            let key: Vec<String> = table
                .columns
                .iter()
                .filter(|column| column.3)
                .map(|column| format!("\"{}\"", column.0))
                .collect();
            format!(
                "SELECT 0, {selected} FROM \"{}\" ORDER BY {} LIMIT ?2 OFFSET ?1",
                table.name,
                key.join(", ")
            )
        } else {
            format!(
                "SELECT rowid, {selected} FROM \"{}\" WHERE rowid > ?1 ORDER BY rowid LIMIT ?2",
                table.name
            )
        };
        let connection = self.lock();
        let mut statement = connection.prepare(&sql)?;
        let mut last_rowid = 0;
        let rows = statement
            .query_map(params![cursor_sql, limit], |row| {
                last_rowid = row.get::<_, i64>(0)?;
                read_row(row, table)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let full = rows.len() == usize::try_from(limit).unwrap_or(usize::MAX);
        let next_cursor = full.then(|| {
            if table.without_rowid {
                cursor.saturating_add(u64::from(limit))
            } else {
                u64::try_from(last_rowid).unwrap_or(0)
            }
        });
        Ok(Some(RowsPage { rows, next_cursor }))
    }
}

fn columns(table: &Table) -> Vec<ColumnInfo> {
    table
        .columns
        .iter()
        .map(|&(name, kind, _, primary_key)| ColumnInfo {
            name,
            kind,
            primary_key,
            redacted: is_secret(table.name, name),
        })
        .collect()
}

/// Two values per column: the value, cut if long, and its full length.
fn selection(table: &Table) -> String {
    table
        .columns
        .iter()
        .map(|&(name, kind, _, _)| {
            if is_secret(table.name, name) {
                return String::from("NULL, NULL");
            }
            match kind {
                "TEXT" => format!("substr(\"{name}\", 1, {TEXT_CUT}), length(\"{name}\")"),
                "BLOB" => format!("substr(\"{name}\", 1, {BLOB_CUT}), length(\"{name}\")"),
                _ => format!("\"{name}\", NULL"),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn read_row(row: &Row<'_>, table: &Table) -> rusqlite::Result<Vec<Cell>> {
    let mut cells = Vec::with_capacity(table.columns.len());
    for (index, &(name, _, _, _)) in table.columns.iter().enumerate() {
        let at = 1 + index * 2;
        let value = if is_secret(table.name, name) {
            Value::Redacted
        } else {
            match row.get_ref(at)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => Value::Integer(value),
                ValueRef::Real(_) => Value::Null,
                ValueRef::Text(bytes) => Value::Text(String::from_utf8_lossy(bytes).into_owned()),
                ValueRef::Blob(bytes) => Value::Blob(bytes.to_vec()),
            }
        };
        let length: Option<i64> = row.get(at + 1)?;
        let shown = match &value {
            Value::Text(text) => text.chars().count(),
            Value::Blob(bytes) => bytes.len(),
            _ => 0,
        };
        let full_length = length
            .and_then(|length| u64::try_from(length).ok())
            .filter(|length| *length > u64::try_from(shown).unwrap_or(u64::MAX));
        cells.push(Cell { value, full_length });
    }
    Ok(cells)
}

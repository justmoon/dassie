//! The node's own workings, for the debug pages
//! (spec/areas/rpc.md#the-inspector): its log ring, its database without
//! secrets, and its services and cells. None needs the engine, so they work
//! before setup too.

use std::sync::Arc;

use prost::Message;
use tokio::time::Instant;

use super::handlers::{Items, Reply};
use super::live::{KEEPALIVE, MIN_ITEM_INTERVAL};
use super::proto::{self, request::Method, response, stream_item};
use super::{RpcError, convert};
use crate::log::{LogLevel, LogLine};
use crate::node::Node;
use crate::store::{Cell, MAX_ROWS, Value};
use crate::supervisor::{ServiceState, ServiceStatus};

/// Lines in one item of the log stream.
const LOG_CHUNK: usize = 200;

/// Bytes of lines in one item, well below the message limit.
const LOG_CHUNK_BYTES: usize = 256 * 1024;

/// Answers the inspector's methods; hands the others back.
pub async fn dispatch(
    node: &Arc<Node>,
    method: Method,
    items: &Items,
) -> Result<Result<Reply, Method>, RpcError> {
    let reply = match method {
        Method::SubscribeLogs(_) => return subscribe_logs(node, items).await.map(Ok),
        Method::ListDatabaseTables(_) => list_tables(node)?,
        Method::ListDatabaseRows(request) => list_rows(node, &request)?,
        Method::ListServices(_) => list_services(node),
        other => return Ok(Err(other)),
    };
    Ok(Ok(Reply::Unary(reply)))
}

/// Streams the ring: what it holds, then each new line.
async fn subscribe_logs(node: &Arc<Node>, items: &Items) -> Result<Reply, RpcError> {
    let _permit = node
        .live_stream_permit()
        .ok_or_else(|| RpcError::exhausted("too many live streams"))?;
    let ring = Arc::clone(node.logs());
    let mut latest = ring.subscribe();
    let mut after = 0;
    let mut reset = true;
    let mut sent_at = Instant::now();
    loop {
        latest.borrow_and_update();
        loop {
            let (lines, skipped) = ring.after(after, LOG_CHUNK);
            let keepalive = sent_at.elapsed() >= KEEPALIVE;
            if lines.is_empty() && skipped == 0 && !reset && !keepalive {
                break;
            }
            let lines = within_bytes(lines.iter().map(log_line));
            if let Some(last) = lines.last() {
                after = last.index;
            }
            let more = !lines.is_empty() && after < ring.latest();
            let event = proto::LogEvent {
                reset,
                lines,
                skipped,
            };
            items
                .send_wait(stream_item::Method::SubscribeLogs(event))
                .await?;
            sent_at = Instant::now();
            reset = false;
            if !more {
                break;
            }
        }
        tokio::time::sleep(MIN_ITEM_INTERVAL).await;
        // A line, or the keepalive.
        let _ = tokio::time::timeout(KEEPALIVE, latest.changed()).await;
    }
}

/// As many lines as fit in an item; at least one.
fn within_bytes(lines: impl Iterator<Item = proto::LogLine>) -> Vec<proto::LogLine> {
    let mut kept = Vec::new();
    let mut bytes = 0;
    for line in lines {
        bytes += line.encoded_len();
        if bytes > LOG_CHUNK_BYTES && !kept.is_empty() {
            break;
        }
        kept.push(line);
    }
    kept
}

fn log_line(line: &LogLine) -> proto::LogLine {
    let level = match line.level {
        LogLevel::Debug => proto::LogLevel::Debug,
        LogLevel::Info => proto::LogLevel::Info,
        LogLevel::Warn => proto::LogLevel::Warn,
        LogLevel::Error => proto::LogLevel::Error,
    };
    proto::LogLine {
        index: line.index,
        time_unix_ms: convert::time(line.time_unix_ms),
        level: level.into(),
        target: line.target.clone(),
        message: line.message.clone(),
        fields: line
            .fields
            .iter()
            .map(|(name, value)| proto::LogField {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn list_tables(node: &Node) -> Result<response::Method, RpcError> {
    let tables = node.store().inspect_tables().map_err(RpcError::internal)?;
    Ok(response::Method::ListDatabaseTables(
        proto::ListDatabaseTablesResponse {
            tables: tables
                .into_iter()
                .map(|table| proto::DatabaseTable {
                    name: table.name.into(),
                    columns: table
                        .columns
                        .into_iter()
                        .map(|column| proto::DatabaseColumn {
                            name: column.name.into(),
                            r#type: column.kind.into(),
                            primary_key: column.primary_key,
                            redacted: column.redacted,
                        })
                        .collect(),
                    rows: table.rows,
                    rows_exact: table.rows_exact,
                })
                .collect(),
        },
    ))
}

fn list_rows(
    node: &Node,
    request: &proto::ListDatabaseRowsRequest,
) -> Result<response::Method, RpcError> {
    let limit = match request.limit {
        0 => 50,
        limit => limit.min(MAX_ROWS),
    };
    let page = node
        .store()
        .inspect_rows(&request.table, request.cursor, limit)
        .map_err(RpcError::internal)?
        .ok_or_else(|| RpcError::invalid("no such table"))?;
    Ok(response::Method::ListDatabaseRows(
        proto::ListDatabaseRowsResponse {
            rows: page
                .rows
                .into_iter()
                .map(|cells| proto::DatabaseRow {
                    values: cells.into_iter().map(value).collect(),
                })
                .collect(),
            next_cursor: page.next_cursor.unwrap_or(0),
        },
    ))
}

fn value(cell: Cell) -> proto::DatabaseValue {
    use proto::database_value::Value as V;
    let value = match cell.value {
        Value::Null => V::Null(true),
        Value::Integer(value) => V::Integer(value),
        Value::Text(text) => V::Text(text),
        Value::Blob(bytes) => V::Blob(bytes),
        Value::Redacted => V::Redacted(true),
    };
    proto::DatabaseValue {
        value: Some(value),
        full_length: cell.full_length.unwrap_or(0),
    }
}

fn list_services(node: &Node) -> response::Method {
    response::Method::ListServices(proto::ListServicesResponse {
        services: node.services().services().iter().map(service).collect(),
        cells: cells(node),
    })
}

fn service(status: &ServiceStatus) -> proto::Service {
    let state = match status.state {
        ServiceState::Running => proto::ServiceState::Running,
        ServiceState::Restarting => proto::ServiceState::Restarting,
        ServiceState::Stopped => proto::ServiceState::Stopped,
    };
    proto::Service {
        name: status.name.into(),
        state: state.into(),
        started_at_unix_ms: convert::time(status.started_at_ms),
        restarts: status.restarts,
        last_error: status.last_error.clone().unwrap_or_default(),
        last_error_at_unix_ms: status.last_error_at_ms.map_or(0, convert::time),
        restart_at_unix_ms: status.restart_at_ms.map_or(0, convert::time),
    }
}

/// The shared state the services follow, each in a line.
fn cells(node: &Node) -> Vec<proto::Cell> {
    let config = node.config();
    let cell = |name: &str, version: u64, summary: String| proto::Cell {
        name: name.into(),
        version,
        summary,
    };
    let engine = node.engine();
    let versions = engine
        .as_ref()
        .map(|engine| *engine.versions().borrow())
        .unwrap_or_default();
    let certificate = node.certificate().info();
    vec![
        cell(
            "config",
            0,
            format!(
                "realm {}, hostname {}, ports {}/{}, alias {:?}",
                config.realm.ilp_allocation_scheme(),
                config
                    .hostname
                    .as_ref()
                    .map_or("none", |name| name.as_str()),
                config.ports.http(),
                config.ports.https(),
                config.alias.as_str(),
            ),
        ),
        cell(
            "identity",
            0,
            node.node_key()
                .map_or_else(|| "none yet".into(), |key| key.node_id().to_string()),
        ),
        cell(
            "engine",
            0,
            if engine.is_some() {
                "running"
            } else {
                "stopped"
            }
            .into(),
        ),
        cell("ledger", versions.ledger, "changes to the books".into()),
        cell("network", versions.network, "changes to the network".into()),
        cell(
            "certificate",
            0,
            format!(
                "{:?}, for {}",
                certificate.source,
                certificate.names.join(", ")
            ),
        ),
        cell(
            "logs",
            *node.logs().subscribe().borrow(),
            format!("{} lines held", node.logs().len()),
        ),
    ]
}

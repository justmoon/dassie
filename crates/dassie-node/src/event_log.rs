//! The event log: an opt-in file to which the node appends every event of
//! its packet trace and of its settlement hubs, one JSON object per line
//! (spec/areas/node.md#the-event-log, spec/decisions/event-log-file.md).
//!
//! The owner turns it on with `DASSIE_EVENT_LOG`; unset, nothing here is
//! made. The engine never waits for it: a recorder task takes the events
//! from the engine's live broadcasts, as an RPC subscriber does, turns them
//! into lines with the RPC's own conversions, and hands them to a writer
//! thread through a bounded queue. What does not fit is dropped and
//! counted, and the count is written as a `lost` line once there is room.
//! The file is rotated at a size cap, so the log never takes more than
//! `(KEPT_FILES + 1) × max_bytes` of disk.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;

use crate::clock::Clock;
use crate::engine::{HubStreamEvent, PacketEvent};
use crate::rpc::proto;

/// The file's size, in bytes, past which it is rotated, unless
/// `DASSIE_EVENT_LOG_MAX_BYTES` says otherwise: 64 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// The smallest size cap, in bytes, the owner may set: 1 MiB, many lines.
pub const MIN_MAX_BYTES: u64 = 1024 * 1024;

/// The largest size cap, in bytes, the owner may set: 64 GiB.
pub const MAX_MAX_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// How many rotated files are kept beside the current one, `<path>.1`
/// (the newest) to `<path>.4`; the oldest is deleted.
pub const KEPT_FILES: u32 = 4;

/// Most lines waiting for the writer; past it, events are dropped and
/// counted.
pub const QUEUE_LINES: usize = 4096;

/// Longest line, in bytes, the writer takes: far more than any event
/// makes (a packet's address is cut at 96 bytes). A longer one is dropped
/// and counted, so a line never passes the size cap on its own.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

/// How long, in milliseconds, the writer waits for a line before it writes
/// what it lost and flushes anyway.
pub const IDLE_MS: u64 = 1000;

const _: () = assert!(MIN_MAX_BYTES > MAX_LINE_BYTES as u64);
const _: () = assert!(MIN_MAX_BYTES <= DEFAULT_MAX_BYTES && DEFAULT_MAX_BYTES <= MAX_MAX_BYTES);

/// Where the log goes, and how large a file grows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventLogConfig {
    /// The file, absolute: a relative `DASSIE_EVENT_LOG` is taken relative
    /// to the state directory.
    pub path: PathBuf,
    /// The size, in bytes, past which it is rotated.
    pub max_bytes: u64,
}

/// What the recorder and the writer share.
#[derive(Debug, Default)]
struct Shared {
    /// Events dropped since the last `lost` line.
    lost: AtomicU64,
    /// This node's ID, once its engine has started.
    node: OnceLock<String>,
}

/// A handle to the event log's writer. Cheap to clone.
#[derive(Clone, Debug)]
pub struct EventLog {
    lines: SyncSender<String>,
    shared: Arc<Shared>,
}

impl EventLog {
    /// Opens the file for appending, creating it readable by the node's
    /// user only, and starts the writer thread.
    ///
    /// # Errors
    ///
    /// The file cannot be opened, or the thread cannot be started.
    pub fn open(config: &EventLogConfig, clock: Arc<dyn Clock>) -> io::Result<Self> {
        let writer = Rotating::open(config.path.clone(), config.max_bytes)?;
        let (lines, queue) = std::sync::mpsc::sync_channel(QUEUE_LINES);
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        // A thread of its own, not a task: it blocks on the disk, and it
        // must not hold up the runtime's shutdown. It ends once every
        // handle is gone.
        std::thread::Builder::new()
            .name("dassie-event-log".into())
            .spawn(move || write_loop(writer, &queue, &thread_shared, clock.as_ref()))?;
        Ok(Self { lines, shared })
    }

    /// Queues `line`, or drops and counts it when the queue is full or the
    /// line too long. Never waits.
    pub fn push(&self, line: String) {
        if line.len() > MAX_LINE_BYTES {
            self.lose(1);
            return;
        }
        match self.lines.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => self.lose(1),
        }
    }

    /// Counts `count` events dropped.
    pub fn lose(&self, count: u64) {
        self.shared.lost.fetch_add(count, Ordering::Relaxed);
    }

    /// Records the engine's packet trace and hub events, from the
    /// broadcasts `packets` and `hubs`, until `generation` ends: a `start`
    /// line first, then a line for each event. The hub events are made
    /// while anyone listens, and this recorder listens.
    pub fn record(
        &self,
        node: dassie_identity::NodeId,
        (packets, hubs): (
            broadcast::Receiver<PacketEvent>,
            broadcast::Receiver<HubStreamEvent>,
        ),
        clock: Arc<dyn Clock>,
        generation: CancellationToken,
    ) {
        let node = node.to_string();
        // A node's ID never changes once it has one.
        let _ = self.shared.node.set(node.clone());
        let log = self.clone();
        tokio::spawn(async move {
            log.push(start_line(clock.now_ms(), &node, crate::VERSION));
            tokio::select! {
                () = generation.cancelled() => {}
                () = log.recorder(&node, packets, hubs, clock.as_ref()) => {}
            }
        });
    }

    async fn recorder(
        &self,
        node: &str,
        mut packets: broadcast::Receiver<PacketEvent>,
        mut hubs: broadcast::Receiver<HubStreamEvent>,
        clock: &dyn Clock,
    ) {
        loop {
            tokio::select! {
                event = packets.recv() => match event {
                    Ok(PacketEvent::Packet(traced)) => {
                        let packet = crate::rpc::packet_record(&traced);
                        self.push(packet_line(clock.now_ms(), node, &packet));
                    }
                    Ok(PacketEvent::Lagged(count)) | Err(RecvError::Lagged(count)) => {
                        self.lose(count);
                    }
                    Err(RecvError::Closed) => return,
                },
                event = hubs.recv() => match event {
                    Ok(HubStreamEvent::Event(event)) => {
                        let event = crate::rpc::hub_event_record(&event);
                        if let Some(line) = hub_line(clock.now_ms(), node, &event) {
                            self.push(line);
                        }
                    }
                    Ok(HubStreamEvent::Lagged(count)) | Err(RecvError::Lagged(count)) => {
                        self.lose(count);
                    }
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }
}

/// Writes lines as they come, a `lost` line whenever events were dropped,
/// and flushes once the queue is empty, until every handle is gone.
fn write_loop(mut writer: Rotating, queue: &Receiver<String>, shared: &Shared, clock: &dyn Clock) {
    let mut failing = false;
    loop {
        let first = match queue.recv_timeout(Duration::from_millis(IDLE_MS)) {
            Ok(line) => Some(line),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                let _ = writer.flush();
                return;
            }
        };
        // What waits, at most a queue's worth, then a flush.
        let waiting = first
            .into_iter()
            .chain(std::iter::from_fn(|| queue.try_recv().ok()))
            .take(QUEUE_LINES);
        for line in waiting {
            let written = writer.write_line(&line);
            failing = report(written, failing, &writer.path);
            if failing {
                shared.lost.fetch_add(1, Ordering::Relaxed);
            }
        }
        let lost = shared.lost.swap(0, Ordering::Relaxed);
        if lost > 0 {
            let node = shared.node.get().map_or("", String::as_str);
            let written = writer.write_line(&lost_line(clock.now_ms(), node, lost));
            failing = report(written, failing, &writer.path);
            if failing {
                // Counted again, for the next try.
                shared.lost.fetch_add(lost, Ordering::Relaxed);
            }
        }
        let flushed = writer.flush();
        failing = report(flushed, failing, &writer.path);
    }
}

/// Whether writing fails now, warning once when it starts to and telling
/// when it works again.
fn report(result: io::Result<()>, failing: bool, path: &Path) -> bool {
    match (result, failing) {
        (Ok(()), false) => false,
        (Ok(()), true) => {
            tracing::info!(path = %path.display(), "the event log is written again");
            false
        }
        (Err(error), false) => {
            tracing::warn!(path = %path.display(), %error, "the event log cannot be written: events are dropped and counted until it can");
            true
        }
        (Err(_), true) => true,
    }
}

/// The file and the size it has, rotated before a line would take it past
/// its cap.
#[derive(Debug)]
struct Rotating {
    path: PathBuf,
    max_bytes: u64,
    /// None after a failed rotation, until the file opens again.
    file: Option<BufWriter<File>>,
    size: u64,
}

impl Rotating {
    fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        assert!(max_bytes >= MIN_MAX_BYTES);
        let file = open_file(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            max_bytes,
            file: Some(BufWriter::new(file)),
            size,
        })
    }

    /// Appends `line` and a newline, rotating first if they would take the
    /// file past its cap.
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        assert!(line.len() <= MAX_LINE_BYTES && !line.contains('\n'));
        let bytes = u64::try_from(line.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if self.size > 0 && self.size.saturating_add(bytes) > self.max_bytes {
            self.flush()?;
            self.file = None;
            rotate(&self.path)?;
            self.size = 0;
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => {
                let file = open_file(&self.path)?;
                self.size = file.metadata()?.len();
                self.file.insert(BufWriter::new(file))
            }
        };
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        self.size = self.size.saturating_add(bytes);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// Opens `path` for appending, made readable by the node's user only.
fn open_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// `<path>.<n>`.
fn numbered(path: &Path, n: u32) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// Shifts `<path>.1` … `<path>.{KEPT_FILES - 1}` up by one, deleting
/// `<path>.{KEPT_FILES}`, and moves `path` to `<path>.1`.
fn rotate(path: &Path) -> io::Result<()> {
    ignore_missing(std::fs::remove_file(numbered(path, KEPT_FILES)))?;
    for n in (1..KEPT_FILES).rev() {
        ignore_missing(std::fs::rename(numbered(path, n), numbered(path, n + 1)))?;
    }
    std::fs::rename(path, numbered(path, 1))
}

fn ignore_missing(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// A JSON object written field by field, in order. Keys are the contract's
/// own, plain ASCII; strings are escaped by `serde_json`.
struct Line(String);

impl Line {
    fn new(ts: u64, node: &str, kind: &str) -> Self {
        let mut line = Self(String::with_capacity(512));
        line.0.push('{');
        line.number("ts", ts);
        line.string("node", node);
        line.string("type", kind);
        line
    }

    fn key(&mut self, key: &str) {
        assert!(key.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        if self.0.len() > 1 {
            self.0.push(',');
        }
        let _ = write!(self.0, "\"{key}\":");
    }

    fn string(&mut self, key: &str, value: &str) {
        self.key(key);
        // Writing a string cannot fail.
        self.0
            .push_str(&serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()));
    }

    fn number(&mut self, key: &str, value: impl std::fmt::Display) {
        self.key(key);
        let _ = write!(self.0, "{value}");
    }

    /// A 64-bit integer, as a decimal string, as proto JSON writes it.
    fn long(&mut self, key: &str, value: impl std::fmt::Display) {
        self.key(key);
        let _ = write!(self.0, "\"{value}\"");
    }

    fn boolean(&mut self, key: &str, value: bool) {
        self.number(key, value);
    }

    fn hex(&mut self, key: &str, bytes: &[u8]) {
        self.key(key);
        self.0.push('"');
        for byte in bytes {
            let _ = write!(self.0, "{byte:02x}");
        }
        self.0.push('"');
    }

    fn longs(&mut self, key: &str, values: &[u64]) {
        self.key(key);
        self.0.push('[');
        for (i, value) in values.iter().enumerate() {
            if i > 0 {
                self.0.push(',');
            }
            let _ = write!(self.0, "\"{value}\"");
        }
        self.0.push(']');
    }

    fn finish(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

/// The line that opens what one engine records.
fn start_line(ts: u64, node: &str, version: &str) -> String {
    let mut line = Line::new(ts, node, "start");
    line.string("version", version);
    line.finish()
}

/// The line that says `count` events were dropped.
fn lost_line(ts: u64, node: &str, count: u64) -> String {
    let mut line = Line::new(ts, node, "lost");
    line.number("count", count);
    line.finish()
}

/// A packet's record, as `SubscribePackets` streams it.
fn packet_line(ts: u64, node: &str, packet: &proto::Packet) -> String {
    let mut line = Line::new(ts, node, "packet");
    let trace = packet.trace.unwrap_or_default();
    line.string("trace", &format!("{}:{}", trace.epoch, trace.sequence));
    let state = match proto::PacketState::try_from(packet.state) {
        Ok(proto::PacketState::Pending) => "pending",
        Ok(proto::PacketState::Fulfilled) => "fulfilled",
        Ok(proto::PacketState::Rejected) => "rejected",
        _ => "unspecified",
    };
    line.string("state", state);
    line.string("rejectCode", &packet.reject_code);
    line.string("source", &packet.source);
    line.string("destination", &packet.destination);
    line.string("sourceNode", &packet.source_node);
    line.string("destinationNode", &packet.destination_node);
    line.string("destinationAddress", &packet.destination_address);
    line.long("amountIn", packet.amount_in);
    line.long("amountOut", packet.amount_out);
    line.string("ledgerIn", &packet.ledger_in);
    line.string("ledgerOut", &packet.ledger_out);
    line.hex("condition", &packet.condition);
    line.long("receivedAtUnixMs", packet.received_at_unix_ms);
    line.long("resolvedAtUnixMs", packet.resolved_at_unix_ms);
    line.longs("transfers", &packet.transfers);
    line.finish()
}

fn role(role: i32) -> &'static str {
    match proto::HubRole::try_from(role) {
        Ok(proto::HubRole::Payer) => "payer",
        Ok(proto::HubRole::Payee) => "payee",
        _ => "unspecified",
    }
}

/// What the node saw of a hub, as `SubscribeHubEvents` streams it, with the
/// case's fields in the same object; none for a `lagged` item, which the
/// log counts instead.
fn hub_line(ts: u64, node: &str, event: &proto::HubEvent) -> Option<String> {
    use proto::hub_event::Event;
    let kind = match event.event.as_ref()? {
        Event::Lagged(_) => return None,
        Event::Block(_) => "block",
        Event::Operation(_) => "operation",
        Event::Account(_) => "account",
        Event::Unfunded(_) => "unfunded",
        Event::Reduced(_) => "reduced",
        Event::ClaimsPaid(_) => "claimsPaid",
        Event::Resynced(_) => "resynced",
        Event::Dropped(_) => "dropped",
        Event::Commitment(_) => "commitment",
        Event::Claim(_) => "claim",
        Event::Move(_) => "move",
    };
    let mut line = Line::new(ts, node, "hub");
    line.string("scheme", &event.scheme);
    line.long("observedAtUnixMs", event.observed_at_unix_ms);
    line.string("event", kind);
    hub_fields(&mut line, event.event.as_ref()?);
    Some(line.finish())
}

fn hub_fields(line: &mut Line, event: &proto::hub_event::Event) {
    use proto::hub_event::Event;
    match event {
        Event::Lagged(_) | Event::Unfunded(_) => {}
        Event::Block(block) => {
            line.long("height", block.height);
            line.long("timestampUnixMs", block.timestamp_unix_ms);
        }
        Event::Operation(operation) => operation_fields(line, operation),
        Event::Account(account) => {
            line.long("height", account.height);
            line.number("number", account.number);
            line.long("balance", account.balance);
            line.long("reserve", account.reserve);
            line.long("sequence", account.sequence);
            line.long("deposited", account.deposited);
            line.long("withdrawn", account.withdrawn);
            line.long("returned", account.returned);
            line.boolean("closing", account.closing);
        }
        Event::Reduced(reduced) => {
            line.long("hold", reduced.hold);
            line.long("amount", reduced.amount);
        }
        Event::ClaimsPaid(paid) => {
            line.long("hold", paid.hold);
            line.number("count", paid.count);
            line.long("firstSequence", paid.first_sequence);
            line.long("lastSequence", paid.last_sequence);
            line.boolean("unlisted", paid.unlisted);
        }
        Event::Resynced(resynced) => {
            line.long("height", resynced.height);
            line.long("timestampUnixMs", resynced.timestamp_unix_ms);
            line.number("holdsBefore", resynced.holds_before);
            line.string("unread", &resynced.unread);
        }
        Event::Dropped(dropped) => line.string("purpose", &dropped.purpose),
        Event::Commitment(commitment) => {
            line.string("role", role(commitment.role));
            line.string("peer", &commitment.peer);
            line.long("hold", commitment.hold);
            line.long("sequence", commitment.sequence);
            line.long("amount", commitment.amount);
            line.hex("condition", &commitment.condition);
            line.long("expiryUnixMs", commitment.expiry_unix_ms);
        }
        Event::Claim(claim) => {
            line.string("role", role(claim.role));
            line.string("peer", &claim.peer);
            line.long("hold", claim.hold);
            line.long("total", claim.total);
            line.long("through", claim.through);
        }
        Event::Move(moved) => move_fields(line, moved),
    }
}

fn operation_fields(line: &mut Line, operation: &proto::HubOperation) {
    line.long("height", operation.height);
    line.long("timestampUnixMs", operation.timestamp_unix_ms);
    line.string("kind", &operation.kind);
    line.number("signer", operation.signer);
    line.long("sequence", operation.sequence);
    line.long("hold", operation.hold);
    line.number("peerAccount", operation.peer_account);
    line.long("amount", operation.amount);
    line.string("outcome", &operation.outcome);
    line.string("reason", &operation.reason);
    line.long("fee", operation.fee);
    line.string("purpose", &operation.purpose);
}

fn move_fields(line: &mut Line, moved: &proto::HubMoveEvent) {
    use proto::hub_move::Direction;
    use proto::hub_move_event::Stage;
    let stage = match Stage::try_from(moved.stage) {
        Ok(Stage::Started) => "started",
        Ok(Stage::Done) => "done",
        Ok(Stage::Failed) => "failed",
        Ok(Stage::Late) => "late",
        Ok(Stage::GivenUp) => "given_up",
        _ => "unspecified",
    };
    let direction = match Direction::try_from(moved.direction) {
        Ok(Direction::Deposit) => "deposit",
        Ok(Direction::Withdraw) => "withdraw",
        _ => "unspecified",
    };
    line.string("stage", stage);
    line.string("direction", direction);
    line.long("amount", moved.amount);
    line.string("payment", &moved.payment);
    line.boolean("byOwner", moved.by_owner);
}

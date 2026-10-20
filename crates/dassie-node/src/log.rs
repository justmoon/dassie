//! Logging: `tracing` events under the original's `das:node:*` targets.
//!
//! Which events reach standard output follows the original
//! (spec/areas/node.md#logging):
//!
//! - `DASSIE_LOG_LEVEL` (`debug`, `info`, `warn`, `error` or `none`) sets the
//!   lowest level shown;
//! - debug events are shown only for targets that match a `DEBUG` pattern.
//!   Patterns are separated by commas or whitespace, `*` matches any run of
//!   characters, a leading `-` excludes, and the first pattern that matches
//!   decides. The original replaced only the first `*` of a pattern (RT-28);
//!   here every `*` is a wildcard.
//!
//! Lines carry no timestamp: journald adds one.
//!
//! The node also keeps its last lines in a [`LogRing`] for the wallet's logs
//! page (spec/areas/node.md#logging): at most [`MAX_LINES`] of them and
//! [`MAX_RING_BYTES`], info and above whatever `DASSIE_LOG_LEVEL` says, and
//! debug for the targets `DEBUG` names. Fields named like secrets are
//! redacted.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tokio::sync::watch;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

use crate::clock::{Clock, SystemClock};

/// Most lines the ring keeps.
pub const MAX_LINES: usize = 1_000;
/// Most bytes of text the ring keeps.
pub const MAX_RING_BYTES: usize = 1 << 20;
/// Longest message kept; longer ones are cut.
const MAX_MESSAGE_BYTES: usize = 1_024;
/// Most fields kept per line.
const MAX_FIELDS: usize = 16;
/// Longest field value kept.
const MAX_FIELD_BYTES: usize = 256;

/// Targets of the node's own events.
pub mod target {
    /// Start, stop, and whether setup is still to be done.
    pub const DAEMON: &str = "das:node:daemon";
    /// The database.
    pub const DATABASE: &str = "das:node:database";
    /// The HTTP and HTTPS listeners.
    pub const HTTP: &str = "das:node:http";
    /// The IPC socket.
    pub const IPC: &str = "das:node:ipc";
    /// Certificates and ACME.
    pub const ACME: &str = "das:node:acme";
    /// Setup, login and sessions.
    pub const AUTH: &str = "das:node:auth";
    /// The RPC server.
    pub const RPC: &str = "das:node:rpc";
    /// Socket activation and readiness.
    pub const SYSTEMD: &str = "das:node:systemd";
    /// The ledger's engine.
    pub const ENGINE: &str = "das:node:accounting";
    /// BTP clients.
    pub const BTP: &str = "das:node:btp";
    /// The peer protocol: link states, membership, peering.
    pub const PEER: &str = "das:node:peer-protocol";
    /// Routes.
    pub const ROUTING: &str = "das:node:routing";
    /// Payments to the node's STREAM receiver.
    pub const PAYMENT: &str = "das:node:payment";
    /// Settlement schemes.
    pub const SETTLEMENT: &str = "das:node:settlement";
    /// A front door: its listeners, certificate and the nodes it reaches.
    pub const FRONT_DOOR: &str = "das:front-door";
}

/// The lowest level shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Everything, with debug events still filtered by `DEBUG`.
    Debug,
    /// The default.
    Info,
    /// Problems only.
    Warn,
    /// Failures only.
    Error,
    /// Nothing.
    None,
}

impl LogLevel {
    /// Reads `debug`, `info`, `warn`, `error` or `none`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    fn of(level: Level) -> Self {
        match level {
            Level::TRACE | Level::DEBUG => Self::Debug,
            Level::INFO => Self::Info,
            Level::WARN => Self::Warn,
            Level::ERROR => Self::Error,
        }
    }
}

/// `DASSIE_LOG_LEVEL` and `DEBUG` together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFilter {
    level: LogLevel,
    patterns: Vec<(bool, String)>,
}

impl LogFilter {
    /// A filter from a level and the text of `DEBUG`.
    #[must_use]
    pub fn new(level: LogLevel, debug: &str) -> Self {
        let patterns = debug
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|pattern| !pattern.is_empty())
            .map(|pattern| match pattern.strip_prefix('-') {
                Some(rest) => (false, rest.to_string()),
                None => (true, pattern.to_string()),
            })
            .collect();
        Self { level, patterns }
    }

    /// Whether an event of `level` from `target` is shown.
    #[must_use]
    pub fn enabled(&self, level: Level, target: &str) -> bool {
        let level = LogLevel::of(level);
        if self.level == LogLevel::None || level < self.level {
            return false;
        }
        level != LogLevel::Debug || self.debug_enabled(target)
    }

    fn debug_enabled(&self, target: &str) -> bool {
        self.patterns
            .iter()
            .find(|(_, pattern)| wildcard_match(pattern, target))
            .is_some_and(|(include, _)| *include)
    }
}

/// Whether `text` matches `pattern` in full, where `*` matches any run of
/// characters. Iterative, with a single backtrack point, so it runs in
/// O(pattern × text) at worst and never recurses (D16).
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match (pattern.get(p), text.get(t)) {
            (Some(b'*'), _) => {
                star = Some((p, t));
                p += 1;
            }
            (Some(expected), Some(actual)) if expected == actual => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern.iter().skip(p).all(|&byte| byte == b'*')
}

impl LogFilter {
    /// The ring's filter: the same `DEBUG` patterns, and info and above
    /// whatever the level shown.
    #[must_use]
    pub fn for_ring(&self) -> Self {
        Self {
            level: self.level.min(LogLevel::Info),
            patterns: self.patterns.clone(),
        }
    }
}

/// Sends events through `filter` to standard output, and keeps the last
/// ones in the [`ring`]. Called once, at start.
pub fn init(filter: LogFilter) {
    let ring_filter = filter.for_ring();
    let layer = fmt::layer()
        .without_time()
        .with_ansi(std::io::stdout().is_terminal())
        .with_filter(filter_fn(move |metadata: &Metadata<'_>| {
            filter.enabled(*metadata.level(), metadata.target())
        }));
    let ring_layer =
        RingLayer { ring: ring() }.with_filter(filter_fn(move |metadata: &Metadata<'_>| {
            ring_filter.enabled(*metadata.level(), metadata.target())
        }));
    // A second call (from tests) keeps the first subscriber.
    let _ = tracing_subscriber::registry()
        .with(layer)
        .with(ring_layer)
        .try_init();
}

/// A line the ring keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    /// Its number, counting from 1 since the process started.
    pub index: u64,
    /// When it was logged.
    pub time_unix_ms: u64,
    /// Its level.
    pub level: LogLevel,
    /// Its target.
    pub target: String,
    /// The message.
    pub message: String,
    /// The other fields, as text.
    pub fields: Vec<(String, String)>,
}

impl LogLine {
    fn bytes(&self) -> usize {
        self.target.len()
            + self.message.len()
            + self
                .fields
                .iter()
                .map(|(name, value)| name.len() + value.len())
                .sum::<usize>()
    }
}

#[derive(Debug, Default)]
struct RingState {
    lines: VecDeque<LogLine>,
    bytes: usize,
    last: u64,
}

/// The node's last log lines.
#[derive(Debug)]
pub struct LogRing {
    state: Mutex<RingState>,
    latest: watch::Sender<u64>,
}

impl Default for LogRing {
    fn default() -> Self {
        Self {
            state: Mutex::new(RingState::default()),
            latest: watch::Sender::new(0),
        }
    }
}

impl LogRing {
    /// Adds a line, numbering it, and drops the oldest beyond the bounds.
    pub fn push(&self, mut line: LogLine) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.last += 1;
        line.index = state.last;
        state.bytes += line.bytes();
        state.lines.push_back(line);
        while state.lines.len() > MAX_LINES || state.bytes > MAX_RING_BYTES {
            let Some(oldest) = state.lines.pop_front() else {
                break;
            };
            state.bytes -= oldest.bytes();
        }
        let last = state.last;
        drop(state);
        self.latest.send_replace(last);
    }

    /// Up to `limit` lines after the one numbered `after`, and how many
    /// lines after it were dropped before they could be read.
    pub fn after(&self, after: u64, limit: usize) -> (Vec<LogLine>, u64) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let first = state
            .lines
            .front()
            .map_or(state.last + 1, |line| line.index);
        let skipped = first.saturating_sub(after.saturating_add(1));
        let lines = state
            .lines
            .iter()
            .filter(|line| line.index > after)
            .take(limit)
            .cloned()
            .collect();
        (lines, skipped)
    }

    /// The number of the newest line.
    pub fn latest(&self) -> u64 {
        *self.latest.borrow()
    }

    /// The number of the newest line, as it changes.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.latest.subscribe()
    }

    /// How many lines it holds.
    pub fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .lines
            .len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The process's ring.
pub fn ring() -> Arc<LogRing> {
    static RING: OnceLock<Arc<LogRing>> = OnceLock::new();
    Arc::clone(RING.get_or_init(|| Arc::new(LogRing::default())))
}

struct RingLayer {
    ring: Arc<LogRing>,
}

/// What `f` logs, as the ring would keep it: for tests of what is logged.
#[cfg(test)]
pub(crate) fn capture(f: impl FnOnce()) -> Vec<LogLine> {
    let ring = Arc::new(LogRing::default());
    let layer = RingLayer {
        ring: Arc::clone(&ring),
    };
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), f);
    ring.after(0, MAX_LINES).0
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = LineVisitor::default();
        event.record(&mut visitor);
        // The clock module is the one place the node reads the real clock.
        let time_unix_ms = SystemClock.now_ms();
        self.ring.push(LogLine {
            index: 0,
            time_unix_ms,
            level: LogLevel::of(*metadata.level()),
            target: metadata.target().to_string(),
            message: visitor.message,
            fields: visitor.fields,
        });
    }
}

#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

/// Whether a field's value must not be kept.
fn is_secret(name: &str) -> bool {
    matches!(
        name,
        "secret" | "token" | "password" | "cookie" | "private_key" | "credentials" | "setup_url"
    ) || name.ends_with("_secret")
        || name.ends_with("_token")
}

/// `text`, cut to at most `limit` bytes at a character boundary.
fn cut(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

impl Visit for LineVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let mut text = String::new();
        let _ = write!(text, "{value:?}");
        self.record(field, text);
    }
}

impl LineVisitor {
    fn record(&mut self, field: &Field, value: String) {
        let name = field.name();
        if name == "message" {
            self.message = cut(value, MAX_MESSAGE_BYTES);
            return;
        }
        if self.fields.len() >= MAX_FIELDS {
            return;
        }
        let value = if is_secret(name) {
            String::from("[redacted]")
        } else {
            cut(value, MAX_FIELD_BYTES)
        };
        self.fields.push((name.to_string(), value));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;

    fn line(message: &str) -> LogLine {
        LogLine {
            index: 0,
            time_unix_ms: 1,
            level: LogLevel::Info,
            target: "das:node:test".into(),
            message: message.into(),
            fields: vec![],
        }
    }

    #[test]
    fn the_ring_keeps_the_last_lines() {
        let ring = LogRing::default();
        let mut latest = ring.subscribe();
        for n in 0..MAX_LINES + 5 {
            ring.push(line(&n.to_string()));
        }
        assert_eq!(ring.len(), MAX_LINES);
        assert_eq!(*latest.borrow_and_update(), 1_005);
        let (lines, skipped) = ring.after(0, 3);
        assert_eq!(skipped, 5, "the first five were dropped");
        assert_eq!(lines[0].index, 6);
        assert_eq!(lines[0].message, "5");
        let (lines, skipped) = ring.after(1_003, 10);
        assert_eq!((lines.len(), skipped), (2, 0));
        // Bytes bound it too.
        let big = "x".repeat(MAX_MESSAGE_BYTES);
        for _ in 0..MAX_LINES {
            ring.push(line(&big));
        }
        assert!(ring.len() * MAX_MESSAGE_BYTES <= MAX_RING_BYTES);
    }

    #[test]
    fn secrets_are_redacted_and_values_cut() {
        assert!(is_secret("token") && is_secret("setup_token") && !is_secret("tokens"));
        assert_eq!(cut("äöü".into(), 3), "ä…");
    }

    // The ring keeps info whatever the level shown, and debug as `DEBUG`
    // says.
    #[test]
    fn the_rings_filter() {
        let ring = LogFilter::new(LogLevel::None, "das:node:*").for_ring();
        assert!(ring.enabled(Level::INFO, "hyper"));
        assert!(!ring.enabled(Level::DEBUG, "hyper"));
        let ring = LogFilter::new(LogLevel::Debug, "das:node:*").for_ring();
        assert!(ring.enabled(Level::DEBUG, "das:node:http"));
    }

    #[test]
    fn wildcards() {
        assert!(wildcard_match("das:node:*", "das:node:http"));
        assert!(wildcard_match("*", ""));
        assert!(wildcard_match("das:*:http", "das:node:http"));
        assert!(!wildcard_match("das:node", "das:node:http"));
        assert!(wildcard_match("a*b*c", "aXbYc"));
        assert!(!wildcard_match("a*b*c", "aXbY"));
    }

    // RT-28: the original replaced only the first `*`, so this second
    // wildcard was a literal.
    #[test]
    fn every_star_is_a_wildcard() {
        assert!(wildcard_match("das:*:*", "das:node:acme"));
        let filter = LogFilter::new(LogLevel::Debug, "das:*:*");
        assert!(filter.enabled(Level::DEBUG, "das:node:acme"));
    }

    #[test]
    fn levels_and_debug_patterns() {
        let filter = LogFilter::new(LogLevel::Info, "das:node:*");
        assert!(filter.enabled(Level::INFO, "hyper"));
        assert!(
            !filter.enabled(Level::DEBUG, "das:node:http"),
            "below the level"
        );
        let filter = LogFilter::new(LogLevel::Debug, "-das:node:database, das:node:*");
        assert!(filter.enabled(Level::DEBUG, "das:node:http"));
        assert!(!filter.enabled(Level::DEBUG, "das:node:database"));
        assert!(!filter.enabled(Level::DEBUG, "hyper::proto"));
        assert!(filter.enabled(Level::WARN, "hyper::proto"));
        let none = LogFilter::new(LogLevel::None, "*");
        assert!(!none.enabled(Level::ERROR, "das:node:daemon"));
    }
}

//! Logging: `tracing` events under the node's `dassie::*` targets.
//!
//! Which events reach standard output is `DASSIE_LOG`'s
//! (spec/areas/node.md#logging), in the form of `tracing`'s `EnvFilter`:
//! directives separated by commas, each a level (`trace`, `debug`, `info`,
//! `warn`, `error` or `off`) for every target, or `<target>=<level>` for a
//! target and the targets beneath it (`dassie::peer` covers
//! `dassie::peer::links`, not `dassie::peering`). The most specific
//! directive that matches decides; a target none matches takes the bare
//! level, `info` if there is none. So `debug,hyper=warn` shows debug events
//! but hyper's warnings only, and `dassie::peer=debug` shows info and above
//! everywhere and the peer protocol's debug events too.
//!
//! Lines carry no timestamp: journald adds one.
//!
//! The node also keeps its last lines in a [`LogRing`] for the wallet's logs
//! page (spec/areas/node.md#logging): at most [`MAX_LINES`] of them and
//! [`MAX_RING_BYTES`], info and above whatever `DASSIE_LOG` says, and what
//! it shows below that. Fields named like secrets are redacted.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tokio::sync::watch;
use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
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
/// Most directives `DASSIE_LOG` holds.
pub const MAX_DIRECTIVES: usize = 64;
/// Longest target a directive names.
const MAX_TARGET_BYTES: usize = 128;

/// Targets of the node's own events.
pub mod target {
    /// Start, stop, and whether setup is still to be done.
    pub const DAEMON: &str = "dassie::daemon";
    /// The database.
    pub const DATABASE: &str = "dassie::database";
    /// The HTTP and HTTPS listeners.
    pub const HTTP: &str = "dassie::http";
    /// The IPC socket.
    pub const IPC: &str = "dassie::ipc";
    /// Certificates and ACME.
    pub const ACME: &str = "dassie::acme";
    /// Setup, login and sessions.
    pub const AUTH: &str = "dassie::auth";
    /// The RPC server.
    pub const RPC: &str = "dassie::rpc";
    /// Socket activation and readiness.
    pub const SYSTEMD: &str = "dassie::systemd";
    /// The ledger's engine.
    pub const ENGINE: &str = "dassie::engine";
    /// BTP clients.
    pub const BTP: &str = "dassie::btp";
    /// The peer protocol: link states, membership, peering.
    pub const PEER: &str = "dassie::peer";
    /// Routes.
    pub const ROUTING: &str = "dassie::routing";
    /// Payments to the node's STREAM receiver.
    pub const PAYMENT: &str = "dassie::payment";
    /// Settlement schemes.
    pub const SETTLEMENT: &str = "dassie::settlement";
    /// A front door: its listeners, certificate and the nodes it reaches.
    pub const FRONT_DOOR: &str = "dassie::front_door";
    /// The settlement hubs the node validates.
    pub const HUB: &str = "dassie::hub";
    /// The market maker, `dassie market-maker`.
    pub const MARKET_MAKER: &str = "dassie::market_maker";
    /// The desktop app around its node.
    pub const DESKTOP: &str = "dassie::desktop";
}

/// A line's level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Debug and trace events.
    Debug,
    /// What the node does.
    Info,
    /// Problems.
    Warn,
    /// Failures.
    Error,
}

impl LogLevel {
    fn of(level: Level) -> Self {
        match level {
            Level::TRACE | Level::DEBUG => Self::Debug,
            Level::INFO => Self::Info,
            Level::WARN => Self::Warn,
            Level::ERROR => Self::Error,
        }
    }
}

/// `DASSIE_LOG`: the most verbose level shown, per target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFilter {
    /// The level of a target no directive names.
    default: LevelFilter,
    /// Each target a directive names, and its level.
    targets: Vec<(String, LevelFilter)>,
}

impl Default for LogFilter {
    /// `info` for every target.
    fn default() -> Self {
        Self {
            default: LevelFilter::INFO,
            targets: Vec::new(),
        }
    }
}

impl LogFilter {
    /// Reads the text of `DASSIE_LOG`; empty is the default, `info`.
    ///
    /// # Errors
    ///
    /// What is wrong with a directive, for people.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut filter = Self::default();
        let mut bare = false;
        for directive in text.split(',').map(str::trim) {
            if directive.is_empty() {
                continue;
            }
            if filter.targets.len() + usize::from(bare) >= MAX_DIRECTIVES {
                return Err(format!("holds more than {MAX_DIRECTIVES} directives"));
            }
            match directive.split_once('=') {
                None => {
                    if bare {
                        return Err(format!(
                            "names a level for every target twice: {directive:?}"
                        ));
                    }
                    filter.default = level(directive)?;
                    bare = true;
                }
                Some((target, text)) => {
                    let target = target.trim();
                    let valid = !target.is_empty()
                        && target.len() <= MAX_TARGET_BYTES
                        && !target.starts_with(':')
                        && !target.ends_with(':')
                        && target
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || b"_-:".contains(&byte));
                    if !valid {
                        return Err(format!("names no target in {directive:?}"));
                    }
                    if filter.targets.iter().any(|(other, _)| other == target) {
                        return Err(format!("names {target:?} twice"));
                    }
                    filter
                        .targets
                        .push((target.to_owned(), level(text.trim())?));
                }
            }
        }
        Ok(filter)
    }

    /// Whether an event of `level` from `target` is shown.
    #[must_use]
    pub fn enabled(&self, level: Level, target: &str) -> bool {
        level <= self.level_of(target)
    }

    /// The level of the most specific directive that covers `target`.
    fn level_of(&self, target: &str) -> LevelFilter {
        self.targets
            .iter()
            .filter(|(directive, _)| covers(directive, target))
            .max_by_key(|(directive, _)| directive.len())
            .map_or(self.default, |(_, level)| *level)
    }

    /// The ring's filter: what this one shows, and info and above whatever
    /// it says.
    #[must_use]
    pub fn for_ring(&self) -> Self {
        let at_least_info = |level: LevelFilter| level.max(LevelFilter::INFO);
        Self {
            default: at_least_info(self.default),
            targets: self
                .targets
                .iter()
                .map(|(target, level)| (target.clone(), at_least_info(*level)))
                .collect(),
        }
    }
}

/// A level by its name, as `EnvFilter` names them.
fn level(text: &str) -> Result<LevelFilter, String> {
    match text {
        "trace" => Ok(LevelFilter::TRACE),
        "debug" => Ok(LevelFilter::DEBUG),
        "info" => Ok(LevelFilter::INFO),
        "warn" => Ok(LevelFilter::WARN),
        "error" => Ok(LevelFilter::ERROR),
        "off" => Ok(LevelFilter::OFF),
        _ => Err(format!(
            "names the level {text:?}, not trace, debug, info, warn, error or off"
        )),
    }
}

/// Whether the directive's `prefix` covers `target`: the target itself, or
/// one beneath it, as a module path is (`a::b` covers `a::b::c`).
fn covers(prefix: &str, target: &str) -> bool {
    target
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
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
///
/// Tracing caches, for each line in the code, whether anyone wants it.
/// While at most one subscriber is registered, it asks only the default
/// of whichever thread reaches the line first: another test's thread, with
/// none, cached that no one does, and the capture lost the line. Two
/// silent subscribers, kept for the process, make it ask every registered
/// one instead, the capture's among them.
#[cfg(test)]
pub(crate) fn capture(f: impl FnOnce()) -> Vec<LogLine> {
    static SILENT: OnceLock<[tracing::Dispatch; 2]> = OnceLock::new();
    SILENT.get_or_init(|| {
        [
            tracing::Dispatch::new(Silent),
            tracing::Dispatch::new(Silent),
        ]
    });
    let ring = Arc::new(LogRing::default());
    let layer = RingLayer {
        ring: Arc::clone(&ring),
    };
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), f);
    ring.after(0, MAX_LINES).0
}

/// A subscriber that takes nothing, but leaves each line for the others to
/// decide: see [`capture`].
#[cfg(test)]
struct Silent;

#[cfg(test)]
impl Subscriber for Silent {
    fn register_callsite(&self, _: &'static Metadata<'static>) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, _: &Event<'_>) {}

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
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
            target: "dassie::test".into(),
            message: message.into(),
            fields: vec![],
        }
    }

    fn filter(text: &str) -> LogFilter {
        LogFilter::parse(text).unwrap()
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

    // The ring keeps info whatever the level shown, and below it what
    // `DASSIE_LOG` shows.
    #[test]
    fn the_rings_filter() {
        let ring = filter("off").for_ring();
        assert!(ring.enabled(Level::INFO, "hyper"));
        assert!(!ring.enabled(Level::DEBUG, "hyper"));
        let ring = filter("warn,dassie=debug").for_ring();
        assert!(ring.enabled(Level::DEBUG, "dassie::http"));
        assert!(ring.enabled(Level::INFO, "hyper"));
        assert!(!ring.enabled(Level::DEBUG, "hyper"));
    }

    #[test]
    fn info_unless_it_says_otherwise() {
        for text in ["", " , ", "info"] {
            let filter = filter(text);
            assert_eq!(filter, LogFilter::default());
            assert!(filter.enabled(Level::INFO, "dassie::daemon"));
            assert!(!filter.enabled(Level::DEBUG, "dassie::daemon"));
        }
        // A directive for a target leaves the rest at info.
        let filter = filter("dassie::peer=debug");
        assert!(filter.enabled(Level::DEBUG, "dassie::peer"));
        assert!(filter.enabled(Level::INFO, "hyper"));
        assert!(!filter.enabled(Level::DEBUG, "hyper"));
    }

    #[test]
    fn the_most_specific_directive_decides() {
        let filter = filter("debug, hyper=warn, dassie=info, dassie::peer=trace");
        assert!(filter.enabled(Level::DEBUG, "quinn"));
        assert!(!filter.enabled(Level::INFO, "hyper::proto::h1"));
        assert!(filter.enabled(Level::WARN, "hyper::proto::h1"));
        assert!(!filter.enabled(Level::DEBUG, "dassie::http"));
        assert!(filter.enabled(Level::TRACE, "dassie::peer"));
        assert!(filter.enabled(Level::TRACE, "dassie::peer::links"));
        // A target covers those beneath it, as a module path does, and no
        // other that only starts alike.
        assert!(!filter.enabled(Level::DEBUG, "dassie::peering"));
        assert!(filter.enabled(Level::DEBUG, "dassiex"));
        let off = self::filter("off");
        assert!(!off.enabled(Level::ERROR, "dassie::daemon"));
    }

    #[test]
    fn a_bad_directive_is_an_error() {
        for text in [
            "verbose",
            "none",
            "dassie::peer=loud",
            "=debug",
            "dassie::=debug",
            "info,warn",
            "dassie=debug,dassie=info",
            "a b=debug",
        ] {
            assert!(LogFilter::parse(text).is_err(), "{text:?}");
        }
        let many: Vec<String> = (0..=MAX_DIRECTIVES)
            .map(|n| format!("t{n}=debug"))
            .collect();
        assert!(LogFilter::parse(&many.join(",")).is_err());
        assert!(LogFilter::parse(&many[1..].join(",")).is_ok());
    }

    // A test of what is logged lost lines at random when another test's
    // thread, with no subscriber, reached the same line first: tracing
    // cached that no one wanted it.
    #[test]
    fn a_capture_keeps_lines_another_thread_reached_first() {
        fn log() {
            tracing::info!(target: "dassie::test", "reached");
        }
        let lines = capture(|| {
            std::thread::spawn(log).join().unwrap();
            log();
        });
        assert_eq!(lines.len(), 1);
    }
}

//! Logging: `tracing` events under the original's `das:node:*` targets.
//!
//! Which events reach standard output follows the original (spec 2 §13):
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

use std::io::IsTerminal;

use tracing::{Level, Metadata};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

/// Targets of the node's own events.
pub mod target {
    /// Start, stop and the setup URL.
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
    /// Payments to the node's STREAM receiver.
    pub const PAYMENT: &str = "das:node:payment";
    /// Settlement schemes.
    pub const SETTLEMENT: &str = "das:node:settlement";
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

/// Sends events through `filter` to standard output. Called once, at start.
pub fn init(filter: LogFilter) {
    let layer = fmt::layer()
        .without_time()
        .with_ansi(std::io::stdout().is_terminal())
        .with_filter(filter_fn(move |metadata: &Metadata<'_>| {
            filter.enabled(*metadata.level(), metadata.target())
        }));
    // A second call (from tests) keeps the first subscriber.
    let _ = tracing_subscriber::registry().with(layer).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

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

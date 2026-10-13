//! The owner's browser sessions.
//!
//! The original kept a session as its plaintext token, forever, with a
//! cookie that lasted ten years and nothing to list or end sessions (spec 2
//! §8.7, RT-11). Here:
//!
//! - the browser holds a random token, and the node stores only its SHA-256,
//!   the [`SessionId`] (D10), so a copy of the database logs no one in;
//! - a session ends after [`SessionPolicy::idle_timeout_ms`] unused, or
//!   [`SessionPolicy::max_lifetime_ms`] after login, whichever comes first;
//! - the cookie lasts exactly as long as the session can;
//! - at most [`SessionPolicy::max_sessions`] are kept, so a script that logs
//!   in over and over cannot grow the table without bound (D16).
//!
//! The cookie's name and attributes are kept (spec 8 §2).

use alloc::format;
use alloc::string::String;
use core::fmt;

use rand_core::CryptoRng;
use sha2::{Digest, Sha256};

use crate::{DAY_MS, HOUR_MS, SECOND_MS, hex};

/// The cookie's name. `__Host-` makes browsers insist on `Secure`, `Path=/`
/// and no `Domain`.
pub const SESSION_COOKIE_NAME: &str = "__Host-Dassie-Session";

/// Longest user agent kept with a session, in bytes.
pub const MAX_USER_AGENT_BYTES: usize = 256;

/// What a browser presents: 32 random bytes, as 64 hex digits in the cookie.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken([u8; 32]);

impl SessionToken {
    /// Draws a new token.
    pub fn generate(rng: &mut (impl CryptoRng + ?Sized)) -> Self {
        let mut bytes = [0; 32];
        rng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Reads a cookie value: 64 hex digits.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        hex::decode_exact(text).map(Self)
    }

    /// The cookie value.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// The ID the node stores and lists.
    #[must_use]
    pub fn id(&self) -> SessionId {
        SessionId(Sha256::digest(self.0).into())
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionToken(<redacted>)")
    }
}

/// The SHA-256 of a session token: what the node stores, and how the owner
/// names a session to revoke it. It cannot be turned back into a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub [u8; 32]);

impl SessionId {
    /// As 64 hex digits.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    /// Reads 64 hex digits.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        hex::decode_exact(text).map(Self)
    }
}

/// When a session was made and last used, in ms since the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionTimes {
    /// At login.
    pub created_at_ms: u64,
    /// At the last request, to within [`SessionPolicy::touch_interval_ms`].
    pub last_used_at_ms: u64,
}

/// What [`SessionPolicy::check`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionCheck {
    /// The session is valid. If `touch` is set, store `now` as its last use.
    Valid {
        /// Whether the stored last use is old enough to update.
        touch: bool,
    },
    /// The session has ended; delete it.
    Expired,
}

/// How long sessions last, and how many there may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPolicy {
    /// A session unused this long ends.
    pub idle_timeout_ms: u64,
    /// No session outlives this.
    pub max_lifetime_ms: u64,
    /// The last use is written at most this often, so that ordinary requests
    /// don't each write to the database.
    pub touch_interval_ms: u64,
    /// The most sessions kept; the least recently used goes first.
    pub max_sessions: u32,
}

impl SessionPolicy {
    /// 30 days idle, one year at most, touched hourly, 64 sessions.
    pub const DEFAULT: Self = Self {
        idle_timeout_ms: 30 * DAY_MS,
        max_lifetime_ms: 365 * DAY_MS,
        touch_interval_ms: HOUR_MS,
        max_sessions: 64,
    };

    /// When a session with these times ends.
    #[must_use]
    pub const fn expires_at_ms(&self, times: SessionTimes) -> u64 {
        let idle = times.last_used_at_ms.saturating_add(self.idle_timeout_ms);
        let absolute = times.created_at_ms.saturating_add(self.max_lifetime_ms);
        if idle < absolute { idle } else { absolute }
    }

    /// Whether a session is still valid at `now_ms`.
    #[must_use]
    pub const fn check(&self, times: SessionTimes, now_ms: u64) -> SessionCheck {
        if now_ms >= self.expires_at_ms(times) {
            return SessionCheck::Expired;
        }
        let since_use = now_ms.saturating_sub(times.last_used_at_ms);
        SessionCheck::Valid {
            touch: since_use >= self.touch_interval_ms,
        }
    }

    /// The cookie's `Max-Age` for a session at `now_ms`, in seconds: until
    /// the absolute end, since the browser cannot see the idle timeout.
    #[must_use]
    pub const fn cookie_max_age_s(&self, times: SessionTimes, now_ms: u64) -> u64 {
        let end = times.created_at_ms.saturating_add(self.max_lifetime_ms);
        end.saturating_sub(now_ms) / SECOND_MS
    }
}

/// The `Set-Cookie` value that logs a browser in.
#[must_use]
pub fn set_cookie(token: &SessionToken, max_age_s: u64) -> String {
    format!(
        "{SESSION_COOKIE_NAME}={}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age_s}",
        token.to_hex()
    )
}

/// The `Set-Cookie` value that logs a browser out.
#[must_use]
pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0")
}

/// The session token in a request's `Cookie` headers: the first cookie with
/// the session cookie's name. A malformed value counts as no session.
pub fn find_session_cookie<'a>(headers: impl IntoIterator<Item = &'a str>) -> Option<SessionToken> {
    let value = headers
        .into_iter()
        .flat_map(|header| header.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE_NAME)?
        .1;
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value);
    SessionToken::parse(value)
}

/// A user agent as kept with a session: control characters dropped, at
/// most [`MAX_USER_AGENT_BYTES`], cut at a character boundary.
#[must_use]
pub fn clean_user_agent(user_agent: &str) -> String {
    let mut clean = String::new();
    for character in user_agent.chars().filter(|c| !c.is_control()) {
        if clean.len().saturating_add(character.len_utf8()) > MAX_USER_AGENT_BYTES {
            break;
        }
        clean.push(character);
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::SeededRng;

    const POLICY: SessionPolicy = SessionPolicy::DEFAULT;

    fn times(created: u64, used: u64) -> SessionTimes {
        SessionTimes {
            created_at_ms: created,
            last_used_at_ms: used,
        }
    }

    // RT-11: the original's sessions never ended.
    #[test]
    fn sessions_end_when_idle() {
        let start = 1_000 * DAY_MS;
        let session = times(start, start);
        assert_eq!(
            POLICY.check(session, start),
            SessionCheck::Valid { touch: false }
        );
        assert_eq!(
            POLICY.check(session, start + HOUR_MS),
            SessionCheck::Valid { touch: true }
        );
        assert_eq!(
            POLICY.check(session, start + 30 * DAY_MS - 1),
            SessionCheck::Valid { touch: true }
        );
        assert_eq!(
            POLICY.check(session, start + 30 * DAY_MS),
            SessionCheck::Expired
        );
    }

    #[test]
    fn sessions_end_after_a_year_however_used() {
        let start = 1_000 * DAY_MS;
        let session = times(start, start + 360 * DAY_MS);
        assert_eq!(POLICY.expires_at_ms(session), start + 365 * DAY_MS);
        assert_eq!(
            POLICY.check(session, start + 365 * DAY_MS),
            SessionCheck::Expired
        );
    }

    // The original's cookie lasted ten years whatever happened on the node.
    #[test]
    fn cookie_lasts_until_the_absolute_end() {
        let start = 1_000 * DAY_MS;
        let session = times(start, start);
        assert_eq!(POLICY.cookie_max_age_s(session, start), 365 * 24 * 3600);
        assert_eq!(
            POLICY.cookie_max_age_s(session, start + 365 * DAY_MS + 1),
            0
        );
        let token = SessionToken::generate(&mut SeededRng::new(3));
        let cookie = set_cookie(&token, 60);
        assert!(cookie.starts_with("__Host-Dassie-Session="));
        assert!(cookie.ends_with("; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=60"));
        assert!(clear_cookie().ends_with("Max-Age=0"));
    }

    #[test]
    fn tokens_hash_to_ids() {
        let token = SessionToken::generate(&mut SeededRng::new(3));
        assert_eq!(SessionToken::parse(&token.to_hex()), Some(token.clone()));
        assert_ne!(token.id().0, token.0);
        assert_eq!(SessionId::parse(&token.id().to_hex()), Some(token.id()));
    }

    #[test]
    fn finds_the_first_session_cookie() {
        let first = SessionToken::generate(&mut SeededRng::new(1)).to_hex();
        let second = SessionToken::generate(&mut SeededRng::new(2)).to_hex();
        let header = format!("a=b; {SESSION_COOKIE_NAME}={first}; {SESSION_COOKIE_NAME}={second}");
        let found = find_session_cookie([header.as_str()]).map(|t| t.to_hex());
        assert_eq!(found, Some(first.clone()));
        let quoted = format!("{SESSION_COOKIE_NAME}=\"{first}\"");
        assert!(find_session_cookie([quoted.as_str()]).is_some());
        assert!(find_session_cookie(["x=y", "__Host-Dassie-Session=zz"]).is_none());
        assert!(find_session_cookie([]).is_none());
    }

    #[test]
    fn user_agents_are_bounded() {
        assert_eq!(clean_user_agent("Mozilla/5.0\n(X11)"), "Mozilla/5.0(X11)");
        let long = "é".repeat(200);
        let clean = clean_user_agent(&long);
        assert_eq!(clean.len(), 256);
        assert!(clean.chars().all(|c| c == 'é'));
    }
}

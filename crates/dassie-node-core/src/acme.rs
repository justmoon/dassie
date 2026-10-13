//! When to renew the certificate, how to back off, and the http-01 tokens.
//!
//! The original checked hourly and, after a failure, simply tried again an
//! hour later, forever, which walks into Let's Encrypt's limits on failed
//! validations (spec 2 §7, RT-13). It kept challenge tokens in a table that
//! threw on a duplicate and never enforced their expiry.
//!
//! Here [`Renewal`] decides when to check and whether to renew, backing off
//! exponentially from [`RenewalPolicy::backoff_min_ms`] to
//! [`RenewalPolicy::backoff_max_ms`] and honouring the server's
//! `Retry-After`. [`ChallengeTokens`] is a bounded table that replaces a
//! duplicate and forgets expired entries.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::certificate::{CertificateInfo, CertificateSource};
use crate::config::Hostname;
use crate::{DAY_MS, HOUR_MS, MINUTE_MS};

/// When to renew, and how to back off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenewalPolicy {
    /// How often to look at the certificate.
    pub check_interval_ms: u64,
    /// Renew when this little validity is left (the original's 30 days).
    pub renew_before_ms: u64,
    /// The wait after a first failure; it doubles with each further one.
    pub backoff_min_ms: u64,
    /// The longest wait after failures.
    pub backoff_max_ms: u64,
    /// The longest `Retry-After` honoured.
    pub retry_after_max_ms: u64,
}

impl RenewalPolicy {
    /// Hourly checks, 30 days' margin, backoff from 1 hour to 1 day, and a
    /// `Retry-After` of up to a week.
    pub const DEFAULT: Self = Self {
        check_interval_ms: HOUR_MS,
        renew_before_ms: 30 * DAY_MS,
        backoff_min_ms: HOUR_MS,
        backoff_max_ms: DAY_MS,
        retry_after_max_ms: 7 * DAY_MS,
    };
}

/// What the node has, as far as renewal is concerned.
#[derive(Clone, Copy, Debug)]
pub struct RenewalInput<'a> {
    /// Whether an ACME account is stored. Without one the owner has not
    /// chosen ACME, and there is nothing to renew with.
    pub has_account: bool,
    /// The configured hostname.
    pub hostname: Option<&'a Hostname>,
    /// The certificate being served.
    pub certificate: Option<&'a CertificateInfo>,
}

/// What [`Renewal::poll`] decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenewalDecision {
    /// Nothing to do before `until_ms`; poll again then.
    Wait {
        /// When to poll again.
        until_ms: u64,
    },
    /// Order a certificate now, then report with
    /// [`Renewal::succeeded`] or [`Renewal::failed`].
    Renew,
}

/// The renewal schedule.
#[derive(Clone, Debug)]
pub struct Renewal {
    policy: RenewalPolicy,
    next_check_ms: u64,
    failures: u32,
}

impl Renewal {
    /// A schedule that checks right away.
    #[must_use]
    pub const fn new(policy: RenewalPolicy, now_ms: u64) -> Self {
        Self {
            policy,
            next_check_ms: now_ms,
            failures: 0,
        }
    }

    /// When the next check is due.
    #[must_use]
    pub const fn next_check_ms(&self) -> u64 {
        self.next_check_ms
    }

    /// Failures since the last success.
    #[must_use]
    pub const fn failures(&self) -> u32 {
        self.failures
    }

    /// Checks at once on the next poll, unless backing off. Called when the
    /// hostname changes or an account is created.
    pub fn wake(&mut self, now_ms: u64) {
        if self.failures == 0 {
            self.next_check_ms = self.next_check_ms.min(now_ms);
        }
    }

    /// Decides what to do at `now_ms`.
    pub fn poll(&mut self, input: RenewalInput<'_>, now_ms: u64) -> RenewalDecision {
        if now_ms < self.next_check_ms {
            return RenewalDecision::Wait {
                until_ms: self.next_check_ms,
            };
        }
        self.next_check_ms = now_ms.saturating_add(self.policy.check_interval_ms);
        if self.needs_renewal(input, now_ms) {
            RenewalDecision::Renew
        } else {
            RenewalDecision::Wait {
                until_ms: self.next_check_ms,
            }
        }
    }

    fn needs_renewal(&self, input: RenewalInput<'_>, now_ms: u64) -> bool {
        let Some(hostname) = input.hostname else {
            return false;
        };
        if !input.has_account {
            return false;
        }
        match input.certificate {
            None => true,
            Some(certificate) => match certificate.source {
                CertificateSource::Manual => false,
                CertificateSource::SelfSigned => true,
                CertificateSource::Acme => {
                    let remaining = certificate.not_after_ms.saturating_sub(now_ms);
                    !certificate.covers(hostname) || remaining <= self.policy.renew_before_ms
                }
            },
        }
    }

    /// A certificate was installed.
    pub fn succeeded(&mut self, now_ms: u64) {
        self.failures = 0;
        self.next_check_ms = now_ms.saturating_add(self.policy.check_interval_ms);
    }

    /// The order failed. Returns when the next attempt is due: the backoff,
    /// or the server's `Retry-After` if that is later.
    pub fn failed(&mut self, retry_after_ms: Option<u64>, now_ms: u64) -> u64 {
        self.failures = self.failures.saturating_add(1);
        let doublings = self.failures.saturating_sub(1).min(32);
        let backoff = self
            .policy
            .backoff_min_ms
            .saturating_mul(1_u64 << doublings)
            .min(self.policy.backoff_max_ms);
        let server = retry_after_ms
            .unwrap_or(0)
            .min(self.policy.retry_after_max_ms);
        self.next_check_ms = now_ms.saturating_add(backoff.max(server));
        self.next_check_ms
    }
}

/// Most challenge tokens held at once. An order for one name has one.
pub const MAX_CHALLENGES: usize = 16;
/// Longest token accepted, in bytes. Let's Encrypt's are 43.
pub const MAX_TOKEN_BYTES: usize = 128;
/// Longest key authorization accepted: the token, a dot and a 43-byte
/// thumbprint, with room to spare.
pub const MAX_KEY_AUTHORIZATION_BYTES: usize = 256;
/// How long a token is served when the server gives no expiry.
pub const DEFAULT_CHALLENGE_LIFETIME_MS: u64 = 10 * MINUTE_MS;

/// A token was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChallengeError {
    /// Not base64url, or too long.
    InvalidToken,
    /// Not printable ASCII, or too long.
    InvalidKeyAuthorization,
    /// [`MAX_CHALLENGES`] unexpired tokens are held already.
    Full,
}

#[derive(Clone, Debug)]
struct Challenge {
    token: String,
    key_authorization: String,
    expires_at_ms: u64,
}

/// The http-01 tokens the HTTP listener answers for.
#[derive(Clone, Debug)]
pub struct ChallengeTokens {
    entries: Vec<Challenge>,
}

impl Default for ChallengeTokens {
    fn default() -> Self {
        Self {
            entries: Vec::with_capacity(MAX_CHALLENGES),
        }
    }
}

impl ChallengeTokens {
    /// Serves `key_authorization` for `token` until `expires_at_ms`. A token
    /// already held is replaced.
    pub fn insert(
        &mut self,
        token: &str,
        key_authorization: &str,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> Result<(), ChallengeError> {
        let valid_token = !token.is_empty()
            && token.len() <= MAX_TOKEN_BYTES
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !valid_token {
            return Err(ChallengeError::InvalidToken);
        }
        let valid_key = !key_authorization.is_empty()
            && key_authorization.len() <= MAX_KEY_AUTHORIZATION_BYTES
            && key_authorization.bytes().all(|b| b.is_ascii_graphic());
        if !valid_key {
            return Err(ChallengeError::InvalidKeyAuthorization);
        }
        self.entries
            .retain(|entry| entry.token != token && entry.expires_at_ms > now_ms);
        if self.entries.len() >= MAX_CHALLENGES {
            return Err(ChallengeError::Full);
        }
        self.entries.push(Challenge {
            token: token.to_string(),
            key_authorization: key_authorization.to_string(),
            expires_at_ms,
        });
        Ok(())
    }

    /// The key authorization for `token`, unless it is unknown or expired.
    #[must_use]
    pub fn lookup(&self, token: &str, now_ms: u64) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.token == token && entry.expires_at_ms > now_ms)
            .map(|entry| entry.key_authorization.as_str())
    }

    /// Stops serving `token`.
    pub fn remove(&mut self, token: &str) {
        self.entries.retain(|entry| entry.token != token);
    }

    /// How many tokens are held, expired or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no token is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn hostname() -> Hostname {
        Hostname::parse("node.example").unwrap_or_else(|error| panic!("{error}"))
    }

    fn acme_cert(not_after_ms: u64) -> CertificateInfo {
        CertificateInfo {
            source: CertificateSource::Acme,
            names: vec!["node.example".to_string()],
            not_before_ms: 0,
            not_after_ms,
        }
    }

    fn input<'a>(hostname: &'a Hostname, certificate: &'a CertificateInfo) -> RenewalInput<'a> {
        RenewalInput {
            has_account: true,
            hostname: Some(hostname),
            certificate: Some(certificate),
        }
    }

    #[test]
    fn renews_thirty_days_before_the_end() {
        let name = hostname();
        let cert = acme_cert(100 * DAY_MS);
        let mut renewal = Renewal::new(RenewalPolicy::DEFAULT, 0);
        assert_eq!(
            renewal.poll(input(&name, &cert), 0),
            RenewalDecision::Wait { until_ms: HOUR_MS }
        );
        assert_eq!(
            renewal.poll(input(&name, &cert), HOUR_MS - 1),
            RenewalDecision::Wait { until_ms: HOUR_MS }
        );
        assert_eq!(
            renewal.poll(input(&name, &cert), 70 * DAY_MS),
            RenewalDecision::Renew
        );
    }

    #[test]
    fn nothing_to_renew_without_account_hostname_or_with_a_manual_cert() {
        let name = hostname();
        let cert = acme_cert(0);
        let mut renewal = Renewal::new(RenewalPolicy::DEFAULT, 0);
        let no_account = RenewalInput {
            has_account: false,
            ..input(&name, &cert)
        };
        assert!(matches!(
            renewal.poll(no_account, 0),
            RenewalDecision::Wait { .. }
        ));
        let mut renewal = Renewal::new(RenewalPolicy::DEFAULT, 0);
        let no_name = RenewalInput {
            hostname: None,
            ..input(&name, &cert)
        };
        assert!(matches!(
            renewal.poll(no_name, 0),
            RenewalDecision::Wait { .. }
        ));
        let manual = CertificateInfo {
            source: CertificateSource::Manual,
            ..acme_cert(0)
        };
        let mut renewal = Renewal::new(RenewalPolicy::DEFAULT, 0);
        assert!(matches!(
            renewal.poll(input(&name, &manual), 0),
            RenewalDecision::Wait { .. }
        ));
    }

    #[test]
    fn self_signed_or_wrong_name_means_order_now() {
        let name = hostname();
        let self_signed = CertificateInfo {
            source: CertificateSource::SelfSigned,
            ..acme_cert(u64::MAX)
        };
        let mut renewal = Renewal::new(RenewalPolicy::DEFAULT, 0);
        assert_eq!(
            renewal.poll(input(&name, &self_signed), 0),
            RenewalDecision::Renew
        );
        let other = CertificateInfo {
            names: vec!["other.example".to_string()],
            ..acme_cert(u64::MAX)
        };
        let mut renewal = Renewal::new(RenewalPolicy::DEFAULT, 0);
        assert_eq!(
            renewal.poll(input(&name, &other), 0),
            RenewalDecision::Renew
        );
    }

    // RT-13: the original retried hourly forever after a failure.
    #[test]
    fn failures_back_off_exponentially_and_honour_retry_after() {
        let mut renewal = Renewal::new(RenewalPolicy::DEFAULT, 0);
        assert_eq!(renewal.failed(None, 0), HOUR_MS);
        assert_eq!(renewal.failed(None, 0), 2 * HOUR_MS);
        assert_eq!(renewal.failed(None, 0), 4 * HOUR_MS);
        for _ in 0..40 {
            renewal.failed(None, 0);
        }
        assert_eq!(renewal.failed(None, 0), DAY_MS);
        assert_eq!(renewal.failed(Some(3 * DAY_MS), 0), 3 * DAY_MS);
        assert_eq!(renewal.failed(Some(u64::MAX), 0), 7 * DAY_MS);
        renewal.wake(0);
        assert_eq!(
            renewal.next_check_ms(),
            7 * DAY_MS,
            "waking does not cut a backoff short"
        );
        renewal.succeeded(10);
        assert_eq!(
            (renewal.failures(), renewal.next_check_ms()),
            (0, 10 + HOUR_MS)
        );
        renewal.wake(20);
        assert_eq!(renewal.next_check_ms(), 20);
    }

    // RT-13: the original threw on a duplicate token and ignored expiry.
    #[test]
    fn challenge_tokens() {
        let mut tokens = ChallengeTokens::default();
        assert_eq!(tokens.insert("abc_-1", "abc_-1.thumb", 100, 0), Ok(()));
        assert_eq!(tokens.insert("abc_-1", "abc_-1.other", 100, 0), Ok(()));
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens.lookup("abc_-1", 99), Some("abc_-1.other"));
        assert_eq!(tokens.lookup("abc_-1", 100), None);
        assert_eq!(
            tokens.insert("a/b", "x", 100, 0),
            Err(ChallengeError::InvalidToken)
        );
        assert_eq!(
            tokens.insert("ab", "x y", 100, 0),
            Err(ChallengeError::InvalidKeyAuthorization)
        );
        tokens.remove("abc_-1");
        assert!(tokens.is_empty());
    }

    #[test]
    fn the_table_is_bounded_and_expired_tokens_make_room() {
        let mut tokens = ChallengeTokens::default();
        for index in 0..MAX_CHALLENGES {
            assert_eq!(tokens.insert(&std::format!("t{index}"), "k", 50, 0), Ok(()));
        }
        assert_eq!(tokens.insert("more", "k", 50, 0), Err(ChallengeError::Full));
        assert_eq!(tokens.insert("more", "k", 150, 50), Ok(()));
        assert_eq!(tokens.len(), 1);
    }
}

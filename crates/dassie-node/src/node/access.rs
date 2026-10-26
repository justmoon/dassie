//! Setup, login, logout and sessions.

use std::fmt;
use std::sync::Arc;

use dassie_kit::ClientKey;
use dassie_node_core::session::{
    self, SessionCheck, SessionId, SessionTimes, SessionToken, clean_user_agent,
};
use dassie_node_core::setup::{self, SetupError, SetupRequest};

use super::{Node, Revocation, lock};
use crate::log::target;
use crate::store::{SessionRecord, StoreError};

/// A browser was logged in.
#[derive(Debug)]
pub struct NewSession {
    /// The session's ID.
    pub id: SessionId,
    /// The `Set-Cookie` value that carries it.
    pub cookie: String,
}

/// Setup or login was refused.
#[derive(Debug)]
pub enum AuthError {
    /// Setup was already done.
    AlreadySetUp,
    /// Login before setup.
    NotSetUp,
    /// The setup request was wrong.
    Setup(SetupError),
    /// The login proof was wrong.
    InvalidLogin,
    /// Too many attempts; try again after this many ms.
    RateLimited(u64),
    /// The database failed.
    Store(StoreError),
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadySetUp => f.write_str("Node is already set up"),
            Self::NotSetUp => f.write_str("Node is not set up yet"),
            Self::Setup(error) => error.fmt(f),
            Self::InvalidLogin => f.write_str("Invalid login authorization signature"),
            Self::RateLimited(_) => f.write_str("Too many attempts, try again later"),
            Self::Store(error) => error.fmt(f),
        }
    }
}

impl From<StoreError> for AuthError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// A session as the owner sees it in a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionView {
    /// The stored record.
    pub record: SessionRecord,
    /// When it will end, unless used.
    pub expires_at_ms: u64,
    /// Whether it is the session asking.
    pub current: bool,
}

impl Node {
    /// Claims the node with the key in `request`, and logs the browser in.
    pub fn setup(
        &self,
        request: &SetupRequest<'_>,
        client: ClientKey,
        user_agent: &str,
    ) -> Result<NewSession, AuthError> {
        self.check_rate(client)?;
        if self.is_set_up() {
            return Err(AuthError::AlreadySetUp);
        }
        let key = setup::verify_setup(&self.setup_token, request).map_err(AuthError::Setup)?;
        if !self.store.set_node_key_once(&key, self.now_ms())? {
            return Err(AuthError::AlreadySetUp);
        }
        tracing::info!(target: target::AUTH, node_id = %key.node_id(), "node identity set up");
        self.node_key.send_replace(Some(Arc::new(key)));
        self.new_session(user_agent)
    }

    /// Checks a login proof, and logs the browser in.
    pub fn login(
        &self,
        proof_hex: &str,
        client: ClientKey,
        user_agent: &str,
    ) -> Result<NewSession, AuthError> {
        self.check_rate(client)?;
        let key = self.node_key().ok_or(AuthError::NotSetUp)?;
        if !setup::verify_login(&key, proof_hex) {
            tracing::info!(target: target::AUTH, "login refused");
            return Err(AuthError::InvalidLogin);
        }
        self.new_session(user_agent)
    }

    fn check_rate(&self, client: ClientKey) -> Result<(), AuthError> {
        let now_ms = self.now_ms();
        lock(&self.login_limiter)
            .check(client, now_ms)
            .map_err(|limited| AuthError::RateLimited(limited.retry_after_ms))
    }

    fn new_session(&self, user_agent: &str) -> Result<NewSession, AuthError> {
        let now_ms = self.now_ms();
        let token = SessionToken::generate(&mut **lock(&self.rng));
        let times = SessionTimes {
            created_at_ms: now_ms,
            last_used_at_ms: now_ms,
        };
        let record = SessionRecord {
            id: token.id(),
            times,
            user_agent: clean_user_agent(user_agent),
        };
        // Sessions evicted to make room close their sockets.
        if self
            .store
            .insert_session(&record, &self.policies.sessions)?
            > 0
        {
            self.revoke(Revocation::Recheck);
        }
        let max_age_s = self.policies.sessions.cookie_max_age_s(times, now_ms);
        Ok(NewSession {
            id: record.id,
            cookie: session::set_cookie(&token, max_age_s),
        })
    }

    /// Ends the session of `token`, if it has one, and closes its sockets.
    pub fn logout(&self, token: Option<&SessionToken>) -> Result<(), StoreError> {
        if let Some(token) = token {
            let id = token.id();
            self.store.delete_session(&id)?;
            self.revoke(Revocation::Session(id));
        }
        Ok(())
    }

    /// The session `token` belongs to, if it is valid now. Records the use,
    /// at most once per touch interval, and deletes an expired session.
    pub fn authenticate(&self, token: &SessionToken) -> Result<Option<SessionId>, StoreError> {
        let id = token.id();
        self.check_session(&id)
    }

    /// Like [`authenticate`](Self::authenticate), by ID.
    pub fn check_session(&self, id: &SessionId) -> Result<Option<SessionId>, StoreError> {
        let Some(record) = self.store.session(id)? else {
            // Ended some other way (pruned, evicted): its sockets close too.
            self.revoke(Revocation::Session(*id));
            return Ok(None);
        };
        let now_ms = self.now_ms();
        match self.policies.sessions.check(record.times, now_ms) {
            SessionCheck::Valid { touch } => {
                if touch {
                    self.store.touch_session(id, now_ms)?;
                }
                Ok(Some(*id))
            }
            SessionCheck::Expired => {
                self.store.delete_session(id)?;
                self.revoke(Revocation::Session(*id));
                Ok(None)
            }
        }
    }

    /// The sessions, most recently used first.
    pub fn sessions(&self, current: Option<&SessionId>) -> Result<Vec<SessionView>, StoreError> {
        Ok(self
            .store
            .sessions()?
            .into_iter()
            .map(|record| SessionView {
                expires_at_ms: self.policies.sessions.expires_at_ms(record.times),
                current: current == Some(&record.id),
                record,
            })
            .collect())
    }

    /// Ends one session; says whether it existed.
    pub fn revoke_session(&self, id: &SessionId) -> Result<bool, StoreError> {
        let existed = self.store.delete_session(id)?;
        self.revoke(Revocation::Session(*id));
        Ok(existed)
    }

    /// Ends every session but `keep`; says how many.
    pub fn revoke_sessions(&self, keep: Option<&SessionId>) -> Result<u32, StoreError> {
        let count = self.store.delete_sessions_except(keep)?;
        self.revoke(Revocation::AllExcept(keep.copied()));
        Ok(count)
    }

    /// Deletes expired sessions, and closes their sockets.
    pub fn prune_sessions(&self) -> Result<u32, StoreError> {
        let pruned = self
            .store
            .prune_sessions(&self.policies.sessions, self.now_ms())?;
        // Their sockets close too.
        if pruned > 0 {
            self.revoke(Revocation::Recheck);
        }
        Ok(pruned)
    }

    pub(super) fn revoke(&self, revocation: Revocation) {
        // No receivers just means no socket is open.
        let _ = self.revocations.send(revocation);
    }
}

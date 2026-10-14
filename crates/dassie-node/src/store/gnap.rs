//! GNAP's tables: grants and their access tokens, which the authorization
//! server writes directly; they move no money, so they need not wait for
//! the engine. Tokens and continuation tokens are stored as SHA-256
//! hashes only (D10).

use rusqlite::{OptionalExtension, Row, params};

use super::{PaymentId, Store, StoreError, from_sql_time, to_sql_time};

/// Most grants kept at once: past it, grant requests are refused until
/// old ones are pruned.
pub const MAX_GRANTS: u32 = 16_384;

/// How long a grant that needed no owner is kept once its tokens are gone.
const SPENT_GRANT_MS: u64 = 60 * 60 * 1_000;

/// Where a grant is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantState {
    /// Waiting for the owner.
    Pending,
    /// Approved: its tokens work.
    Approved,
    /// The owner refused it.
    Denied,
    /// Revoked, by its client or the owner.
    Revoked,
}

impl GrantState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Revoked => "revoked",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "pending" => Ok(Self::Pending),
            "approved" => Ok(Self::Approved),
            "denied" => Ok(Self::Denied),
            "revoked" => Ok(Self::Revoked),
            _ => Err(StoreError::Corrupt("grant state")),
        }
    }
}

/// A grant as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantRow {
    /// Its ID.
    pub id: PaymentId,
    /// Where it is.
    pub state: GrantState,
    /// The client, as resources record it.
    pub client: String,
    /// The client's key, as JWK JSON.
    pub client_key: String,
    /// What it grants, as the JSON of the access items.
    pub access: String,
    /// Whether the owner had to approve it.
    pub needs_owner: bool,
    /// The hash of its continuation token.
    pub continue_token_hash: [u8; 32],
    /// The interaction, as JSON, for grants the owner decides.
    pub interaction: Option<String>,
    /// When it was requested.
    pub created_at_ms: u64,
    /// When it was decided.
    pub decided_at_ms: Option<u64>,
    /// When it lapses if still pending.
    pub expires_at_ms: u64,
}

/// An access token as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRow {
    /// The token's hash.
    pub token_hash: [u8; 32],
    /// Its grant.
    pub grant: PaymentId,
    /// The ID in its management URL.
    pub manage_id: PaymentId,
    /// When it was issued.
    pub created_at_ms: u64,
    /// When it stops working.
    pub expires_at_ms: u64,
}

const GRANT_COLUMNS: &str = "id, state, client, client_key, access, needs_owner, \
     continue_token_hash, interaction, created_at_ms, decided_at_ms, expires_at_ms";

const NEXT_REVISION: &str = "(SELECT coalesce(max(revision), 0) + 1 FROM gnap_grants)";

impl Store {
    /// Stores a new grant, and its first token if it has one, in one
    /// transaction.
    pub fn insert_grant(
        &self,
        grant: &GrantRow,
        token: Option<&TokenRow>,
    ) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            &format!(
                "INSERT INTO gnap_grants ({GRANT_COLUMNS}, revision) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, {NEXT_REVISION})"
            ),
            params![
                grant.id.0.as_slice(),
                grant.state.as_str(),
                grant.client,
                grant.client_key,
                grant.access,
                grant.needs_owner,
                grant.continue_token_hash.as_slice(),
                grant.interaction,
                to_sql_time(grant.created_at_ms),
                grant.decided_at_ms.map(to_sql_time),
                to_sql_time(grant.expires_at_ms),
            ],
        )?;
        if let Some(token) = token {
            insert_token(&transaction, token)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// The token with this hash, unexpired at `now_ms`, with its grant.
    pub fn access_token(
        &self,
        hash: &[u8; 32],
        now_ms: u64,
    ) -> Result<Option<(TokenRow, GrantRow)>, StoreError> {
        let connection = self.lock();
        let columns = GRANT_COLUMNS
            .split(", ")
            .map(|column| format!("g.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .query_row(
                &format!(
                    "SELECT t.token_hash, t.grant_id, t.manage_id, t.created_at_ms, \
                     t.expires_at_ms, {columns} FROM gnap_tokens t \
                     JOIN gnap_grants g ON g.id = t.grant_id \
                     WHERE t.token_hash = ?1 AND t.expires_at_ms > ?2"
                ),
                params![hash.as_slice(), to_sql_time(now_ms)],
                |row| Ok((read_token(row), read_grant_at(row, 5)?)),
            )
            .optional()?
            .map(|(token, grant)| Ok((token?, grant?)))
            .transpose()
    }

    /// The grant `id`, if `continue_hash` is its continuation token's hash.
    pub fn continued_grant(
        &self,
        id: PaymentId,
        continue_hash: &[u8; 32],
    ) -> Result<Option<GrantRow>, StoreError> {
        let connection = self.lock();
        connection
            .query_row(
                &format!(
                    "SELECT {GRANT_COLUMNS} FROM gnap_grants \
                     WHERE id = ?1 AND continue_token_hash = ?2"
                ),
                params![id.0.as_slice(), continue_hash.as_slice()],
                read_grant,
            )
            .optional()?
            .transpose()
    }

    /// The token at management ID `manage_id`, if `hash` is its hash, with
    /// its grant; expired or not, since an expired token may be rotated.
    pub fn managed_token(
        &self,
        manage_id: PaymentId,
        hash: &[u8; 32],
    ) -> Result<Option<(TokenRow, GrantRow)>, StoreError> {
        let connection = self.lock();
        let columns = GRANT_COLUMNS
            .split(", ")
            .map(|column| format!("g.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .query_row(
                &format!(
                    "SELECT t.token_hash, t.grant_id, t.manage_id, t.created_at_ms, \
                     t.expires_at_ms, {columns} FROM gnap_tokens t \
                     JOIN gnap_grants g ON g.id = t.grant_id \
                     WHERE t.manage_id = ?1 AND t.token_hash = ?2"
                ),
                params![manage_id.0.as_slice(), hash.as_slice()],
                |row| Ok((read_token(row), read_grant_at(row, 5)?)),
            )
            .optional()?
            .map(|(token, grant)| Ok((token?, grant?)))
            .transpose()
    }

    /// Replaces a token with a new one of the same grant, in one
    /// transaction; false if the old one is gone.
    pub fn rotate_token(&self, old_hash: &[u8; 32], new: &TokenRow) -> Result<bool, StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let removed = transaction.execute(
            "DELETE FROM gnap_tokens WHERE token_hash = ?1",
            [old_hash.as_slice()],
        )?;
        if removed != 1 {
            return Ok(false);
        }
        insert_token(&transaction, new)?;
        transaction.commit()?;
        Ok(true)
    }

    /// Revokes a token; false if it was gone.
    pub fn revoke_token(&self, hash: &[u8; 32]) -> Result<bool, StoreError> {
        let removed = self.lock().execute(
            "DELETE FROM gnap_tokens WHERE token_hash = ?1",
            [hash.as_slice()],
        )?;
        Ok(removed == 1)
    }

    /// Revokes a grant and every token it issued.
    pub fn revoke_grant(&self, id: PaymentId, now_ms: u64) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM gnap_tokens WHERE grant_id = ?1",
            [id.0.as_slice()],
        )?;
        transaction.execute(
            &format!(
                "UPDATE gnap_grants SET state = 'revoked', \
                 decided_at_ms = coalesce(decided_at_ms, ?2), revision = {NEXT_REVISION} \
                 WHERE id = ?1"
            ),
            params![id.0.as_slice(), to_sql_time(now_ms)],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// How many grants are kept.
    pub fn grant_count(&self) -> Result<u32, StoreError> {
        let count: i64 = self
            .lock()
            .query_row("SELECT count(*) FROM gnap_grants", [], |row| row.get(0))?;
        u32::try_from(count).map_err(|_| StoreError::Corrupt("grant count"))
    }

    /// Forgets what no request can use any more: grants the owner did not
    /// decide in time, denied and revoked ones, expired tokens, and grants
    /// that needed no owner once their tokens are gone and an hour has
    /// passed since they were made. Returns how many grants went.
    pub fn prune_grants(&self, now_ms: u64) -> Result<u32, StoreError> {
        let now = to_sql_time(now_ms);
        let day_ago = to_sql_time(now_ms.saturating_sub(SPENT_GRANT_MS));
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM gnap_tokens WHERE expires_at_ms <= ?1", [now])?;
        let removed = transaction.execute(
            "DELETE FROM gnap_grants WHERE (state = 'pending' AND expires_at_ms <= ?1) \
             OR state IN ('denied', 'revoked') \
             OR (state = 'approved' AND needs_owner = 0 AND created_at_ms <= ?2 \
                 AND NOT EXISTS (SELECT 1 FROM gnap_tokens t WHERE t.grant_id = gnap_grants.id))",
            params![now, day_ago],
        )?;
        transaction.commit()?;
        u32::try_from(removed).map_err(|_| StoreError::Corrupt("grant count"))
    }
}

fn insert_token(
    transaction: &rusqlite::Transaction<'_>,
    token: &TokenRow,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO gnap_tokens (token_hash, grant_id, manage_id, created_at_ms, expires_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            token.token_hash.as_slice(),
            token.grant.0.as_slice(),
            token.manage_id.0.as_slice(),
            to_sql_time(token.created_at_ms),
            to_sql_time(token.expires_at_ms),
        ],
    )?;
    Ok(())
}

fn id(bytes: Vec<u8>, what: &'static str) -> Result<PaymentId, StoreError> {
    bytes
        .try_into()
        .map(PaymentId)
        .map_err(|_| StoreError::Corrupt(what))
}

fn read_token(row: &Row<'_>) -> Result<TokenRow, StoreError> {
    let read = || -> rusqlite::Result<_> {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
        ))
    };
    let (hash, grant, manage, created, expires) = read()?;
    Ok(TokenRow {
        token_hash: hash
            .try_into()
            .map_err(|_| StoreError::Corrupt("token hash"))?,
        grant: id(grant, "grant ID")?,
        manage_id: id(manage, "manage ID")?,
        created_at_ms: from_sql_time(created),
        expires_at_ms: from_sql_time(expires),
    })
}

type GrantResult = Result<GrantRow, StoreError>;

fn read_grant(row: &Row<'_>) -> rusqlite::Result<GrantResult> {
    read_grant_at(row, 0)
}

fn read_grant_at(row: &Row<'_>, at: usize) -> rusqlite::Result<GrantResult> {
    let grant_id: Vec<u8> = row.get(at)?;
    let state: String = row.get(at + 1)?;
    let client: String = row.get(at + 2)?;
    let client_key: String = row.get(at + 3)?;
    let access: String = row.get(at + 4)?;
    let needs_owner: bool = row.get(at + 5)?;
    let continue_hash: Vec<u8> = row.get(at + 6)?;
    let interaction: Option<String> = row.get(at + 7)?;
    let created: i64 = row.get(at + 8)?;
    let decided: Option<i64> = row.get(at + 9)?;
    let expires: i64 = row.get(at + 10)?;
    Ok((|| {
        Ok(GrantRow {
            id: id(grant_id, "grant ID")?,
            state: GrantState::parse(&state)?,
            client,
            client_key,
            access,
            needs_owner,
            continue_token_hash: continue_hash
                .try_into()
                .map_err(|_| StoreError::Corrupt("continuation hash"))?,
            interaction,
            created_at_ms: from_sql_time(created),
            decided_at_ms: decided.map(from_sql_time),
            expires_at_ms: from_sql_time(expires),
        })
    })())
}

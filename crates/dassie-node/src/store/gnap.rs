//! GNAP's tables: grants and their access tokens, which the authorization
//! server writes directly; they move no money, so they need not wait for
//! the engine. Tokens and continuation tokens are stored as SHA-256
//! hashes only (spec/decisions/secrets-at-rest.md).

use dassie_kit::ClientKey;
use rusqlite::{OptionalExtension, Row, params};

use super::{Cached, PaymentId, Store, StoreError, from_sql_time, to_sql_time};

/// Most grants kept at once: past it, grant requests are refused until
/// old ones are pruned.
pub const MAX_GRANTS: u32 = 16_384;

/// How long a grant that needed no owner is kept once its tokens are gone.
const SPENT_GRANT_MS: u64 = 60 * 60 * 1_000;

/// How long after it expires a token of a grant the owner approved may
/// still be rotated.
pub const ROTATABLE_MS: u64 = 90 * 24 * 60 * 60 * 1_000;

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
    /// As stored.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
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

/// What a new grant would join: the grants kept and those waiting for the
/// owner, in all, of its client and from its network, and whether the
/// owner approved its client before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GrantCounts {
    /// Grants kept.
    pub kept: u32,
    /// Grants waiting for the owner.
    pub pending: u32,
    /// Grants of the client.
    pub client_kept: u32,
    /// Grants of the client waiting for the owner.
    pub client_pending: u32,
    /// Grants strangers asked for from the network.
    pub network_kept: u32,
    /// Grants strangers asked for from the network waiting for the owner.
    pub network_pending: u32,
    /// Whether the client has a grant the owner approved.
    pub known: bool,
}

const GRANT_COLUMNS: &str = "id, state, client, client_key, access, needs_owner, \
     continue_token_hash, interaction, created_at_ms, decided_at_ms, expires_at_ms";

const NEXT_REVISION: &str = "(SELECT coalesce(max(revision), 0) + 1 FROM gnap_grants)";

fn count(value: i64) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| StoreError::Corrupt("grant count"))
}

impl Store {
    /// Whether the owner approved a grant of `client` that is still kept:
    /// a client on the owner's side (spec/decisions/owners-share.md).
    pub fn client_approved(&self, client: &str) -> Result<bool, StoreError> {
        let approved: bool = self.lock().query_row_cached(
            "SELECT EXISTS (SELECT 1 FROM gnap_grants \
             WHERE client = ?1 AND state = 'approved' AND needs_owner = 1)",
            [client],
            |row| row.get(0),
        )?;
        Ok(approved)
    }

    /// What a new grant of `client`, asked for from `network`, would join.
    pub fn grant_counts(
        &self,
        client: &str,
        network: Option<ClientKey>,
    ) -> Result<GrantCounts, StoreError> {
        grant_counts(&self.lock(), client, network)
    }

    /// Stores a new grant, asked for from `network`, and its first token if
    /// it has one, in one transaction, if `fits` says it fits beside the
    /// grants kept: `fits`'s refusal otherwise.
    pub fn insert_grant<E>(
        &self,
        grant: &GrantRow,
        token: Option<&TokenRow>,
        network: Option<ClientKey>,
        fits: impl FnOnce(&GrantCounts) -> Result<(), E>,
    ) -> Result<Result<(), E>, StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        if let Err(refusal) = fits(&grant_counts(&transaction, &grant.client, network)?) {
            return Ok(Err(refusal));
        }
        transaction.execute_cached(
            &format!(
                "INSERT INTO gnap_grants ({GRANT_COLUMNS}, network, revision) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, {NEXT_REVISION})"
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
                network.map(|network| network.to_bytes().to_vec()),
            ],
        )?;
        if let Some(token) = token {
            insert_token(&transaction, token)?;
        }
        transaction.commit()?;
        Ok(Ok(()))
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
            .query_row_cached(
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
            .query_row_cached(
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
            .query_row_cached(
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
        let removed = transaction.execute_cached(
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
        let removed = self.lock().execute_cached(
            "DELETE FROM gnap_tokens WHERE token_hash = ?1",
            [hash.as_slice()],
        )?;
        Ok(removed == 1)
    }

    /// Revokes a grant and every token it issued.
    pub fn revoke_grant(&self, id: PaymentId, now_ms: u64) -> Result<(), StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute_cached(
            "DELETE FROM gnap_tokens WHERE grant_id = ?1",
            [id.0.as_slice()],
        )?;
        transaction.execute_cached(
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

    /// The grant `id`.
    pub fn grant(&self, id: PaymentId) -> Result<Option<GrantRow>, StoreError> {
        let connection = self.lock();
        connection
            .query_row_cached(
                &format!("SELECT {GRANT_COLUMNS} FROM gnap_grants WHERE id = ?1"),
                [id.0.as_slice()],
                read_grant,
            )
            .optional()?
            .transpose()
    }

    /// The owner's decision on a pending grant, with its interaction as it
    /// now is; false if it was no longer pending.
    pub fn decide_grant(
        &self,
        id: PaymentId,
        state: GrantState,
        interaction: &str,
        at_ms: u64,
    ) -> Result<bool, StoreError> {
        let changed = self.lock().execute_cached(
            &format!(
                "UPDATE gnap_grants SET state = ?2, interaction = ?3, decided_at_ms = ?4, \
                 revision = {NEXT_REVISION} WHERE id = ?1 AND state = 'pending'"
            ),
            params![
                id.0.as_slice(),
                state.as_str(),
                interaction,
                to_sql_time(at_ms)
            ],
        )?;
        Ok(changed == 1)
    }

    /// Writes a grant's interaction, and the token its continuation issued
    /// if any, in one transaction, if the interaction and state are still
    /// what the caller read: false otherwise, when another request or the
    /// owner's decision came first.
    pub fn update_interaction(
        &self,
        grant: &GrantRow,
        interaction: &str,
        token: Option<&TokenRow>,
    ) -> Result<bool, StoreError> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let changed = transaction.execute_cached(
            &format!(
                "UPDATE gnap_grants SET interaction = ?2, revision = {NEXT_REVISION} \
                 WHERE id = ?1 AND interaction IS ?3 AND state = ?4"
            ),
            params![
                grant.id.0.as_slice(),
                interaction,
                grant.interaction,
                grant.state.as_str()
            ],
        )?;
        if changed != 1 {
            return Ok(false);
        }
        if let Some(token) = token {
            insert_token(&transaction, token)?;
        }
        transaction.commit()?;
        Ok(true)
    }

    /// The grants the owner approved and has not revoked, newest first, at
    /// most `limit`.
    pub fn consented_grants(&self, limit: u32) -> Result<Vec<GrantRow>, StoreError> {
        let connection = self.lock();
        let mut statement = connection.prepare_cached(&format!(
            "SELECT {GRANT_COLUMNS} FROM gnap_grants \
             WHERE state = 'approved' AND needs_owner = 1 \
             ORDER BY decided_at_ms DESC, id LIMIT ?1"
        ))?;
        statement
            .query_map([limit], read_grant)?
            .map(|row| row?)
            .collect()
    }

    /// Forgets what no request can use any more: grants the owner did not
    /// decide in time, denied and revoked ones, and expired tokens, except
    /// those of grants the owner approved, which may be rotated for
    /// [`ROTATABLE_MS`] after they expire, as such grants are meant to
    /// last. A grant goes once its tokens are gone and an hour has passed
    /// since it was made or decided. Returns how many grants went.
    pub fn prune_grants(&self, now_ms: u64) -> Result<u32, StoreError> {
        let now = to_sql_time(now_ms);
        let hour_ago = to_sql_time(now_ms.saturating_sub(SPENT_GRANT_MS));
        let rotatable = to_sql_time(now_ms.saturating_sub(ROTATABLE_MS));
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        transaction.execute_cached(
            "DELETE FROM gnap_tokens WHERE expires_at_ms <= ?1 AND (expires_at_ms <= ?2 \
             OR NOT EXISTS (SELECT 1 FROM gnap_grants g \
               WHERE g.id = gnap_tokens.grant_id AND g.needs_owner = 1))",
            params![now, rotatable],
        )?;
        // A denied grant stays until its time, so its client learns why.
        let removed = transaction.execute_cached(
            "DELETE FROM gnap_grants WHERE (state IN ('pending', 'denied') AND expires_at_ms <= ?1) \
             OR state = 'revoked' \
             OR (state = 'approved' AND coalesce(decided_at_ms, created_at_ms) <= ?2 \
                 AND NOT EXISTS (SELECT 1 FROM gnap_tokens t WHERE t.grant_id = gnap_grants.id))",
            params![now, hour_ago],
        )?;
        // What grants that are gone spent, they no longer need.
        transaction.execute_cached(
            "DELETE FROM grant_spend WHERE grant_id NOT IN (SELECT id FROM gnap_grants)",
            [],
        )?;
        transaction.commit()?;
        u32::try_from(removed).map_err(|_| StoreError::Corrupt("grant count"))
    }
}

/// What a new grant of `client` from `network` would join, counting a network's grants of strangers only: two passes over
/// the table, which [`MAX_GRANTS`] bounds.
fn grant_counts(
    connection: &rusqlite::Connection,
    client: &str,
    network: Option<ClientKey>,
) -> Result<GrantCounts, StoreError> {
    let network = network.map(|network| network.to_bytes().to_vec());
    let counts = connection.query_row_cached(
        "SELECT count(*), \
           coalesce(sum(state = 'pending'), 0), \
           coalesce(sum(client = ?1), 0), \
           coalesce(sum(client = ?1 AND state = 'pending'), 0), \
           coalesce(sum(stranger AND network = ?2), 0), \
           coalesce(sum(stranger AND network = ?2 AND state = 'pending'), 0), \
           coalesce(max(client = ?1 AND state = 'approved' AND needs_owner = 1), 0) \
         FROM (SELECT *, network IS NOT NULL AND client NOT IN \
           (SELECT client FROM gnap_grants WHERE state = 'approved' AND needs_owner = 1) \
           AS stranger FROM gnap_grants)",
        params![client, network],
        |row| {
            Ok([
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ])
        },
    )?;
    let [
        kept,
        pending,
        client_kept,
        client_pending,
        network_kept,
        network_pending,
        known,
    ] = counts;
    Ok(GrantCounts {
        kept: count(kept)?,
        pending: count(pending)?,
        client_kept: count(client_kept)?,
        client_pending: count(client_pending)?,
        network_kept: count(network_kept)?,
        network_pending: count(network_pending)?,
        known: known == 1,
    })
}

fn insert_token(
    transaction: &rusqlite::Transaction<'_>,
    token: &TokenRow,
) -> Result<(), StoreError> {
    transaction.execute_cached(
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

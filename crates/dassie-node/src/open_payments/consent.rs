//! Grants the owner decides (D9): outgoing payments, proof of a wallet
//! address, and reading or listing what others created.
//!
//! Such a grant waits, pending, for the owner. The client sends the owner
//! to `GET /gnap/interact/{id}/{key}`, which leads to the wallet's consent
//! page; the key is random per grant, so only the client and the owner
//! know the way there. Once the owner has decided in the wallet, the page
//! sends them back to the client's finish URI with the interaction hash
//! and a reference, and the client continues the grant with that
//! reference to get its token. A grant waits at most
//! [`PENDING_GRANT_LIFETIME_MS`]; at most [`MAX_PENDING_GRANTS`] wait at
//! once.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dassie_payments::gnap::{
    self, Access, AccessToken, Continue, ContinueRequest, ContinueToken, ErrorCode, Finish,
    GrantRequest, GrantResponse, InteractResponse, Subject, SubjectId,
};
use dassie_payments::jwk::Jwk;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;

use super::auth::{
    ACCESS_TOKEN_LIFETIME_MS, OpError, PENDING_GRANT_LIFETIME_MS, new_token, random_bytes,
};
use super::{GRANT_PATH, Urls, WalletAddresses};
use crate::log::target;
use crate::node::Node;
use crate::store::{GrantRow, GrantState, MAX_GRANTS, PaymentId, TokenRow};

/// Most grants waiting for the owner at once.
pub const MAX_PENDING_GRANTS: u32 = 256;
/// Seconds a client waits between continuations of a pending grant.
pub const CONTINUE_WAIT_S: u64 = 5;
/// The wallet's consent page.
pub const CONSENT_PATH: &str = "/consent/";
/// The longest public name of a client that is kept.
const MAX_PUBLIC_NAME_CHARS: usize = 128;

/// What a pending grant keeps of its interaction, as JSON in the grant's
/// row. The key and the reference are secrets of the owner's and the
/// client's; the column is never shown (D10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interaction {
    /// The key in the interaction URL.
    pub key: String,
    /// Our nonce for the interaction hash.
    pub server_nonce: String,
    /// Where to send the owner back, and the client's nonce.
    pub finish: Option<Finish>,
    /// The reference the owner's return carries, once decided.
    pub interact_ref: Option<String>,
    /// The identity asked about, for a subject request.
    pub subject: Option<Subject>,
    /// The client's wallet address's public name, if it has one.
    pub public_name: Option<String>,
    /// When the client last continued, or asked.
    pub continued_at_ms: u64,
    /// Whether the continuation issued the grant's token.
    pub finalized: bool,
}

impl Interaction {
    fn of(grant: &GrantRow) -> Result<Self, OpError> {
        let text = grant
            .interaction
            .as_deref()
            .ok_or_else(|| OpError::internal("a grant the owner decides has no interaction"))?;
        serde_json::from_str(text).map_err(OpError::internal)
    }

    fn json(&self) -> Result<String, OpError> {
        serde_json::to_string(self).map_err(OpError::internal)
    }
}

/// A grant request as the owner sees it on the consent page.
#[derive(Clone, Debug)]
pub struct GrantView {
    /// The grant.
    pub grant: GrantRow,
    /// Its access.
    pub access: Vec<Access>,
    /// The identity asked about.
    pub subject: Option<String>,
    /// The client's public name, if its wallet address has one.
    pub public_name: Option<String>,
}

fn random_text<const N: usize>(node: &Node) -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<N>(node))
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Stores a grant that waits for the owner, and tells the client where to
/// send them.
///
/// # Errors
///
/// 503 while too many grants wait or are kept.
pub async fn request(
    node: &Node,
    urls: &Urls,
    client: String,
    jwk: &Jwk,
    access: Vec<Access>,
    request: &GrantRequest,
) -> Result<GrantResponse, OpError> {
    let now = node.now_ms();
    let store = node.store();
    store.prune_grants(now)?;
    if store.pending_grant_count()? >= MAX_PENDING_GRANTS || store.grant_count()? >= MAX_GRANTS {
        return Err(OpError::new(
            503,
            ErrorCode::RequestDenied.as_str(),
            "too many grants wait for the owner; try again later",
        ));
    }
    let public_name = public_name(node, &client).await;
    let interaction = Interaction {
        key: random_text::<24>(node),
        server_nonce: random_text::<24>(node),
        finish: request
            .interact
            .as_ref()
            .and_then(|interact| interact.finish.clone()),
        interact_ref: None,
        subject: request.subject.clone(),
        public_name,
        continued_at_ms: now,
        finalized: false,
    };
    let id = PaymentId::from_random(random_bytes(node));
    let (continue_token, continue_hash) = new_token(node);
    let row = GrantRow {
        id,
        state: GrantState::Pending,
        client,
        client_key: serde_json::to_string(jwk).map_err(OpError::internal)?,
        access: serde_json::to_string(&access).map_err(OpError::internal)?,
        needs_owner: true,
        continue_token_hash: continue_hash,
        interaction: Some(interaction.json()?),
        created_at_ms: now,
        decided_at_ms: None,
        expires_at_ms: now + PENDING_GRANT_LIFETIME_MS,
    };
    store.insert_grant(&row, None)?;
    changed(node).await;
    tracing::info!(target: target::PAYMENT, grant = %id, client = %row.client, "a grant waits for the owner");
    Ok(GrantResponse {
        interact: Some(InteractResponse {
            redirect: format!(
                "{}{GRANT_PATH}/interact/{id}/{}",
                urls.origin(),
                interaction.key
            ),
            finish: interaction.server_nonce,
        }),
        access_token: None,
        continuation: continuation(urls, id, continue_token),
        subject: None,
    })
}

fn continuation(urls: &Urls, id: PaymentId, token: String) -> Continue {
    Continue {
        access_token: ContinueToken { value: token },
        uri: format!("{}{GRANT_PATH}/continue/{id}", urls.origin()),
        wait: Some(CONTINUE_WAIT_S),
    }
}

/// The public name of the client's wallet address: what the consent page
/// shows beside its URL. None if it has none or cannot be read.
async fn public_name(node: &Node, client: &str) -> Option<String> {
    let peer_client = node.peer_client()?;
    let address = super::client::fetch_wallet_address(peer_client, client)
        .await
        .ok()?;
    let name = address.public_name?;
    let name: String = name.chars().take(MAX_PUBLIC_NAME_CHARS).collect();
    (!name.trim().is_empty()).then_some(name)
}

/// Tells live views of the grants that something changed.
async fn changed(node: &Node) {
    if let Some(engine) = node.engine() {
        engine.payments_changed().await;
    }
}

/// A pending grant, if `key` is its interaction key.
fn pending(node: &Node, id: PaymentId, key: &str) -> Result<(GrantRow, Interaction), OpError> {
    let gone = || OpError::not_found("no such grant waits for the owner");
    let grant = node.store().grant(id)?.ok_or_else(gone)?;
    if grant.state != GrantState::Pending || grant.expires_at_ms <= node.now_ms() {
        return Err(gone());
    }
    let interaction = Interaction::of(&grant)?;
    if !same(&interaction.key, key) {
        return Err(gone());
    }
    Ok((grant, interaction))
}

/// `GET /gnap/interact/{id}/{key}`: where to send the owner, the wallet's
/// consent page for the grant.
///
/// # Errors
///
/// 404 for a grant that does not wait, or a wrong key.
pub fn start(node: &Node, urls: &Urls, id: PaymentId, key: &str) -> Result<String, OpError> {
    pending(node, id, key)?;
    Ok(format!("{}{CONSENT_PATH}{id}?key={key}", urls.origin()))
}

/// The grant as the consent page shows it.
///
/// # Errors
///
/// 404 for a grant that does not wait, or a wrong key.
pub fn describe(node: &Node, id: PaymentId, key: &str) -> Result<GrantView, OpError> {
    let (grant, interaction) = pending(node, id, key)?;
    let access = serde_json::from_str(&grant.access).map_err(OpError::internal)?;
    let subject = interaction
        .subject
        .and_then(|subject| subject.sub_ids.into_iter().next())
        .map(|id| id.id);
    Ok(GrantView {
        grant,
        access,
        subject,
        public_name: interaction.public_name,
    })
}

/// The owner approves or denies a waiting grant. Returns where to send
/// them: the client's finish URI with the interaction hash and reference,
/// or with `result=grant_rejected`; none if the client named none.
///
/// # Errors
///
/// 404 for a grant that does not wait, or a wrong key.
pub async fn decide(
    node: &Node,
    urls: &Urls,
    id: PaymentId,
    key: &str,
    approve: bool,
) -> Result<Option<String>, OpError> {
    let (_, mut interaction) = pending(node, id, key)?;
    let interact_ref = random_text::<24>(node);
    interaction.interact_ref = Some(interact_ref.clone());
    let state = if approve {
        GrantState::Approved
    } else {
        GrantState::Denied
    };
    if !node
        .store()
        .decide_grant(id, state, &interaction.json()?, node.now_ms())?
    {
        return Err(OpError::not_found("no such grant waits for the owner"));
    }
    changed(node).await;
    tracing::info!(target: target::PAYMENT, grant = %id, approve, "the owner decided a grant");
    let Some(finish) = interaction.finish else {
        return Ok(None);
    };
    let separator = if finish.uri.contains('?') { '&' } else { '?' };
    let query = if approve {
        let hash = gnap::interaction_hash(
            &finish.nonce,
            &interaction.server_nonce,
            &interact_ref,
            &format!("{}{GRANT_PATH}", urls.origin()),
        );
        format!("hash={hash}&interact_ref={interact_ref}")
    } else {
        "result=grant_rejected".to_string()
    };
    Ok(Some(format!("{}{separator}{query}", finish.uri)))
}

/// `POST /gnap/continue/{id}` for a grant the owner decides: still
/// waiting, too soon, refused, or approved, when it issues the token.
///
/// # Errors
///
/// `too_fast` before the wait, `request_denied` once denied, and
/// `invalid_continuation` for a wrong reference or a grant continued
/// already.
pub async fn continue_grant(
    node: &Node,
    urls: &Urls,
    grant: GrantRow,
    body: &[u8],
    continue_token: String,
) -> Result<GrantResponse, OpError> {
    let invalid = |code: ErrorCode, status, description: &str| {
        OpError::new(status, code.as_str(), description.to_string())
    };
    let continued: ContinueRequest = if body.is_empty() {
        ContinueRequest::default()
    } else {
        serde_json::from_slice(body).map_err(|_| {
            invalid(
                ErrorCode::InvalidRequest,
                400,
                "the continuation is malformed",
            )
        })?
    };
    let now = node.now_ms();
    let mut interaction = Interaction::of(&grant)?;
    match grant.state {
        GrantState::Pending => {
            if now < interaction.continued_at_ms + CONTINUE_WAIT_S * 1_000 {
                return Err(invalid(ErrorCode::TooFast, 400, "continued too soon"));
            }
            interaction.continued_at_ms = now;
            node.store()
                .update_interaction(grant.id, &interaction.json()?, None)?;
            Ok(GrantResponse {
                interact: None,
                access_token: None,
                continuation: continuation(urls, grant.id, continue_token),
                subject: None,
            })
        }
        GrantState::Denied | GrantState::Revoked => Err(invalid(
            ErrorCode::RequestDenied,
            401,
            "the owner did not approve the grant",
        )),
        GrantState::Approved => {
            finalize(node, urls, grant, interaction, &continued, continue_token).await
        }
    }
}

/// Issues an approved grant's token, once: with the reference the owner's
/// return carried, when the client asked to be sent one. The continuation
/// stays, to revoke the grant with.
async fn finalize(
    node: &Node,
    urls: &Urls,
    grant: GrantRow,
    mut interaction: Interaction,
    continued: &ContinueRequest,
    continue_token: String,
) -> Result<GrantResponse, OpError> {
    let invalid = |description: &str| {
        OpError::new(
            400,
            ErrorCode::InvalidContinuation.as_str(),
            description.to_string(),
        )
    };
    if interaction.finalized {
        return Err(invalid("the grant was continued already; rotate its token"));
    }
    let expected = interaction.interact_ref.as_deref().unwrap_or_default();
    let reference_valid = match (&interaction.finish, continued.interact_ref.as_deref()) {
        (Some(_), Some(given)) => same(given, expected),
        (Some(_), None) => false,
        (None, given) => given.is_none_or(|given| same(given, expected)),
    };
    if !reference_valid {
        return Err(invalid("the interaction reference is wrong"));
    }
    let now = node.now_ms();
    let access: Vec<Access> = serde_json::from_str(&grant.access).map_err(OpError::internal)?;
    let token = (!access.is_empty()).then(|| {
        let (value, hash) = new_token(node);
        let manage_id = PaymentId::from_random(random_bytes(node));
        let row = TokenRow {
            token_hash: hash,
            grant: grant.id,
            manage_id,
            created_at_ms: now,
            expires_at_ms: now + ACCESS_TOKEN_LIFETIME_MS,
        };
        (value, row)
    });
    interaction.finalized = true;
    interaction.continued_at_ms = now;
    node.store().update_interaction(
        grant.id,
        &interaction.json()?,
        token.as_ref().map(|(_, row)| row),
    )?;
    changed(node).await;
    let subject = match &interaction.subject {
        Some(asked) => Some(proven(node, urls, asked).await?),
        None => None,
    };
    Ok(GrantResponse {
        interact: None,
        access_token: token.map(|(value, row)| AccessToken {
            value,
            manage: format!("{}{GRANT_PATH}/token/{}", urls.origin(), row.manage_id),
            expires_in: Some(ACCESS_TOKEN_LIFETIME_MS / 1_000),
            access,
        }),
        continuation: Continue {
            wait: None,
            ..continuation(urls, grant.id, continue_token)
        },
        subject,
    })
}

/// The identity the owner proved: the wallet address asked about, if it
/// is one of the node's.
async fn proven(node: &Node, urls: &Urls, asked: &Subject) -> Result<Subject, OpError> {
    let addresses = WalletAddresses::of(node)
        .await
        .map_err(|_| OpError::unavailable())?;
    let sub_ids = asked
        .sub_ids
        .iter()
        .filter(|id| addresses.ledger_of_url(urls, &id.id).is_some())
        .map(|id| SubjectId {
            id: id.id.clone(),
            format: "uri".into(),
        })
        .collect();
    Ok(Subject { sub_ids })
}

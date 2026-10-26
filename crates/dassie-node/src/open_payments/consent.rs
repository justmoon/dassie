//! Grants the owner decides (spec/decisions/open-payments.md): outgoing
//! payments, proof of a wallet address, and reading or listing what others
//! created.
//!
//! Such a grant waits, pending, for the owner. The client sends the owner
//! to `GET /gnap/interact/{id}/{key}`, which leads to the wallet's consent
//! page; the key is random per grant, so only the client and the owner
//! know the way there. Once the owner has decided in the wallet, the page
//! sends them back to the client's finish URI with the interaction hash
//! and a reference, and the client continues the grant with that
//! reference to get its token. A grant waits at most
//! [`PENDING_GRANT_LIFETIME_MS`]; at most [`MAX_PENDING_GRANTS`] wait at
//! once, of which strangers leave a share to clients the owner approved
//! before, and no one client or network takes more than a small share
//! ([`grant_fits`]).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dassie_kit::ClientKey;
use dassie_payments::gnap::{
    self, Access, AccessToken, Continue, ContinueRequest, ContinueToken, ErrorCode, Finish,
    GrantRequest, GrantResponse, InteractResponse, Subject, SubjectId,
};
use dassie_payments::jwk::Jwk;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;

use super::auth::{
    ACCESS_TOKEN_LIFETIME_MS, OpError, PENDING_GRANT_LIFETIME_MS, grant_fits, new_token,
    random_bytes,
};
use super::{GRANT_PATH, Urls, WalletAddresses};
use crate::log::target;
use crate::node::Node;
use crate::store::{GrantRow, GrantState, PaymentId, TokenRow};

/// Most grants waiting for the owner at once.
pub const MAX_PENDING_GRANTS: u32 = 256;
/// Seconds a client waits between continuations of a pending grant.
pub const CONTINUE_WAIT_S: u64 = 5;
/// The wallet's consent page.
pub const CONSENT_PATH: &str = "/consent/";
/// The longest public name of a client that is kept.
const MAX_PUBLIC_NAME_CHARS: usize = 128;
/// The longest interaction a grant's row keeps, in characters: the bound
/// on `gnap_grants.interaction` in `store/schema.rs`.
pub const MAX_INTERACTION_CHARS: usize = 4_096;
/// Random bytes in the interaction key, our nonce and the reference.
const INTERACTION_TEXT_BYTES: usize = 24;

/// What a pending grant keeps of its interaction, as JSON in the grant's
/// row. The key and the reference are secrets of the owner's and the
/// client's; the column is never shown (spec/decisions/secrets-at-rest.md).
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
    /// The origin the grant was asked for under, `https://<name>`, whose
    /// grant endpoint the interaction hash covers. None in a grant asked
    /// for before the node kept it: the node's main name then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
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

    /// Whether the row can keep this interaction at its longest: once the
    /// owner has decided, with the reference `decide` adds, continued at
    /// the latest time there is. Nothing else in it grows.
    fn fits(&self) -> Result<bool, OpError> {
        let mut longest = self.clone();
        let reference_chars = (INTERACTION_TEXT_BYTES * 4).div_ceil(3);
        longest.interact_ref = Some("x".repeat(reference_chars));
        longest.continued_at_ms = u64::MAX;
        longest.finalized = false;
        Ok(longest.json()?.chars().count() <= MAX_INTERACTION_CHARS)
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

/// Stores a grant that waits for the owner, asked for from `network`, and
/// tells the client where to send them.
///
/// # Errors
///
/// As [`grant_fits`]: 503 while too many grants wait or are kept, 429
/// while too many are the client's or its network's.
pub async fn request(
    node: &Node,
    urls: &Urls,
    client: String,
    network: Option<ClientKey>,
    jwk: &Jwk,
    access: Vec<Access>,
    request: &GrantRequest,
) -> Result<GrantResponse, OpError> {
    let now = node.now_ms();
    let store = node.store();
    store.prune_grants(now)?;
    // Before the client's wallet address is read: a request that will not
    // fit costs no fetch. The insert checks again.
    grant_fits(&store.grant_counts(&client, network)?, true)?;
    let mut interaction = Interaction {
        key: random_text::<INTERACTION_TEXT_BYTES>(node),
        server_nonce: random_text::<INTERACTION_TEXT_BYTES>(node),
        finish: request
            .interact
            .as_ref()
            .and_then(|interact| interact.finish.clone()),
        interact_ref: None,
        subject: request.subject.clone(),
        public_name: None,
        continued_at_ms: now,
        finalized: false,
        origin: Some(urls.origin().to_string()),
    };
    // The finish URI and the subject are the client's, each up to a
    // URI's bound: together they may not fit the row.
    if !interaction.fits()? {
        return Err(OpError::invalid(
            "the finish URI and subject are too long together",
        ));
    }
    // The client's public name is only shown: left out if it does not fit.
    interaction.public_name = public_name(node, &client).await;
    if !interaction.fits()? {
        interaction.public_name = None;
    }
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
    store.insert_grant(&row, None, network, |counts| grant_fits(counts, true))??;
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
    let address = super::client::fetch_wallet_address(node, client)
        .await
        .ok()?;
    let name = address.public_name?;
    let name: String = name.chars().take(MAX_PUBLIC_NAME_CHARS).collect();
    (!name.trim().is_empty()).then_some(name)
}

/// Tells live views of the grants that something changed.
pub(super) async fn changed(node: &Node) {
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
/// or with `result=grant_rejected`; none if the client named none. The
/// hash covers the grant endpoint under the name the client asked at,
/// which `urls`, the node's main name, stands in for only in a grant that
/// kept none.
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
    let interact_ref = random_text::<INTERACTION_TEXT_BYTES>(node);
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
        let origin = interaction.origin.as_deref().unwrap_or(urls.origin());
        let hash = gnap::interaction_hash(
            &finish.nonce,
            &interaction.server_nonce,
            &interact_ref,
            &format!("{origin}{GRANT_PATH}"),
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
        GrantState::Pending if grant.expires_at_ms <= now => Err(invalid(
            ErrorCode::InvalidContinuation,
            401,
            "the grant lapsed: the owner did not decide in time",
        )),
        GrantState::Pending => {
            if now < interaction.continued_at_ms + CONTINUE_WAIT_S * 1_000 {
                return Err(invalid(ErrorCode::TooFast, 400, "continued too soon"));
            }
            interaction.continued_at_ms = now;
            // The owner may decide meanwhile: the decision stands, and the
            // next continuation sees it.
            node.store()
                .update_interaction(&grant, &interaction.json()?, None)?;
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
            401,
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
    let mut access: Vec<Access> = serde_json::from_str(&grant.access).map_err(OpError::internal)?;
    super::auth::rebase_access(urls, &mut access);
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
    // Once: of two continuations at the same moment, one issues it.
    if !node.store().update_interaction(
        &grant,
        &interaction.json()?,
        token.as_ref().map(|(_, row)| row),
    )? {
        return Err(invalid("the grant was continued already; rotate its token"));
    }
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn chars(interaction: &Interaction) -> usize {
        interaction.json().unwrap().chars().count()
    }

    /// The finish URI and the subject are each up to a URI's bound; the
    /// row keeps the interaction up to its own, with the reference added.
    #[test]
    fn an_interaction_fits_its_row_up_to_the_bound() {
        let reference = URL_SAFE_NO_PAD.encode([0xff; INTERACTION_TEXT_BYTES]);
        let mut full = Interaction {
            key: reference.clone(),
            server_nonce: reference.clone(),
            finish: Some(Finish {
                method: "redirect".into(),
                uri: "https://client.example/".into(),
                nonce: "c".repeat(256),
            }),
            interact_ref: None,
            subject: Some(Subject {
                sub_ids: vec![SubjectId {
                    id: format!("https://wallet.example/{}", "s".repeat(1_500)),
                    format: "uri".into(),
                }],
            }),
            public_name: None,
            continued_at_ms: 0,
            finalized: false,
            origin: Some("https://pay.example.com".into()),
        };
        let mut decided = full.clone();
        decided.interact_ref = Some(reference.clone());
        decided.continued_at_ms = u64::MAX;
        let room = MAX_INTERACTION_CHARS - chars(&decided);
        let pad = "f".repeat(room);
        for interaction in [&mut full, &mut decided] {
            interaction.finish.as_mut().unwrap().uri.push_str(&pad);
        }
        assert!(full.fits().unwrap());
        // As the owner's decision leaves it, at the latest time there is,
        // the row keeps it at exactly its bound.
        assert_eq!(chars(&decided), MAX_INTERACTION_CHARS);
        full.finish.as_mut().unwrap().uri.push('f');
        assert!(!full.fits().unwrap());
    }
}

//! The authorization server, and the checks the resource server makes of
//! every request (D9).
//!
//! A client asks for a grant at `POST /gnap`, signing the request with its
//! key: the key its wallet address publishes, or one it sends along
//! ("directed identity"). Grants the owner need not approve, for incoming
//! payments and quotes, are approved at once. Every later request carries
//! the grant's access token and is signed with the same key, which the
//! grant keeps: a stolen token is of no use without the key.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dassie_payments::VerifyingKey;
use dassie_payments::gnap::{
    self, Access, AccessToken, Client, Continue, ContinueToken, ErrorCode, GrantRequest,
    GrantResponse, Scope,
};
use dassie_payments::httpsig::{self, Signature};
use dassie_payments::jwk::Jwk;
use sha2::{Digest as _, Sha256};

use super::keys::{self, KeyError};
use super::{GRANT_PATH, Urls, WalletAddresses};
use crate::log::target;
use crate::node::Node;
use crate::store::{GrantRow, GrantState, PaymentId, StoreError, TokenRow};

/// How long an access token works.
pub const ACCESS_TOKEN_LIFETIME_MS: u64 = 60 * 60 * 1_000;
/// How long a grant may wait for the owner.
pub const PENDING_GRANT_LIFETIME_MS: u64 = 10 * 60 * 1_000;

/// A request as signatures see it.
#[derive(Clone, Copy, Debug)]
pub struct SignedRequest<'a> {
    /// The method.
    pub method: &'a str,
    /// The URL the client sent it to.
    pub target_uri: &'a str,
    /// Its header lines.
    pub fields: &'a [(&'a str, &'a str)],
    /// Its body.
    pub body: &'a [u8],
}

impl<'a> SignedRequest<'a> {
    fn http(&self) -> httpsig::Request<'a> {
        httpsig::Request {
            method: self.method,
            target_uri: self.target_uri,
            fields: self.fields,
        }
    }

    fn header(&self, name: &str) -> Option<&'a str> {
        self.fields
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
    }
}

/// An Open Payments error: a status and `{"error": {"code", "description"}}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpError {
    /// The HTTP status.
    pub status: u16,
    /// The machine-readable code.
    pub code: &'static str,
    /// What went wrong, for people.
    pub description: String,
}

impl OpError {
    /// An error with a status, code and description.
    pub fn new(status: u16, code: &'static str, description: impl Into<String>) -> Self {
        Self {
            status,
            code,
            description: description.into(),
        }
    }

    fn gnap(status: u16, code: ErrorCode, description: impl Into<String>) -> Self {
        Self::new(status, code.as_str(), description)
    }

    /// 401 for a request without a usable token or signature.
    pub fn unauthorized(description: impl Into<String>) -> Self {
        Self::new(401, "unauthorized", description)
    }

    /// 403 for a token whose grant does not allow the request.
    pub fn forbidden(description: impl Into<String>) -> Self {
        Self::new(403, "forbidden", description)
    }

    /// 400 for a malformed request.
    pub fn invalid(description: impl Into<String>) -> Self {
        Self::new(400, "invalid_request", description)
    }

    /// 404.
    pub fn not_found(description: impl Into<String>) -> Self {
        Self::new(404, "not_found", description)
    }

    /// 503, while the node cannot answer.
    pub fn unavailable() -> Self {
        Self::new(503, "unavailable", "the node is not ready")
    }

    /// 500, for a bug; the details are logged.
    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(target: target::PAYMENT, %error, "Open Payments");
        Self::new(500, "internal_error", "internal error")
    }
}

impl From<StoreError> for OpError {
    fn from(error: StoreError) -> Self {
        Self::internal(error)
    }
}

fn random_bytes<const N: usize>(node: &Node) -> [u8; N] {
    let mut bytes = [0u8; N];
    node.with_rng(|rng| rng.fill_bytes(&mut bytes));
    bytes
}

/// A new token: its text, and the hash the database keeps.
fn new_token(node: &Node) -> (String, [u8; 32]) {
    let token = URL_SAFE_NO_PAD.encode(random_bytes::<32>(node));
    let hash = token_hash(&token);
    (token, hash)
}

/// The hash of a token, as the database keeps it.
#[must_use]
pub fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

/// Rewrites each access item's wallet address to the canonical URL of the
/// ledger it names, so that `$host` and `/pay/<slug>` mean the same.
fn canonical(
    access: &mut [Access],
    addresses: &WalletAddresses,
    urls: &Urls,
) -> Result<(), OpError> {
    for item in access {
        if let Some(identifier) = &item.identifier {
            let ledger = addresses.ledger_of_url(urls, identifier).ok_or_else(|| {
                OpError::gnap(
                    400,
                    ErrorCode::InvalidRequest,
                    "the identifier is not a wallet address of this node",
                )
            })?;
            item.identifier = Some(urls.wallet_address(ledger, false));
        }
    }
    Ok(())
}

/// The components every signature must cover.
fn required(request: &SignedRequest<'_>) -> Vec<&'static str> {
    let mut components = vec!["@method", "@target-uri"];
    if request.header("authorization").is_some() {
        components.push("authorization");
    }
    if !request.body.is_empty() {
        components.push("content-digest");
    }
    components
}

/// Finds the request's signature and checks it with `key`, whose ID it
/// must name.
fn check_signature(
    request: &SignedRequest<'_>,
    jwk: &Jwk,
    key: &VerifyingKey,
    now_ms: u64,
    failure: impl Fn(String) -> OpError,
) -> Result<(), OpError> {
    let signature = Signature::find(&request.http(), &required(request), now_ms / 1_000)
        .map_err(|error| failure(format!("the signature is not accepted: {error:?}")))?;
    if signature.keyid() != jwk.kid {
        return Err(failure("the signature names another key".into()));
    }
    signature
        .verify(&request.http(), request.body, key)
        .map_err(|error| failure(format!("the signature is not accepted: {error:?}")))
}

/// The client's key, and its name as resources record it.
async fn client_identity(
    node: &Node,
    client: &Client,
    request: &SignedRequest<'_>,
    now_ms: u64,
) -> Result<(String, Jwk, VerifyingKey), OpError> {
    let invalid = |description: String| OpError::gnap(401, ErrorCode::InvalidClient, description);
    match client {
        Client::Key(key) => {
            let verifying = key
                .jwk
                .verifying_key()
                .map_err(|_| invalid("the key is not an Ed25519 key".into()))?;
            Ok((format!("jwk:{}", key.jwk.x), key.jwk.clone(), verifying))
        }
        Client::WalletAddress(_) | Client::Legacy(_) => {
            let address = client.wallet_address().unwrap_or_default();
            let signature = Signature::find(&request.http(), &required(request), now_ms / 1_000)
                .map_err(|error| invalid(format!("the signature is not accepted: {error:?}")))?;
            let peer_client = node
                .peer_client()
                .ok_or_else(|| OpError::internal("no HTTPS client"))?;
            let (jwk, key) = keys::client_key(
                &node.open_payments().keys,
                peer_client,
                address,
                signature.keyid(),
                now_ms,
            )
            .await
            .map_err(|error| match error {
                KeyError::InvalidAddress => {
                    invalid("the client's wallet address is invalid".into())
                }
                KeyError::UnknownKey => {
                    invalid("the client's wallet address has no such key".into())
                }
                KeyError::Unavailable(error) => invalid(format!("the client's keys: {error}")),
            })?;
            Ok((address.to_string(), jwk, key))
        }
    }
}

/// `POST /gnap`: asks for a grant.
///
/// # Errors
///
/// What the authorization server answers instead.
pub async fn request_grant(
    node: &Node,
    urls: &Urls,
    request: &SignedRequest<'_>,
) -> Result<GrantResponse, OpError> {
    let now = node.now_ms();
    let grant: GrantRequest = serde_json::from_slice(request.body).map_err(|_| {
        OpError::gnap(
            400,
            ErrorCode::InvalidRequest,
            "the grant request is malformed",
        )
    })?;
    grant
        .validate()
        .map_err(|code| OpError::gnap(400, code, "the grant request is not accepted"))?;
    if grant.needs_owner() {
        return Err(OpError::gnap(
            400,
            ErrorCode::InvalidRequest,
            "grants that need the owner's consent are not offered yet",
        ));
    }
    let addresses = WalletAddresses::of(node)
        .await
        .map_err(|_| OpError::unavailable())?;
    let mut access = grant.access().to_vec();
    canonical(&mut access, &addresses, urls)?;
    let (client, jwk, key) = client_identity(node, &grant.client, request, now).await?;
    check_signature(request, &jwk, &key, now, |description| {
        OpError::gnap(401, ErrorCode::InvalidClient, description)
    })?;
    approve(node, urls, client, &jwk, access)
}

/// Stores an approved grant with its first token, and answers with both.
fn approve(
    node: &Node,
    urls: &Urls,
    client: String,
    jwk: &Jwk,
    access: Vec<Access>,
) -> Result<GrantResponse, OpError> {
    let now = node.now_ms();
    let store = node.store();
    store.prune_grants(now)?;
    let id = PaymentId::from_random(random_bytes(node));
    let (continue_token, continue_hash) = new_token(node);
    let (access_token, access_hash) = new_token(node);
    let manage_id = PaymentId::from_random(random_bytes(node));
    let row = GrantRow {
        id,
        state: GrantState::Approved,
        client,
        client_key: serde_json::to_string(jwk).map_err(OpError::internal)?,
        access: serde_json::to_string(&access).map_err(OpError::internal)?,
        needs_owner: false,
        continue_token_hash: continue_hash,
        interaction: None,
        created_at_ms: now,
        decided_at_ms: Some(now),
        expires_at_ms: now + PENDING_GRANT_LIFETIME_MS,
    };
    let token = TokenRow {
        token_hash: access_hash,
        grant: id,
        manage_id,
        created_at_ms: now,
        expires_at_ms: now + ACCESS_TOKEN_LIFETIME_MS,
    };
    store.insert_grant(&row, Some(&token))?;
    tracing::debug!(target: target::PAYMENT, grant = %id, client = %row.client, "grant approved");
    Ok(GrantResponse {
        interact: None,
        access_token: Some(AccessToken {
            value: access_token,
            manage: format!("{}{GRANT_PATH}/token/{manage_id}", urls.origin()),
            expires_in: Some(ACCESS_TOKEN_LIFETIME_MS / 1_000),
            access,
        }),
        continuation: Continue {
            access_token: ContinueToken {
                value: continue_token,
            },
            uri: format!("{}{GRANT_PATH}/continue/{id}", urls.origin()),
            wait: None,
        },
        subject: None,
    })
}

/// What a request's token allows.
#[derive(Clone, Debug)]
pub struct Authorized {
    /// The grant.
    pub grant: GrantRow,
    /// Its access, with canonical wallet addresses.
    pub access: Vec<Access>,
}

impl Authorized {
    /// What the grant allows for `action` on resources of `kind` of the
    /// wallet address of `ledger`.
    #[must_use]
    pub fn permits(
        &self,
        urls: &Urls,
        kind: gnap::AccessType,
        action: gnap::Action,
        ledger: dassie_ledger::LedgerId,
    ) -> Option<Scope> {
        gnap::permits(
            &self.access,
            kind,
            action,
            &urls.wallet_address(ledger, false),
        )
    }
}

/// Checks a resource server request's token and signature.
///
/// # Errors
///
/// 401 without a valid token and signature.
pub fn authorize(node: &Node, request: &SignedRequest<'_>) -> Result<Authorized, OpError> {
    let now = node.now_ms();
    let token = request
        .header("authorization")
        .and_then(gnap::bearer)
        .ok_or_else(|| OpError::unauthorized("a GNAP access token is required"))?;
    let (_, grant) = node
        .store()
        .access_token(&token_hash(token), now)?
        .ok_or_else(|| OpError::unauthorized("the access token is not valid"))?;
    if grant.state != GrantState::Approved {
        return Err(OpError::unauthorized("the grant is not approved"));
    }
    let jwk: Jwk = serde_json::from_str(&grant.client_key).map_err(OpError::internal)?;
    let key = jwk
        .verifying_key()
        .map_err(|_| OpError::internal("a stored client key is invalid"))?;
    check_signature(request, &jwk, &key, now, OpError::unauthorized)?;
    let access = serde_json::from_str(&grant.access).map_err(OpError::internal)?;
    Ok(Authorized { grant, access })
}

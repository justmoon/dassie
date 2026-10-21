//! The authorization server, and the checks the resource server makes of
//! every request (spec/decisions/open-payments.md).
//!
//! A client asks for a grant at `POST /gnap`, signing the request with its
//! key: the key its wallet address publishes, or one it sends along
//! ("directed identity"). Grants the owner need not approve, for incoming
//! payments and quotes, are approved at once; the others wait for the
//! owner ([`super::consent`]). Every later request carries the grant's
//! access token and is signed with the same key, which the grant keeps: a
//! stolen token is of no use without the key.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dassie_node_core::rate_limit::ClientKey;
use dassie_payments::VerifyingKey;
use dassie_payments::amount::asset_name;
use dassie_payments::gnap::{
    self, Access, AccessToken, AccessType, Action, Client, Continue, ContinueToken, ErrorCode,
    GrantRequest, GrantResponse, Scope,
};
use dassie_payments::httpsig::{self, Signature};
use dassie_payments::jwk::Jwk;
use sha2::{Digest as _, Sha256};

use super::consent::MAX_PENDING_GRANTS;
use super::keys::{self, KeyError};
use super::{GRANT_PATH, Urls, WalletAddresses};
use crate::log::target;
use crate::node::Node;
use crate::store::{
    GrantCounts, GrantRow, GrantState, Listed, MAX_GRANTS, MAX_LISTED, PaymentId, StoreError,
    TokenRow,
};

/// How long an access token works.
pub const ACCESS_TOKEN_LIFETIME_MS: u64 = 60 * 60 * 1_000;
/// How long a grant may wait for the owner.
pub const PENDING_GRANT_LIFETIME_MS: u64 = 10 * 60 * 1_000;

/// Grants only clients the owner approved before may take: strangers'
/// grants never fill the table (spec/decisions/owners-share.md).
pub const OWNER_RESERVED_GRANTS: u32 = MAX_GRANTS / 16;
/// Places among the grants waiting for the owner that only clients the
/// owner approved before may take.
pub const OWNER_RESERVED_PENDING: u32 = MAX_PENDING_GRANTS / 4;
/// Most grants one stranger keeps.
pub const MAX_GRANTS_PER_CLIENT: u32 = MAX_GRANTS / 64;
/// Most grants kept that strangers asked for from one network.
pub const MAX_GRANTS_PER_NETWORK: u32 = MAX_GRANTS / 16;
/// Most grants of one stranger waiting for the owner.
pub const MAX_PENDING_PER_CLIENT: u32 = 4;
/// Most grants waiting for the owner that strangers asked for from one
/// network.
pub const MAX_PENDING_PER_NETWORK: u32 = MAX_PENDING_GRANTS / 16;

/// Whether a new grant fits beside those kept
/// (spec/decisions/owners-share.md).
///
/// Clients the owner approved before, by their grant the owner approved,
/// are the owner's side: strangers leave them a share of the grants and
/// of the places waiting for the owner. Grants approved at once leave
/// room for every grant that may wait, so the table never stops one from
/// asking. No one stranger, and no one network of strangers, holds more
/// than a small share. A client the owner approved has no share of its
/// own, only the pools, as before: the owner chose it, its grants the
/// owner approved last for months, and a network is shared by many.
///
/// # Errors
///
/// 503 when the pool the grant goes into is full, 429 when a stranger or its
/// network has its share.
pub fn grant_fits(counts: &GrantCounts, needs_owner: bool) -> Result<(), OpError> {
    let mut most = MAX_GRANTS;
    if !needs_owner {
        most -= MAX_PENDING_GRANTS;
    }
    if !counts.known {
        most -= OWNER_RESERVED_GRANTS;
    }
    let most_pending = if counts.known {
        MAX_PENDING_GRANTS
    } else {
        MAX_PENDING_GRANTS - OWNER_RESERVED_PENDING
    };
    if needs_owner && counts.pending >= most_pending {
        return Err(OpError::gnap(
            503,
            ErrorCode::RequestDenied,
            "too many grants wait for the owner; try again later",
        ));
    }
    if counts.kept >= most {
        return Err(OpError::gnap(
            503,
            ErrorCode::RequestDenied,
            "too many grants; try again later",
        ));
    }
    if counts.known {
        return Ok(());
    }
    let has_share = |of_client: u32, most_client: u32, of_network: u32, most_network: u32| {
        of_client >= most_client || of_network >= most_network
    };
    let kept = (counts.client_kept, counts.network_kept);
    let pending = (counts.client_pending, counts.network_pending);
    if has_share(
        kept.0,
        MAX_GRANTS_PER_CLIENT,
        kept.1,
        MAX_GRANTS_PER_NETWORK,
    ) || (needs_owner
        && has_share(
            pending.0,
            MAX_PENDING_PER_CLIENT,
            pending.1,
            MAX_PENDING_PER_NETWORK,
        ))
    {
        return Err(OpError::new(
            429,
            "too_many_requests",
            "too many grants of yours; try again later",
        ));
    }
    Ok(())
}

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

pub(super) fn random_bytes<const N: usize>(node: &Node) -> [u8; N] {
    let mut bytes = [0u8; N];
    node.with_rng(|rng| rng.fill_bytes(&mut bytes));
    bytes
}

/// A new token: its text, and the hash the database keeps.
pub(super) fn new_token(node: &Node) -> (String, [u8; 32]) {
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
/// ledger it names, so that `$host` and `/pay/<slug>` mean the same; then
/// refuses two items that create outgoing payments from one.
///
/// Refuses too an item that creates outgoing payments with a debit limit
/// in another asset than its wallet address: no payment from it could
/// fit the limit (spec/decisions/unusable-limits.md).
fn canonical(
    access: &mut [Access],
    addresses: &WalletAddresses,
    urls: &Urls,
) -> Result<(), OpError> {
    for item in access.iter_mut() {
        if let Some(identifier) = &item.identifier {
            let ledger = addresses.ledger_of_url(urls, identifier).ok_or_else(|| {
                OpError::gnap(
                    400,
                    ErrorCode::InvalidRequest,
                    "the identifier is not a wallet address of this node",
                )
            })?;
            item.identifier = Some(urls.wallet_address(ledger, false));
            debits_in_own_asset(item, ledger)?;
        }
    }
    // Only now are two spellings of one wallet address the same.
    if gnap::creates_twice(access) {
        return Err(OpError::gnap(
            400,
            ErrorCode::InvalidRequest,
            "two items create outgoing payments from one wallet address",
        ));
    }
    Ok(())
}

/// Refuses an item that creates outgoing payments from `ledger` with a
/// debit limit in another asset, naming the asset the limit must be in.
fn debits_in_own_asset(item: &Access, ledger: dassie_ledger::LedgerId) -> Result<(), OpError> {
    let creates =
        item.kind == AccessType::OutgoingPayment && item.actions.contains(&Action::Create);
    let asset = ledger.asset();
    let Some(limit) = item
        .limits
        .as_ref()
        .filter(|limits| creates && !limits.debits_in(asset.code, asset.scale))
        .and_then(|limits| limits.debit_amount.as_ref())
    else {
        return Ok(());
    };
    Err(OpError::gnap(
        400,
        ErrorCode::InvalidRequest,
        format!(
            "the debit limit must be in the wallet address's asset, {}; it is in {}",
            asset_name(asset.code, asset.scale),
            asset_name(&limit.asset_code, limit.asset_scale),
        ),
    ))
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

/// `POST /gnap`: asks for a grant, from `network` (none where it counts
/// in the totals only).
///
/// # Errors
///
/// What the authorization server answers instead.
pub async fn request_grant(
    node: &Node,
    urls: &Urls,
    request: &SignedRequest<'_>,
    network: Option<ClientKey>,
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
    let addresses = WalletAddresses::of(node)
        .await
        .map_err(|_| OpError::unavailable())?;
    let mut access = grant.access().to_vec();
    canonical(&mut access, &addresses, urls)?;
    let (client, jwk, key) = client_identity(node, &grant.client, request, now).await?;
    check_signature(request, &jwk, &key, now, |description| {
        OpError::gnap(401, ErrorCode::InvalidClient, description)
    })?;
    if grant.needs_owner() {
        return super::consent::request(node, urls, client, network, &jwk, access, &grant).await;
    }
    approve(node, urls, client, network, &jwk, access)
}

/// Stores an approved grant with its first token, and answers with both.
fn approve(
    node: &Node,
    urls: &Urls,
    client: String,
    network: Option<ClientKey>,
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
    store.insert_grant(&row, Some(&token), network, |counts| {
        grant_fits(counts, false)
    })??;
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

/// The GNAP token a request carries, and the hash the database keeps.
fn presented_token(request: &SignedRequest<'_>) -> Result<[u8; 32], OpError> {
    presented(request).map(token_hash)
}

fn presented<'a>(request: &SignedRequest<'a>) -> Result<&'a str, OpError> {
    request
        .header("authorization")
        .and_then(gnap::bearer)
        .ok_or_else(|| OpError::gnap(401, ErrorCode::InvalidClient, "a GNAP token is required"))
}

/// Checks a management request's signature against the grant's key.
fn signed_by_client(
    node: &Node,
    request: &SignedRequest<'_>,
    grant: &GrantRow,
) -> Result<(), OpError> {
    let jwk: Jwk = serde_json::from_str(&grant.client_key).map_err(OpError::internal)?;
    let key = jwk
        .verifying_key()
        .map_err(|_| OpError::internal("a stored client key is invalid"))?;
    check_signature(request, &jwk, &key, node.now_ms(), |description| {
        OpError::gnap(401, ErrorCode::InvalidClient, description)
    })
}

/// `POST /gnap/token/{id}`: a new token for the same access, in place of
/// the one presented, which stops working.
///
/// # Errors
///
/// 401 without the token and its client's signature, 404 for an unknown
/// one, 400 when its grant is no longer approved.
pub fn rotate_token(
    node: &Node,
    urls: &Urls,
    manage_id: PaymentId,
    request: &SignedRequest<'_>,
) -> Result<AccessToken, OpError> {
    let hash = presented_token(request)?;
    let (_, grant) = node
        .store()
        .managed_token(manage_id, &hash)?
        .ok_or_else(|| OpError::gnap(404, ErrorCode::InvalidRotation, "no such token"))?;
    signed_by_client(node, request, &grant)?;
    if grant.state != GrantState::Approved {
        return Err(OpError::gnap(
            400,
            ErrorCode::InvalidRotation,
            "the grant is not approved",
        ));
    }
    let now = node.now_ms();
    let (value, new_hash) = new_token(node);
    let new_manage = PaymentId::from_random(random_bytes(node));
    let row = TokenRow {
        token_hash: new_hash,
        grant: grant.id,
        manage_id: new_manage,
        created_at_ms: now,
        expires_at_ms: now + ACCESS_TOKEN_LIFETIME_MS,
    };
    if !node.store().rotate_token(&hash, &row)? {
        return Err(OpError::gnap(
            404,
            ErrorCode::InvalidRotation,
            "no such token",
        ));
    }
    let mut access: Vec<Access> = serde_json::from_str(&grant.access).map_err(OpError::internal)?;
    rebase_access(urls, &mut access);
    Ok(AccessToken {
        value,
        manage: format!("{}{GRANT_PATH}/token/{new_manage}", urls.origin()),
        expires_in: Some(ACCESS_TOKEN_LIFETIME_MS / 1_000),
        access,
    })
}

/// `DELETE /gnap/token/{id}`: revokes the token presented.
///
/// # Errors
///
/// 401 without the token and its client's signature.
pub fn revoke_token(
    node: &Node,
    manage_id: PaymentId,
    request: &SignedRequest<'_>,
) -> Result<(), OpError> {
    let hash = presented_token(request)?;
    // An unknown token is gone already: revoking is idempotent.
    if let Some((_, grant)) = node.store().managed_token(manage_id, &hash)? {
        signed_by_client(node, request, &grant)?;
        node.store().revoke_token(&hash)?;
    }
    Ok(())
}

/// `DELETE /gnap/continue/{id}`: revokes the grant, and its tokens, with
/// its continuation token.
///
/// # Errors
///
/// 401 without the continuation token and the client's signature.
pub async fn revoke_grant(
    node: &Node,
    grant_id: PaymentId,
    request: &SignedRequest<'_>,
) -> Result<(), OpError> {
    let hash = presented_token(request)?;
    let grant = node
        .store()
        .continued_grant(grant_id, &hash)?
        .ok_or_else(|| {
            OpError::gnap(401, ErrorCode::InvalidContinuation, "no such continuation")
        })?;
    signed_by_client(node, request, &grant)?;
    node.store().revoke_grant(grant.id, node.now_ms())?;
    super::consent::changed(node).await;
    tracing::debug!(target: target::PAYMENT, grant = %grant.id, "grant revoked by its client");
    Ok(())
}

/// `POST /gnap/continue/{id}`: continuing a grant the owner decides. The
/// others are final when issued.
///
/// # Errors
///
/// 401 without the continuation token and the client's signature; what
/// [`super::consent::continue_grant`] answers for one the owner decides;
/// 400 for any other.
pub async fn continue_grant(
    node: &Node,
    urls: &Urls,
    grant_id: PaymentId,
    request: &SignedRequest<'_>,
) -> Result<GrantResponse, OpError> {
    let token = presented(request)?;
    let grant = node
        .store()
        .continued_grant(grant_id, &token_hash(token))?
        .ok_or_else(|| {
            OpError::gnap(401, ErrorCode::InvalidContinuation, "no such continuation")
        })?;
    signed_by_client(node, request, &grant)?;
    if !grant.needs_owner {
        return Err(OpError::gnap(
            400,
            ErrorCode::InvalidContinuation,
            "the grant is final: there is nothing to continue",
        ));
    }
    super::consent::continue_grant(node, urls, grant, request.body, token.to_string()).await
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

    /// Whether the grant may `action` a resource of `kind` of the wallet
    /// address of `ledger`, that `client` made (none for the owner), and
    /// that pays `receiver` (none for resources that pay none): whether
    /// one access item allows it within its own limits.
    #[must_use]
    pub fn covers(
        &self,
        urls: &Urls,
        kind: gnap::AccessType,
        action: gnap::Action,
        ledger: dassie_ledger::LedgerId,
        client: Option<&str>,
        receiver: Option<&str>,
    ) -> bool {
        let own = client == Some(self.grant.client.as_str());
        gnap::reaches(
            &self.access,
            kind,
            action,
            &urls.wallet_address(ledger, false),
        )
        .iter()
        .any(|reach| reach.covers(own, receiver))
    }

    /// What a list of resources of `kind` of the wallet address of
    /// `ledger` shows: what any access item that lists reaches. Empty when
    /// none does.
    #[must_use]
    pub fn listed(
        &self,
        urls: &Urls,
        kind: gnap::AccessType,
        ledger: dassie_ledger::LedgerId,
    ) -> Vec<Listed<'_>> {
        let client = self.grant.client.as_str();
        let reaches = gnap::reaches(
            &self.access,
            kind,
            gnap::Action::List,
            &urls.wallet_address(ledger, false),
        );
        assert!(reaches.len() <= MAX_LISTED, "a grant has too many items");
        reaches
            .iter()
            .map(|reach| Listed {
                client: (reach.scope == Scope::Own).then_some(client),
                receiver: reach.receiver,
            })
            .collect()
    }

    /// The limits of the outgoing payments the grant may create from the
    /// wallet address of `ledger`: none when its item names none.
    #[must_use]
    pub fn limits(&self, urls: &Urls, ledger: dassie_ledger::LedgerId) -> gnap::Limits {
        gnap::creating(&self.access, &urls.wallet_address(ledger, false))
            .and_then(|item| item.limits.clone())
            .unwrap_or(gnap::Limits {
                receiver: None,
                interval: None,
                debit_amount: None,
                receive_amount: None,
            })
    }
}

/// Writes each access item's wallet address under the name `urls` answer
/// under. A grant keeps the URL it was made under, which is the node's by
/// [`canonical`], so it holds under every name the node answers to.
pub fn rebase_access(urls: &Urls, access: &mut [Access]) {
    for item in access {
        if let Some(identifier) = item.identifier.as_mut() {
            *identifier = urls.rebase(identifier);
        }
    }
}

/// Checks a resource server request's token and signature, and reads the
/// grant's access under the name the request came through.
///
/// # Errors
///
/// 401 without a valid token and signature.
pub fn authorize(
    node: &Node,
    urls: &Urls,
    request: &SignedRequest<'_>,
) -> Result<Authorized, OpError> {
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
    let mut access: Vec<Access> = serde_json::from_str(&grant.access).map_err(OpError::internal)?;
    rebase_access(urls, &mut access);
    Ok(Authorized { grant, access })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::{IpAddr, Ipv4Addr};

    use super::*;
    use crate::store::Store;

    /// A grant of `client`, the `n`th made.
    fn grant(n: u32, client: &str, state: GrantState, needs_owner: bool) -> GrantRow {
        let mut id = [0; 16];
        id[..4].copy_from_slice(&n.to_be_bytes());
        let mut hash = [0; 32];
        hash[..4].copy_from_slice(&n.to_be_bytes());
        GrantRow {
            id: PaymentId(id),
            state,
            client: client.into(),
            client_key: "{}".into(),
            access: "[]".into(),
            needs_owner,
            continue_token_hash: hash,
            interaction: None,
            created_at_ms: 1,
            decided_at_ms: (state != GrantState::Pending).then_some(1),
            expires_at_ms: 2,
        }
    }

    fn pending(n: u32, client: &str) -> GrantRow {
        grant(n, client, GrantState::Pending, true)
    }

    /// The `n`th IPv4 /24.
    fn network(n: u32) -> Option<ClientKey> {
        let [_, a, b, c] = n.to_be_bytes();
        Some(ClientKey::network_of(IpAddr::V4(Ipv4Addr::new(a, b, c, 1))))
    }

    /// Asks for a grant as the authorization server does.
    fn ask(store: &Store, row: &GrantRow, network: Option<ClientKey>) -> Result<(), OpError> {
        store
            .insert_grant(row, None, network, |counts| {
                grant_fits(counts, row.needs_owner)
            })
            .unwrap()
    }

    // Grants the owner decides need no owner to ask for, so strangers can
    // fill the places where they wait. A client the owner approved before
    // still gets one (spec/decisions/owners-share.md).
    #[test]
    fn a_client_the_owner_approved_asks_while_strangers_fill_the_waiting_grants() {
        let store = Store::open_in_memory().unwrap();
        let known = "https://app.example/me";
        let approved = grant(0, known, GrantState::Approved, true);
        ask(&store, &approved, network(0)).unwrap();
        let mut n = 1;
        let refused = loop {
            // As many networks as it takes.
            let row = pending(n, &format!("https://x{n}.example/"));
            match ask(&store, &row, network(n / MAX_PENDING_PER_NETWORK + 1)) {
                Ok(()) => n += 1,
                Err(refusal) => break refusal,
            }
        };
        assert_eq!(refused.status, 503);
        assert_eq!(n - 1, MAX_PENDING_GRANTS - OWNER_RESERVED_PENDING);
        let stranger = pending(n, "https://new.example/");
        assert_eq!(
            ask(&store, &stranger, network(9_999)).unwrap_err().status,
            503
        );
        // From a strangers' network too: it is known by its key.
        assert_eq!(ask(&store, &pending(n + 1, known), network(1)), Ok(()));
    }

    // No one client or network holds more than a small share of what
    // waits for the owner, or of what is kept.
    #[test]
    fn no_one_client_or_network_takes_more_than_its_share() {
        let store = Store::open_in_memory().unwrap();
        let mut n = 0;
        let mut next = |client: &str, state, needs_owner| {
            n += 1;
            grant(n, client, state, needs_owner)
        };
        for _ in 0..MAX_PENDING_PER_CLIENT {
            let row = next("https://one.example/", GrantState::Pending, true);
            ask(&store, &row, network(1)).unwrap();
        }
        let row = next("https://one.example/", GrantState::Pending, true);
        assert_eq!(ask(&store, &row, network(2)).unwrap_err().status, 429);
        for client in MAX_PENDING_PER_CLIENT..MAX_PENDING_PER_NETWORK {
            let row = next(
                &format!("https://{client}.example/"),
                GrantState::Pending,
                true,
            );
            ask(&store, &row, network(1)).unwrap();
        }
        let row = next("https://other.example/", GrantState::Pending, true);
        assert_eq!(ask(&store, &row, network(1)).unwrap_err().status, 429);
        assert_eq!(ask(&store, &row, network(2)), Ok(()));
        // Loopback in development counts in the totals only.
        let row = next("https://local.example/", GrantState::Pending, true);
        assert_eq!(ask(&store, &row, None), Ok(()));
        // Grants approved at once, here of a client known by its key.
        for _ in 0..MAX_GRANTS_PER_CLIENT {
            let row = next("jwk:many", GrantState::Approved, false);
            ask(&store, &row, network(3)).unwrap();
        }
        let row = next("jwk:many", GrantState::Approved, false);
        assert_eq!(ask(&store, &row, network(4)).unwrap_err().status, 429);
        let counts = store.grant_counts("jwk:many", network(3)).unwrap();
        assert_eq!(counts.client_kept, MAX_GRANTS_PER_CLIENT);
        assert_eq!(counts.network_kept, MAX_GRANTS_PER_CLIENT);
        assert_eq!(counts.network_pending, 0);
        assert!(!counts.known);
    }

    // An app the owner approved may ask for a grant at every payment, and
    // the owner's grants last months: it has no share of its own, nor its
    // network's, only the pools, as before.
    #[test]
    fn a_client_the_owner_approved_is_held_to_the_pools_only() {
        let store = Store::open_in_memory().unwrap();
        let known = "https://app.example/me";
        let mut n = 0;
        let mut next = |state, needs_owner| {
            n += 1;
            grant(n, known, state, needs_owner)
        };
        for _ in 0..=MAX_GRANTS_PER_CLIENT {
            ask(&store, &next(GrantState::Approved, true), network(1)).unwrap();
        }
        for _ in 0..=MAX_PENDING_PER_CLIENT {
            ask(&store, &next(GrantState::Pending, true), network(1)).unwrap();
        }
        // Strangers fill their network's share; the app still asks there.
        for stranger in 0..MAX_PENDING_PER_NETWORK {
            let row = pending(10_000 + stranger, &format!("https://s{stranger}.example/"));
            ask(&store, &row, network(1)).unwrap();
        }
        let row = pending(20_000, "https://late.example/");
        assert_eq!(ask(&store, &row, network(1)).unwrap_err().status, 429);
        assert_eq!(
            ask(&store, &next(GrantState::Pending, true), network(1)),
            Ok(())
        );
        assert_eq!(
            ask(&store, &next(GrantState::Approved, false), network(1)),
            Ok(())
        );
    }

    // Strangers' grants approved at once never take the room of grants
    // that wait for the owner, nor that of the clients the owner approved.
    #[test]
    fn strangers_leave_room_for_waiting_grants_and_the_owners_clients() {
        let stranger = GrantCounts {
            kept: MAX_GRANTS - MAX_PENDING_GRANTS - OWNER_RESERVED_GRANTS,
            ..GrantCounts::default()
        };
        assert_eq!(grant_fits(&stranger, false).unwrap_err().status, 503);
        assert_eq!(grant_fits(&stranger, true), Ok(()));
        let known = GrantCounts {
            known: true,
            ..stranger
        };
        assert_eq!(grant_fits(&known, false), Ok(()));
        let full = GrantCounts {
            kept: MAX_GRANTS - OWNER_RESERVED_GRANTS,
            ..stranger
        };
        assert_eq!(grant_fits(&full, true).unwrap_err().status, 503);
        let known = GrantCounts {
            known: true,
            ..full
        };
        assert_eq!(grant_fits(&known, true), Ok(()));
        let all = GrantCounts {
            kept: MAX_GRANTS,
            ..known
        };
        assert_eq!(grant_fits(&all, true).unwrap_err().status, 503);
    }
}

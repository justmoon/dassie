//! GNAP (RFC 9635) as Open Payments profiles it: grant requests and their
//! validation, which grants need the owner, what an access token permits,
//! the responses, and the interaction hash.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;

use crate::amount::Amount;
use crate::interval::Interval;
use crate::jwk::Jwk;
use crate::uri::Uri;

/// The most access items one grant may ask for.
pub const MAX_ACCESS_ITEMS: usize = 3;
/// The longest nonce, interaction reference or identifier accepted.
pub const MAX_TOKEN_TEXT_BYTES: usize = 256;

/// The error codes of the authorization server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// The client is unknown, or its key or signature is not accepted.
    InvalidClient,
    /// The request is malformed or asks for something not offered.
    InvalidRequest,
    /// The owner or the policy refused the request.
    RequestDenied,
    /// The client polled before `wait` was up.
    TooFast,
    /// The continuation does not exist or is in the wrong state.
    InvalidContinuation,
    /// The token cannot be rotated.
    InvalidRotation,
}

impl ErrorCode {
    /// The code as the specs write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidClient => "invalid_client",
            Self::InvalidRequest => "invalid_request",
            Self::RequestDenied => "request_denied",
            Self::TooFast => "too_fast",
            Self::InvalidContinuation => "invalid_continuation",
            Self::InvalidRotation => "invalid_rotation",
        }
    }
}

/// The kinds of resource a grant reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessType {
    /// Incoming payments.
    IncomingPayment,
    /// Outgoing payments.
    OutgoingPayment,
    /// Quotes.
    Quote,
}

/// What a grant may do with its resources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Create resources.
    Create,
    /// Complete incoming payments.
    Complete,
    /// Read resources the client created.
    Read,
    /// Read every resource of the wallet address.
    ReadAll,
    /// List resources the client created.
    List,
    /// List every resource of the wallet address.
    ListAll,
}

/// Limits on an outgoing-payment grant (`limits-outgoing`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    /// The only incoming payment the grant may pay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// The ISO 8601 repeating interval the amounts apply to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// The most that may be debited (per interval).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debit_amount: Option<Amount>,
    /// The most that may be received (per interval).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_amount: Option<Amount>,
}

/// One access item (`access-item`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Access {
    /// The kind of resource.
    #[serde(rename = "type")]
    pub kind: AccessType,
    /// What may be done.
    pub actions: Vec<Action>,
    /// The wallet address the access is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    /// Limits, for outgoing payments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    /// Card authorization (v1.4.0), which the node does not offer.
    #[serde(default, skip_serializing)]
    pub card_authorization: Option<serde_json::Value>,
}

impl Access {
    fn validate(&self) -> Result<(), ErrorCode> {
        let allowed: &[Action] = match self.kind {
            AccessType::IncomingPayment => &[
                Action::Create,
                Action::Complete,
                Action::Read,
                Action::ReadAll,
                Action::List,
                Action::ListAll,
            ],
            AccessType::OutgoingPayment => &[
                Action::Create,
                Action::Read,
                Action::ReadAll,
                Action::List,
                Action::ListAll,
            ],
            AccessType::Quote => &[Action::Create, Action::Read, Action::ReadAll],
        };
        let actions_valid = !self.actions.is_empty()
            && self.actions.iter().all(|action| allowed.contains(action))
            && unique(&self.actions);
        let shape_valid = match self.kind {
            AccessType::IncomingPayment => self.limits.is_none(),
            AccessType::OutgoingPayment => self.identifier.is_some(),
            AccessType::Quote => self.identifier.is_none() && self.limits.is_none(),
        };
        let identifier_valid = self
            .identifier
            .as_deref()
            .is_none_or(|identifier| Uri::parse(identifier).is_ok());
        if !actions_valid || !shape_valid || !identifier_valid || self.card_authorization.is_some()
        {
            return Err(ErrorCode::InvalidRequest);
        }
        self.limits.as_ref().map_or(Ok(()), Limits::validate)
    }
}

impl Limits {
    fn validate(&self) -> Result<(), ErrorCode> {
        let receiver_valid = self.receiver.as_deref().is_none_or(is_receiver);
        let amounts_valid = [&self.debit_amount, &self.receive_amount]
            .into_iter()
            .flatten()
            .all(|amount| amount.value > 0);
        let interval_valid = self.interval.as_deref().is_none_or(|interval| {
            Interval::parse(interval).is_ok_and(|interval| interval.windows_are_long_enough())
        });
        if receiver_valid && amounts_valid && interval_valid {
            Ok(())
        } else {
            Err(ErrorCode::InvalidRequest)
        }
    }

    /// The window of the limits' interval that holds `now_ms`, as
    /// `(start, end)`: all time without an interval, and `None` when no
    /// window holds it, which no payment may be made in.
    #[must_use]
    pub fn window(&self, now_ms: u64) -> Option<(u64, u64)> {
        match self.interval.as_deref() {
            None => Some((0, u64::MAX)),
            Some(text) => Interval::parse(text).ok()?.window(now_ms),
        }
    }

    /// Whether a payment debiting `debit` and delivering `receive` fits,
    /// on top of what the window has spent: `spent` as `(debit, receive)`.
    /// The asset of each limit is the caller's to check.
    #[must_use]
    pub fn admit(&self, spent: (u64, u64), debit: u64, receive: u64) -> bool {
        let fits = |limit: &Option<Amount>, spent: u64, more: u64| {
            limit
                .as_ref()
                .is_none_or(|limit| u128::from(spent) + u128::from(more) <= u128::from(limit.value))
        };
        fits(&self.debit_amount, spent.0, debit) && fits(&self.receive_amount, spent.1, receive)
    }
}

/// Whether a URL is an incoming payment's, as `receiver` must be.
#[must_use]
pub fn is_receiver(url: &str) -> bool {
    Uri::parse(url).is_ok_and(|uri| {
        uri.query.is_none()
            && uri.fragment.is_none()
            && uri
                .path
                .split_once("/incoming-payments/")
                .is_some_and(|(_, id)| !id.is_empty())
    })
}

fn unique<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .all(|(index, item)| !items.get(..index).unwrap_or_default().contains(item))
}

/// How the client identifies itself (`client`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Client {
    /// By wallet address, whose `jwks.json` holds its keys.
    WalletAddress(ClientWalletAddress),
    /// By key alone ("directed identity"), for non-interactive grants.
    Key(ClientKey),
    /// By wallet address as a bare string (deprecated).
    Legacy(String),
}

/// A client named by its wallet address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientWalletAddress {
    /// The client's wallet address.
    #[serde(rename = "walletAddress")]
    pub wallet_address: String,
}

/// A client known only by its key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientKey {
    /// The client's key.
    pub jwk: Jwk,
}

impl Client {
    /// The client's wallet address, if it named one.
    #[must_use]
    pub fn wallet_address(&self) -> Option<&str> {
        match self {
            Self::WalletAddress(ClientWalletAddress { wallet_address })
            | Self::Legacy(wallet_address) => Some(wallet_address),
            Self::Key(_) => None,
        }
    }
}

/// How the client can send the owner to the authorization server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractRequest {
    /// Must include `redirect`.
    pub start: Vec<String>,
    /// How to send the owner back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<Finish>,
}

/// The client's return address and nonce.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finish {
    /// Always `redirect`.
    pub method: String,
    /// Where to send the owner when they have decided.
    pub uri: String,
    /// The client's nonce for the interaction hash.
    pub nonce: String,
}

/// Which identities the client asks about (`subject`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    /// Exactly one identity.
    pub sub_ids: Vec<SubjectId>,
}

/// An identity the client asks about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectId {
    /// The identity: a wallet address.
    pub id: String,
    /// Always `uri`.
    pub format: String,
}

/// The `access_token` member of a grant request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessTokenRequest {
    /// The access asked for.
    pub access: Vec<Access>,
}

/// A grant request (`POST /` of the authorization server).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRequest {
    /// The access asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<AccessTokenRequest>,
    /// Who asks.
    pub client: Client,
    /// How the owner can be involved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interact: Option<InteractRequest>,
    /// The identities asked about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
}

impl GrantRequest {
    /// The access items asked for.
    #[must_use]
    pub fn access(&self) -> &[Access] {
        self.access_token
            .as_ref()
            .map_or(&[], |token| token.access.as_slice())
    }

    /// Checks the request against the specs and this server's rules.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::InvalidRequest`] for a malformed request,
    /// [`ErrorCode::InvalidClient`] for a key-only client that needs the
    /// owner.
    pub fn validate(&self) -> Result<(), ErrorCode> {
        let access = self.access();
        let empty_access = self
            .access_token
            .as_ref()
            .is_some_and(|token| token.access.is_empty());
        if (self.access_token.is_none() && self.subject.is_none())
            || empty_access
            || access.len() > MAX_ACCESS_ITEMS
            || !unique(access)
        {
            return Err(ErrorCode::InvalidRequest);
        }
        access.iter().try_for_each(Access::validate)?;
        if let Some(wallet_address) = self.client.wallet_address()
            && Uri::parse(wallet_address).is_err()
        {
            return Err(ErrorCode::InvalidClient);
        }
        self.validate_subject()?;
        self.validate_interact()?;
        if self.needs_owner() {
            if self.interact.is_none() {
                return Err(ErrorCode::InvalidRequest);
            }
            if matches!(self.client, Client::Key(_)) {
                return Err(ErrorCode::InvalidClient);
            }
        }
        Ok(())
    }

    fn validate_subject(&self) -> Result<(), ErrorCode> {
        let Some(subject) = &self.subject else {
            return Ok(());
        };
        match subject.sub_ids.as_slice() {
            [only] if only.format == "uri" && Uri::parse(&only.id).is_ok() => Ok(()),
            _ => Err(ErrorCode::InvalidRequest),
        }
    }

    fn validate_interact(&self) -> Result<(), ErrorCode> {
        let Some(interact) = &self.interact else {
            return Ok(());
        };
        let start_valid = interact.start.iter().any(|start| start == "redirect");
        let finish_valid = interact.finish.as_ref().is_none_or(|finish| {
            finish.method == "redirect"
                && Uri::parse(&finish.uri).is_ok_and(|uri| uri.fragment.is_none())
                && !finish.nonce.is_empty()
                && finish.nonce.len() <= MAX_TOKEN_TEXT_BYTES
        });
        if start_valid && finish_valid {
            Ok(())
        } else {
            Err(ErrorCode::InvalidRequest)
        }
    }

    /// Whether the owner must approve: for outgoing payments, for proof of
    /// a wallet address, for listing what others created, and for reading
    /// others' quotes. Reading any incoming payment needs no owner: that is
    /// how a sender's wallet learns how to pay a payment it did not create,
    /// and all it shows is what paying needs.
    #[must_use]
    pub fn needs_owner(&self) -> bool {
        self.subject.is_some()
            || self.access().iter().any(|access| {
                access.kind == AccessType::OutgoingPayment
                    || access.actions.contains(&Action::ListAll)
                    || (access.kind == AccessType::Quote
                        && access.actions.contains(&Action::ReadAll))
            })
    }
}

/// Whose resources an access token reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    /// Only those its client created.
    Own,
    /// All of the wallet address's.
    All,
}

/// What one access item allows for an action: whose resources, and, for
/// outgoing payments, those to its limits' receiver only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reach<'a> {
    /// Whose resources.
    pub scope: Scope,
    /// The only incoming payment the resources may pay.
    pub receiver: Option<&'a str>,
}

impl Reach<'_> {
    /// Whether it reaches a resource the grant's client made or not
    /// (`own`), paying `receiver` (none for resources that pay none).
    #[must_use]
    pub fn covers(&self, own: bool, receiver: Option<&str>) -> bool {
        (self.scope == Scope::All || own) && self.receiver.is_none_or(|only| Some(only) == receiver)
    }
}

/// What a grant's access allows for `action` on resources of `kind` of
/// one wallet address, item by item: at most [`MAX_ACCESS_ITEMS`]. The
/// `*-all` actions include their plain forms. Creating reaches the whole
/// wallet address; completing, which has no `-all` form, only what the
/// client created. Each item's limits limit what it allows, and no other
/// item's.
#[must_use]
pub fn reaches<'a>(
    access: &'a [Access],
    kind: AccessType,
    action: Action,
    wallet_address: &str,
) -> Vec<Reach<'a>> {
    let broader = match action {
        Action::Read => Some(Action::ReadAll),
        Action::List => Some(Action::ListAll),
        _ => None,
    };
    let narrow = if broader.is_some() || action == Action::Complete {
        Scope::Own
    } else {
        Scope::All
    };
    access
        .iter()
        .take(MAX_ACCESS_ITEMS)
        .filter(|item| item.kind == kind)
        .filter(|item| {
            item.identifier
                .as_deref()
                .is_none_or(|identifier| identifier == wallet_address)
        })
        .filter_map(|item| {
            let scope = if broader.is_some_and(|broader| item.actions.contains(&broader)) {
                Scope::All
            } else if item.actions.contains(&action) {
                narrow
            } else {
                return None;
            };
            let receiver = item
                .limits
                .as_ref()
                .and_then(|limits| limits.receiver.as_deref());
            Some(Reach { scope, receiver })
        })
        .collect()
}

/// What a grant's access allows for `action` on resources of `kind` of
/// one wallet address, as [`reaches`], at its widest: nothing, its
/// client's own resources, or all, whatever the receiver.
#[must_use]
pub fn permits(
    access: &[Access],
    kind: AccessType,
    action: Action,
    wallet_address: &str,
) -> Option<Scope> {
    reaches(access, kind, action, wallet_address)
        .iter()
        .map(|reach| reach.scope)
        .max()
}

/// The item that allows creating outgoing payments from `wallet_address`:
/// its limits are those payments'. A grant has one at most.
#[must_use]
pub fn creating<'a>(access: &'a [Access], wallet_address: &str) -> Option<&'a Access> {
    access.iter().find(|item| {
        item.kind == AccessType::OutgoingPayment
            && item.actions.contains(&Action::Create)
            && item.identifier.as_deref() == Some(wallet_address)
    })
}

/// Whether two items allow creating outgoing payments from one wallet
/// address: which limits apply would be unclear. Compare canonical wallet
/// addresses.
#[must_use]
pub fn creates_twice(access: &[Access]) -> bool {
    let creating: Vec<Option<&str>> = access
        .iter()
        .filter(|item| {
            item.kind == AccessType::OutgoingPayment && item.actions.contains(&Action::Create)
        })
        .map(|item| item.identifier.as_deref())
        .collect();
    !unique(&creating)
}

/// An access token in a response (`access_token`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessToken {
    /// The token.
    pub value: String,
    /// The URL to rotate or revoke it at.
    pub manage: String,
    /// Seconds until it expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    /// What it permits.
    pub access: Vec<Access>,
}

/// The continuation token alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueToken {
    /// The token.
    pub value: String,
}

/// How to continue a grant (`continue`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Continue {
    /// The token to continue with.
    pub access_token: ContinueToken,
    /// Where to continue.
    pub uri: String,
    /// Seconds to wait before continuing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<u64>,
}

/// Where to send the owner (`interact-response`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractResponse {
    /// The URL to send the owner to.
    pub redirect: String,
    /// The server's nonce for the interaction hash.
    pub finish: String,
}

/// A response of `POST /` or `POST /continue/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantResponse {
    /// Where to send the owner, while the grant waits for them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interact: Option<InteractResponse>,
    /// The token, once the grant is approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<AccessToken>,
    /// How to continue.
    #[serde(rename = "continue")]
    pub continuation: Continue,
    /// The proven identities, for a subject request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
}

/// The body of `POST /continue/{id}` (`continuation-request`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinueRequest {
    /// The reference the owner's redirect carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interact_ref: Option<String>,
}

/// The interaction hash (RFC 9635 §4.2.3): SHA-256 over the client's nonce,
/// the server's nonce, the interaction reference and the grant endpoint,
/// one per line, as unpadded base64url.
#[must_use]
pub fn interaction_hash(
    client_nonce: &str,
    server_nonce: &str,
    interact_ref: &str,
    grant_uri: &str,
) -> String {
    URL_SAFE_NO_PAD.encode(hash_input(
        client_nonce,
        server_nonce,
        interact_ref,
        grant_uri,
    ))
}

/// Checks an interaction hash. Padded standard base64 is accepted too,
/// since some servers send that.
#[must_use]
pub fn check_interaction_hash(
    hash: &str,
    client_nonce: &str,
    server_nonce: &str,
    interact_ref: &str,
    grant_uri: &str,
) -> bool {
    let expected = hash_input(client_nonce, server_nonce, interact_ref, grant_uri);
    let decoded = URL_SAFE_NO_PAD
        .decode(hash)
        .or_else(|_| STANDARD.decode(hash))
        .unwrap_or_default();
    decoded.len() == expected.len() && bool::from(decoded.ct_eq(&expected))
}

fn hash_input(
    client_nonce: &str,
    server_nonce: &str,
    interact_ref: &str,
    grant_uri: &str,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for (index, part) in [client_nonce, server_nonce, interact_ref, grant_uri]
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            hasher.update(b"\n");
        }
        hasher.update(part.as_bytes());
    }
    hasher.finalize().into()
}

/// A `WWW-Authenticate` value pointing at the grant endpoint.
#[must_use]
pub fn www_authenticate(grant_uri: &str) -> String {
    let mut value = "GNAP as_uri=".to_string();
    value.push_str(grant_uri);
    value
}

/// The token of an `Authorization: GNAP <token>` header.
#[must_use]
pub fn bearer(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.split_once(' ')?;
    let token = token.trim();
    let valid = scheme == "GNAP"
        && !token.is_empty()
        && token.len() <= MAX_TOKEN_TEXT_BYTES
        && token.bytes().all(|byte| byte.is_ascii_graphic());
    valid.then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(json: &str) -> GrantRequest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn computes_the_documented_hash() {
        // The example of the Open Payments docs (identity/hash-verification).
        let hash = interaction_hash(
            "VJLO6A4CATR0KRO",
            "MBDOFXG4Y5CVJCX821LH",
            "4IFWWIKYB2PQ6U56NL1",
            "https://server.example.com/tx",
        );
        assert_eq!(hash, "x-gguKWTj8rQf7d7i3w3UhzvuJ5bpOlKyAlVpLxBffY");
        let parts = [
            "VJLO6A4CATR0KRO",
            "MBDOFXG4Y5CVJCX821LH",
            "4IFWWIKYB2PQ6U56NL1",
            "https://server.example.com/tx",
        ];
        let [a, b, c, d] = parts;
        assert!(check_interaction_hash(&hash, a, b, c, d));
        assert!(check_interaction_hash(
            "x+gguKWTj8rQf7d7i3w3UhzvuJ5bpOlKyAlVpLxBffY=",
            a,
            b,
            c,
            d
        ));
        assert!(!check_interaction_hash(&hash, a, b, "other", d));
    }

    #[test]
    fn an_incoming_payment_grant_needs_no_owner() {
        let grant = request(
            r#"{"access_token":{"access":[{"type":"incoming-payment","actions":["create","read"]}]},"client":"https://a.example/pay"}"#,
        );
        assert_eq!(grant.validate(), Ok(()));
        assert!(!grant.needs_owner());
        assert_eq!(grant.client.wallet_address(), Some("https://a.example/pay"));
    }

    #[test]
    fn an_outgoing_payment_grant_needs_the_owner() {
        let json = r#"{"access_token":{"access":[{"type":"outgoing-payment","actions":["create","read"],"identifier":"https://b.example/.well-known/pay","limits":{"debitAmount":{"value":"500","assetCode":"USD","assetScale":2},"interval":"R/2022-08-24T14:15:22Z/P1M"}}]},"client":{"walletAddress":"https://a.example/pay"},"interact":{"start":["redirect"],"finish":{"method":"redirect","uri":"https://a.example/done","nonce":"n"}}}"#;
        let grant = request(json);
        assert_eq!(grant.validate(), Ok(()));
        assert!(grant.needs_owner());
        let without_interact = json.replace(
            r#","interact":{"start":["redirect"],"finish":{"method":"redirect","uri":"https://a.example/done","nonce":"n"}}"#,
            "",
        );
        assert_eq!(
            request(&without_interact).validate(),
            Err(ErrorCode::InvalidRequest)
        );
    }

    #[test]
    fn refuses_malformed_access() {
        for access in [
            r#"{"type":"quote","actions":["list"]}"#,
            r#"{"type":"quote","actions":["create"],"identifier":"https://a.example/"}"#,
            r#"{"type":"incoming-payment","actions":[]}"#,
            r#"{"type":"incoming-payment","actions":["read","read"]}"#,
            r#"{"type":"outgoing-payment","actions":["create"]}"#,
            r#"{"type":"outgoing-payment","actions":["create"],"identifier":"https://b.example/","cardAuthorization":{"tlvData":"00","requestId":"1"}}"#,
            r#"{"type":"outgoing-payment","actions":["create"],"identifier":"https://b.example/","limits":{"receiver":"https://c.example/x"}}"#,
        ] {
            let json = alloc::format!(
                r#"{{"access_token":{{"access":[{access}]}},"client":"https://a.example/pay","interact":{{"start":["redirect"]}}}}"#
            );
            let parsed = serde_json::from_str::<GrantRequest>(&json);
            assert!(
                parsed.is_err() || parsed.unwrap().validate() == Err(ErrorCode::InvalidRequest),
                "{access}"
            );
        }
    }

    #[test]
    fn reading_any_incoming_payment_needs_no_owner_but_listing_does() {
        let read_all = request(
            r#"{"access_token":{"access":[{"type":"incoming-payment","actions":["read-all"]}]},"client":"https://a.example/pay"}"#,
        );
        assert_eq!(read_all.validate(), Ok(()));
        assert!(!read_all.needs_owner());
        let empty = request(r#"{"access_token":{"access":[]},"client":"https://a.example/pay"}"#);
        assert_eq!(empty.validate(), Err(ErrorCode::InvalidRequest));
    }

    #[test]
    fn a_bare_key_cannot_ask_for_the_owner() {
        let jwk = r#"{"kid":"k","alg":"EdDSA","kty":"OKP","crv":"Ed25519","x":"AAAA"}"#;
        let json = alloc::format!(
            r#"{{"access_token":{{"access":[{{"type":"incoming-payment","actions":["list-all"]}}]}},"client":{{"jwk":{jwk}}},"interact":{{"start":["redirect"]}}}}"#
        );
        assert_eq!(request(&json).validate(), Err(ErrorCode::InvalidClient));
    }

    #[test]
    fn permits_by_kind_action_and_wallet_address() {
        let access: Vec<Access> = serde_json::from_str(
            r#"[{"type":"incoming-payment","actions":["create","read"],"identifier":"https://b.example/pay"},{"type":"quote","actions":["read-all"]}]"#,
        )
        .unwrap();
        let ip = AccessType::IncomingPayment;
        assert_eq!(
            permits(&access, ip, Action::Create, "https://b.example/pay"),
            Some(Scope::All)
        );
        assert_eq!(
            permits(&access, ip, Action::Read, "https://b.example/pay"),
            Some(Scope::Own)
        );
        assert_eq!(
            permits(&access, ip, Action::Read, "https://b.example/other"),
            None
        );
        assert_eq!(
            permits(&access, ip, Action::List, "https://b.example/pay"),
            None
        );
        assert_eq!(
            permits(&access, AccessType::Quote, Action::Read, "x"),
            Some(Scope::All)
        );
        let complete: Vec<Access> =
            serde_json::from_str(r#"[{"type":"incoming-payment","actions":["complete"]}]"#)
                .unwrap();
        assert_eq!(
            permits(&complete, ip, Action::Complete, "x"),
            Some(Scope::Own)
        );
    }

    // Each item's limits limit what it allows: a receiver on an item that
    // reads narrows what it reads, and the item that creates has its own.
    #[test]
    fn each_item_reaches_within_its_own_limits() {
        let access: Vec<Access> = serde_json::from_str(
            r#"[
                {"type":"outgoing-payment","actions":["read-all","list-all"],"identifier":"https://b.example/pay",
                 "limits":{"receiver":"https://c.example/incoming-payments/1"}},
                {"type":"outgoing-payment","actions":["create","read"],"identifier":"https://b.example/pay",
                 "limits":{"debitAmount":{"value":"10","assetCode":"USD","assetScale":2}}}
            ]"#,
        )
        .unwrap();
        let op = AccessType::OutgoingPayment;
        let wallet = "https://b.example/pay";
        let read = reaches(&access, op, Action::Read, wallet);
        let to = "https://c.example/incoming-payments/1";
        let other = "https://c.example/incoming-payments/2";
        assert!(read.iter().any(|reach| reach.covers(false, Some(to))));
        assert!(!read.iter().any(|reach| reach.covers(false, Some(other))));
        assert!(read.iter().any(|reach| reach.covers(true, Some(other))));
        let list = reaches(&access, op, Action::List, wallet);
        assert_eq!(
            list,
            [Reach {
                scope: Scope::All,
                receiver: Some(to)
            }]
        );
        let limits = creating(&access, wallet).unwrap().limits.as_ref().unwrap();
        assert_eq!(limits.debit_amount.as_ref().unwrap().value, 10);
        assert!(creating(&access, "https://b.example/other").is_none());
        assert!(!creates_twice(&access));
        let twice: Vec<Access> = serde_json::from_str(
            r#"[
                {"type":"outgoing-payment","actions":["create"],"identifier":"https://b.example/pay"},
                {"type":"outgoing-payment","actions":["create","read"],"identifier":"https://b.example/pay"}
            ]"#,
        )
        .unwrap();
        assert!(creates_twice(&twice));
    }

    #[test]
    fn reads_authorization_headers() {
        assert_eq!(bearer("GNAP abc"), Some("abc"));
        assert_eq!(bearer("Bearer abc"), None);
        assert_eq!(bearer("GNAP "), None);
        assert_eq!(bearer("GNAP"), None);
    }
}

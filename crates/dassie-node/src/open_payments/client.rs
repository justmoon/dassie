//! The node as an Open Payments client of other wallets (D9): reading the
//! incoming payment it is to pay.
//!
//! An incoming payment's `methods`, the STREAM credentials, are readable
//! only with a grant. The node asks the payment's authorization server for
//! a non-interactive `incoming-payment` `read-all` grant, as its own
//! `$host` wallet address, whose `jwks.json` publishes its key, and signs
//! every request with that key. Tokens are kept per authorization server
//! until they expire, in a bounded cache. Requests go through the guarded
//! HTTPS client: public addresses only, no redirects, bounded bodies.

use std::collections::BTreeMap;
use std::sync::Mutex;

use dassie_payments::gnap::{
    Access, AccessTokenRequest, AccessType, Action, Client, ClientWalletAddress, GrantRequest,
    GrantResponse,
};
use dassie_payments::httpsig;
use dassie_payments::resources::{IncomingPayment, PublicIncomingPayment, WalletAddress};

use super::Urls;
use crate::node::Node;
use crate::peer_client::{PeerClient, WebError, WebRequest, WebResponse};

/// How long a request to another wallet may take.
const TIMEOUT_MS: u64 = 10_000;
/// The longest response read from another wallet.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Most authorization servers whose tokens are kept.
const MAX_CACHED_TOKENS: usize = 64;
/// How long before its expiry a cached token is no longer used.
const TOKEN_MARGIN_MS: u64 = 30_000;
/// The longest a cached token is kept, whatever its server says.
const MAX_TOKEN_LIFETIME_MS: u64 = 60 * 60 * 1_000;

/// Why another wallet could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientError {
    /// The request did not get through, or took too long.
    Unreachable(String),
    /// The other server answered, but not as the specs say.
    Invalid(String),
    /// The other server refused, with this status.
    Refused(u16),
    /// The node has no hostname or key to act as a client with.
    Unavailable,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(error) => write!(f, "the other wallet cannot be reached: {error}"),
            Self::Invalid(what) => write!(f, "the other wallet answered unexpectedly: {what}"),
            Self::Refused(status) => write!(f, "the other wallet refused, with {status}"),
            Self::Unavailable => f.write_str("the node cannot act as a client yet"),
        }
    }
}

impl From<WebError> for ClientError {
    fn from(error: WebError) -> Self {
        Self::Unreachable(error.to_string())
    }
}

/// Access tokens per authorization server, with their expiry.
#[derive(Debug, Default)]
pub struct TokenCache {
    tokens: BTreeMap<String, (String, u64)>,
}

impl TokenCache {
    fn get(&self, auth_server: &str, now_ms: u64) -> Option<String> {
        self.tokens
            .get(auth_server)
            .filter(|(_, expires)| now_ms + TOKEN_MARGIN_MS < *expires)
            .map(|(token, _)| token.clone())
    }

    fn put(&mut self, auth_server: String, token: String, expires_ms: u64, now_ms: u64) {
        self.tokens.retain(|_, (_, expires)| now_ms < *expires);
        if self.tokens.len() >= MAX_CACHED_TOKENS
            && let Some(soonest) = self
                .tokens
                .iter()
                .min_by_key(|(_, (_, expires))| *expires)
                .map(|(server, _)| server.clone())
        {
            self.tokens.remove(&soonest);
        }
        self.tokens.insert(auth_server, (token, expires_ms));
    }

    fn forget(&mut self, auth_server: &str) {
        self.tokens.remove(auth_server);
    }
}

/// A GET for JSON, without a token.
async fn get_json<T: serde::de::DeserializeOwned>(
    client: &PeerClient,
    url: &str,
) -> Result<T, ClientError> {
    let response = client
        .web(WebRequest {
            method: http::Method::GET,
            url: url.to_string(),
            headers: vec![(http::header::ACCEPT, "application/json".into())],
            body: None,
            timeout_ms: TIMEOUT_MS,
            max_response_bytes: MAX_RESPONSE_BYTES,
        })
        .await?;
    parse(&response, 200)
}

fn parse<T: serde::de::DeserializeOwned>(
    response: &WebResponse,
    status: u16,
) -> Result<T, ClientError> {
    if response.status != status {
        return Err(ClientError::Refused(response.status));
    }
    serde_json::from_slice(&response.body).map_err(|error| ClientError::Invalid(error.to_string()))
}

/// Reads a wallet address.
///
/// # Errors
///
/// If it cannot be read, or is not one.
pub async fn fetch_wallet_address(
    client: &PeerClient,
    url: &str,
) -> Result<WalletAddress, ClientError> {
    get_json(client, url).await
}

/// A request signed with the node's key, as the node's `$host` client.
async fn signed(
    node: &Node,
    method: http::Method,
    url: &str,
    token: Option<&str>,
    body: Option<Vec<u8>>,
) -> Result<WebResponse, ClientError> {
    let client = node.peer_client().ok_or(ClientError::Unavailable)?;
    let node_key = node.node_key().ok_or(ClientError::Unavailable)?;
    let key = node
        .open_payments()
        .client_key(&node_key)
        .ok_or(ClientError::Unavailable)?;
    let mut fields: Vec<(String, String)> = vec![("accept".into(), "application/json".into())];
    if let Some(token) = token {
        fields.push(("authorization".into(), format!("GNAP {token}")));
    }
    if let Some(body) = &body {
        fields.push(("content-type".into(), "application/json".into()));
        fields.push(("content-length".into(), body.len().to_string()));
        fields.push(("content-digest".into(), httpsig::content_digest(body)));
    }
    let refs: Vec<(&str, &str)> = fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let request = httpsig::Request {
        method: method.as_str(),
        target_uri: url,
        fields: &refs,
    };
    let components = httpsig::profile(&request, body.is_some());
    let signature = httpsig::sign(
        &request,
        &components,
        key.signing_key(),
        key.kid(),
        node.now_ms() / 1_000,
    )
    .map_err(|error| ClientError::Invalid(format!("signing: {error:?}")))?;
    fields.push(("signature-input".into(), signature.signature_input));
    fields.push(("signature".into(), signature.signature));
    let headers = fields
        .into_iter()
        .filter(|(name, _)| name != "content-length")
        .filter_map(|(name, value)| {
            Some((http::HeaderName::from_bytes(name.as_bytes()).ok()?, value))
        })
        .collect();
    Ok(client
        .web(WebRequest {
            method,
            url: url.to_string(),
            headers,
            body,
            timeout_ms: TIMEOUT_MS,
            max_response_bytes: MAX_RESPONSE_BYTES,
        })
        .await?)
}

/// A token to read incoming payments at `auth_server` for resources at
/// `origin`: cached, or asked for anew. The cache keys by both, so a
/// payment that names another wallet's authorization server is never
/// sent that wallet's token.
async fn read_token(
    node: &Node,
    urls: &Urls,
    auth_server: &str,
    origin: &str,
) -> Result<String, ClientError> {
    let now = node.now_ms();
    let key = format!("{auth_server} {origin}");
    if let Some(token) = crate::node::lock(&node.open_payments().tokens).get(&key, now) {
        return Ok(token);
    }
    let request = GrantRequest {
        access_token: Some(AccessTokenRequest {
            access: vec![Access {
                kind: AccessType::IncomingPayment,
                actions: vec![Action::ReadAll],
                identifier: None,
                limits: None,
                card_authorization: None,
            }],
        }),
        client: Client::WalletAddress(ClientWalletAddress {
            wallet_address: format!("{}{}", urls.origin(), super::WELL_KNOWN_PATH),
        }),
        interact: None,
        subject: None,
    };
    let body =
        serde_json::to_vec(&request).map_err(|error| ClientError::Invalid(error.to_string()))?;
    let response = signed(node, http::Method::POST, auth_server, None, Some(body)).await?;
    let grant: GrantResponse = parse(&response, 200)?;
    let token = grant
        .access_token
        .ok_or_else(|| ClientError::Invalid("the grant needs interaction".into()))?;
    let lifetime = token
        .expires_in
        .unwrap_or(60)
        .saturating_mul(1_000)
        .min(MAX_TOKEN_LIFETIME_MS);
    crate::node::lock(&node.open_payments().tokens).put(
        key,
        token.value.clone(),
        now.saturating_add(lifetime),
        now,
    );
    Ok(token.value)
}

/// Reads an incoming payment on another node, with the methods to pay it.
///
/// # Errors
///
/// If it cannot be read, the grant is refused, or it has no ILP method.
pub async fn read_incoming(node: &Node, url: &str) -> Result<IncomingPayment, ClientError> {
    let urls = Urls::of(node).ok_or(ClientError::Unavailable)?;
    let client = node.peer_client().ok_or(ClientError::Unavailable)?;
    let public: PublicIncomingPayment = get_json(client, url).await?;
    let origin = dassie_payments::uri::Uri::parse(url)
        .map(|uri| format!("{}://{}", uri.scheme, uri.authority))
        .map_err(|_| ClientError::Invalid("the receiver is not a URL".into()))?;
    let mut retried = false;
    loop {
        let token = read_token(node, &urls, &public.auth_server, &origin).await?;
        let response = signed(node, http::Method::GET, url, Some(&token), None).await?;
        if response.status == 401 && !retried {
            // The token was revoked or rotated away: ask once more.
            crate::node::lock(&node.open_payments().tokens)
                .forget(&format!("{} {origin}", public.auth_server));
            retried = true;
            continue;
        }
        let payment: IncomingPayment = parse(&response, 200)?;
        if payment.id != url {
            return Err(ClientError::Invalid("the payment names another URL".into()));
        }
        return Ok(payment);
    }
}

/// The client's state beside the node: tokens per authorization server.
pub type Tokens = Mutex<TokenCache>;

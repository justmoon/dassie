//! HTTP message signatures (RFC 9421) as Open Payments profiles them:
//! Ed25519 over `@method`, `@target-uri`, `authorization` when present and
//! `content-digest` when there is a body, with `keyid` and `created`; and
//! `Content-Digest` (RFC 9530).
//!
//! Verifying takes two steps, so that the caller can fetch the key in
//! between: [`Signature::find`] picks the signature, checks its components
//! and time and names the key; [`Signature::verify`] checks it with that
//! key.
//!
//! A covered `content-length` that the transport dropped, as clients do
//! that send their body chunked, is the length of the body received. The
//! digest, which a body needs covered, binds the body itself.
//!
//! A node signs its own responses too, with its node key under
//! [`RESPONSE_TAG`], so that a Dassie-aware client can tell that the answer
//! came from the node, whoever passed it on
//! (spec/areas/payments.md#signed-responses): [`sign_response`] and
//! [`ResponseSignature`], and [`check_node_response`], which also checks the
//! key against the node the client meant to reach.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use dassie_identity::{NodeId, base64url};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest as _, Sha256, Sha512};
use subtle::ConstantTimeEq as _;

use crate::sf::{self, BareItem, Item, Member, Parameters};
use crate::uri::Uri;

/// The label Dassie signs under, as the reference client does.
pub const LABEL: &str = "sig1";
/// How far `created` may be from the verifier's clock, in seconds.
pub const MAX_CLOCK_SKEW_S: u64 = 300;
/// The most header lines of one name that are joined into a value.
pub const MAX_FIELD_LINES: usize = 16;
/// The `tag` of a node's signature on its responses.
pub const RESPONSE_TAG: &str = "dassie";
/// The request field with the client's nonce, which a response's signature
/// covers so that an old response cannot be replayed.
pub const NONCE_FIELD: &str = "dassie-nonce";
/// The algorithm a node's response signature names.
const ALGORITHM: &str = "ed25519";

/// A request, as signing and verifying see it.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    /// The method, in upper case.
    pub method: &'a str,
    /// The absolute URL of the request.
    pub target_uri: &'a str,
    /// Header lines as `(name, value)`; names in any case.
    pub fields: &'a [(&'a str, &'a str)],
}

impl Request<'_> {
    fn has(&self, name: &str) -> bool {
        has(self.fields, name)
    }
}

/// A response, as signing and verifying see it.
#[derive(Clone, Copy, Debug)]
pub struct Response<'a> {
    /// The status code.
    pub status: u16,
    /// Header lines as `(name, value)`; names in any case.
    pub fields: &'a [(&'a str, &'a str)],
}

/// All lines of a field, trimmed and joined with `, ` (RFC 9421 §2.1).
fn field(fields: &[(&str, &str)], name: &str) -> Option<String> {
    let mut value: Option<String> = None;
    let lines = fields
        .iter()
        .filter(|(field, _)| field.eq_ignore_ascii_case(name))
        .take(MAX_FIELD_LINES);
    for (_, line) in lines {
        let line = line.trim_matches([' ', '\t']);
        match value.as_mut() {
            Some(joined) => {
                joined.push_str(", ");
                joined.push_str(line);
            }
            None => value = Some(line.to_string()),
        }
    }
    value
}

fn has(fields: &[(&str, &str)], name: &str) -> bool {
    fields
        .iter()
        .any(|(field, _)| field.eq_ignore_ascii_case(name))
}

/// The message a signature is on: a request, or a response to one, whose
/// components marked `req` are the request's.
#[derive(Clone, Copy, Debug)]
struct Message<'a> {
    request: &'a Request<'a>,
    response: Option<&'a Response<'a>>,
}

impl<'a> Message<'a> {
    const fn request(request: &'a Request<'a>) -> Self {
        Self {
            request,
            response: None,
        }
    }

    const fn response(request: &'a Request<'a>, response: &'a Response<'a>) -> Self {
        Self {
            request,
            response: Some(response),
        }
    }

    /// The signed message's own fields, which carry its signatures.
    fn fields(&self) -> &'a [(&'a str, &'a str)] {
        self.response
            .map_or(self.request.fields, |response| response.fields)
    }
}

/// Why a signature was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// No `Signature-Input` or `Signature` field.
    Missing,
    /// A field is malformed, or a component cannot be computed.
    Malformed,
    /// No signature covers every component the profile requires.
    Components,
    /// `created` is missing or too far from now, or `expires` has passed.
    Time,
    /// `alg` names something other than Ed25519.
    Algorithm,
    /// The signature does not verify under the key.
    Invalid,
    /// `Content-Digest` is missing, unsupported or does not match the body.
    Digest,
}

/// The components Open Payments wants signed for this request.
#[must_use]
pub fn profile(request: &Request<'_>, has_body: bool) -> Vec<&'static str> {
    let mut components = Vec::from(["@method", "@target-uri"]);
    if request.has("authorization") {
        components.push("authorization");
    }
    if has_body {
        components.extend(["content-digest", "content-length", "content-type"]);
    }
    components
}

/// The `Content-Digest` value for a body: its SHA-512.
#[must_use]
pub fn content_digest(body: &[u8]) -> String {
    let digest = Sha512::digest(body);
    sf::serialize_byte_dictionary(&[("sha-512", digest.as_slice())])
}

/// The two header values a signature adds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// The `Signature-Input` value.
    pub signature_input: String,
    /// The `Signature` value.
    pub signature: String,
}

/// A component identifier: a name, and `;req` when it is the request's.
fn item(name: &str, req: bool) -> Item {
    let mut parameters = Parameters::new();
    if req {
        parameters.push(("req".to_string(), BareItem::Boolean(true)));
    }
    Item {
        bare: BareItem::String(name.to_string()),
        parameters,
    }
}

/// Signs `components` of a request under [`LABEL`].
///
/// # Errors
///
/// [`SignatureError::Malformed`] if a component cannot be computed, such
/// as a header the request lacks.
pub fn sign(
    request: &Request<'_>,
    components: &[&str],
    key: &SigningKey,
    keyid: &str,
    created_s: u64,
) -> Result<Signed, SignatureError> {
    let items: Vec<Item> = components.iter().map(|&name| item(name, false)).collect();
    let created = i64::try_from(created_s).map_err(|_| SignatureError::Malformed)?;
    let parameters = Vec::from([
        ("created".to_string(), BareItem::Integer(created)),
        ("keyid".to_string(), BareItem::String(keyid.to_string())),
    ]);
    sign_items(Message::request(request), &items, &parameters, |base| {
        key.sign(base).to_bytes()
    })
}

fn sign_items(
    message: Message<'_>,
    items: &[Item],
    parameters: &Parameters,
    signer: impl FnOnce(&[u8]) -> [u8; 64],
) -> Result<Signed, SignatureError> {
    let params = sf::serialize_inner_list(items, parameters);
    let base = signature_base(message, items, &params, None)?;
    let signature = signer(base.as_bytes());
    let mut signature_input = String::from(LABEL);
    signature_input.push('=');
    signature_input.push_str(&params);
    Ok(Signed {
        signature_input,
        signature: sf::serialize_byte_dictionary(&[(LABEL, signature.as_slice())]),
    })
}

/// What a node's signature on a response covers: `@status`, the
/// `content-digest` when the response has a body, and the request's
/// `@method`, `@target-uri` and nonce, when it sent one.
fn response_components(request: &Request<'_>, has_body: bool) -> Vec<Item> {
    let mut items = Vec::from([item("@status", false)]);
    if has_body {
        items.push(item("content-digest", false));
    }
    items.extend([item("@method", true), item("@target-uri", true)]);
    if request.has(NONCE_FIELD) {
        items.push(item(NONCE_FIELD, true));
    }
    items
}

/// Signs a response to `request` as a node does, under [`LABEL`] with
/// `tag="dassie"` and `alg="ed25519"`. `signer` signs with the node key,
/// which never leaves it; `keyid` is the node's public key, base64url.
///
/// A response with a body must carry its `Content-Digest`
/// ([`content_digest`]) among its fields.
///
/// # Errors
///
/// [`SignatureError::Malformed`] if a component cannot be computed, such
/// as a body without its digest.
pub fn sign_response(
    request: &Request<'_>,
    response: &Response<'_>,
    has_body: bool,
    signer: impl FnOnce(&[u8]) -> [u8; 64],
    keyid: &str,
    created_s: u64,
) -> Result<Signed, SignatureError> {
    let items = response_components(request, has_body);
    let created = i64::try_from(created_s).map_err(|_| SignatureError::Malformed)?;
    let parameters = Vec::from([
        ("created".to_string(), BareItem::Integer(created)),
        ("keyid".to_string(), BareItem::String(keyid.to_string())),
        ("alg".to_string(), BareItem::String(ALGORITHM.to_string())),
        (
            "tag".to_string(),
            BareItem::String(RESPONSE_TAG.to_string()),
        ),
    ]);
    sign_items(
        Message::response(request, response),
        &items,
        &parameters,
        signer,
    )
}

/// A signature found on a request, not yet verified.
#[derive(Clone, Debug)]
pub struct Signature {
    label: String,
    items: Vec<Item>,
    params: String,
    keyid: String,
    created_s: u64,
    bytes: [u8; 64],
    covers_digest: bool,
}

impl Signature {
    /// Picks the first signature on the request that covers `required`,
    /// and checks its time and algorithm.
    ///
    /// # Errors
    ///
    /// See [`SignatureError`].
    pub fn find(
        request: &Request<'_>,
        required: &[&str],
        now_s: u64,
    ) -> Result<Self, SignatureError> {
        let required: Vec<Item> = required.iter().map(|&name| item(name, false)).collect();
        Self::find_in(Message::request(request), &required, None, now_s)
    }

    /// The first signature on `message` that covers `required` and, when
    /// `tag` is given, carries that tag.
    fn find_in(
        message: Message<'_>,
        required: &[Item],
        tag: Option<&str>,
        now_s: u64,
    ) -> Result<Self, SignatureError> {
        let fields = message.fields();
        let (Some(input), Some(signatures)) =
            (field(fields, "signature-input"), field(fields, "signature"))
        else {
            return Err(SignatureError::Missing);
        };
        let inputs = sf::parse_dictionary(&input).map_err(|_| SignatureError::Malformed)?;
        let signatures =
            sf::parse_dictionary(&signatures).map_err(|_| SignatureError::Malformed)?;
        for (label, member) in &inputs {
            let Member::InnerList(items, parameters) = member else {
                continue;
            };
            let tagged = tag.is_none_or(|tag| {
                sf::parameter(parameters, "tag") == Some(&BareItem::String(tag.to_string()))
            });
            if !tagged || !covers(items, required) {
                continue;
            }
            let bytes = signatures
                .iter()
                .find(|(name, _)| name == label)
                .and_then(|(_, member)| match member {
                    Member::Item(Item {
                        bare: BareItem::Bytes(bytes),
                        ..
                    }) => <[u8; 64]>::try_from(bytes.as_slice()).ok(),
                    _ => None,
                })
                .ok_or(SignatureError::Malformed)?;
            return Self::checked(label, items, parameters, bytes, now_s);
        }
        Err(SignatureError::Components)
    }

    fn checked(
        label: &str,
        items: &[Item],
        parameters: &Parameters,
        bytes: [u8; 64],
        now_s: u64,
    ) -> Result<Self, SignatureError> {
        let integer = |key| match sf::parameter(parameters, key) {
            Some(BareItem::Integer(value)) => u64::try_from(*value).ok(),
            _ => None,
        };
        let created_s = integer("created").ok_or(SignatureError::Time)?;
        if created_s.abs_diff(now_s) > MAX_CLOCK_SKEW_S {
            return Err(SignatureError::Time);
        }
        if sf::parameter(parameters, "expires").is_some()
            && integer("expires").is_none_or(|expires| expires < now_s)
        {
            return Err(SignatureError::Time);
        }
        match sf::parameter(parameters, "alg") {
            None => {}
            Some(BareItem::String(alg)) if alg == ALGORITHM => {}
            Some(_) => return Err(SignatureError::Algorithm),
        }
        let Some(BareItem::String(keyid)) = sf::parameter(parameters, "keyid") else {
            return Err(SignatureError::Malformed);
        };
        Ok(Self {
            label: label.to_string(),
            items: items.to_vec(),
            params: sf::serialize_inner_list(items, parameters),
            keyid: keyid.clone(),
            created_s,
            bytes,
            covers_digest: covers(items, &[item("content-digest", false)]),
        })
    }

    /// The key the signature names.
    #[must_use]
    pub fn keyid(&self) -> &str {
        &self.keyid
    }

    /// The label it was found under.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// When the signer says it signed, in seconds since the epoch.
    #[must_use]
    pub fn created_s(&self) -> u64 {
        self.created_s
    }

    /// Verifies the signature under `key`, and the body against the
    /// `Content-Digest` when the signature covers it.
    ///
    /// # Errors
    ///
    /// [`SignatureError::Invalid`], [`SignatureError::Digest`], or
    /// [`SignatureError::Malformed`] if a component cannot be computed.
    pub fn verify(
        &self,
        request: &Request<'_>,
        body: &[u8],
        key: &VerifyingKey,
    ) -> Result<(), SignatureError> {
        self.verify_on(Message::request(request), body, key)
    }

    fn verify_on(
        &self,
        message: Message<'_>,
        body: &[u8],
        key: &VerifyingKey,
    ) -> Result<(), SignatureError> {
        let base = signature_base(message, &self.items, &self.params, Some(body.len()))?;
        let signature = ed25519_dalek::Signature::from_bytes(&self.bytes);
        key.verify_strict(base.as_bytes(), &signature)
            .map_err(|_| SignatureError::Invalid)?;
        if self.covers_digest {
            let field = field(message.fields(), "content-digest").ok_or(SignatureError::Digest)?;
            check_digest(&field, body)?;
        }
        Ok(())
    }
}

/// A node's signature found on a response, not yet verified.
#[derive(Clone, Debug)]
pub struct ResponseSignature(Signature);

impl ResponseSignature {
    /// Picks the signature tagged [`RESPONSE_TAG`] that covers what a
    /// node's response must: `@status`, the request's `@method` and
    /// `@target-uri`, its nonce when it sent one, and the `content-digest`
    /// when the response has a body. Checks its time and algorithm.
    ///
    /// # Errors
    ///
    /// See [`SignatureError`]: [`SignatureError::Components`] for one that
    /// leaves out the nonce the client sent.
    pub fn find(
        request: &Request<'_>,
        response: &Response<'_>,
        body: &[u8],
        now_s: u64,
    ) -> Result<Self, SignatureError> {
        let required = response_components(request, !body.is_empty());
        Signature::find_in(
            Message::response(request, response),
            &required,
            Some(RESPONSE_TAG),
            now_s,
        )
        .map(Self)
    }

    /// The key the signature names: a node's public key, base64url.
    #[must_use]
    pub fn keyid(&self) -> &str {
        self.0.keyid()
    }

    /// When the node says it signed, in seconds since the epoch.
    #[must_use]
    pub fn created_s(&self) -> u64 {
        self.0.created_s()
    }

    /// Verifies the signature under `key`, and the body against the
    /// `Content-Digest` when the signature covers it.
    ///
    /// # Errors
    ///
    /// As [`Signature::verify`].
    pub fn verify(
        &self,
        request: &Request<'_>,
        response: &Response<'_>,
        body: &[u8],
        key: &VerifyingKey,
    ) -> Result<(), SignatureError> {
        self.0
            .verify_on(Message::response(request, response), body, key)
    }
}

/// Why a node's response was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeResponseError {
    /// The name has no node label, and no node ID was given for it.
    UnknownNode,
    /// The signature's `keyid` is not an Ed25519 public key.
    Key,
    /// The key is not the node's: it does not hash to the expected ID.
    OtherNode,
    /// The signature was not found, or does not verify.
    Signature(SignatureError),
}

/// Checks that a response came from the node a Dassie-aware client meant
/// to reach (spec/areas/payments.md#signed-responses): that its signature's
/// key hashes to that node's ID, and that the signature verifies.
///
/// The node is `node_id` when the caller knows it, as from an owner's
/// domain's TXT record; otherwise the one whose label is the first label
/// of `name`, the host the response was fetched from, as a front door's
/// name carries it. Returns the node's ID.
///
/// # Errors
///
/// See [`NodeResponseError`].
pub fn check_node_response(
    request: &Request<'_>,
    response: &Response<'_>,
    body: &[u8],
    name: &str,
    node_id: Option<NodeId>,
    now_s: u64,
) -> Result<NodeId, NodeResponseError> {
    let expected = match node_id {
        Some(id) => id,
        None => {
            let label = name.split(['.', ':']).next().unwrap_or_default();
            NodeId::from_label(label).map_err(|_| NodeResponseError::UnknownNode)?
        }
    };
    let signature = ResponseSignature::find(request, response, body, now_s)
        .map_err(NodeResponseError::Signature)?;
    let mut key = [0u8; 32];
    if base64url::decoded_len(signature.keyid().len()) != Some(key.len()) {
        return Err(NodeResponseError::Key);
    }
    base64url::decode(signature.keyid().as_bytes(), &mut key)
        .map_err(|_| NodeResponseError::Key)?;
    if NodeId::from_public_key(&key) != expected {
        return Err(NodeResponseError::OtherNode);
    }
    let key = VerifyingKey::from_bytes(&key).map_err(|_| NodeResponseError::Key)?;
    if key.is_weak() {
        return Err(NodeResponseError::Key);
    }
    signature
        .verify(request, response, body, &key)
        .map_err(NodeResponseError::Signature)?;
    Ok(expected)
}

/// Whether `items` include each of `required`, parameters and all.
fn covers(items: &[Item], required: &[Item]) -> bool {
    required.iter().all(|wanted| items.contains(wanted))
}

/// Checks a body against a `Content-Digest`: at least one of SHA-512 and
/// SHA-256 must be there, and every one that is there must match.
///
/// # Errors
///
/// [`SignatureError::Digest`] otherwise.
pub fn check_digest(field: &str, body: &[u8]) -> Result<(), SignatureError> {
    let members = sf::parse_dictionary(field).map_err(|_| SignatureError::Digest)?;
    let mut checked = false;
    for (algorithm, member) in &members {
        let Member::Item(Item {
            bare: BareItem::Bytes(expected),
            ..
        }) = member
        else {
            return Err(SignatureError::Digest);
        };
        let matches: bool = match algorithm.as_str() {
            "sha-512" => Sha512::digest(body).as_slice().ct_eq(expected).into(),
            "sha-256" => Sha256::digest(body).as_slice().ct_eq(expected).into(),
            _ => continue,
        };
        if !matches {
            return Err(SignatureError::Digest);
        }
        checked = true;
    }
    if checked {
        Ok(())
    } else {
        Err(SignatureError::Digest)
    }
}

/// The signature base (RFC 9421 §2.5).
fn signature_base(
    message: Message<'_>,
    items: &[Item],
    params: &str,
    body_length: Option<usize>,
) -> Result<String, SignatureError> {
    let mut base = String::new();
    for (index, item) in items.iter().enumerate() {
        let BareItem::String(name) = &item.bare else {
            return Err(SignatureError::Malformed);
        };
        let duplicate = items.get(..index).unwrap_or_default().contains(item);
        let req = is_req(item)?;
        if duplicate {
            return Err(SignatureError::Malformed);
        }
        let value = match (component_value(message, name, req), body_length) {
            (Err(SignatureError::Malformed), Some(length))
                if name == "content-length" && message.response.is_none() =>
            {
                alloc::format!("{length}")
            }
            (value, _) => value?,
        };
        if value.contains(['\n', '\r']) {
            return Err(SignatureError::Malformed);
        }
        base.push('"');
        base.push_str(name);
        base.push('"');
        if req {
            base.push_str(";req");
        }
        base.push_str(": ");
        base.push_str(&value);
        base.push('\n');
    }
    base.push_str("\"@signature-params\": ");
    base.push_str(params);
    Ok(base)
}

/// Whether a component is the request's: no parameters, or only `req`.
fn is_req(item: &Item) -> Result<bool, SignatureError> {
    match item.parameters.as_slice() {
        [] => Ok(false),
        [(key, BareItem::Boolean(true))] if key == "req" => Ok(true),
        _ => Err(SignatureError::Malformed),
    }
}

/// A component's value on `message`: the request's when `req`, or a
/// request's own, and otherwise the response's.
fn component_value(message: Message<'_>, name: &str, req: bool) -> Result<String, SignatureError> {
    match (message.response, req) {
        (None, true) => Err(SignatureError::Malformed),
        (None, false) | (Some(_), true) => component(message.request, name),
        (Some(response), false) => response_component(response, name),
    }
}

fn response_component(response: &Response<'_>, name: &str) -> Result<String, SignatureError> {
    match name {
        "@status" if (100..=999).contains(&response.status) => {
            Ok(alloc::format!("{}", response.status))
        }
        _ if name.starts_with('@') => Err(SignatureError::Malformed),
        _ => field_component(response.fields, name),
    }
}

/// A field as a component: lowercase names only.
fn field_component(fields: &[(&str, &str)], name: &str) -> Result<String, SignatureError> {
    let lower = name.bytes().all(|byte| !byte.is_ascii_uppercase());
    if !lower || name.is_empty() {
        return Err(SignatureError::Malformed);
    }
    field(fields, name).ok_or(SignatureError::Malformed)
}

fn component(request: &Request<'_>, name: &str) -> Result<String, SignatureError> {
    let Some(derived) = name.strip_prefix('@') else {
        return field_component(request.fields, name);
    };
    if derived == "method" {
        return Ok(request.method.to_string());
    }
    let uri = Uri::parse(request.target_uri).map_err(|_| SignatureError::Malformed)?;
    let path = if uri.path.is_empty() { "/" } else { uri.path };
    let query = || {
        let mut query = String::from("?");
        query.push_str(uri.query.unwrap_or_default());
        query
    };
    match derived {
        "target-uri" => Ok(request.target_uri.to_string()),
        "authority" => Ok(uri.authority.to_ascii_lowercase()),
        "scheme" => Ok(uri.scheme.to_string()),
        "path" => Ok(path.to_string()),
        "query" => Ok(query()),
        "request-target" => {
            let mut target = path.to_string();
            if uri.query.is_some() {
                target.push_str(&query());
            }
            Ok(target)
        }
        _ => Err(SignatureError::Malformed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    #[test]
    fn signs_what_it_verifies() {
        let body = br#"{"walletAddress":"https://a.example/pay"}"#;
        let digest = content_digest(body);
        let length = format!("{}", body.len());
        let unsigned = [
            ("Authorization", "GNAP token"),
            ("Content-Type", "application/json"),
            ("Content-Digest", digest.as_str()),
            ("Content-Length", length.as_str()),
        ];
        let request = Request {
            method: "POST",
            target_uri: "https://b.example/incoming-payments",
            fields: &unsigned,
        };
        let components = profile(&request, true);
        let signed = sign(&request, &components, &key(), "k1", 1_000).unwrap();
        let mut fields = unsigned.to_vec();
        fields.push(("Signature-Input", &signed.signature_input));
        fields.push(("Signature", &signed.signature));
        let request = Request {
            fields: &fields,
            ..request
        };
        let found = Signature::find(&request, &components, 1_100).unwrap();
        assert_eq!(found.keyid(), "k1");
        let public = key().verifying_key();
        assert_eq!(found.verify(&request, body, &public), Ok(()));
        assert_eq!(
            found.verify(&request, b"{}", &public),
            Err(SignatureError::Digest)
        );
        let other = SigningKey::from_bytes(&[8; 32]).verifying_key();
        assert_eq!(
            found.verify(&request, body, &other),
            Err(SignatureError::Invalid)
        );
        assert_eq!(
            Signature::find(&request, &components, 1_400).map(|_| ()),
            Err(SignatureError::Time)
        );
        assert_eq!(
            Signature::find(&request, &["@method", "@authority"], 1_000).map(|_| ()),
            Err(SignatureError::Components)
        );
    }

    #[test]
    fn a_dropped_content_length_is_the_body_length() {
        let body = br#"{"a":1}"#;
        let digest = content_digest(body);
        let signed_fields = [
            ("Content-Type", "application/json"),
            ("Content-Digest", digest.as_str()),
            ("Content-Length", "7"),
        ];
        let request = Request {
            method: "POST",
            target_uri: "https://b.example/gnap",
            fields: &signed_fields,
        };
        let components = profile(&request, true);
        let signed = sign(&request, &components, &key(), "k1", 1_000).unwrap();
        // Sent chunked: no Content-Length arrives.
        let fields = [
            ("Content-Type", "application/json"),
            ("Content-Digest", digest.as_str()),
            ("Transfer-Encoding", "chunked"),
            ("Signature-Input", signed.signature_input.as_str()),
            ("Signature", signed.signature.as_str()),
        ];
        let received = Request {
            fields: &fields,
            ..request
        };
        let found = Signature::find(&received, &components, 1_000).unwrap();
        assert_eq!(
            found.verify(&received, body, &key().verifying_key()),
            Ok(())
        );
        assert_eq!(
            found.verify(&received, br#"{"a":12}"#, &key().verifying_key()),
            Err(SignatureError::Invalid)
        );
    }

    #[test]
    fn a_signature_does_not_survive_another_target() {
        let request = Request {
            method: "GET",
            target_uri: "https://b.example/incoming-payments/1",
            fields: &[],
        };
        let components = profile(&request, false);
        let signed = sign(&request, &components, &key(), "k1", 1_000).unwrap();
        let fields = [
            ("Signature-Input", signed.signature_input.as_str()),
            ("Signature", signed.signature.as_str()),
        ];
        let moved = Request {
            method: "GET",
            target_uri: "https://b.example/incoming-payments/2",
            fields: &fields,
        };
        let found = Signature::find(&moved, &components, 1_000).unwrap();
        assert_eq!(
            found.verify(&moved, b"", &key().verifying_key()),
            Err(SignatureError::Invalid)
        );
    }

    #[test]
    fn refuses_a_foreign_algorithm() {
        let fields = [
            (
                "Signature-Input",
                r#"sig1=("@method");created=1000;keyid="k";alg="rsa-pss-sha512""#,
            ),
            (
                "Signature",
                "sig1=:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==:",
            ),
        ];
        let request = Request {
            method: "GET",
            target_uri: "https://b.example/",
            fields: &fields,
        };
        assert_eq!(
            Signature::find(&request, &["@method"], 1_000).map(|_| ()),
            Err(SignatureError::Algorithm)
        );
    }

    /// A node's key, its `keyid`, and its label.
    fn node() -> (SigningKey, String, String) {
        let key = SigningKey::from_bytes(&[9; 32]);
        let public = key.verifying_key().to_bytes();
        let mut keyid = [0u8; 43];
        let length = base64url::encode(&public, &mut keyid);
        let keyid = String::from(core::str::from_utf8(&keyid[..length]).unwrap());
        let label = NodeId::from_public_key(&public).label().unwrap();
        (key, keyid, label.as_str().to_string())
    }

    const BODY: &[u8] = br#"{"id":"https://x/pay"}"#;
    const NOW_S: u64 = 1_800_000_000;

    /// The response's fields once signed: its digest and signature.
    fn signed_response(request: &Request<'_>, status: u16, body: &[u8]) -> Vec<(String, String)> {
        let (key, keyid, _) = node();
        let mut fields = Vec::from([("Content-Type".to_string(), "application/json".to_string())]);
        if !body.is_empty() {
            fields.push(("Content-Digest".to_string(), content_digest(body)));
        }
        let field_refs = refs(&fields);
        let response = Response {
            status,
            fields: &field_refs,
        };
        let signed = sign_response(
            request,
            &response,
            !body.is_empty(),
            |base| key.sign(base).to_bytes(),
            &keyid,
            NOW_S,
        )
        .unwrap();
        fields.push(("Signature-Input".to_string(), signed.signature_input));
        fields.push(("Signature".to_string(), signed.signature));
        fields
    }

    fn refs(fields: &[(String, String)]) -> Vec<(&str, &str)> {
        fields
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect()
    }

    fn check(
        request: &Request<'_>,
        status: u16,
        fields: &[(String, String)],
        body: &[u8],
        name: &str,
    ) -> Result<NodeId, NodeResponseError> {
        let fields = refs(fields);
        let response = Response {
            status,
            fields: &fields,
        };
        check_node_response(request, &response, body, name, None, NOW_S)
    }

    // The signature base of a response, as RFC 9421 §2.5 writes it, with
    // the request's components marked `;req`.
    #[test]
    fn a_response_signature_covers_the_request_it_answers() {
        let (_, keyid, _) = node();
        let request = Request {
            method: "GET",
            target_uri: "https://a.example/pay",
            fields: &[("Dassie-Nonce", "n1")],
        };
        let digest = content_digest(BODY);
        let fields = [("Content-Digest", digest.as_str())];
        let response = Response {
            status: 200,
            fields: &fields,
        };
        let items = response_components(&request, true);
        let parameters = Vec::from([
            ("created".to_string(), BareItem::Integer(1)),
            ("keyid".to_string(), BareItem::String(keyid.clone())),
            ("alg".to_string(), BareItem::String(ALGORITHM.to_string())),
            (
                "tag".to_string(),
                BareItem::String(RESPONSE_TAG.to_string()),
            ),
        ]);
        let params = sf::serialize_inner_list(&items, &parameters);
        let base = signature_base(
            Message::response(&request, &response),
            &items,
            &params,
            None,
        )
        .unwrap();
        let expected = format!(
            concat!(
                "\"@status\": 200\n",
                "\"content-digest\": {digest}\n",
                "\"@method\";req: GET\n",
                "\"@target-uri\";req: https://a.example/pay\n",
                "\"dassie-nonce\";req: n1\n",
                "\"@signature-params\": (\"@status\" \"content-digest\" \"@method\";req ",
                "\"@target-uri\";req \"dassie-nonce\";req);created=1;keyid=\"{keyid}\";",
                "alg=\"ed25519\";tag=\"dassie\"",
            ),
            digest = digest,
            keyid = keyid,
        );
        assert_eq!(base, expected);
    }

    #[test]
    fn a_node_signs_what_a_client_checks() {
        let (_, _, label) = node();
        let name = format!("{label}.dassiepay.net");
        let target = format!("https://{name}/pay");
        let with_nonce = [("Dassie-Nonce", "n1")];
        let request = Request {
            method: "GET",
            target_uri: &target,
            fields: &with_nonce,
        };
        let fields = signed_response(&request, 200, BODY);
        let id = check(&request, 200, &fields, BODY, &name).unwrap();
        assert_eq!(id.label().map(|l| l.as_str() == label), Ok(true));
        // In either case, and with a port, as a Host header may name it.
        let upper = format!("{}:443", name.to_ascii_uppercase());
        assert_eq!(check(&request, 200, &fields, BODY, &upper), Ok(id));
        // Without a body, there is no digest to cover.
        let empty = signed_response(&request, 204, b"");
        assert_eq!(check(&request, 204, &empty, b"", &name), Ok(id));
        // A node ID the caller knows, for a name without a label.
        let response_fields = refs(&fields);
        let response = Response {
            status: 200,
            fields: &response_fields,
        };
        let owners = "pay.example.com";
        assert_eq!(
            check_node_response(&request, &response, BODY, owners, Some(id), NOW_S),
            Ok(id)
        );
        assert_eq!(
            check_node_response(&request, &response, BODY, owners, None, NOW_S),
            Err(NodeResponseError::UnknownNode)
        );
    }

    #[test]
    fn a_client_refuses_what_the_node_did_not_say() {
        let (_, _, label) = node();
        let name = format!("{label}.dassiepay.net");
        let target = format!("https://{name}/pay");
        let with_nonce = [("Dassie-Nonce", "n1")];
        let request = Request {
            method: "GET",
            target_uri: &target,
            fields: &with_nonce,
        };
        let fields = signed_response(&request, 200, BODY);
        let invalid = Err(NodeResponseError::Signature(SignatureError::Invalid));
        // Another body, status, nonce, method or URL.
        assert_eq!(
            check(&request, 200, &fields, br#"{"id":"https://y/pay"}"#, &name),
            Err(NodeResponseError::Signature(SignatureError::Digest))
        );
        assert_eq!(check(&request, 404, &fields, BODY, &name), invalid);
        let replayed = [("Dassie-Nonce", "n2")];
        let other = Request {
            fields: &replayed,
            ..request
        };
        assert_eq!(check(&other, 200, &fields, BODY, &name), invalid);
        let other = Request {
            method: "HEAD",
            ..request
        };
        assert_eq!(check(&other, 200, &fields, BODY, &name), invalid);
        let moved = format!("https://{name}/pay/other");
        let other = Request {
            target_uri: &moved,
            ..request
        };
        assert_eq!(check(&other, 200, &fields, BODY, &name), invalid);
        // A body where the node signed none: the digest must be covered.
        let empty = signed_response(&request, 200, b"");
        assert_eq!(
            check(&request, 200, &empty, BODY, &name),
            Err(NodeResponseError::Signature(SignatureError::Components))
        );
        // A response signed without the nonce the client sent.
        let without_nonce = Request {
            fields: &[],
            ..request
        };
        let unbound = signed_response(&without_nonce, 200, BODY);
        assert_eq!(
            check(&request, 200, &unbound, BODY, &name),
            Err(NodeResponseError::Signature(SignatureError::Components))
        );
        // Too old.
        let response_fields = refs(&fields);
        let response = Response {
            status: 200,
            fields: &response_fields,
        };
        assert_eq!(
            check_node_response(&request, &response, BODY, &name, None, NOW_S + 600),
            Err(NodeResponseError::Signature(SignatureError::Time))
        );
    }

    #[test]
    fn a_client_refuses_another_nodes_key() {
        let (_, keyid, label) = node();
        // Another node's label: the key does not hash to it.
        let other = NodeId::from_public_key(&[1; 32]).label().unwrap();
        let name = format!("{}.dassiepay.net", other.as_str());
        let target = format!("https://{name}/pay");
        let request = Request {
            method: "GET",
            target_uri: &target,
            fields: &[],
        };
        let fields = signed_response(&request, 200, BODY);
        assert_eq!(
            check(&request, 200, &fields, BODY, &name),
            Err(NodeResponseError::OtherNode)
        );
        // The node's key named, another key signing.
        let honest = format!("{label}.dassiepay.net");
        let impostor = SigningKey::from_bytes(&[10; 32]);
        let response = Response {
            status: 200,
            fields: &[],
        };
        let forged = sign_response(
            &request,
            &response,
            false,
            |base| impostor.sign(base).to_bytes(),
            &keyid,
            NOW_S,
        )
        .unwrap();
        let fields = [
            ("Signature-Input".to_string(), forged.signature_input),
            ("Signature".to_string(), forged.signature.clone()),
        ];
        assert_eq!(
            check(&request, 200, &fields, b"", &honest),
            Err(NodeResponseError::Signature(SignatureError::Invalid))
        );
        // A keyid that is not a key.
        let input = concat!(
            "sig1=(\"@status\" \"@method\";req \"@target-uri\";req);",
            "created=1800000000;keyid=\"abc\";tag=\"dassie\"",
        );
        let bad = [
            ("Signature-Input".to_string(), input.to_string()),
            ("Signature".to_string(), forged.signature),
        ];
        assert_eq!(
            check(&request, 200, &bad, b"", &honest),
            Err(NodeResponseError::Key)
        );
    }

    #[test]
    fn only_a_signature_tagged_dassie_counts() {
        let (key, keyid, label) = node();
        let name = format!("{label}.dassiepay.net");
        let target = format!("https://{name}/pay");
        let request = Request {
            method: "GET",
            target_uri: &target,
            fields: &[],
        };
        let response = Response {
            status: 204,
            fields: &[],
        };
        let items = response_components(&request, false);
        let parameters = Vec::from([
            ("created".to_string(), BareItem::Integer(1_800_000_000)),
            ("keyid".to_string(), BareItem::String(keyid)),
            ("tag".to_string(), BareItem::String("other".to_string())),
        ]);
        let message = Message::response(&request, &response);
        let signed = sign_items(message, &items, &parameters, |base| {
            key.sign(base).to_bytes()
        })
        .unwrap();
        let fields = [
            ("Signature-Input".to_string(), signed.signature_input),
            ("Signature".to_string(), signed.signature),
        ];
        assert_eq!(
            check(&request, 204, &fields, b"", &name),
            Err(NodeResponseError::Signature(SignatureError::Components))
        );
    }

    // A request signature never takes components of a response.
    #[test]
    fn a_request_signature_has_no_request_components() {
        let request = Request {
            method: "GET",
            target_uri: "https://b.example/",
            fields: &[],
        };
        let items = [item("@method", true)];
        let base = signature_base(Message::request(&request), &items, "()", None);
        assert_eq!(base, Err(SignatureError::Malformed));
    }
}

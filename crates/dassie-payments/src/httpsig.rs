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

use alloc::string::{String, ToString};
use alloc::vec::Vec;
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
    /// All lines of a field, trimmed and joined with `, ` (RFC 9421 §2.1).
    fn field(&self, name: &str) -> Option<String> {
        let mut value: Option<String> = None;
        let lines = self
            .fields
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

    fn has(&self, name: &str) -> bool {
        self.fields
            .iter()
            .any(|(field, _)| field.eq_ignore_ascii_case(name))
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
    let items: Vec<Item> = components
        .iter()
        .map(|&name| Item {
            bare: BareItem::String(name.to_string()),
            parameters: Parameters::new(),
        })
        .collect();
    let created = i64::try_from(created_s).map_err(|_| SignatureError::Malformed)?;
    let parameters = Vec::from([
        ("created".to_string(), BareItem::Integer(created)),
        ("keyid".to_string(), BareItem::String(keyid.to_string())),
    ]);
    let params = sf::serialize_inner_list(&items, &parameters);
    let base = signature_base(request, &items, &params, None)?;
    let signature = key.sign(base.as_bytes()).to_bytes();
    let mut signature_input = String::from(LABEL);
    signature_input.push('=');
    signature_input.push_str(&params);
    Ok(Signed {
        signature_input,
        signature: sf::serialize_byte_dictionary(&[(LABEL, signature.as_slice())]),
    })
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
        let (Some(input), Some(signatures)) =
            (request.field("signature-input"), request.field("signature"))
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
            if !covers(items, required) {
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
            Some(BareItem::String(alg)) if alg == "ed25519" => {}
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
            covers_digest: covers(items, &["content-digest"]),
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
        let base = signature_base(request, &self.items, &self.params, Some(body.len()))?;
        let signature = ed25519_dalek::Signature::from_bytes(&self.bytes);
        key.verify_strict(base.as_bytes(), &signature)
            .map_err(|_| SignatureError::Invalid)?;
        if self.covers_digest {
            let field = request
                .field("content-digest")
                .ok_or(SignatureError::Digest)?;
            check_digest(&field, body)?;
        }
        Ok(())
    }
}

fn covers(items: &[Item], required: &[&str]) -> bool {
    required.iter().all(|name| {
        items
            .iter()
            .any(|item| item.bare == BareItem::String((*name).to_string()))
    })
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
    request: &Request<'_>,
    items: &[Item],
    params: &str,
    body_length: Option<usize>,
) -> Result<String, SignatureError> {
    let mut base = String::new();
    for (index, item) in items.iter().enumerate() {
        let BareItem::String(name) = &item.bare else {
            return Err(SignatureError::Malformed);
        };
        let duplicate = items
            .get(..index)
            .unwrap_or_default()
            .iter()
            .any(|earlier| earlier.bare == item.bare);
        if !item.parameters.is_empty() || duplicate {
            return Err(SignatureError::Malformed);
        }
        let value = match (component(request, name), body_length) {
            (Err(SignatureError::Malformed), Some(length)) if name == "content-length" => {
                alloc::format!("{length}")
            }
            (value, _) => value?,
        };
        if value.contains(['\n', '\r']) {
            return Err(SignatureError::Malformed);
        }
        base.push('"');
        base.push_str(name);
        base.push_str("\": ");
        base.push_str(&value);
        base.push('\n');
    }
    base.push_str("\"@signature-params\": ");
    base.push_str(params);
    Ok(base)
}

fn component(request: &Request<'_>, name: &str) -> Result<String, SignatureError> {
    let Some(derived) = name.strip_prefix('@') else {
        let lower = name.bytes().all(|byte| !byte.is_ascii_uppercase());
        if !lower || name.is_empty() {
            return Err(SignatureError::Malformed);
        }
        return request.field(name).ok_or(SignatureError::Malformed);
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
}

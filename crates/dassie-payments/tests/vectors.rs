//! The examples of the standards Open Payments builds on, as golden vectors
//! (spec 7 §7): RFC 9421 B.2.6 (an Ed25519 signature), RFC 9530 (the
//! `Content-Digest` of RFC 9421's test request), and the interaction hash
//! of the Open Payments docs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use dassie_payments::gnap;
use dassie_payments::httpsig::{self, Request, Signature, SignatureError};
use ed25519_dalek::SigningKey;

/// RFC 9421 B.1.4, `test-key-ed25519`.
fn test_key() -> SigningKey {
    let secret = URL_SAFE_NO_PAD
        .decode("n4Ni-HpISpVObnQMW0wOhCKROaIKqKtW_2ZYb2p9KcU")
        .unwrap();
    SigningKey::from_bytes(&secret.try_into().unwrap())
}

const COMPONENTS: [&str; 6] = [
    "date",
    "@method",
    "@path",
    "@authority",
    "content-type",
    "content-length",
];
const SIGNATURE: &str = "sig-b26=:wqcAqbmYJ2ji2glfAMaRy4gruYYnx2nEFN2HN6jrnDnQCK1u02Gb04v9EDgwUPiu4A0w6vuQv5lIp5WPpBKRCw==:";
const SIGNATURE_INPUT: &str = r#"sig-b26=("date" "@method" "@path" "@authority" "content-type" "content-length");created=1618884473;keyid="test-key-ed25519""#;
const BODY: &[u8] = br#"{"hello": "world"}"#;
const DIGEST: &str = "sha-512=:WZDPaVn/7XgHaAy8pmojAkGWoRx2UFChF41A2svX+TaPm+AbwAgBWnrIiYllu7BNNyealdVLvRwEmTHWXvJwew==:";

fn fields() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Host", "example.com"),
        ("Date", "Tue, 20 Apr 2021 02:07:55 GMT"),
        ("Content-Type", "application/json"),
        ("Content-Digest", DIGEST),
        ("Content-Length", "18"),
    ]
}

fn request<'a>(fields: &'a [(&'a str, &'a str)]) -> Request<'a> {
    Request {
        method: "POST",
        target_uri: "https://example.com/foo?param=Value&Pet=dog",
        fields,
    }
}

#[test]
fn rfc_9421_b_2_6_verifies() {
    let mut fields = fields();
    fields.push(("Signature-Input", SIGNATURE_INPUT));
    fields.push(("Signature", SIGNATURE));
    let request = request(&fields);
    let signature = Signature::find(&request, &COMPONENTS, 1_618_884_473).unwrap();
    assert_eq!(signature.label(), "sig-b26");
    assert_eq!(signature.keyid(), "test-key-ed25519");
    let public = test_key().verifying_key();
    assert_eq!(signature.verify(&request, BODY, &public), Ok(()));
    // The digest is not covered by this signature, so a changed body still
    // verifies: covering it is what the Open Payments profile adds.
    assert_eq!(signature.verify(&request, b"{}", &public), Ok(()));
}

#[test]
fn rfc_9421_b_2_6_signs() {
    let fields = fields();
    let signed = httpsig::sign(
        &request(&fields),
        &COMPONENTS,
        &test_key(),
        "test-key-ed25519",
        1_618_884_473,
    )
    .unwrap();
    assert_eq!(
        signed.signature_input,
        SIGNATURE_INPUT.replace("sig-b26", httpsig::LABEL)
    );
    assert_eq!(
        signed.signature,
        SIGNATURE.replace("sig-b26", httpsig::LABEL)
    );
}

#[test]
fn a_changed_component_fails() {
    let mut fields = fields();
    fields.retain(|(name, _)| *name != "Date");
    fields.push(("Date", "Tue, 20 Apr 2021 02:07:56 GMT"));
    fields.push(("Signature-Input", SIGNATURE_INPUT));
    fields.push(("Signature", SIGNATURE));
    let request = request(&fields);
    let signature = Signature::find(&request, &COMPONENTS, 1_618_884_473).unwrap();
    assert_eq!(
        signature.verify(&request, BODY, &test_key().verifying_key()),
        Err(SignatureError::Invalid)
    );
}

#[test]
fn rfc_9530_digests_the_test_request() {
    assert_eq!(httpsig::content_digest(BODY), DIGEST);
    assert_eq!(httpsig::check_digest(DIGEST, BODY), Ok(()));
    // RFC 9530 §2: SHA-256 of the same body, as another server might send.
    let both = "sha-256=:X48E9qOokqqrvdts8nOJRJN3OWDUoyWxBf7kbu9DBPE=:, unknown=:AA==:";
    assert_eq!(httpsig::check_digest(both, BODY), Ok(()));
    assert_eq!(
        httpsig::check_digest("unknown=:AA==:", BODY),
        Err(SignatureError::Digest)
    );
    assert_eq!(
        httpsig::check_digest(DIGEST, b"{\"hello\": \"world\"} "),
        Err(SignatureError::Digest)
    );
}

#[test]
fn the_open_payments_hash_example() {
    assert_eq!(
        gnap::interaction_hash(
            "VJLO6A4CATR0KRO",
            "MBDOFXG4Y5CVJCX821LH",
            "4IFWWIKYB2PQ6U56NL1",
            "https://server.example.com/tx",
        ),
        "x-gguKWTj8rQf7d7i3w3UhzvuJ5bpOlKyAlVpLxBffY"
    );
}

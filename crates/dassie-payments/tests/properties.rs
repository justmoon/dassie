//! Every decoder of untrusted input returns an error instead of panicking
//! (spec/decisions/tigerstyle.md), and the encoders round-trip.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use dassie_payments::amount::Amount;
use dassie_payments::gnap::{self, ContinueRequest, GrantRequest, GrantResponse};
use dassie_payments::httpsig::{self, Request, Signature, SignatureError};
use dassie_payments::interval::{Duration, Interval};
use dassie_payments::jwk::{Jwk, Jwks};
use dassie_payments::quote::Rate;
use dassie_payments::resources::{
    self, CreateIncomingPayment, CreateOutgoingPayment, CreateQuote, IncomingPayment, ListQuery,
    PublicIncomingPayment, WalletAddress,
};
use dassie_payments::{SigningKey, pointer, sf, time, uri};
use proptest::prelude::*;

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        "[ -~]{0,200}",
        r#"[a-z0-9*=,;:()"?\- ]{0,120}"#,
    ]
}

proptest! {
    #[test]
    fn structured_fields_never_panic(input in text()) {
        let _ = sf::parse_dictionary(&input);
    }

    #[test]
    fn times_never_panic(input in text()) {
        let _ = time::parse_rfc3339(&input);
    }

    #[test]
    fn times_round_trip(ms in 0..=time::MAX_TIMESTAMP_MS) {
        prop_assert_eq!(time::parse_rfc3339(&time::format_rfc3339(ms)), Ok(ms));
    }

    #[test]
    fn urls_and_queries_never_panic(input in text()) {
        let _ = uri::Uri::parse(&input).map(|uri| uri.host());
        let _ = uri::query_pairs(&input);
        let _ = ListQuery::parse(&input);
        let _ = pointer::wallet_address_url(&input);
        let _ = gnap::bearer(&input);
    }

    #[test]
    fn recipients_are_https_urls(input in text()) {
        if let Ok(recipient) = pointer::Recipient::parse(&input) {
            prop_assert!(recipient.url().starts_with("https://"));
            prop_assert!(pointer::check_wallet_address(recipient.url()).is_ok());
        }
    }

    #[test]
    fn encoded_query_values_decode(value in any::<String>()) {
        let query = format!("v={}", uri::percent_encode(&value));
        prop_assert_eq!(uri::query_pairs(&query), Ok(vec![("v".to_string(), value)]));
    }

    #[test]
    fn json_never_panics(input in text()) {
        if let Ok(request) = serde_json::from_str::<GrantRequest>(&input) {
            let _ = request.validate();
        }
        let _ = serde_json::from_str::<CreateIncomingPayment>(&input);
        let _ = serde_json::from_str::<CreateQuote>(&input);
        let _ = serde_json::from_str::<CreateOutgoingPayment>(&input);
    }

    #[test]
    fn intervals_never_panic(input in prop_oneof![
        text(),
        "R[0-9-]{0,3}/[0-9TZ:.-]{0,24}/P[0-9YMWDTHS.]{0,16}",
        "R[0-9]{0,2}/P[0-9YMWDTHS]{1,12}/20[0-9]{2}-0[1-9]-1[0-9]T00:00:00Z",
    ], now in 0..=time::MAX_TIMESTAMP_MS) {
        let _ = Duration::parse(&input);
        if let Ok(interval) = Interval::parse(&input) {
            let _ = interval.window(now);
        }
    }

    // A window holds the moment asked about, and a later moment's window
    // never starts before an earlier one's.
    #[test]
    fn windows_hold_now_and_move_forward(
        start in 0u64..4_000_000_000_000,
        months in 0u32..15,
        days in 0u64..40,
        hours in 0u64..30,
        repetitions in prop::option::of(0u64..50),
        offset in 0u64..200_000_000_000,
        later in 0u64..50_000_000_000,
    ) {
        prop_assume!(months > 0 || days > 0 || hours > 0);
        let mut duration = String::from("P");
        if months > 0 { duration += &format!("{months}M"); }
        if days > 0 { duration += &format!("{days}D"); }
        if hours > 0 { duration += &format!("T{hours}H"); }
        let head = repetitions.map_or_else(|| "R".to_string(), |count| format!("R{count}"));
        let text = format!("{head}/{}/{duration}", time::format_rfc3339(start));
        let interval = Interval::parse(&text).unwrap();
        let now = start + offset;
        if let Some((from, to)) = interval.window(now) {
            prop_assert!(from <= now && now < to, "{text} at {now}: {from}..{to}");
            if let Some((next_from, _)) = interval.window(now + later) {
                prop_assert!(next_from >= from);
            }
        }
    }

    // A fixed-receive quote covers its delivery; a fixed-send one never
    // delivers more than the debit buys.
    #[test]
    fn quotes_cover_what_they_deliver(
        destination in 1u64..1_000_000,
        source in 1u64..1_000_000,
        amount in 0u64..u64::MAX / 2,
    ) {
        let rate = Rate { destination, source };
        if let Some(delivered) = rate.cost(amount).and_then(|debit| rate.delivered(debit)) {
            prop_assert!(delivered >= amount);
        }
        if let Some(delivered) = rate.delivered(amount) {
            prop_assert!(u128::from(delivered) * u128::from(source)
                <= u128::from(amount) * u128::from(destination));
        }
    }

    #[test]
    fn signature_fields_never_panic(input in text(), signature in text()) {
        let fields = [("Signature-Input", input.as_str()), ("Signature", signature.as_str())];
        let request = Request { method: "GET", target_uri: "https://a.example/", fields: &fields };
        let _ = Signature::find(&request, &["@method"], 1_000);
    }
}

// JSON shaped like the bodies the node reads and the responses it reads
// from other wallets. A clean document has the members it usually has, of
// the right kind if not always of the right value; a noisy one drops
// members, puts values of any kind in their place and adds members nobody
// expects. Values of any kind nest at most three levels deep.

type Json = BoxedStrategy<String>;

fn quote(text: &str) -> String {
    serde_json::Value::String(text.to_string()).to_string()
}

fn strings(text: impl Strategy<Value = String> + 'static) -> Json {
    text.prop_map(|text| quote(&text)).boxed()
}

fn json_object(members: impl IntoIterator<Item = (String, String)>) -> String {
    let members: Vec<String> = members
        .into_iter()
        .map(|(key, value)| format!("{}:{value}", quote(&key)))
        .collect();
    format!("{{{}}}", members.join(","))
}

fn array(item: Json, most: usize) -> Json {
    prop::collection::vec(item, 0..=most)
        .prop_map(|items| format!("[{}]", items.join(",")))
        .boxed()
}

fn name_text() -> impl Strategy<Value = String> {
    prop_oneof!["[a-zA-Z_]{1,12}", any::<String>()]
}

// Numbers past `u64` and `i64`, negative ones, fractions and exponents.
fn number() -> Json {
    prop_oneof![
        (0u16..300).prop_map(|number| number.to_string()),
        any::<i64>().prop_map(|number| number.to_string()),
        any::<u64>().prop_map(|number| number.to_string()),
        "-?[1-9][0-9]{19,40}",
        r"-?(0|[1-9][0-9]{0,3})(\.[0-9]{1,4})?([eE][+-]?[0-9]{1,4})?",
    ]
    .boxed()
}

// What an amount's `value` might hold: canonical decimals up to and past
// `u64::MAX`, signs, leading zeros, fractions, nothing.
fn decimal_text() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "0|[1-9][0-9]{0,19}",
        1 => Just(u64::MAX.to_string()),
        1 => Just("18446744073709551616".to_string()),
        1 => r"([+-]?0[0-9]{1,3}|[+-][0-9]{1,5}|[0-9]{1,4}\.[0-9]{1,3})?",
    ]
}

fn any_json() -> Json {
    let leaf = prop_oneof![
        Just("null".to_string()),
        any::<bool>().prop_map(|value| value.to_string()),
        number(),
        strings(prop_oneof![text(), decimal_text()]),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            array(inner.clone(), 4),
            prop::collection::vec((name_text(), inner), 0..4).prop_map(json_object),
        ]
    })
    .boxed()
}

// How often a member is there.
#[derive(Clone, Copy)]
enum Chance {
    Usually,
    Maybe,
    Rarely,
    // Mostly there, and never of another kind even in a noisy document:
    // for a type that reads every member leniently, so that one odd member
    // does not spoil a whole key set.
    Lenient,
}

type Field = (&'static str, Chance, Json);

fn usually(key: &'static str, value: Json) -> Field {
    (key, Chance::Usually, value)
}

fn maybe(key: &'static str, value: Json) -> Field {
    (key, Chance::Maybe, value)
}

fn rarely(key: &'static str, value: Json) -> Field {
    (key, Chance::Rarely, value)
}

fn lenient(key: &'static str, value: Json) -> Field {
    (key, Chance::Lenient, value)
}

fn object(noisy: bool, fields: Vec<Field>) -> Json {
    let fields: Vec<BoxedStrategy<Option<(String, String)>>> = fields
        .into_iter()
        .map(|(key, chance, value)| {
            let value = match (chance, noisy) {
                (Chance::Lenient, _) | (_, false) => value,
                _ => prop_oneof![8 => value, 1 => any_json()].boxed(),
            };
            let member = value.prop_map(move |value| (key.to_string(), value));
            match (chance, noisy) {
                (Chance::Usually, false) => member.prop_map(Some).boxed(),
                (Chance::Usually | Chance::Lenient, _) => {
                    prop::option::weighted(0.9, member).boxed()
                }
                (Chance::Maybe, _) => prop::option::weighted(0.5, member).boxed(),
                (Chance::Rarely, false) => prop::option::weighted(0.1, member).boxed(),
                (Chance::Rarely, true) => prop::option::weighted(0.2, member).boxed(),
            }
        })
        .collect();
    let unknown = if noisy {
        prop::option::weighted(0.1, (name_text(), any_json())).boxed()
    } else {
        Just(None).boxed()
    };
    (fields, unknown)
        .prop_map(|(fields, unknown)| json_object(fields.into_iter().flatten().chain(unknown)))
        .boxed()
}

// One of the words a member takes, or, in a noisy document, any string.
fn one_of(noisy: bool, choices: &[&'static str]) -> Json {
    let choice = prop::sample::select(choices.to_vec()).prop_map(quote);
    if noisy {
        prop_oneof![4 => choice, 1 => text().prop_map(|text| quote(&text))].boxed()
    } else {
        choice.boxed()
    }
}

// Mostly the word a member should have.
fn mostly(noisy: bool, usual: &'static str, other: &'static str) -> Json {
    prop_oneof![3 => Just(quote(usual)), 1 => one_of(noisy, &[usual, other])].boxed()
}

// Distinct words of a list in a clean document, any words in a noisy one.
fn words(noisy: bool, choices: &[&'static str], most: usize) -> Json {
    if noisy {
        return array(one_of(noisy, choices), most);
    }
    prop::sample::subsequence(choices.to_vec(), 1..=most.min(choices.len()))
        .prop_map(|words| {
            let words: Vec<String> = words.into_iter().map(quote).collect();
            format!("[{}]", words.join(","))
        })
        .boxed()
}

// Each shape, clean or noisy.
fn both(shape: fn(bool) -> Json) -> Json {
    prop_oneof![shape(false), shape(true)].boxed()
}

fn url_text() -> impl Strategy<Value = String> {
    prop_oneof![
        2 => "https://[a-c]\\.example/incoming-payments/[a-z0-9]{0,6}",
        3 => r"https?://[a-c]\.example(:[0-9]{1,5})?(/[a-z.-]{0,8}){0,3}(\?[a-z=&]{0,8})?(#[a-z]{0,4})?",
        1 => text(),
    ]
}

fn url() -> Json {
    strings(url_text())
}

fn token_text() -> impl Strategy<Value = String> {
    prop_oneof![3 => "[A-Za-z0-9]{1,20}", 1 => "[a-z]{250,260}", 1 => text()]
}

fn time_text() -> impl Strategy<Value = String> {
    prop_oneof![
        r"20[0-9]{2}-[01][0-9]-[0-3][0-9]T[0-2][0-9]:[0-5][0-9]:[0-6][0-9](\.[0-9]{1,3})?Z",
        text(),
    ]
}

fn interval_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "R[0-9]{0,2}/20[0-9]{2}-0[1-9]-1[0-9]T00:00:00Z/P[0-9]{1,2}[YMWD]",
        "R[0-9-]{0,3}/[0-9TZ:.-]{0,24}/P[0-9YMWDTHS.]{0,16}",
    ]
}

// An Ed25519 key that works, the identity point, 32 bytes that may not be
// a point, other lengths, and anything.
fn public_key_text() -> impl Strategy<Value = String> {
    let ours = URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(&[3; 32]).verifying_key().as_bytes());
    let mut identity = [0u8; 32];
    identity[0] = 1;
    prop_oneof![
        3 => Just(ours),
        1 => Just(URL_SAFE_NO_PAD.encode(identity)),
        1 => any::<[u8; 32]>().prop_map(|bytes| URL_SAFE_NO_PAD.encode(bytes)),
        1 => prop::collection::vec(any::<u8>(), 0..48).prop_map(|bytes| STANDARD.encode(bytes)),
        1 => text(),
    ]
}

fn boolean() -> Json {
    any::<bool>().prop_map(|value| value.to_string()).boxed()
}

fn asset_code() -> Json {
    strings(prop_oneof![6 => "[A-Z]{3}", 1 => "[ -~]{0,40}", 1 => any::<String>()])
}

fn asset_scale() -> Json {
    prop_oneof![
        4 => (0u8..=255).prop_map(|scale| scale.to_string()),
        1 => number(),
    ]
    .boxed()
}

fn metadata() -> Json {
    prop::collection::vec((name_text(), any_json()), 0..4)
        .prop_map(json_object)
        .boxed()
}

fn amount(noisy: bool) -> Json {
    object(
        noisy,
        vec![
            usually("value", strings(decimal_text())),
            usually("assetCode", asset_code()),
            usually("assetScale", asset_scale()),
        ],
    )
}

fn kid_text() -> impl Strategy<Value = String> {
    prop_oneof![3 => "k[12]", 1 => "[a-z]{250,260}", 1 => text()]
}

fn jwk(noisy: bool) -> Json {
    object(
        noisy,
        vec![
            lenient("kid", strings(kid_text())),
            lenient("alg", mostly(noisy, "EdDSA", "RS256")),
            maybe("use", mostly(noisy, "sig", "enc")),
            lenient("kty", mostly(noisy, "OKP", "RSA")),
            lenient("crv", mostly(noisy, "Ed25519", "X25519")),
            lenient("x", strings(public_key_text())),
        ],
    )
}

// Up to past `MAX_KEYS`.
fn jwks(noisy: bool) -> Json {
    object(noisy, vec![usually("keys", array(jwk(noisy), 18))])
}

fn client(noisy: bool) -> Json {
    prop_oneof![
        url(),
        object(noisy, vec![usually("walletAddress", url())]),
        object(noisy, vec![usually("jwk", jwk(noisy))]),
    ]
    .boxed()
}

fn limits(noisy: bool) -> Json {
    object(
        noisy,
        vec![
            maybe("receiver", url()),
            maybe("interval", strings(interval_text())),
            maybe("debitAmount", amount(noisy)),
            maybe("receiveAmount", amount(noisy)),
        ],
    )
}

const KINDS: [&str; 3] = ["incoming-payment", "outgoing-payment", "quote"];
const ACTIONS: [&str; 6] = ["create", "complete", "read", "read-all", "list", "list-all"];

// An access item: in a clean document mostly with the actions and members
// its kind allows, in a noisy one with any.
fn access(noisy: bool) -> Json {
    let item = |kind: &'static str, allowed: &[&'static str], identifier, limited| {
        let kind = if noisy {
            one_of(noisy, &KINDS)
        } else {
            one_of(noisy, &[kind])
        };
        let actions = words(noisy, if noisy { &ACTIONS } else { allowed }, 4);
        object(
            noisy,
            vec![
                usually("type", kind),
                usually("actions", actions),
                ("identifier", identifier, url()),
                ("limits", limited, limits(noisy)),
                rarely("cardAuthorization", any_json()),
            ],
        )
    };
    prop_oneof![
        item("incoming-payment", &ACTIONS, Chance::Maybe, Chance::Rarely),
        item(
            "outgoing-payment",
            &["create", "read", "read-all", "list", "list-all"],
            Chance::Usually,
            Chance::Maybe,
        ),
        item(
            "quote",
            &["create", "read", "read-all"],
            Chance::Rarely,
            Chance::Rarely
        ),
    ]
    .boxed()
}

fn subject(noisy: bool) -> Json {
    let id = object(
        noisy,
        vec![
            usually("id", url()),
            usually("format", one_of(noisy, &["uri", "email"])),
        ],
    );
    object(noisy, vec![usually("sub_ids", array(id, 2))])
}

fn grant_request(noisy: bool) -> Json {
    let finish = object(
        noisy,
        vec![
            usually("method", one_of(noisy, &["redirect"])),
            usually("uri", url()),
            usually("nonce", strings(token_text())),
        ],
    );
    let interact = object(
        noisy,
        vec![
            usually("start", words(noisy, &["redirect", "user_code"], 2)),
            maybe("finish", finish),
        ],
    );
    let access_token = object(noisy, vec![usually("access", array(access(noisy), 4))]);
    object(
        noisy,
        vec![
            usually("access_token", access_token),
            usually("client", client(noisy)),
            usually("interact", interact),
            rarely("subject", subject(noisy)),
        ],
    )
}

fn continue_request(noisy: bool) -> Json {
    object(noisy, vec![maybe("interact_ref", strings(token_text()))])
}

fn grant_response(noisy: bool) -> Json {
    let interact = object(
        noisy,
        vec![
            usually("redirect", url()),
            usually("finish", strings(token_text())),
        ],
    );
    let token = object(
        noisy,
        vec![
            usually("value", strings(token_text())),
            usually("manage", url()),
            maybe("expires_in", number()),
            usually("access", array(access(noisy), 3)),
        ],
    );
    let continue_token = object(noisy, vec![usually("value", strings(token_text()))]);
    let continuation = object(
        noisy,
        vec![
            usually("access_token", continue_token),
            usually("uri", url()),
            maybe("wait", number()),
        ],
    );
    object(
        noisy,
        vec![
            maybe("interact", interact),
            maybe("access_token", token),
            usually("continue", continuation),
            rarely("subject", subject(noisy)),
        ],
    )
}

fn payment_request(noisy: bool) -> Json {
    let incoming_payment = vec![
        usually("walletAddress", url()),
        maybe("incomingAmount", amount(noisy)),
        maybe("expiresAt", strings(time_text())),
        maybe("metadata", metadata()),
    ];
    let quote = vec![
        usually("walletAddress", url()),
        usually("receiver", url()),
        usually("method", one_of(noisy, &["ilp"])),
        maybe("debitAmount", amount(noisy)),
        maybe("receiveAmount", amount(noisy)),
    ];
    let outgoing_payment = vec![
        usually("walletAddress", url()),
        maybe("quoteId", url()),
        maybe("incomingPayment", url()),
        maybe("debitAmount", amount(noisy)),
        maybe("metadata", metadata()),
    ];
    prop_oneof![
        object(noisy, incoming_payment),
        object(noisy, quote),
        object(noisy, outgoing_payment),
    ]
    .boxed()
}

fn wallet_response(noisy: bool) -> Json {
    let wallet_address = vec![
        usually("id", url()),
        maybe("publicName", strings(text())),
        usually("assetCode", asset_code()),
        usually("assetScale", asset_scale()),
        usually("authServer", url()),
        usually("resourceServer", url()),
    ];
    let method = object(
        noisy,
        vec![
            usually("type", one_of(noisy, &["ilp"])),
            usually("ilpAddress", strings(r"test\.[a-z.]{0,20}")),
            usually("sharedSecret", strings(public_key_text())),
        ],
    );
    let incoming_payment = vec![
        usually("id", url()),
        usually("walletAddress", url()),
        usually("completed", boolean()),
        maybe("incomingAmount", amount(noisy)),
        usually("receivedAmount", amount(noisy)),
        maybe("expiresAt", strings(time_text())),
        maybe("metadata", metadata()),
        usually("createdAt", strings(time_text())),
        maybe("methods", array(method, 2)),
    ];
    let public_incoming_payment = vec![
        maybe("receivedAmount", amount(noisy)),
        usually("authServer", url()),
    ];
    prop_oneof![
        object(noisy, wallet_address),
        object(noisy, incoming_payment),
        object(noisy, public_incoming_payment),
    ]
    .boxed()
}

proptest! {
    #[test]
    fn amounts_never_panic(input in both(amount)) {
        let _ = serde_json::from_str::<Amount>(&input);
    }

    #[test]
    fn grant_requests_never_panic(input in both(grant_request)) {
        if let Ok(request) = serde_json::from_str::<GrantRequest>(&input) {
            let _ = request.validate();
        }
    }

    #[test]
    fn continue_requests_never_panic(input in both(continue_request)) {
        let _ = serde_json::from_str::<ContinueRequest>(&input);
    }

    #[test]
    fn grant_responses_never_panic(input in both(grant_response)) {
        let _ = serde_json::from_str::<GrantResponse>(&input);
    }

    #[test]
    fn payment_requests_never_panic(input in both(payment_request)) {
        if let Ok(request) = serde_json::from_str::<CreateIncomingPayment>(&input) {
            let _ = request.metadata.as_ref().map(resources::metadata_fits);
        }
        let _ = serde_json::from_str::<CreateQuote>(&input);
        let _ = serde_json::from_str::<CreateOutgoingPayment>(&input);
    }

    #[test]
    fn wallet_responses_never_panic(input in both(wallet_response)) {
        let _ = serde_json::from_str::<WalletAddress>(&input);
        let _ = serde_json::from_str::<IncomingPayment>(&input);
        let _ = serde_json::from_str::<PublicIncomingPayment>(&input);
    }

    // A key set comes from another wallet's `jwks.json`.
    #[test]
    fn key_sets_never_panic(input in prop_oneof![both(jwk), both(jwks)], kid in kid_text()) {
        if let Ok(key) = serde_json::from_str::<Jwk>(&input) {
            let _ = key.verifying_key();
        }
        if let Ok(set) = serde_json::from_str::<Jwks>(&input) {
            for key in &set.keys {
                let _ = key.verifying_key();
            }
            let _ = set.find(&kid);
        }
    }

    #[test]
    fn interaction_hashes_never_panic(
        hash in prop_oneof![
            text(),
            prop::collection::vec(any::<u8>(), 0..40).prop_map(|bytes| URL_SAFE_NO_PAD.encode(bytes)),
            prop::collection::vec(any::<u8>(), 0..40).prop_map(|bytes| STANDARD.encode(bytes)),
        ],
        (client, server, reference, grant) in (text(), text(), text(), url_text()),
    ) {
        let _ = gnap::check_interaction_hash(&hash, &client, &server, &reference, &grant);
        let right = gnap::interaction_hash(&client, &server, &reference, &grant);
        prop_assert!(gnap::check_interaction_hash(&right, &client, &server, &reference, &grant));
    }
}

// Signed requests: signature fields under one label, so that `find` gets
// past the label and length and on to the time, `alg` and `keyid`, and
// `verify` and the digest get bodies and `Content-Digest` values of every
// kind. The verifier's clock is `NOW_S`.

const NOW_S: u64 = 1_700_000_000;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

fn component_name() -> impl Strategy<Value = String> {
    let names = vec![
        "@method",
        "@target-uri",
        "@authority",
        "@scheme",
        "@path",
        "@query",
        "@request-target",
        "@status",
        "authorization",
        "content-digest",
        "content-length",
        "content-type",
        "date",
        "Date",
        "signature",
    ];
    prop_oneof![
        4 => prop::sample::select(names).prop_map(str::to_string),
        1 => "[a-z@-]{0,10}",
    ]
}

// Components as `(name, required, form)`; see `signature_input`.
fn components() -> impl Strategy<Value = Vec<(String, bool, u8)>> {
    prop::collection::vec((component_name(), any::<bool>(), 0u8..10), 0..8)
}

// What a signer covers: the profile's components in any order, or any.
fn signed_components() -> impl Strategy<Value = Vec<String>> {
    let profile = vec![
        "@method",
        "@target-uri",
        "authorization",
        "content-digest",
        "content-length",
        "content-type",
    ];
    prop_oneof![
        2 => prop::sample::subsequence(profile, 0..=6)
            .prop_map(|names| names.into_iter().map(str::to_string).collect::<Vec<_>>())
            .prop_shuffle(),
        1 => prop::collection::vec(component_name(), 0..8),
    ]
}

// A time near the verifier's clock, or no time at all.
fn time_parameter() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => (NOW_S - 400..NOW_S + 400).prop_map(|seconds| seconds.to_string()),
        1 => "-?[0-9]{1,16}",
        1 => r#""[0-9]{1,10}"|[0-9]{1,4}\.[0-9]{1,3}|\?[01]|[a-z]{1,5}"#,
    ]
}

// `created`, `expires`, `alg`, `keyid` and `nonce`, each there or not, in
// any order.
fn signature_parameters() -> impl Strategy<Value = Vec<String>> {
    (
        prop::option::weighted(0.95, time_parameter()),
        prop::option::weighted(0.2, time_parameter()),
        prop::option::weighted(
            0.4,
            prop_oneof![Just(r#""ed25519""#.to_string()), r#""[a-z0-9-]{0,16}"|ed25519|[0-9]{1,3}"#],
        ),
        prop::option::weighted(
            0.95,
            prop_oneof![3 => Just(r#""k1""#.to_string()), 1 => r#""[ !#-\[\]-~]{0,20}"|[a-z][a-z0-9]{0,8}|[0-9]{1,3}"#],
        ),
        prop::option::weighted(0.1, r#""[a-z]{0,8}"|\?[01]"#),
    )
        .prop_map(|(created, expires, alg, keyid, nonce)| {
            [("created", created), ("expires", expires), ("alg", alg), ("keyid", keyid), ("nonce", nonce)]
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| format!(";{key}={value}")))
                .collect::<Vec<_>>()
        })
        .prop_shuffle()
}

// Each component quoted, quoted with a parameter, or as a token.
fn signature_input(
    label: &str,
    components: &[(String, bool, u8)],
    parameters: &[String],
) -> String {
    let items: Vec<String> = components
        .iter()
        .map(|(name, _, form)| match form {
            0 => format!("\"{name}\";req"),
            1 => name.clone(),
            _ => format!("\"{name}\""),
        })
        .collect();
    format!("{label}=({}){}", items.join(" "), parameters.concat())
}

// A `Content-Digest` of any algorithm and length, or any text.
fn digest_text() -> impl Strategy<Value = String> {
    let algorithms = vec!["sha-256", "sha-512", "md5"];
    prop_oneof![
        (
            prop::sample::select(algorithms),
            prop::collection::vec(any::<u8>(), 0..70)
        )
            .prop_map(|(algorithm, bytes)| sf::serialize_byte_dictionary(&[(algorithm, &bytes)])),
        text(),
    ]
}

// A body with its header lines: `Content-Digest` and `Content-Length`
// right, wrong or missing, and the others there or not.
fn body_and_headers() -> impl Strategy<Value = (Vec<u8>, Vec<(String, String)>)> {
    prop::collection::vec(any::<u8>(), 0..48).prop_flat_map(|body| {
        let digest = httpsig::content_digest(&body);
        let length = body.len().to_string();
        let lines = (
            prop::option::weighted(0.8, prop_oneof![Just(digest), digest_text()]),
            prop::option::weighted(0.8, prop_oneof![Just(length), "[0-9]{1,3}"]),
            prop::option::weighted(0.8, "GNAP [a-z]{1,8}"),
            prop::option::weighted(0.8, Just("application/json".to_string())),
            prop::option::of("[ -~]{0,30}"),
        )
            .prop_map(|(digest, length, authorization, content_type, date)| {
                [
                    ("Content-Digest", digest),
                    ("Content-Length", length),
                    ("Authorization", authorization),
                    ("Content-Type", content_type),
                    ("Date", date),
                ]
                .into_iter()
                .filter_map(|(name, value)| value.map(|value| (name.to_string(), value)))
                .collect::<Vec<_>>()
            });
        (Just(body), lines)
    })
}

fn target() -> impl Strategy<Value = (&'static str, String)> {
    (
        prop::sample::select(vec!["GET", "POST"]),
        prop_oneof![
            Just("https://b.example/incoming-payments?x=1".to_string()),
            url_text()
        ],
    )
}

fn lines(fields: &[(String, String)]) -> Vec<(&str, &str)> {
    fields
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

proptest! {
    #[test]
    fn signatures_never_panic(
        label in prop_oneof![Just("sig1".to_string()), "[a-z][a-z0-9-]{0,6}"],
        components in components(),
        parameters in signature_parameters(),
        bytes in prop_oneof![
            9 => prop::collection::vec(any::<u8>(), 64),
            1 => prop::collection::vec(any::<u8>(), 0..80),
        ],
        (body, mut fields) in body_and_headers(),
        (method, target_uri) in target(),
    ) {
        let required: Vec<&str> = components
            .iter()
            .filter(|(_, required, form)| *required && *form != 1)
            .map(|(name, _, _)| name.as_str())
            .collect();
        let digest = fields
            .iter()
            .find(|(name, _)| name == "Content-Digest")
            .map(|(_, value)| value.clone());
        fields.push(("Signature-Input".to_string(), signature_input(&label, &components, &parameters)));
        fields.push(("Signature".to_string(), sf::serialize_byte_dictionary(&[(&label, &bytes)])));
        let fields = lines(&fields);
        let request = Request { method, target_uri: &target_uri, fields: &fields };
        if let Ok(signature) = Signature::find(&request, &required, NOW_S) {
            let _ = signature.verify(&request, &body, &key().verifying_key());
        }
        if let Some(digest) = digest {
            let _ = httpsig::check_digest(&digest, &body);
        }
    }

    // Signed for real, so that `verify` gets past the signature to the
    // digest; the body received may not be the one signed.
    #[test]
    fn signed_requests_never_panic(
        components in signed_components(),
        created in NOW_S - 400..NOW_S + 400,
        keyid in "[ -~]{0,12}",
        (body, mut fields) in body_and_headers(),
        other in prop::option::weighted(0.2, prop::collection::vec(any::<u8>(), 0..48)),
        (method, target_uri) in target(),
    ) {
        let names: Vec<&str> = components.iter().map(String::as_str).collect();
        let signed = {
            let unsigned = lines(&fields);
            let request = Request { method, target_uri: &target_uri, fields: &unsigned };
            httpsig::sign(&request, &names, &key(), &keyid, created)
        };
        let Ok(signed) = signed else {
            return Ok(());
        };
        fields.push(("Signature-Input".to_string(), signed.signature_input));
        fields.push(("Signature".to_string(), signed.signature));
        let fields = lines(&fields);
        let request = Request { method, target_uri: &target_uri, fields: &fields };
        match Signature::find(&request, &names, NOW_S) {
            Ok(signature) => {
                prop_assert_eq!(signature.keyid(), keyid.as_str());
                let received = other.as_deref().unwrap_or(&body);
                let verified = signature.verify(&request, received, &key().verifying_key());
                if other.is_none() {
                    prop_assert!(matches!(verified, Ok(()) | Err(SignatureError::Digest)), "{verified:?}");
                }
            }
            Err(error) => prop_assert_eq!(error, SignatureError::Time),
        }
    }
}

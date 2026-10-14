//! Every decoder of untrusted input returns an error instead of panicking
//! (D16), and the encoders round-trip.

use dassie_payments::gnap::{self, GrantRequest};
use dassie_payments::httpsig::{Request, Signature};
use dassie_payments::resources::{CreateIncomingPayment, ListQuery};
use dassie_payments::{pointer, sf, time, uri};
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
    }

    #[test]
    fn signature_fields_never_panic(input in text(), signature in text()) {
        let fields = [("Signature-Input", input.as_str()), ("Signature", signature.as_str())];
        let request = Request { method: "GET", target_uri: "https://a.example/", fields: &fields };
        let _ = Signature::find(&request, &["@method"], 1_000);
    }
}

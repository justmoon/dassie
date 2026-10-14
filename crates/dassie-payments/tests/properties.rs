//! Every decoder of untrusted input returns an error instead of panicking
//! (D16), and the encoders round-trip.

use dassie_payments::gnap::{self, GrantRequest};
use dassie_payments::httpsig::{Request, Signature};
use dassie_payments::interval::{Duration, Interval};
use dassie_payments::quote::Rate;
use dassie_payments::resources::{
    CreateIncomingPayment, CreateOutgoingPayment, CreateQuote, ListQuery,
};
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

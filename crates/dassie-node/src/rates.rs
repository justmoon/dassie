//! Exchange rates (D7), without IO: where they come from, what each
//! source says, and the rate the node takes from what its sources say.
//!
//! Every rate is how many units of a currency one US dollar buys, times
//! 10^[`RATE_PRECISION`], as the connector converts with. Each source is
//! read in exact decimal arithmetic. A currency's rate is the median of
//! the sources that cover it and answered, once more than half of them did;
//! otherwise it keeps its last rate, which the connector stops using once
//! it is an hour old.

use dassie_connector::RATE_PRECISION;

/// The currency every rate is relative to.
pub const BASE: &str = "USD";

/// The currencies the node keeps rates for: its ledgers' assets, and
/// those its owner may see totals in.
pub const CURRENCIES: [&str; 8] = ["USD", "XRP", "EUR", "GBP", "JPY", "CHF", "CAD", "AUD"];

/// Most rate sources.
pub const MAX_SOURCES: usize = 8;

/// The longest answer read from a source.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

/// One unit, as a rate.
const ONE: u128 = 10u128.pow(RATE_PRECISION);

/// The currency of `code`, if it is one the node keeps.
#[must_use]
pub fn currency(code: &str) -> Option<&'static str> {
    CURRENCIES.iter().copied().find(|known| *known == code)
}

/// How a source answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SourceKind {
    /// Coinbase's `exchange-rates`, `{data: {currency, rates: {CODE:
    /// "decimal"}}}`: units of each currency per unit of `currency`. Others
    /// serve the format too, such as the dev environment's `rates.json`.
    Coinbase,
    /// Kraken's ticker for XRP in US dollars.
    Kraken,
    /// Bitstamp's ticker for XRP in US dollars.
    Bitstamp,
}

impl SourceKind {
    /// Every kind.
    pub const ALL: [Self; 3] = [Self::Coinbase, Self::Kraken, Self::Bitstamp];

    /// Its name, as the database and the wallet have it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coinbase => "coinbase",
            Self::Kraken => "kraken",
            Self::Bitstamp => "bitstamp",
        }
    }

    /// The kind called `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    /// Where it answers, unless told otherwise.
    #[must_use]
    pub const fn default_url(self) -> &'static str {
        match self {
            Self::Coinbase => "https://api.coinbase.com/v2/exchange-rates?currency=USD",
            Self::Kraken => "https://api.kraken.com/0/public/Ticker?pair=XRPUSD",
            Self::Bitstamp => "https://www.bitstamp.net/api/v2/ticker/xrpusd/",
        }
    }

    /// Whether it gives a rate for `code` when it answers well.
    #[must_use]
    pub fn covers(self, code: &str) -> bool {
        match self {
            Self::Coinbase => true,
            Self::Kraken | Self::Bitstamp => code == "XRP",
        }
    }
}

/// Where the node fetches rates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateSource {
    /// How it answers.
    pub kind: SourceKind,
    /// Where, if not the kind's own server: Coinbase's format only.
    pub url: Option<String>,
}

impl RateSource {
    /// A kind's own server.
    #[must_use]
    pub const fn of(kind: SourceKind) -> Self {
        Self { kind, url: None }
    }

    /// The sources a new node starts with: all three, so that one wrong
    /// or silent source does not move a rate.
    #[must_use]
    pub fn defaults() -> Vec<Self> {
        SourceKind::ALL.into_iter().map(Self::of).collect()
    }

    /// Where to fetch it.
    #[must_use]
    pub fn url(&self) -> &str {
        self.url.as_deref().unwrap_or(self.kind.default_url())
    }

    /// Why it cannot be kept, if it cannot: a URL only for Coinbase's
    /// format, and only HTTPS.
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let url = self.url.as_deref()?;
        if self.kind != SourceKind::Coinbase {
            return Some("only a Coinbase-format source takes a URL");
        }
        let valid = url.len() <= 2048
            && url
                .strip_prefix("https://")
                .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'));
        (!valid).then_some("a source's URL must be an HTTPS URL")
    }
}

/// Reads a non-negative decimal, such as `"0.4123"`, as a rate: times
/// 10^[`RATE_PRECISION`], rounded down. `None` for anything else, or a
/// number too large.
#[must_use]
pub fn decimal(text: &str) -> Option<u128> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) || whole.len() > 21 {
        return None;
    }
    if text.ends_with('.') {
        return None;
    }
    let mut value: u128 = 0;
    for byte in whole.bytes() {
        value = value
            .checked_mul(10)?
            .checked_add(u128::from(byte - b'0'))?;
    }
    let precision = usize::try_from(RATE_PRECISION).ok()?;
    let mut places = 0;
    for byte in fraction.bytes().take(precision) {
        value = value
            .checked_mul(10)?
            .checked_add(u128::from(byte - b'0'))?;
        places += 1;
    }
    for _ in places..precision {
        value = value.checked_mul(10)?;
    }
    Some(value)
}

/// Writes a rate as a decimal, without trailing zeros: `"0.5"`.
#[must_use]
pub fn format(rate: u128) -> String {
    let whole = rate / ONE;
    let fraction = rate % ONE;
    if fraction == 0 {
        return whole.to_string();
    }
    let width = usize::try_from(RATE_PRECISION).unwrap_or(15);
    let digits = format!("{fraction:0width$}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

/// `1 / rate`, as a rate: dollars per unit become units per dollar.
fn invert(rate: u128) -> Option<u128> {
    if rate == 0 {
        return None;
    }
    let inverted = ONE.checked_mul(ONE)? / rate;
    (inverted > 0).then_some(inverted)
}

/// Why an answer was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable(pub &'static str);

/// What a source's answer says: the rates of the currencies the node keeps,
/// in units per dollar.
///
/// # Errors
///
/// The answer is not what the kind answers.
pub fn parse(kind: SourceKind, body: &[u8]) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let json: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| Unreadable("not JSON"))?;
    match kind {
        SourceKind::Coinbase => coinbase(&json),
        SourceKind::Kraken => {
            let pair = json
                .get("result")
                .and_then(serde_json::Value::as_object)
                .and_then(|result| result.values().next())
                .ok_or(Unreadable("no ticker"))?;
            let last = pair
                .get("c")
                .and_then(|trade| trade.get(0))
                .and_then(serde_json::Value::as_str)
                .ok_or(Unreadable("no last trade"))?;
            dollars_per_xrp(last)
        }
        SourceKind::Bitstamp => {
            let last = json
                .get("last")
                .and_then(serde_json::Value::as_str)
                .ok_or(Unreadable("no last trade"))?;
            dollars_per_xrp(last)
        }
    }
}

fn dollars_per_xrp(text: &str) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let price = decimal(text).ok_or(Unreadable("not a price"))?;
    let rate = invert(price).ok_or(Unreadable("not a price"))?;
    Ok(vec![("XRP", rate)])
}

/// Coinbase's format, from any base: each rate over the dollar's.
fn coinbase(json: &serde_json::Value) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let data = json.get("data").ok_or(Unreadable("no data"))?;
    let base = data
        .get("currency")
        .and_then(serde_json::Value::as_str)
        .ok_or(Unreadable("no base currency"))?;
    let rates = data
        .get("rates")
        .and_then(serde_json::Value::as_object)
        .ok_or(Unreadable("no rates"))?;
    let read = |code: &str| {
        if code == base {
            return Some(ONE);
        }
        rates
            .get(code)
            .and_then(serde_json::Value::as_str)
            .and_then(decimal)
            .filter(|rate| *rate > 0)
    };
    // Units of the base one dollar buys.
    let dollar = read(BASE).ok_or(Unreadable("no rate for the dollar"))?;
    Ok(CURRENCIES
        .iter()
        .filter_map(|&code| {
            let rate = read(code)?.checked_mul(ONE)? / dollar;
            (rate > 0).then_some((code, rate))
        })
        .collect())
}

/// What one source said in a cycle: its rates, or nothing, when it did
/// not answer or answered what could not be read.
pub type Answer = (SourceKind, Option<Vec<(&'static str, u128)>>);

/// The rates the node takes for `wanted` from its sources' answers: for
/// each currency, the median of the sources that cover it and gave a rate
/// for it, once more than half of those that cover it did. The dollar is
/// one by definition.
#[must_use]
pub fn combine(wanted: &[&'static str], answers: &[Answer]) -> Vec<(&'static str, u128)> {
    wanted
        .iter()
        .filter_map(|&code| {
            if code == BASE {
                return Some((code, ONE));
            }
            let covering = answers.iter().filter(|(kind, _)| kind.covers(code)).count();
            let mut said: Vec<u128> = answers
                .iter()
                .filter(|(kind, _)| kind.covers(code))
                .filter_map(|(_, rates)| {
                    rates
                        .as_ref()?
                        .iter()
                        .find(|(c, _)| *c == code)
                        .map(|(_, rate)| *rate)
                })
                .collect();
            if said.is_empty() || said.len() * 2 <= covering {
                return None;
            }
            said.sort_unstable();
            let middle = said.len() / 2;
            let median = if said.len() % 2 == 1 {
                said.get(middle).copied()
            } else {
                // Two in the middle must agree, within 1%: else one wrong
                // source would move the rate halfway to it.
                // A spread too large to scale is far past 1% of either.
                let (low, high) = (said.get(middle - 1)?, said.get(middle)?);
                if high.checked_sub(*low)?.checked_mul(100)? > *high {
                    return None;
                }
                Some(low / 2 + high / 2 + (low % 2 + high % 2) / 2)
            };
            median.map(|rate| (code, rate))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use proptest::prelude::*;

    use super::*;

    fn rate(text: &str) -> u128 {
        decimal(text).unwrap()
    }

    #[test]
    fn decimals_are_exact() {
        assert_eq!(decimal("1"), Some(ONE));
        assert_eq!(decimal("0.5"), Some(ONE / 2));
        assert_eq!(decimal("2.123456789012345678"), Some(2_123_456_789_012_345));
        assert_eq!(decimal("0.000000000000001"), Some(1));
        assert_eq!(decimal("0.0000000000000009"), Some(0));
        assert_eq!(decimal("150.25"), Some(150_250_000_000_000_000));
        for bad in [
            "", ".", "1.", ".5", "-1", "1e3", "0x10", " 1", "1,5", "1.2.3",
        ] {
            assert_eq!(decimal(bad), None, "{bad}");
        }
        assert_eq!(decimal(&"9".repeat(22)), None);
        assert_eq!(format(rate("0.5")), "0.5");
        assert_eq!(format(rate("2")), "2");
        assert_eq!(format(rate("1.000000000000001")), "1.000000000000001");
    }

    // Recorded answers, shortened.
    const COINBASE: &str = r#"{"data":{"currency":"USD","rates":{"AED":"3.6725","EUR":"0.9238","USD":"1.0","XRP":"1.8656716417910448","JPY":"149.62"}}}"#;
    const KRAKEN: &str = r#"{"error":[],"result":{"XXRPZUSD":{"a":["0.53710000","1","1.000"],"b":["0.53700000","5","5.000"],"c":["0.53600000","25.00000000"],"v":["1","2"]}}}"#;
    const BITSTAMP: &str = r#"{"timestamp": "1729000000", "open": "0.53", "high": "0.54", "low": "0.52", "last": "0.5370", "volume": "1"}"#;

    #[test]
    fn each_kind_is_read() {
        let coinbase = parse(SourceKind::Coinbase, COINBASE.as_bytes()).unwrap();
        assert!(coinbase.contains(&("USD", ONE)));
        assert!(coinbase.contains(&("XRP", 1_865_671_641_791_044)));
        assert!(coinbase.contains(&("JPY", 149_620_000_000_000_000)));
        assert!(!coinbase.iter().any(|(code, _)| *code == "AED"), "not kept");
        // 1 / 0.536 XRP per dollar.
        let kraken = parse(SourceKind::Kraken, KRAKEN.as_bytes()).unwrap();
        assert_eq!(kraken, [("XRP", 1_865_671_641_791_044)]);
        let bitstamp = parse(SourceKind::Bitstamp, BITSTAMP.as_bytes()).unwrap();
        assert_eq!(bitstamp, [("XRP", 1_862_197_392_923_649)]);
    }

    #[test]
    fn another_base_is_rebased_on_the_dollar() {
        let euro = r#"{"data":{"currency":"EUR","rates":{"USD":"1.25","XRP":"2.5"}}}"#;
        let rates = parse(SourceKind::Coinbase, euro.as_bytes()).unwrap();
        assert!(rates.contains(&("USD", ONE)));
        assert!(rates.contains(&("XRP", 2 * ONE)));
        assert!(rates.contains(&("EUR", 800_000_000_000_000)));
        let no_dollar = r#"{"data":{"currency":"EUR","rates":{"XRP":"2.5"}}}"#;
        assert!(parse(SourceKind::Coinbase, no_dollar.as_bytes()).is_err());
    }

    #[test]
    fn what_is_not_an_answer_is_refused() {
        for (kind, body) in [
            (SourceKind::Coinbase, "{}"),
            (SourceKind::Coinbase, "[1]"),
            (SourceKind::Coinbase, "not json"),
            (
                SourceKind::Kraken,
                r#"{"error":["EQuery:Unknown asset pair"],"result":{}}"#,
            ),
            (SourceKind::Kraken, r#"{"result":{"X":{"c":[0.5]}}}"#),
            (SourceKind::Bitstamp, r#"{"last": "0"}"#),
            (SourceKind::Bitstamp, r#"{"last": 0.5}"#),
        ] {
            assert!(parse(kind, body.as_bytes()).is_err(), "{kind:?} {body}");
        }
        // A rate of zero, or unreadable, is left out.
        let zero = r#"{"data":{"currency":"USD","rates":{"XRP":"0","EUR":"x"}}}"#;
        assert_eq!(
            parse(SourceKind::Coinbase, zero.as_bytes()).unwrap(),
            [("USD", ONE)]
        );
    }

    #[test]
    fn the_median_of_more_than_half() {
        let said = |rates: &[(&'static str, &str)]| {
            Some(
                rates
                    .iter()
                    .map(|(code, text)| (*code, rate(text)))
                    .collect(),
            )
        };
        let answers = [
            (SourceKind::Coinbase, said(&[("XRP", "2"), ("EUR", "0.9")])),
            (SourceKind::Kraken, said(&[("XRP", "1.9")])),
            (SourceKind::Bitstamp, said(&[("XRP", "100")])),
        ];
        let wanted = ["USD", "XRP", "EUR", "GBP"];
        let combined = combine(&wanted, &answers);
        // One wild source does not move the rate; only Coinbase covers
        // the euro, and it answered; nobody gave the pound.
        assert_eq!(
            combined,
            [("USD", ONE), ("XRP", rate("2")), ("EUR", rate("0.9"))]
        );
        // Two of three answered: the middle of two, if they agree.
        let two = [
            answers[0].clone(),
            (SourceKind::Kraken, said(&[("XRP", "1.99")])),
            (SourceKind::Bitstamp, None),
        ];
        assert_eq!(combine(&["XRP"], &two), [("XRP", rate("1.995"))]);
        let apart = [
            answers[0].clone(),
            answers[1].clone(),
            (SourceKind::Bitstamp, None),
        ];
        assert_eq!(combine(&["XRP"], &apart), [], "1.9 and 2 are 5% apart");
        // One of three is not more than half: the last rate stays.
        let one = [
            answers[0].clone(),
            (SourceKind::Kraken, None),
            (SourceKind::Bitstamp, None),
        ];
        assert_eq!(combine(&["XRP"], &one), []);
        // Nor one of two.
        assert_eq!(combine(&["XRP"], &one[..2]), []);
        assert_eq!(combine(&["XRP"], &[]), []);
    }

    // A source may name rates up to about u128::MAX: scaling the spread of
    // two such once overflowed, and the panic ended the rate task.
    #[test]
    fn rates_far_apart_do_not_overflow() {
        let wild =
            r#"{"data":{"currency":"EUR","rates":{"USD":"0.000000000000001","XRP":"340000000"}}}"#;
        let coinbase = parse(SourceKind::Coinbase, wild.as_bytes()).unwrap();
        assert!(coinbase.contains(&("XRP", 340_000_000 * ONE * ONE)));
        let answers = [
            (SourceKind::Coinbase, Some(coinbase)),
            (SourceKind::Kraken, Some(vec![("XRP", 1)])),
            (SourceKind::Bitstamp, None),
        ];
        assert_eq!(combine(&["XRP"], &answers), []);
    }

    /// What a source may have said: any rates, or nothing.
    fn said() -> impl Strategy<Value = Option<Vec<(&'static str, u128)>>> {
        let code = proptest::sample::select(CURRENCIES.as_slice());
        let rate = prop_oneof![any::<u128>(), 0..10 * ONE, Just(u128::MAX)];
        proptest::option::of(proptest::collection::vec((code, rate), 0..4))
    }

    /// Bodies shaped like each kind's answers, with any numbers in them.
    fn body() -> impl Strategy<Value = Vec<u8>> {
        let number = prop_oneof![
            "[0-9]{1,25}(\\.[0-9]{0,20})?",
            "[0-9.e+-]{0,8}",
            "\\PC{0,8}"
        ];
        let code = prop_oneof![
            proptest::sample::select(CURRENCIES.as_slice()).prop_map(str::to_owned),
            "[A-Z]{3}",
        ];
        let coinbase = (
            code.clone(),
            proptest::collection::vec((code, number.clone()), 0..6),
        )
            .prop_map(|(base, rates)| {
                let rates: serde_json::Map<_, _> = rates
                    .into_iter()
                    .map(|(code, rate)| (code, serde_json::Value::String(rate)))
                    .collect();
                serde_json::json!({"data": {"currency": base, "rates": rates}}).to_string()
            });
        let kraken = number.clone().prop_map(|last| {
            serde_json::json!({"result": {"XXRPZUSD": {"c": [last, "1"]}}}).to_string()
        });
        let bitstamp = number.prop_map(|last| serde_json::json!({ "last": last }).to_string());
        prop_oneof![
            prop_oneof![coinbase, kraken, bitstamp].prop_map(String::into_bytes),
            proptest::collection::vec(any::<u8>(), 0..64),
        ]
    }

    proptest! {
        #[test]
        fn answers_never_panic(
            kind in proptest::sample::select(SourceKind::ALL.as_slice()),
            body in body(),
        ) {
            if let Ok(rates) = parse(kind, &body) {
                prop_assert!(rates.iter().all(|(code, rate)| CURRENCIES.contains(code) && *rate > 0));
            }
        }

        #[test]
        fn combining_never_panics(said in proptest::collection::vec(said(), 0..4)) {
            let answers: Vec<Answer> = SourceKind::ALL.into_iter().cycle().zip(said).collect();
            for (code, rate) in combine(&CURRENCIES, &answers) {
                if code == BASE {
                    continue;
                }
                // The median lies within what the sources said.
                let given = answers
                    .iter()
                    .filter_map(|(_, rates)| rates.as_ref())
                    .flatten()
                    .filter(|(c, _)| *c == code)
                    .map(|(_, rate)| *rate);
                prop_assert!(given.clone().min() <= Some(rate) && Some(rate) <= given.max());
            }
        }
    }

    #[test]
    fn sources_are_checked() {
        assert_eq!(RateSource::of(SourceKind::Kraken).problem(), None);
        let custom = |kind, url: &str| RateSource {
            kind,
            url: Some(url.into()),
        };
        assert_eq!(
            custom(SourceKind::Coinbase, "https://localhost:10100/rates.json").problem(),
            None
        );
        assert!(
            custom(SourceKind::Kraken, "https://example.com")
                .problem()
                .is_some()
        );
        for url in [
            "http://example.com",
            "https://",
            "https:///x",
            "example.com",
        ] {
            assert!(
                custom(SourceKind::Coinbase, url).problem().is_some(),
                "{url}"
            );
        }
        assert_eq!(
            RateSource::of(SourceKind::Bitstamp).url(),
            SourceKind::Bitstamp.default_url()
        );
        assert_eq!(RateSource::defaults().len(), 3);
        for kind in SourceKind::ALL {
            assert_eq!(SourceKind::parse(kind.as_str()), Some(kind));
        }
    }
}

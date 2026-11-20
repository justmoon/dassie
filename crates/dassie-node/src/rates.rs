//! Exchange rates (spec/decisions/exchange-rates.md), without IO: where they
//! come from, what each source says, and the rate the node takes from what
//! its sources say.
//!
//! Every rate is how many units of a currency one US dollar buys, times
//! 10^[`RATE_PRECISION`], as the connector converts with. Each source is
//! read in exact decimal arithmetic. A currency's rate is the median of
//! the sources that cover it and answered, once more than half of them did;
//! otherwise it keeps its last rate, which the connector stops using once
//! it is an hour old. A new node's sources cover every currency three
//! times, from three upstreams that do not share their data, so that one
//! wrong or silent source cannot take a rate outside what the others say.

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

/// How many days old the European Central Bank's newest rates may be. It
/// publishes them once on each TARGET business day, in the afternoon, and
/// its longest gap, at Easter, leaves Thursday's rates standing until
/// Tuesday's: five days. Older ones mean it, or the node's clock, stopped.
pub const MAX_ECB_AGE_DAYS: u64 = 5;

/// How many days old Norges Bank's newest rates may be. It publishes them
/// at about 16:00 CET on each Norwegian business day, and not on Christmas
/// Eve. Its longest gaps leave Wednesday's rates standing until Tuesday's
/// over Easter (Maundy Thursday, Good Friday, Easter Monday), and, with
/// Christmas Eve on a Monday, Friday's until Thursday's: six days.
pub const MAX_NORGES_BANK_AGE_DAYS: u64 = 6;

/// How many days old the Bank of Canada's newest rates may be. It
/// publishes them at 16:30 ET on each of its business days. Its longest
/// gap, Christmas and Boxing Day beside a weekend, leaves the rates of
/// the 23rd or 24th standing for five days; Easter, with Good Friday only,
/// takes four.
pub const MAX_BANK_OF_CANADA_AGE_DAYS: u64 = 5;

/// How many days old the Czech National Bank's newest fixing may be. It
/// fixes its rates after 14:30 CET on each Czech business day, and the
/// 24th to the 26th of December are holidays: with Christmas Eve on a
/// Monday, Friday's fixing stands until Thursday's, and on a Wednesday,
/// Tuesday's until Monday's: six days. Easter takes five.
pub const MAX_CNB_AGE_DAYS: u64 = 6;

/// A day, in milliseconds.
const MS_PER_DAY: u64 = 86_400_000;

/// A pair on Kraken's ticker: the currency, Kraken's two names for the
/// pair, and whether it is quoted in dollars per unit (else in units per
/// dollar).
struct KrakenPair {
    code: &'static str,
    name: &'static str,
    altname: &'static str,
    in_dollars: bool,
}

/// The pairs Kraken's ticker is asked for: one per currency the node
/// keeps, against the dollar. Its URL lists them by their altnames.
const KRAKEN_PAIRS: [KrakenPair; 7] = [
    KrakenPair {
        code: "XRP",
        name: "XXRPZUSD",
        altname: "XRPUSD",
        in_dollars: true,
    },
    KrakenPair {
        code: "EUR",
        name: "ZEURZUSD",
        altname: "EURUSD",
        in_dollars: true,
    },
    KrakenPair {
        code: "GBP",
        name: "ZGBPZUSD",
        altname: "GBPUSD",
        in_dollars: true,
    },
    KrakenPair {
        code: "JPY",
        name: "ZUSDZJPY",
        altname: "USDJPY",
        in_dollars: false,
    },
    KrakenPair {
        code: "CHF",
        name: "USDCHF",
        altname: "USDCHF",
        in_dollars: false,
    },
    KrakenPair {
        code: "CAD",
        name: "ZUSDZCAD",
        altname: "USDCAD",
        in_dollars: false,
    },
    KrakenPair {
        code: "AUD",
        name: "AUDUSD",
        altname: "AUDUSD",
        in_dollars: true,
    },
];

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
    /// Its fiat rates come from Coinbase's own data provider.
    Coinbase,
    /// Kraken's ticker: the last trade of each currency against the US
    /// dollar on its own market.
    Kraken,
    /// Bitstamp's ticker for XRP in US dollars.
    Bitstamp,
    /// The European Central Bank's daily reference rates, `eurofxref`, in
    /// euros: the fiat currencies, from the central banks' daily
    /// concertation.
    Ecb,
    /// Norges Bank's daily exchange rates, from its SDMX API's `EXR`
    /// dataflow, in Norwegian kroner: the fiat currencies. Open data under
    /// the Norwegian Licence for Open Government Data (NLOD 2.0), with
    /// Norges Bank named as the source
    /// (<https://data.norge.no/en/datasets/23076ce8-b442-407c-817f-0d4ec3cbe744>).
    NorgesBank,
    /// The Bank of Canada's daily average exchange rates, from its Valet
    /// API's `FX_RATES_DAILY` series, in Canadian dollars: the fiat
    /// currencies. Its terms let anyone use and redistribute its content
    /// with the Bank named as the source (<https://www.bankofcanada.ca/terms/>).
    BankOfCanada,
    /// The Czech National Bank's daily exchange rate fixing, `daily.txt`,
    /// in Czech koruny: the fiat currencies. Its terms let anyone store,
    /// distribute and reproduce it with the CNB named as the source
    /// (<https://www.cnb.cz/en/privacy-statement-and-disclaimer/disclaimer-copyright/>).
    Cnb,
}

impl SourceKind {
    /// Every kind.
    pub const ALL: [Self; 7] = [
        Self::Coinbase,
        Self::Kraken,
        Self::Bitstamp,
        Self::Ecb,
        Self::NorgesBank,
        Self::BankOfCanada,
        Self::Cnb,
    ];

    /// Its name, as the database and the wallet have it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Coinbase => "coinbase",
            Self::Kraken => "kraken",
            Self::Bitstamp => "bitstamp",
            Self::Ecb => "ecb",
            Self::NorgesBank => "norges-bank",
            Self::BankOfCanada => "bank-of-canada",
            Self::Cnb => "cnb",
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
            Self::Kraken => {
                "https://api.kraken.com/0/public/Ticker?pair=XRPUSD,EURUSD,GBPUSD,USDJPY,USDCHF,USDCAD,AUDUSD"
            }
            Self::Bitstamp => "https://www.bitstamp.net/api/v2/ticker/xrpusd/",
            Self::Ecb => "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-daily.xml",
            Self::NorgesBank => {
                "https://data.norges-bank.no/api/data/EXR/B.USD+EUR+GBP+JPY+CHF+CAD+AUD.NOK.SP?format=csv&labels=id&lastNObservations=1"
            }
            Self::BankOfCanada => {
                "https://www.bankofcanada.ca/valet/observations/FXUSDCAD,FXEURCAD,FXGBPCAD,FXJPYCAD,FXCHFCAD,FXAUDCAD/json?recent=1"
            }
            Self::Cnb => {
                "https://www.cnb.cz/en/financial-markets/foreign-exchange-market/central-bank-exchange-rate-fixing/central-bank-exchange-rate-fixing/daily.txt"
            }
        }
    }

    /// The media type it answers in.
    #[must_use]
    pub const fn accept(self) -> &'static str {
        match self {
            Self::Coinbase | Self::Kraken | Self::Bitstamp | Self::BankOfCanada => {
                "application/json"
            }
            Self::Ecb => "text/xml",
            Self::NorgesBank => "text/csv",
            Self::Cnb => "text/plain",
        }
    }

    /// Whether it gives a rate for `code` when it answers well.
    #[must_use]
    pub fn covers(self, code: &str) -> bool {
        match self {
            Self::Coinbase | Self::Kraken => true,
            Self::Bitstamp => code == "XRP",
            Self::Ecb | Self::NorgesBank | Self::BankOfCanada | Self::Cnb => code != "XRP",
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

    /// The sources a new node starts with: Coinbase, Kraken, Bitstamp and
    /// the ECB, which cover every currency three times, so that one wrong
    /// or silent source cannot take a rate outside what the others say.
    #[must_use]
    pub fn defaults() -> Vec<Self> {
        [
            SourceKind::Coinbase,
            SourceKind::Kraken,
            SourceKind::Bitstamp,
            SourceKind::Ecb,
        ]
        .into_iter()
        .map(Self::of)
        .collect()
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
/// in units per dollar. `now_ms` dates the answer of a daily source.
///
/// # Errors
///
/// The answer is not what the kind answers, or a daily source's rates are
/// not of the last few days.
pub fn parse(
    kind: SourceKind,
    body: &[u8],
    now_ms: u64,
) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let json = || -> Result<serde_json::Value, Unreadable> {
        serde_json::from_slice(body).map_err(|_| Unreadable("not JSON"))
    };
    match kind {
        SourceKind::Coinbase => coinbase(&json()?),
        SourceKind::Kraken => kraken(&json()?),
        SourceKind::Bitstamp => bitstamp(&json()?),
        SourceKind::Ecb => ecb(body, now_ms),
        SourceKind::NorgesBank => norges_bank(body, now_ms),
        SourceKind::BankOfCanada => bank_of_canada(&json()?, now_ms),
        SourceKind::Cnb => cnb(body, now_ms),
    }
}

/// Bitstamp's ticker: XRP's last trade.
fn bitstamp(json: &serde_json::Value) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let last = json
        .get("last")
        .and_then(serde_json::Value::as_str)
        .ok_or(Unreadable("no last trade"))?;
    let price = decimal(last).ok_or(Unreadable("not a price"))?;
    let rate = invert(price).ok_or(Unreadable("not a price"))?;
    Ok(vec![("XRP", rate)])
}

/// Coinbase's format, from any base.
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
    rebase(base, |code| {
        rates
            .get(code)
            .and_then(serde_json::Value::as_str)
            .and_then(decimal)
    })
}

/// Kraken's ticker: the last trade of each pair it gave. A pair it left
/// out, or that does not read, is left out; an answer without any is
/// refused.
fn kraken(json: &serde_json::Value) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let result = json
        .get("result")
        .and_then(serde_json::Value::as_object)
        .ok_or(Unreadable("no ticker"))?;
    let rates: Vec<(&'static str, u128)> = KRAKEN_PAIRS
        .iter()
        .filter_map(|pair| {
            let ticker = result.get(pair.name).or_else(|| result.get(pair.altname))?;
            let last = decimal(ticker.get("c")?.get(0)?.as_str()?)?;
            let rate = if pair.in_dollars { invert(last)? } else { last };
            (rate > 0).then_some((pair.code, rate))
        })
        .collect();
    if rates.is_empty() {
        return Err(Unreadable("no ticker"));
    }
    Ok(rates)
}

/// The ECB's `eurofxref-daily.xml`: one day's `<Cube time='…'>`, holding a
/// `<Cube currency='…' rate='…'/>` per currency, in units per euro. Its
/// tags are read by their attributes, without an XML parser: the format is
/// fixed, and a body that is not it reads as no rates. Rates of a day to
/// come, or more than [`MAX_ECB_AGE_DAYS`] old, are refused.
fn ecb(body: &[u8], now_ms: u64) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let text = std::str::from_utf8(body).map_err(|_| Unreadable("not text"))?;
    let mut day = None;
    let mut rates = Vec::new();
    for tag in text.split("<Cube").skip(1) {
        let tag = tag
            .split_once('>')
            .map_or(tag, |(attributes, _)| attributes);
        if let Some(time) = attribute(tag, "time")
            && day.replace(time).is_some()
        {
            return Err(Unreadable("more than one day"));
        }
        if let (Some(code), Some(rate)) = (attribute(tag, "currency"), attribute(tag, "rate")) {
            rates.push((code, rate));
        }
    }
    let day = day.ok_or(Unreadable("no day"))?;
    recent(
        day_number(day).ok_or(Unreadable("not a day"))?,
        now_ms,
        MAX_ECB_AGE_DAYS,
    )?;
    rebase("EUR", |code| {
        rates
            .iter()
            .find(|(given, _)| *given == code)
            .and_then(|(_, rate)| decimal(rate))
    })
}

/// The value of attribute `name` among a tag's attributes, quoted either
/// way.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    tag.split_whitespace().find_map(|token| {
        let value = token
            .strip_prefix(name)?
            .strip_prefix('=')?
            .trim_end_matches('/');
        ['\'', '"'].into_iter().find_map(|quote| {
            value
                .strip_prefix(quote)
                .and_then(|inner| inner.strip_suffix(quote))
        })
    })
}

/// Norges Bank's `EXR` dataflow as CSV with IDs for labels: a header, then
/// a row per currency with its `BASE_CUR`, `QUOTE_CUR`, `UNIT_MULT`,
/// `TIME_PERIOD` and `OBS_VALUE`, separated by semicolons. A row says what
/// 10^`UNIT_MULT` units of `BASE_CUR` cost in `QUOTE_CUR`, kroner. Columns
/// are found by the header's names. Only the newest day's rows are read; a
/// row of another quote currency, or whose units or rate do not read, is
/// left out, and one whose day does not read refuses the answer. Rates of
/// a day to come, or more than [`MAX_NORGES_BANK_AGE_DAYS`] old, are
/// refused.
fn norges_bank(body: &[u8], now_ms: u64) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let text = std::str::from_utf8(body).map_err(|_| Unreadable("not text"))?;
    let mut lines = text.lines().map(|line| line.trim_end_matches('\r'));
    let header: Vec<&str> = lines
        .next()
        .ok_or(Unreadable("no header"))?
        .split(';')
        .collect();
    let column = |name: &str| {
        header
            .iter()
            .position(|given| *given == name)
            .ok_or(Unreadable("not Norges Bank's columns"))
    };
    let (code, quote, power, time, value) = (
        column("BASE_CUR")?,
        column("QUOTE_CUR")?,
        column("UNIT_MULT")?,
        column("TIME_PERIOD")?,
        column("OBS_VALUE")?,
    );
    let mut rows = Vec::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let fields: Vec<&str> = line.split(';').collect();
        let field = |at: usize| fields.get(at).copied().unwrap_or_default();
        if fields.len() != header.len() || field(quote) != "NOK" {
            continue;
        }
        let day = day_number(field(time)).ok_or(Unreadable("not a day"))?;
        let units = whole(field(power))
            .filter(|power| *power <= 9)
            .and_then(|power| 10u128.checked_pow(u32::try_from(power).ok()?));
        rows.push((day, field(code), decimal(field(value)), units));
    }
    let newest = rows
        .iter()
        .map(|(day, ..)| *day)
        .max()
        .ok_or(Unreadable("no day"))?;
    recent(newest, now_ms, MAX_NORGES_BANK_AGE_DAYS)?;
    crossed("NOK", |wanted| {
        rows.iter()
            .find(|(day, code, ..)| *day == newest && *code == wanted)
            .and_then(|(_, _, price, units)| Some(((*price)?, (*units)?)))
    })
}

/// The Bank of Canada's Valet observations, `{observations: [{d: "day",
/// FXUSDCAD: {v: "decimal"}, …}]}`: for each day, what a unit of each
/// currency costs in Canadian dollars. Only the newest day's observation
/// is read. Rates of a day to come, or more than
/// [`MAX_BANK_OF_CANADA_AGE_DAYS`] old, are refused.
fn bank_of_canada(
    json: &serde_json::Value,
    now_ms: u64,
) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let observations = json
        .get("observations")
        .and_then(serde_json::Value::as_array)
        .ok_or(Unreadable("no observations"))?;
    let mut newest = None;
    for observation in observations {
        let day = observation
            .get("d")
            .and_then(serde_json::Value::as_str)
            .and_then(day_number)
            .ok_or(Unreadable("not a day"))?;
        if newest.is_none_or(|(newest, _)| day > newest) {
            newest = Some((day, observation));
        }
    }
    let (day, observation) = newest.ok_or(Unreadable("no day"))?;
    recent(day, now_ms, MAX_BANK_OF_CANADA_AGE_DAYS)?;
    crossed("CAD", |code| {
        let price = observation
            .get(format!("FX{code}CAD"))?
            .get("v")?
            .as_str()
            .and_then(decimal)?;
        Some((price, 1))
    })
}

/// The Czech National Bank's `daily.txt`, in English: its day, as
/// `20 Nov 2026 #223`, a header, `Country|Currency|Amount|Code|Rate`, and
/// a row per currency saying what `Amount` units of it cost in koruny.
/// Columns are found by the header's names; a row that does not read is
/// left out. Rates of a day to come, or more than [`MAX_CNB_AGE_DAYS`]
/// old, are refused.
fn cnb(body: &[u8], now_ms: u64) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let text = std::str::from_utf8(body).map_err(|_| Unreadable("not text"))?;
    let mut lines = text.lines().map(|line| line.trim_end_matches('\r'));
    let day = lines
        .next()
        .and_then(cnb_day)
        .ok_or(Unreadable("not a day"))?;
    let header: Vec<&str> = lines
        .next()
        .ok_or(Unreadable("no header"))?
        .split('|')
        .collect();
    let column = |name: &str| {
        header
            .iter()
            .position(|given| *given == name)
            .ok_or(Unreadable("not the CNB's columns"))
    };
    let (amount, code, rate) = (column("Amount")?, column("Code")?, column("Rate")?);
    recent(day, now_ms, MAX_CNB_AGE_DAYS)?;
    let rows: Vec<Vec<&str>> = lines
        .map(|line| line.split('|').collect::<Vec<_>>())
        .filter(|fields| fields.len() == header.len())
        .collect();
    crossed("CZK", |wanted| {
        let row = rows
            .iter()
            .find(|fields| fields.get(code) == Some(&wanted))?;
        let units = whole(row.get(amount)?)?;
        Some((decimal(row.get(rate)?)?, units))
    })
}

/// The CNB's day, `20 Nov 2026 #223`, as days since the epoch.
fn cnb_day(line: &str) -> Option<u64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut words = line.split_whitespace();
    let (day, month, year) = (words.next()?, words.next()?, words.next()?);
    let day = whole(day).filter(|day| (1..=31).contains(day))?;
    let month = MONTHS.iter().position(|known| *known == month)? + 1;
    if year.len() != 4 || whole(year).is_none() {
        return None;
    }
    day_number(&format!("{year}-{month:02}-{day:02}"))
}

/// A whole number of at most nine digits, such as `"100"`.
fn whole(text: &str) -> Option<u128> {
    if text.is_empty() || text.len() > 9 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A day, `YYYY-MM-DD`, as days since the epoch.
fn day_number(text: &str) -> Option<u64> {
    if text.len() != 10 {
        return None;
    }
    let ms = dassie_payments::time::parse_rfc3339(&format!("{text}T00:00:00Z")).ok()?;
    Some(ms / MS_PER_DAY)
}

/// Refuses a daily source's rates of `day` at `now_ms`, if the day is to
/// come, or more than `max_age_days` old: a source that stopped, or a
/// node's clock that did, reads as silent rather than as a wrong rate.
fn recent(day: u64, now_ms: u64, max_age_days: u64) -> Result<(), Unreadable> {
    let today = now_ms / MS_PER_DAY;
    if day > today {
        return Err(Unreadable("rates of a day to come"));
    }
    if today - day > max_age_days {
        return Err(Unreadable("rates too old"));
    }
    Ok(())
}

/// Rates from what currencies cost in `base`, in units per dollar:
/// `price(code)` is what some units of `code` cost, as a rate, and how
/// many units, so that a currency's rate is the dollar's price per unit
/// over its own. The base costs one per unit by definition; a price or a
/// number of units of zero is left out, and so is a rate that rounds to
/// nothing or does not fit.
fn crossed(
    base: &str,
    price: impl Fn(&str) -> Option<(u128, u128)>,
) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let price = |code: &str| {
        if code == base {
            return Some((ONE, 1));
        }
        price(code).filter(|(price, units)| *price > 0 && *units > 0)
    };
    let (dollar, dollar_units) = price(BASE).ok_or(Unreadable("no rate for the dollar"))?;
    Ok(CURRENCIES
        .iter()
        .filter_map(|&code| {
            let (own, units) = price(code)?;
            let rate =
                dollar.checked_mul(units)?.checked_mul(ONE)? / dollar_units.checked_mul(own)?;
            (rate > 0).then_some((code, rate))
        })
        .collect())
}

/// Rates in units per one `base`, as `read` gives them, in units per
/// dollar: each over the dollar's. The base is one by definition; a rate
/// of zero is left out.
fn rebase(
    base: &str,
    read: impl Fn(&str) -> Option<u128>,
) -> Result<Vec<(&'static str, u128)>, Unreadable> {
    let read = |code: &str| {
        if code == base {
            return Some(ONE);
        }
        read(code).filter(|rate| *rate > 0)
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
    // Kraken's ticker for every pair the node asks it for.
    const KRAKEN_ALL: &str = r#"{"error":[],"result":{"AUDUSD":{"a":["0.69965","6699","6699.000"],"b":["0.69962","1407","1407.000"],"c":["0.69958","99.01000000"],"o":"0.69796"},"ZEURZUSD":{"a":["1.12305","24","24.000"],"b":["1.12304","371","371.000"],"c":["1.12306","14.30571429"],"o":"1.12123"},"ZGBPZUSD":{"a":["1.33745","69","69.000"],"b":["1.33736","5419","5419.000"],"c":["1.33732","33.10000000"],"o":"1.33124"},"ZUSDZCAD":{"a":["1.40613","258","258.000"],"b":["1.40612","429","429.000"],"c":["1.40618","25.65853517"],"o":"1.41143"},"USDCHF":{"a":["0.82586","3702","3702.000"],"b":["0.82572","794","794.000"],"c":["0.82586","13.68230000"],"o":"0.82675"},"ZUSDZJPY":{"a":["160.686","347","347.000"],"b":["158.066","1512","1512.000"],"c":["158.065","16.24600000"],"o":"157.319"},"XXRPZUSD":{"a":["1.32779000","145","145.000"],"b":["1.32778000","409","409.000"],"c":["1.32820000","756.80790007"],"o":"1.30273000"}}}"#;
    const ECB: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<gesmes:Envelope xmlns:gesmes=\"http://www.gesmes.org/xml/2002-08-01\" xmlns=\"http://www.ecb.int/vocabulary/2002-08-01/eurofxref\">
\t<gesmes:subject>Reference rates</gesmes:subject>
\t<gesmes:Sender>
\t\t<gesmes:name>European Central Bank</gesmes:name>
\t</gesmes:Sender>
\t<Cube>
\t\t<Cube time='2026-10-20'>
\t\t\t<Cube currency='USD' rate='1.1215'/>
\t\t\t<Cube currency='JPY' rate='177.34'/>
\t\t\t<Cube currency='CZK' rate='24.329'/>
\t\t\t<Cube currency='GBP' rate='0.84028'/>
\t\t\t<Cube currency='CHF' rate='0.9237'/>
\t\t\t<Cube currency='AUD' rate='1.6056'/>
\t\t\t<Cube currency='CAD' rate='1.5758'/>
\t\t</Cube>
\t</Cube>
</gesmes:Envelope>
";
    // When the ECB's and Kraken's answers were recorded: 2026-10-20, 17:00.
    const NOW: u64 = 1_792_515_600_000;
    // Midnight of that day.
    const TODAY: u64 = 1_792_454_400_000;

    #[test]
    fn each_kind_is_read() {
        let coinbase = parse(SourceKind::Coinbase, COINBASE.as_bytes(), NOW).unwrap();
        assert!(coinbase.contains(&("USD", ONE)));
        assert!(coinbase.contains(&("XRP", 1_865_671_641_791_044)));
        assert!(coinbase.contains(&("JPY", 149_620_000_000_000_000)));
        assert!(!coinbase.iter().any(|(code, _)| *code == "AED"), "not kept");
        // 1 / 0.536 XRP per dollar.
        let kraken = parse(SourceKind::Kraken, KRAKEN.as_bytes(), NOW).unwrap();
        assert_eq!(kraken, [("XRP", 1_865_671_641_791_044)]);
        let bitstamp = parse(SourceKind::Bitstamp, BITSTAMP.as_bytes(), NOW).unwrap();
        assert_eq!(bitstamp, [("XRP", 1_862_197_392_923_649)]);
    }

    // Kraken quotes some pairs in dollars, some in the other currency.
    #[test]
    fn kraken_gives_every_currency_against_the_dollar() {
        let kraken = parse(SourceKind::Kraken, KRAKEN_ALL.as_bytes(), NOW).unwrap();
        assert_eq!(
            kraken,
            [
                ("XRP", 752_898_659_840_385),
                ("EUR", 890_424_376_257_724),
                ("GBP", 747_764_185_086_591),
                ("JPY", 158_065_000_000_000_000),
                ("CHF", 825_860_000_000_000),
                ("CAD", 1_406_180_000_000_000),
                ("AUD", 1_429_429_086_023_042),
            ]
        );
        // A pair named by its altname reads the same; one left out is left
        // out.
        let altname = r#"{"error":[],"result":{"USDJPY":{"c":["150.5","1"]}}}"#;
        assert_eq!(
            parse(SourceKind::Kraken, altname.as_bytes(), NOW).unwrap(),
            [("JPY", rate("150.5"))]
        );
        // The URL asks for exactly the pairs read.
        let asked = SourceKind::Kraken
            .default_url()
            .split_once("pair=")
            .unwrap()
            .1;
        let pairs: Vec<&str> = KRAKEN_PAIRS.iter().map(|pair| pair.altname).collect();
        assert_eq!(asked, pairs.join(","));
    }

    #[test]
    fn the_ecb_is_read_in_euros_and_rebased_on_the_dollar() {
        let ecb = parse(SourceKind::Ecb, ECB.as_bytes(), NOW).unwrap();
        assert_eq!(
            ecb,
            [
                ("USD", ONE),
                ("EUR", 891_662_951_404_369),
                ("GBP", 749_246_544_806_063),
                ("JPY", 158_127_507_802_050_824),
                ("CHF", 823_629_068_212_215),
                ("CAD", 1_405_082_478_823_004),
                ("AUD", 1_431_654_034_774_855),
            ]
        );
        // Double quotes read the same.
        let double = r#"<Cube time="2026-10-20"><Cube currency="USD" rate="1.25"/></Cube>"#;
        assert_eq!(
            parse(SourceKind::Ecb, double.as_bytes(), NOW).unwrap(),
            [("USD", ONE), ("EUR", rate("0.8"))]
        );
    }

    // The ECB publishes on business days: Thursday's rates stand over
    // Easter until Tuesday's, five days on; older ones mean it stopped.
    #[test]
    fn the_ecb_is_taken_only_while_its_rates_are_recent() {
        let day = 86_400_000;
        let at = |now| parse(SourceKind::Ecb, ECB.as_bytes(), now);
        assert!(at(TODAY).is_ok());
        assert!(at(TODAY + MAX_ECB_AGE_DAYS * day + day - 1).is_ok());
        assert_eq!(
            at(TODAY + (MAX_ECB_AGE_DAYS + 1) * day),
            Err(Unreadable("rates too old"))
        );
        assert_eq!(at(TODAY - 1), Err(Unreadable("rates of a day to come")));
        assert_eq!(at(0), Err(Unreadable("rates of a day to come")));
    }

    // The central banks' answers beside the ECB's, recorded within a
    // minute of each other on Friday, 2026-11-20, at 13:31 UTC: Norges
    // Bank's and the Bank of Canada's newest day was the 19th, and the
    // CNB had fixed the 20th's.
    const NORGES_BANK: &str = include_str!("testdata/rates/norges-bank.csv");
    const BANK_OF_CANADA: &str = include_str!("testdata/rates/bank-of-canada.json");
    const CNB: &str = include_str!("testdata/rates/cnb-daily.txt");
    // When they were recorded.
    const BANKS_NOW: u64 = 1_795_181_477_000;
    // Midnight of the 19th and of the 20th.
    const NOVEMBER_19: u64 = 1_795_046_400_000;
    const NOVEMBER_20: u64 = 1_795_132_800_000;
    const DAY: u64 = 86_400_000;
    const NORGES_HEADER: &str = "BASE_CUR;QUOTE_CUR;UNIT_MULT;TIME_PERIOD;OBS_VALUE\n";
    const CNB_HEADER: &str = "20 Nov 2026 #223\nCountry|Currency|Amount|Code|Rate\n";

    #[test]
    fn norges_bank_is_read_in_kroner_and_crossed_to_the_dollar() {
        let rates = parse(SourceKind::NorgesBank, NORGES_BANK.as_bytes(), BANKS_NOW).unwrap();
        // 9.6866 kroner a dollar over 10.6475 a euro; the yen and the franc
        // are quoted per hundred.
        assert_eq!(
            rates,
            [
                ("USD", ONE),
                ("EUR", 909_753_463_254_285),
                ("GBP", 760_383_386_581_469),
                ("JPY", 159_971_594_662_438_895),
                ("CHF", 842_613_453_492_114),
                ("CAD", 1_430_051_966_458_013),
                ("AUD", 1_489_994_001_015_212),
            ]
        );
        // The columns are found by name, and a row of an older day, or of
        // another quote currency, is left out.
        let moved = "OBS_VALUE;TIME_PERIOD;UNIT_MULT;BASE_CUR;QUOTE_CUR
2;2026-11-19;0;USD;NOK
3;2026-11-18;0;EUR;NOK
4;2026-11-19;0;GBP;SEK
200;2026-11-19;2;JPY;NOK
";
        assert_eq!(
            parse(SourceKind::NorgesBank, moved.as_bytes(), BANKS_NOW).unwrap(),
            [("USD", ONE), ("JPY", rate("1"))]
        );
    }

    #[test]
    fn the_bank_of_canada_is_read_in_canadian_dollars_and_crossed_to_the_dollar() {
        let rates = parse(
            SourceKind::BankOfCanada,
            BANK_OF_CANADA.as_bytes(),
            BANKS_NOW,
        )
        .unwrap();
        // 1.4317 Canadian dollars a dollar, the base itself at one.
        assert_eq!(
            rates,
            [
                ("USD", ONE),
                ("EUR", 910_403_154_012_463),
                ("GBP", 762_231_805_355_906),
                ("JPY", 159_787_946_428_571_428),
                ("CHF", 843_267_758_275_415),
                ("CAD", rate("1.4317")),
                ("AUD", 1_496_341_973_244_147),
            ]
        );
        // Of several days, the newest is read.
        let two = r#"{"observations":[
            {"d":"2026-11-19","FXUSDCAD":{"v":"1.25"},"FXEURCAD":{"v":"1.5"}},
            {"d":"2026-11-18","FXUSDCAD":{"v":"2"}}]}"#;
        assert_eq!(
            parse(SourceKind::BankOfCanada, two.as_bytes(), BANKS_NOW).unwrap(),
            [
                ("USD", ONE),
                ("EUR", rate("0.833333333333333")),
                ("CAD", rate("1.25"))
            ]
        );
        // The series the URL asks for are those of every fiat currency
        // but the base.
        let url = SourceKind::BankOfCanada.default_url();
        for code in CURRENCIES {
            let asked = url.contains(&format!("FX{code}CAD"));
            assert_eq!(asked, !["XRP", "CAD"].contains(&code), "{code}");
        }
    }

    #[test]
    fn the_cnb_is_read_in_koruny_and_crossed_to_the_dollar() {
        let rates = parse(SourceKind::Cnb, CNB.as_bytes(), BANKS_NOW).unwrap();
        // 22.262 koruny a dollar over 24.470 a euro; the yen is quoted per
        // hundred.
        assert_eq!(
            rates,
            [
                ("USD", ONE),
                ("EUR", 909_767_061_708_214),
                ("GBP", 762_110_163_979_322),
                ("JPY", 158_662_960_587_271_042),
                ("CHF", 838_619_754_388_608),
                ("CAD", 1_430_627_851_680_483),
                ("AUD", 1_500_640_377_485_675),
            ]
        );
        // The Czech page's day and decimal commas do not read.
        let czech = "20.11.2026 #223\nzem\u{11b}|m\u{11b}na|mno\u{17e}stv\u{ed}|k\u{f3}d|kurz\nUSA|dolar|1|USD|22,262\n";
        assert_eq!(
            parse(SourceKind::Cnb, czech.as_bytes(), BANKS_NOW),
            Err(Unreadable("not a day"))
        );
        let comma = format!("{CNB_HEADER}USA|dollar|1|USD|22,262\n");
        assert_eq!(
            parse(SourceKind::Cnb, comma.as_bytes(), BANKS_NOW),
            Err(Unreadable("no rate for the dollar"))
        );
    }

    // Each of the three covers every fiat currency, the dollar included,
    // which the cross needs, and none covers XRP.
    #[test]
    fn the_central_banks_cover_every_fiat_currency() {
        let fiat: Vec<&str> = CURRENCIES
            .into_iter()
            .filter(|code| *code != "XRP")
            .collect();
        let mut answers: Vec<Answer> = Vec::new();
        for kind in [
            SourceKind::NorgesBank,
            SourceKind::BankOfCanada,
            SourceKind::Cnb,
        ] {
            let (body, now) = recorded(kind);
            let rates = parse(kind, body.as_bytes(), now).unwrap();
            let codes: Vec<&str> = rates.iter().map(|(code, _)| *code).collect();
            assert_eq!(codes, fiat, "{kind:?}");
            assert!(!kind.covers("XRP") && fiat.iter().all(|code| kind.covers(code)));
            answers.push((kind, Some(rates)));
        }
        // A day apart, they agree within 1%, so the middle two of them
        // agree as a median of an even number asks.
        assert_eq!(combine(&CURRENCIES, &answers).len(), fiat.len());
    }

    // Each bank's answer stands for its calendar's longest gap, and no
    // longer; one of a day to come is refused.
    #[test]
    fn each_bank_is_taken_only_while_its_rates_are_recent() {
        for (kind, published, max) in [
            (
                SourceKind::NorgesBank,
                NOVEMBER_19,
                MAX_NORGES_BANK_AGE_DAYS,
            ),
            (
                SourceKind::BankOfCanada,
                NOVEMBER_19,
                MAX_BANK_OF_CANADA_AGE_DAYS,
            ),
            (SourceKind::Cnb, NOVEMBER_20, MAX_CNB_AGE_DAYS),
        ] {
            let (body, _) = recorded(kind);
            let at = |now| parse(kind, body.as_bytes(), now);
            assert!(at(published).is_ok(), "{kind:?}");
            assert!(at(published + max * DAY + DAY - 1).is_ok(), "{kind:?}");
            assert_eq!(
                at(published + (max + 1) * DAY),
                Err(Unreadable("rates too old")),
                "{kind:?}"
            );
            assert_eq!(
                at(published - 1),
                Err(Unreadable("rates of a day to come")),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn what_a_central_bank_did_not_answer_is_refused() {
        let norges = |row: &str| format!("{NORGES_HEADER}{row}\n");
        let cnb = |row: &str| format!("{CNB_HEADER}{row}\n");
        for (kind, body) in [
            (SourceKind::NorgesBank, String::new()),
            (SourceKind::NorgesBank, "not csv".into()),
            (SourceKind::NorgesBank, NORGES_HEADER.into()),
            (SourceKind::NorgesBank, norges("EUR;NOK;0;2026-11-19;10.6")),
            (SourceKind::NorgesBank, norges("USD;NOK;0;2026-11-31;9.6")),
            (SourceKind::NorgesBank, norges("USD;NOK;x;2026-11-19;9.6")),
            (SourceKind::NorgesBank, norges("USD;NOK;10;2026-11-19;9.6")),
            (SourceKind::BankOfCanada, "{}".into()),
            (SourceKind::BankOfCanada, r#"{"observations":[]}"#.into()),
            (
                SourceKind::BankOfCanada,
                r#"{"observations":[{"FXUSDCAD":{"v":"1.4"}}]}"#.into(),
            ),
            (
                SourceKind::BankOfCanada,
                r#"{"observations":[{"d":"2026-11-19","FXUSDCAD":{"v":1.4}}]}"#.into(),
            ),
            (
                SourceKind::BankOfCanada,
                r#"{"observations":[{"d":"2026-11-19","FXUSDCAD":{"v":"0"}}]}"#.into(),
            ),
            (SourceKind::Cnb, String::new()),
            (SourceKind::Cnb, "20 Nov 2026 #223\n".into()),
            (SourceKind::Cnb, CNB_HEADER.into()),
            (
                SourceKind::Cnb,
                CNB_HEADER.replace("20 Nov", "31 Nov") + "USA|dollar|1|USD|22.262\n",
            ),
            (SourceKind::Cnb, cnb("USA|dollar|0|USD|22.262")),
            (SourceKind::Cnb, cnb("USA|dollar|1|USD")),
            (
                SourceKind::Cnb,
                "20 Nov 2026 #223\nCountry|Currency|Amount|Code\nUSA|dollar|1|USD\n".into(),
            ),
        ] {
            assert!(
                parse(kind, body.as_bytes(), BANKS_NOW).is_err(),
                "{kind:?} {body}"
            );
        }
        // A currency a bank left out is left out, and the others stand.
        let no_franc: String = CNB
            .lines()
            .filter(|line| !line.contains("|CHF|"))
            .map(|line| format!("{line}\n"))
            .collect();
        let rates = parse(SourceKind::Cnb, no_franc.as_bytes(), BANKS_NOW).unwrap();
        assert_eq!(rates.len(), CURRENCIES.len() - 2, "neither XRP nor CHF");
        assert!(!rates.iter().any(|(code, _)| *code == "CHF"));
    }

    #[test]
    fn another_base_is_rebased_on_the_dollar() {
        let euro = r#"{"data":{"currency":"EUR","rates":{"USD":"1.25","XRP":"2.5"}}}"#;
        let rates = parse(SourceKind::Coinbase, euro.as_bytes(), NOW).unwrap();
        assert!(rates.contains(&("USD", ONE)));
        assert!(rates.contains(&("XRP", 2 * ONE)));
        assert!(rates.contains(&("EUR", 800_000_000_000_000)));
        let no_dollar = r#"{"data":{"currency":"EUR","rates":{"XRP":"2.5"}}}"#;
        assert!(parse(SourceKind::Coinbase, no_dollar.as_bytes(), NOW).is_err());
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
            (
                SourceKind::Kraken,
                r#"{"result":{"XXRPZUSD":{"c":["0","1"]}}}"#,
            ),
            (SourceKind::Bitstamp, r#"{"last": "0"}"#),
            (SourceKind::Bitstamp, r#"{"last": 0.5}"#),
            (SourceKind::Ecb, "not xml"),
            (SourceKind::Ecb, "<Cube currency='USD' rate='1.1'/>"),
            (
                SourceKind::Ecb,
                "<Cube time='2026-10-20'><Cube currency='JPY' rate='177'/></Cube>",
            ),
            (
                SourceKind::Ecb,
                "<Cube time='2026-10-19'/><Cube time='2026-10-20'><Cube currency='USD' rate='1.1'/></Cube>",
            ),
            (
                SourceKind::Ecb,
                "<Cube time='2026-10-20T00:00:00Z'><Cube currency='USD' rate='1.1'/></Cube>",
            ),
            (
                SourceKind::Ecb,
                "<Cube time='2026-02-30'><Cube currency='USD' rate='1.1'/></Cube>",
            ),
            (
                SourceKind::Ecb,
                "<Cube time='2026-10-20'><Cube currency='USD' rate='0'/></Cube>",
            ),
        ] {
            assert!(
                parse(kind, body.as_bytes(), NOW).is_err(),
                "{kind:?} {body}"
            );
        }
        // A rate of zero, or unreadable, is left out.
        let zero = r#"{"data":{"currency":"USD","rates":{"XRP":"0","EUR":"x"}}}"#;
        assert_eq!(
            parse(SourceKind::Coinbase, zero.as_bytes(), NOW).unwrap(),
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
        // One wild source does not move the rate; Coinbase and Kraken
        // cover the euro, and one of two gave it, which is not more than
        // half; nobody gave the pound.
        assert_eq!(combined, [("USD", ONE), ("XRP", rate("2"))]);
        // A currency that only one of the owner's sources covers takes
        // what it says.
        assert_eq!(combine(&["EUR"], &answers[..1]), [("EUR", rate("0.9"))]);
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

    // Answers of every default source, recorded within minutes.
    const COINBASE_ALL: &str = r#"{"data":{"currency":"USD","rates":{"AED":"3.6729833333333333","AUD":"1.429074","CAD":"1.4058133333333333","CHF":"0.8260933333333333","EUR":"0.89046","GBP":"0.7477551666666667","JPY":"158.2012083333333333","USD":"1.0","XRP":"0.7536362951239732"}}}"#;
    const BITSTAMP_NOW: &str = r#"{"timestamp":"1792516579","open":"1.30257","high":"1.34467","low":"1.28514","last":"1.32793","volume":"15305708.69416006","vwap":"1.30756","bid":"1.32842","ask":"1.32843"}"#;

    /// A recorded answer of each kind, and when it was recorded.
    fn recorded(kind: SourceKind) -> (&'static str, u64) {
        match kind {
            SourceKind::Coinbase => (COINBASE_ALL, NOW),
            SourceKind::Kraken => (KRAKEN_ALL, NOW),
            SourceKind::Bitstamp => (BITSTAMP_NOW, NOW),
            SourceKind::Ecb => (ECB, NOW),
            SourceKind::NorgesBank => (NORGES_BANK, BANKS_NOW),
            SourceKind::BankOfCanada => (BANK_OF_CANADA, BANKS_NOW),
            SourceKind::Cnb => (CNB, BANKS_NOW),
        }
    }

    /// What each of a new node's sources said.
    fn default_answers() -> Vec<Answer> {
        RateSource::defaults()
            .into_iter()
            .map(|source| {
                let (body, now) = recorded(source.kind);
                let rates = parse(source.kind, body.as_bytes(), now).unwrap();
                (source.kind, Some(rates))
            })
            .collect()
    }

    #[test]
    fn a_new_nodes_sources_cover_every_currency_three_times() {
        let sources = RateSource::defaults();
        for code in CURRENCIES.into_iter().filter(|code| *code != BASE) {
            let covering = sources
                .iter()
                .filter(|source| source.kind.covers(code))
                .count();
            assert!(covering >= 3, "{code} is covered {covering} times");
        }
    }

    // With a new node's sources, any one of them may say anything about
    // every currency, or nothing: each rate stays within what the others
    // say, and every currency still has one.
    #[test]
    fn one_wrong_or_silent_source_leaves_every_rate_within_the_others() {
        let honest = default_answers();
        assert_eq!(combine(&CURRENCIES, &honest).len(), CURRENCIES.len());
        let lies = [
            None,
            Some(CURRENCIES.iter().map(|&code| (code, 1_000 * ONE)).collect()),
            Some(CURRENCIES.iter().map(|&code| (code, 1)).collect()),
        ];
        for (liar, (kind, _)) in honest.iter().enumerate() {
            for lie in &lies {
                let mut answers = honest.clone();
                answers[liar] = (*kind, lie.clone());
                let combined = combine(&CURRENCIES, &answers);
                assert_eq!(combined.len(), CURRENCIES.len(), "{kind:?} {lie:?}");
                for (code, taken) in combined {
                    let others: Vec<u128> = honest
                        .iter()
                        .enumerate()
                        .filter(|(at, (other, _))| *at != liar && other.covers(code))
                        .filter_map(|(_, (_, rates))| {
                            let rates = rates.as_ref()?;
                            rates.iter().find(|(c, _)| *c == code).map(|(_, r)| *r)
                        })
                        .collect();
                    let (low, high) = (others.iter().min(), others.iter().max());
                    assert!(
                        code == BASE || (low <= Some(&taken) && Some(&taken) <= high),
                        "{kind:?} moved {code} to {taken}, outside {others:?}"
                    );
                }
            }
        }
    }

    // A source may name rates up to about u128::MAX: scaling the spread of
    // two such once overflowed, and the panic ended the rate task.
    #[test]
    fn rates_far_apart_do_not_overflow() {
        let wild =
            r#"{"data":{"currency":"EUR","rates":{"USD":"0.000000000000001","XRP":"340000000"}}}"#;
        let coinbase = parse(SourceKind::Coinbase, wild.as_bytes(), NOW).unwrap();
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
            proptest::collection::vec((code.clone(), number.clone()), 0..6),
        )
            .prop_map(|(base, rates)| {
                let rates: serde_json::Map<_, _> = rates
                    .into_iter()
                    .map(|(code, rate)| (code, serde_json::Value::String(rate)))
                    .collect();
                serde_json::json!({"data": {"currency": base, "rates": rates}}).to_string()
            });
        let pair = prop_oneof![
            proptest::sample::select(KRAKEN_PAIRS.map(|pair| pair.name).to_vec()),
            proptest::sample::select(KRAKEN_PAIRS.map(|pair| pair.altname).to_vec()),
        ];
        let kraken = proptest::collection::vec((pair, number.clone()), 0..4).prop_map(|pairs| {
            let result: serde_json::Map<_, _> = pairs
                .into_iter()
                .map(|(pair, last)| (pair.to_owned(), serde_json::json!({ "c": [last, "1"] })))
                .collect();
            serde_json::json!({ "result": result }).to_string()
        });
        let bitstamp = number
            .clone()
            .prop_map(|last| serde_json::json!({ "last": last }).to_string());
        let day = prop_oneof![Just("2026-10-20".to_owned()), "[0-9]{4}-[0-9]{2}-[0-9]{2}"];
        let ecb =
            (day, proptest::collection::vec((code, number), 0..6)).prop_map(|(day, rates)| {
                let cubes: String = rates
                    .into_iter()
                    .map(|(code, rate)| format!("<Cube currency='{code}' rate='{rate}'/>"))
                    .collect();
                format!("<Cube><Cube time='{day}'>{cubes}</Cube></Cube>")
            });
        let banks = bank_bodies();
        prop_oneof![
            prop_oneof![coinbase, kraken, bitstamp, ecb, banks].prop_map(String::into_bytes),
            proptest::collection::vec(any::<u8>(), 0..64),
        ]
    }

    /// Bodies shaped like the central banks' answers beside the ECB's,
    /// with any days, units and numbers in them.
    fn bank_bodies() -> impl Strategy<Value = String> {
        let number = prop_oneof!["[0-9]{1,25}(\\.[0-9]{0,20})?", "\\PC{0,8}"];
        let code = prop_oneof![
            proptest::sample::select(CURRENCIES.as_slice()).prop_map(str::to_owned),
            "[A-Z]{3}",
        ];
        let day = prop_oneof![Just("2026-11-19".to_owned()), "[0-9]{4}-[0-9]{2}-[0-9]{2}"];
        let units = prop_oneof!["[0-9]{1,3}", "[0-9a-z]{0,12}"];
        let row = (code.clone(), units.clone(), day.clone(), number.clone());
        let norges_bank = proptest::collection::vec(row, 0..6).prop_map(|rows| {
            let rows: String = rows
                .into_iter()
                .map(|(code, power, day, value)| format!("{code};NOK;{power};{day};{value}\n"))
                .collect();
            format!("BASE_CUR;QUOTE_CUR;UNIT_MULT;TIME_PERIOD;OBS_VALUE\n{rows}")
        });
        let observation = (
            day,
            proptest::collection::vec((code.clone(), number.clone()), 0..6),
        );
        let bank_of_canada = proptest::collection::vec(observation, 0..3).prop_map(|days| {
            let observations: Vec<serde_json::Value> = days
                .into_iter()
                .map(|(day, rates)| {
                    let mut observation = serde_json::Map::new();
                    observation.insert("d".into(), day.into());
                    for (code, value) in rates {
                        observation
                            .insert(format!("FX{code}CAD"), serde_json::json!({ "v": value }));
                    }
                    serde_json::Value::Object(observation)
                })
                .collect();
            serde_json::json!({ "observations": observations }).to_string()
        });
        let cnb_day = prop_oneof![
            Just("20 Nov 2026 #223".to_owned()),
            "[0-9]{1,2} [A-Z][a-z]{2} [0-9]{4}"
        ];
        let cnb = (
            cnb_day,
            proptest::collection::vec((units, code, number), 0..6),
        )
            .prop_map(|(day, rows)| {
                let rows: String = rows
                    .into_iter()
                    .map(|(amount, code, rate)| format!("X|y|{amount}|{code}|{rate}\n"))
                    .collect();
                format!("{day}\nCountry|Currency|Amount|Code|Rate\n{rows}")
            });
        prop_oneof![norges_bank, bank_of_canada, cnb]
    }

    proptest! {
        #[test]
        fn answers_never_panic(
            kind in proptest::sample::select(SourceKind::ALL.as_slice()),
            body in body(),
        ) {
            if let Ok(rates) = parse(kind, &body, NOW) {
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

        // Of three sources, one may name any rate: it can move the rate
        // only between what the other two say, never beyond.
        #[test]
        fn one_of_three_moves_a_rate_only_between_the_others(
            honest in (1..u128::MAX, 1..u128::MAX),
            lie in any::<u128>(),
            liar in 0..3usize,
        ) {
            let mut said = [honest.0, honest.1];
            said.sort_unstable();
            let mut rates = vec![honest.0, honest.1];
            rates.insert(liar, lie);
            let answers: Vec<Answer> = [SourceKind::Coinbase, SourceKind::Kraken, SourceKind::Bitstamp]
                .into_iter()
                .zip(rates)
                .map(|(kind, rate)| (kind, Some(vec![("XRP", rate)])))
                .collect();
            let taken = combine(&["XRP"], &answers);
            prop_assert_eq!(taken.len(), 1);
            prop_assert!(said[0] <= taken[0].1 && taken[0].1 <= said[1]);
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
        // A new node's sources are still the four it started with; the
        // central banks beside the ECB are the owner's to add.
        let kinds: Vec<SourceKind> = RateSource::defaults()
            .into_iter()
            .map(|source| source.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                SourceKind::Coinbase,
                SourceKind::Kraken,
                SourceKind::Bitstamp,
                SourceKind::Ecb
            ]
        );
        for kind in SourceKind::ALL {
            assert_eq!(SourceKind::parse(kind.as_str()), Some(kind));
            assert!(kind.default_url().starts_with("https://"), "{kind:?}");
        }
        assert_eq!(SourceKind::parse("ECB"), None);
        assert!(
            RateSource {
                kind: SourceKind::Cnb,
                url: Some("https://example.com".into()),
            }
            .problem()
            .is_some()
        );
    }
}

//! Amounts on the XRP Ledger: XRP in drops, or a token of an issuer, such
//! as RLUSD (spec/areas/xrpl.md#tokens).
//!
//! A token's value is decimal: a mantissa of sixteen digits and an
//! exponent, about fifteen significant digits. Values here are exact: a
//! value the ledger cannot express exactly, or whole units a value does
//! not hold exactly, is refused rather than rounded, and nothing passes
//! through a float.

use alloc::string::String;
use core::cmp::Ordering;
use core::fmt;

use crate::keys::AccountId;
use crate::{from_hex, hex_upper};

/// The least mantissa of a value other than zero.
pub const MIN_MANTISSA: u64 = 1_000_000_000_000_000;
/// The largest mantissa.
pub const MAX_MANTISSA: u64 = 9_999_999_999_999_999;
/// The least exponent.
pub const MIN_EXPONENT: i32 = -96;
/// The largest exponent.
pub const MAX_EXPONENT: i32 = 80;

/// Most decimals a conversion to or from whole units takes: a unit of
/// 10^-96 is already the least value the ledger can express.
pub const MAX_DECIMALS: u8 = 96;

/// Longest value text the parser reads. xrpl.js writes the largest value
/// as 96 digits, the least as 83 characters.
const MAX_VALUE_TEXT: usize = 128;

/// The "not XRP" bit of an amount's first eight bytes.
const NOT_XRP: u64 = 0x8000_0000_0000_0000;
/// The "positive" bit.
const POSITIVE: u64 = 0x4000_0000_0000_0000;
/// The bits of the mantissa.
const MANTISSA_BITS: u64 = (1 << 54) - 1;
/// What the exponent is stored above.
const EXPONENT_BIAS: i32 = 97;

/// A token's value: zero, or `mantissa × 10^exponent` with the mantissa
/// in `MIN_MANTISSA..=MAX_MANTISSA` and the exponent in
/// `MIN_EXPONENT..=MAX_EXPONENT`, signed. Every value has one form, so
/// equal values compare equal.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenValue {
    negative: bool,
    /// 0 for zero.
    mantissa: u64,
    /// 0 for zero.
    exponent: i32,
}

impl TokenValue {
    /// Zero.
    pub const ZERO: Self = Self {
        negative: false,
        mantissa: 0,
        exponent: 0,
    };

    /// The largest value the ledger can express: the limit of a line that
    /// never stops a payment.
    pub const MAX: Self = Self {
        negative: false,
        mantissa: MAX_MANTISSA,
        exponent: MAX_EXPONENT,
    };

    /// `±mantissa × 10^exponent`, if the ledger can express it exactly.
    #[must_use]
    pub fn new(negative: bool, mantissa: u64, exponent: i32) -> Option<Self> {
        Self::from_wide(negative, u128::from(mantissa), exponent)
    }

    /// `±magnitude × 10^exponent`, if the ledger can express it exactly.
    fn from_wide(negative: bool, magnitude: u128, exponent: i32) -> Option<Self> {
        if magnitude == 0 {
            return Some(Self::ZERO);
        }
        let (mut magnitude, mut exponent) = (magnitude, exponent);
        // At most 15 steps up and 23 down, for a u128.
        while magnitude < u128::from(MIN_MANTISSA) {
            magnitude *= 10;
            exponent = exponent.checked_sub(1)?;
        }
        while magnitude > u128::from(MAX_MANTISSA) {
            if !magnitude.is_multiple_of(10) {
                return None;
            }
            magnitude /= 10;
            exponent = exponent.checked_add(1)?;
        }
        if !(MIN_EXPONENT..=MAX_EXPONENT).contains(&exponent) {
            return None;
        }
        let value = Self {
            negative,
            mantissa: u64::try_from(magnitude).ok()?,
            exponent,
        };
        value.check();
        Some(value)
    }

    /// The sum, exactly; `None` if the ledger cannot express it (more than
    /// sixteen significant digits, or beyond its exponents). Nothing is
    /// rounded.
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        if self.is_zero() {
            return Some(other);
        }
        if other.is_zero() {
            return Some(self);
        }
        let (high, low) = if self.exponent >= other.exponent {
            (self, other)
        } else {
            (other, self)
        };
        // Past sixteen places apart, the sum spans more than sixteen
        // digits: the high value's last is above the low one's first.
        let apart = high.exponent - low.exponent;
        if apart > 16 {
            return None;
        }
        let signed = |value: Self, shift: u32| {
            let magnitude = i128::from(value.mantissa) * 10_i128.pow(shift);
            if value.negative {
                -magnitude
            } else {
                magnitude
            }
        };
        let sum = signed(high, apart.unsigned_abs()) + signed(low, 0);
        Self::from_wide(sum < 0, sum.unsigned_abs(), low.exponent)
    }

    /// The difference, exactly; `None` as for [`checked_add`](Self::checked_add).
    #[must_use]
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.checked_add(other.negate())
    }

    /// Whether it is zero.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.mantissa == 0
    }

    /// Whether it is below zero.
    #[must_use]
    pub const fn is_negative(&self) -> bool {
        self.negative
    }

    /// Its mantissa: 0, or sixteen digits.
    #[must_use]
    pub const fn mantissa(&self) -> u64 {
        self.mantissa
    }

    /// Its exponent: 0 for zero.
    #[must_use]
    pub const fn exponent(&self) -> i32 {
        self.exponent
    }

    /// The same value, of the other sign; zero stays zero.
    #[must_use]
    pub const fn negate(self) -> Self {
        Self {
            negative: !self.negative && self.mantissa != 0,
            ..self
        }
    }

    /// `units × 10^-decimals`, exactly; `None` if the ledger cannot
    /// express it (more than sixteen significant digits) or `decimals` is
    /// beyond [`MAX_DECIMALS`].
    #[must_use]
    pub fn from_units(units: u64, decimals: u8) -> Option<Self> {
        if decimals > MAX_DECIMALS {
            return None;
        }
        Self::new(false, units, -i32::from(decimals))
    }

    /// The whole units of `10^-decimals` it holds, rounded down (toward
    /// minus infinity): what the books can hold of it. `None` if that is
    /// beyond an `i128` or `decimals` is beyond [`MAX_DECIMALS`].
    #[must_use]
    pub fn floor_units(&self, decimals: u8) -> Option<i128> {
        if decimals > MAX_DECIMALS {
            return None;
        }
        if self.is_zero() {
            return Some(0);
        }
        let mantissa = i128::from(self.mantissa);
        let shift = self.exponent + i32::from(decimals);
        if shift >= 0 {
            let magnitude = mantissa.checked_mul(10_i128.checked_pow(shift.unsigned_abs())?)?;
            return Some(if self.negative { -magnitude } else { magnitude });
        }
        // A mantissa has sixteen digits: past that, under one unit.
        let down = shift.unsigned_abs();
        let (whole, rest) = if down > 16 {
            (0, mantissa)
        } else {
            let divisor = 10_i128.pow(down);
            (mantissa / divisor, mantissa % divisor)
        };
        Some(if self.negative {
            -whole - i128::from(rest != 0)
        } else {
            whole
        })
    }

    /// Reads a value as the ledger's JSON writes it: a decimal (`-12.5`)
    /// or a mantissa and an exponent (`1000000000000000e-3`), with no
    /// leading zeros and at most 128 characters; `None` unless the ledger
    /// can express it exactly. Zero may be negative, and is zero.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty() || text.len() > MAX_VALUE_TEXT {
            return None;
        }
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (number, exponent) = match rest.split_once(['e', 'E']) {
            Some((number, exponent)) => (number, Some(exponent)),
            None => (rest, None),
        };
        let (whole, fraction) = match number.split_once('.') {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (number, None),
        };
        let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
        if !digits(whole) || (whole.len() > 1 && whole.starts_with('0')) {
            return None;
        }
        let fraction = fraction.unwrap_or("");
        if number.contains('.') && !digits(fraction) {
            return None;
        }
        let mut exponent = match exponent {
            None => 0,
            Some(text) => {
                let unsigned = text.strip_prefix(['-', '+']).unwrap_or(text);
                if !digits(unsigned) || unsigned.len() > 4 {
                    return None;
                }
                text.parse::<i32>().ok()?
            }
        };
        // The digits, without leading zeros, and their exponent.
        exponent = exponent.checked_sub(i32::try_from(fraction.len()).ok()?)?;
        let all = whole.bytes().chain(fraction.bytes());
        let significant: String = all.skip_while(|b| *b == b'0').map(char::from).collect();
        let trimmed = significant.trim_end_matches('0');
        exponent = exponent.checked_add(i32::try_from(significant.len() - trimmed.len()).ok()?)?;
        if trimmed.is_empty() {
            return Some(Self::ZERO);
        }
        if trimmed.len() > 16 {
            return None;
        }
        Self::new(negative, trimmed.parse().ok()?, exponent)
    }

    /// Its first eight bytes as an amount: not XRP, the sign, the exponent
    /// above 97 and the mantissa; zero has only the "not XRP" bit.
    #[must_use]
    pub fn to_bits(&self) -> u64 {
        if self.is_zero() {
            return NOT_XRP;
        }
        let exponent = u64::try_from(self.exponent + EXPONENT_BIAS).unwrap_or(0);
        let sign = if self.negative { 0 } else { POSITIVE };
        NOT_XRP | sign | exponent << 54 | self.mantissa
    }

    /// The value in an amount's first eight bytes, if they are a token's
    /// in canonical form.
    #[must_use]
    pub fn from_bits(bits: u64) -> Option<Self> {
        if bits & NOT_XRP == 0 {
            return None;
        }
        if bits == NOT_XRP {
            return Some(Self::ZERO);
        }
        let mantissa = bits & MANTISSA_BITS;
        let exponent = i32::try_from((bits >> 54) & 0xff).ok()? - EXPONENT_BIAS;
        if !(MIN_MANTISSA..=MAX_MANTISSA).contains(&mantissa)
            || !(MIN_EXPONENT..=MAX_EXPONENT).contains(&exponent)
        {
            return None;
        }
        Some(Self {
            negative: bits & POSITIVE == 0,
            mantissa,
            exponent,
        })
    }

    /// Its form holds (a bug otherwise).
    fn check(&self) {
        if self.mantissa == 0 {
            assert!(!self.negative && self.exponent == 0, "one zero");
        } else {
            assert!(
                (MIN_MANTISSA..=MAX_MANTISSA).contains(&self.mantissa)
                    && (MIN_EXPONENT..=MAX_EXPONENT).contains(&self.exponent),
                "a normalised value"
            );
        }
    }
}

impl Ord for TokenValue {
    fn cmp(&self, other: &Self) -> Ordering {
        // By sign (negative, zero, positive), then by magnitude, which for
        // normalised values orders by exponent and then mantissa.
        let class = |value: &Self| match (value.is_zero(), value.negative) {
            (true, _) => 1,
            (false, true) => 0,
            (false, false) => 2,
        };
        let magnitude = (self.exponent, self.mantissa).cmp(&(other.exponent, other.mantissa));
        match class(self).cmp(&class(other)) {
            Ordering::Equal if self.is_zero() => Ordering::Equal,
            Ordering::Equal if self.negative => magnitude.reverse(),
            Ordering::Equal => magnitude,
            unequal => unequal,
        }
    }
}

impl PartialOrd for TokenValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// As rippled writes a value in JSON: zero as `0`; a decimal while the
/// exponent is 0 or from -25 to -5, without trailing zeros; otherwise the
/// mantissa, `e` and the exponent.
impl fmt::Display for TokenValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            return f.write_str("0");
        }
        if self.negative {
            f.write_str("-")?;
        }
        if self.exponent != 0 && !(-25..=-5).contains(&self.exponent) {
            return write!(f, "{}e{}", self.mantissa, self.exponent);
        }
        let down = self.exponent.unsigned_abs();
        let divisor = 10_u128.pow(down);
        let mantissa = u128::from(self.mantissa);
        write!(f, "{}", mantissa / divisor)?;
        let rest = mantissa % divisor;
        if rest != 0 {
            let width = usize::try_from(down).unwrap_or(0);
            let fraction = alloc::format!("{rest:0width$}");
            write!(f, ".{}", fraction.trim_end_matches('0'))?;
        }
        Ok(())
    }
}

impl fmt::Debug for TokenValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A token's currency code: 160 bits. A standard code is three characters
/// at bytes 12 to 14, the rest zero; any other is written in hex, such as
/// RLUSD's.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency(pub [u8; 20]);

/// The characters of a standard code, as rippled takes them.
const ISO_CHARACTERS: &[u8] =
    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789<>(){}[]|?!@#$%^&*";

impl Currency {
    /// A standard three-character code, or 40 hex digits; `None` for XRP
    /// or anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let currency = if text.len() == 3 {
            if !text.bytes().all(|b| ISO_CHARACTERS.contains(&b)) {
                return None;
            }
            let mut bytes = [0; 20];
            bytes.get_mut(12..15)?.copy_from_slice(text.as_bytes());
            Self(bytes)
        } else if text.len() == 40 {
            Self(from_hex(text)?.try_into().ok()?)
        } else {
            return None;
        };
        currency.is_token().then_some(currency)
    }

    /// Whether a token can be of it: not all zeros (XRP itself), nor the
    /// standard code `XRP`, which the ledger refuses.
    #[must_use]
    pub fn is_token(&self) -> bool {
        self.0 != [0; 20] && self.standard() != Some(*b"XRP")
    }

    /// Its three characters, if it is a standard code.
    fn standard(&self) -> Option<[u8; 3]> {
        let (head, rest) = self.0.split_at(12);
        let (code, tail) = rest.split_at(3);
        let code: [u8; 3] = code.try_into().ok()?;
        (head.iter().chain(tail).all(|b| *b == 0)
            && code.iter().all(|b| ISO_CHARACTERS.contains(b)))
        .then_some(code)
    }
}

/// As the ledger writes it: three characters for a standard code, other
/// than `XRP`, and uppercase hex otherwise.
impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.standard() {
            Some(code) if &code != b"XRP" => {
                f.write_str(core::str::from_utf8(&code).map_err(|_| fmt::Error)?)
            }
            _ => f.write_str(&hex_upper(&self.0)),
        }
    }
}

impl fmt::Debug for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// An amount of a token: its value, its currency and its issuer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TokenAmount {
    /// How much.
    pub value: TokenValue,
    /// Of what.
    pub currency: Currency,
    /// Issued by whom.
    pub issuer: AccountId,
}

impl TokenAmount {
    /// Whether the ledger takes it as a token: of a token's currency, by an
    /// issuer other than the zero account.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.currency.is_token() && self.issuer.0 != [0; 20]
    }

    /// Its 48 bytes: the value, the currency, the issuer.
    ///
    /// # Panics
    ///
    /// If it is not [valid](Self::is_valid) (a bug: the node builds every
    /// amount it writes).
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 48] {
        assert!(self.is_valid(), "a token's currency and issuer");
        let mut out = [0; 48];
        let (value, rest) = out.split_at_mut(8);
        let (currency, issuer) = rest.split_at_mut(20);
        value.copy_from_slice(&self.value.to_bits().to_be_bytes());
        currency.copy_from_slice(&self.currency.0);
        issuer.copy_from_slice(&self.issuer.0);
        out
    }

    /// The token amount in 48 bytes, if it is one in canonical form.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; 48]) -> Option<Self> {
        let (value, rest) = bytes.split_at(8);
        let (currency, issuer) = rest.split_at(20);
        let amount = Self {
            value: TokenValue::from_bits(u64::from_be_bytes(value.try_into().ok()?))?,
            currency: Currency(currency.try_into().ok()?),
            issuer: AccountId(issuer.try_into().ok()?),
        };
        amount.is_valid().then_some(amount)
    }
}

/// An amount: XRP in drops, or a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Amount {
    /// Drops of XRP, at most [`MAX_DROPS`](crate::MAX_DROPS).
    Xrp(u64),
    /// A token of an issuer.
    Token(TokenAmount),
}

impl Amount {
    /// The drops, if it is XRP.
    #[must_use]
    pub const fn drops(&self) -> Option<u64> {
        match self {
            Self::Xrp(drops) => Some(*drops),
            Self::Token(_) => None,
        }
    }

    /// The token amount, if it is one.
    #[must_use]
    pub const fn token(&self) -> Option<&TokenAmount> {
        match self {
            Self::Xrp(_) => None,
            Self::Token(token) => Some(token),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    fn value(text: &str) -> TokenValue {
        TokenValue::parse(text).unwrap()
    }

    #[test]
    fn values_normalise_to_one_form() {
        assert_eq!(
            value("1"),
            TokenValue::new(false, MIN_MANTISSA, -15).unwrap()
        );
        assert_eq!(value("1.000"), value("1"));
        assert_eq!(value("100e-2"), value("1"));
        assert_eq!(value("-0"), TokenValue::ZERO);
        assert_eq!(value("0.000"), TokenValue::ZERO);
        assert_eq!(
            value("1e-81"),
            TokenValue::new(false, MIN_MANTISSA, -96).unwrap()
        );
        assert_eq!(value("9999999999999999e80"), TokenValue::MAX);
        assert_eq!(TokenValue::new(false, 12_345_678_901_234_567, 0), None);
        assert_eq!(
            TokenValue::new(false, 12_345_678_901_234_560, 0),
            Some(value("1234567890123456e1"))
        );
    }

    #[test]
    fn values_are_refused_unless_exact_and_canonical() {
        for text in [
            "",
            "-",
            "+1",
            ".5",
            "5.",
            "01",
            "00",
            "1e",
            "1e+",
            "1.e3",
            "1e99999",
            "1x",
            "1e-82",
            "1e97",
            "12345678901234567",
            "1.2345678901234567",
            "0x10",
            " 1",
            "1 ",
            "--1",
            "NaN",
        ] {
            assert_eq!(TokenValue::parse(text), None, "{text}");
        }
        assert!(TokenValue::parse(&"1".repeat(129)).is_none());
    }

    #[test]
    fn values_write_as_rippled_does() {
        for (text, written) in [
            ("0", "0"),
            ("1", "1"),
            ("-1.5", "-1.5"),
            ("0.000001", "0.000001"),
            ("1234567890.123456", "1234567890.123456"),
            ("1000000000000000", "1000000000000000"),
            // Exponents from -4 to -1, and above 0, are scientific.
            ("100000000000", "1000000000000000e-4"),
            ("10000000000000000", "1000000000000000e1"),
            ("1e-81", "1000000000000000e-96"),
            ("0.0000000001", "0.0000000001"),
            ("0.00000000001", "1000000000000000e-26"),
            ("9999999999999999e80", "9999999999999999e80"),
        ] {
            assert_eq!(value(text).to_string(), written, "{text}");
            assert_eq!(value(written), value(text));
        }
    }

    #[test]
    fn values_order_by_what_they_are_worth() {
        let ordered = [
            "-9999999999999999e80",
            "-2",
            "-1",
            "-1e-81",
            "0",
            "1e-81",
            "0.5",
            "1",
            "1.5",
            "10",
            "9999999999999999e80",
        ];
        for pair in ordered.windows(2) {
            if let [low, high] = pair {
                assert!(value(low) < value(high), "{low} < {high}");
            }
        }
    }

    #[test]
    fn whole_units_round_down() {
        assert_eq!(value("1.0000019").floor_units(6), Some(1_000_001));
        assert_eq!(value("-1.0000019").floor_units(6), Some(-1_000_002));
        assert_eq!(value("-1").floor_units(6), Some(-1_000_000));
        assert_eq!(value("1e-81").floor_units(6), Some(0));
        assert_eq!(value("-1e-81").floor_units(6), Some(-1));
        assert_eq!(value("9999999999999999e80").floor_units(6), None);
        assert_eq!(
            value("1e20").floor_units(6),
            Some(100_000_000_000_000_000_000_000_000)
        );
        assert_eq!(TokenValue::from_units(1, 6), Some(value("0.000001")));
        assert_eq!(TokenValue::from_units(0, 6), Some(TokenValue::ZERO));
        assert_eq!(TokenValue::from_units(u64::MAX, 6), None);
        assert_eq!(
            TokenValue::from_units(10_000_000_000_000_000_000, 6),
            Some(value("1e13"))
        );
        assert_eq!(TokenValue::from_units(1, 97), None);
    }

    #[test]
    fn sums_are_exact_or_refused() {
        let sum = |a: &str, b: &str| value(a).checked_add(value(b));
        assert_eq!(sum("1", "2.5"), Some(value("3.5")));
        assert_eq!(sum("1", "-1"), Some(TokenValue::ZERO));
        assert_eq!(sum("-1.5", "1"), Some(value("-0.5")));
        assert_eq!(sum("0", "-7"), Some(value("-7")));
        assert_eq!(sum("1000000000000000", "0.1"), None);
        assert_eq!(
            sum("100000000000000", "0.1"),
            Some(value("100000000000000.1"))
        );
        assert_eq!(sum("9999999999999999", "1"), Some(value("1e16")));
        assert_eq!(sum("9999999999999999e80", "1e80"), None);
        assert_eq!(sum("1e-81", "-1e-81"), Some(TokenValue::ZERO));
        assert_eq!(sum("1e10", "1e-10"), None);
        assert_eq!(value("5").checked_sub(value("7")), Some(value("-2")));
        assert_eq!(
            TokenValue::MAX.checked_sub(TokenValue::MAX),
            Some(TokenValue::ZERO)
        );
    }

    #[test]
    fn bits_are_canonical() {
        assert_eq!(TokenValue::ZERO.to_bits(), 0x8000_0000_0000_0000);
        assert_eq!(value("1").to_bits(), 0xD483_8D7E_A4C6_8000);
        assert_eq!(value("-1").to_bits(), 0x9483_8D7E_A4C6_8000);
        // Negative zero, a short mantissa and a stray exponent are no form.
        for bits in [
            0xC000_0000_0000_0000,
            0x8000_0000_0000_0001,
            0xD480_0000_0000_0000,
            0x8040_0000_0000_0000,
        ] {
            assert_eq!(TokenValue::from_bits(bits), None, "{bits:x}");
        }
        assert_eq!(TokenValue::from_bits(0x4000_0000_0000_0001), None);
    }

    #[test]
    fn currencies_read_and_write_as_the_ledger_does() {
        let rlusd = Currency::parse("524C555344000000000000000000000000000000").unwrap();
        assert_eq!(
            rlusd.to_string(),
            "524C555344000000000000000000000000000000"
        );
        let usd = Currency::parse("USD").unwrap();
        assert_eq!(usd.to_string(), "USD");
        assert_eq!(
            Currency::parse("0000000000000000000000005553440000000000"),
            Some(usd)
        );
        for refused in [
            "XRP",
            "0000000000000000000000000000000000000000",
            "0000000000000000000000005852500000000000",
            "US",
            "USDT",
            "U D",
        ] {
            assert_eq!(Currency::parse(refused), None, "{refused}");
        }
    }
}

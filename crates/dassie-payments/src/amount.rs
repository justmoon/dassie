//! Amounts: an unsigned 64-bit value in an asset's smallest unit, written
//! as a decimal string.

use alloc::string::{String, ToString};
use core::fmt;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The longest asset code accepted, in bytes.
pub const MAX_ASSET_CODE_BYTES: usize = 32;

/// An amount (`amount` in the specs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Amount {
    /// The value in the asset's smallest unit.
    #[serde(with = "decimal")]
    pub value: u64,
    /// The asset, an ISO 4217 code where there is one.
    #[serde(deserialize_with = "asset_code")]
    pub asset_code: String,
    /// Decimal places of the smallest unit.
    pub asset_scale: u8,
}

impl Amount {
    /// An amount of `value` in an asset.
    #[must_use]
    pub fn new(value: u64, asset_code: &str, asset_scale: u8) -> Self {
        Self {
            value,
            asset_code: asset_code.to_string(),
            asset_scale,
        }
    }

    /// Whether `other` is in the same asset.
    #[must_use]
    pub fn same_asset(&self, asset_code: &str, asset_scale: u8) -> bool {
        self.asset_code == asset_code && self.asset_scale == asset_scale
    }
}

/// An asset as errors name it: `USD at scale 9`. Both code and scale,
/// since two assets may differ only in scale.
#[must_use]
pub fn asset_name(asset_code: &str, asset_scale: u8) -> String {
    alloc::format!("{asset_code} at scale {asset_scale}")
}

fn asset_code<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let code = String::deserialize(deserializer)?;
    let printable = code.bytes().all(|byte| byte.is_ascii_graphic());
    if code.is_empty() || code.len() > MAX_ASSET_CODE_BYTES || !printable {
        return Err(de::Error::custom("invalid asset code"));
    }
    Ok(code)
}

/// Decimal places of an asset's unit below which a payment is paid exactly
/// (spec/decisions/paid-within-a-tolerance.md).
const TOLERANCE_PLACES: u8 = 4;

/// The share of a payment's amount its tolerance never exceeds: a
/// thousandth.
const TOLERANCE_SHARE: u64 = 1_000;

/// How far a payment may land from its amount, over or under, and still
/// count as paid exactly, in the smallest unit of an asset at
/// `asset_scale`: a ten-thousandth of the asset's unit, nothing for an
/// asset with fewer than four decimal places
/// (spec/decisions/paid-within-a-tolerance.md).
#[must_use]
pub const fn tolerance(asset_scale: u8) -> u64 {
    if asset_scale < TOLERANCE_PLACES {
        return 0;
    }
    let mut tolerance: u64 = 1;
    let mut place = TOLERANCE_PLACES;
    while place < asset_scale {
        tolerance = tolerance.saturating_mul(10);
        place += 1;
    }
    tolerance
}

/// The tolerance of a payment of `amount` in an asset at `asset_scale`:
/// the asset's [`tolerance`], but never more than a thousandth of the
/// amount, so that a small payment is paid exactly and none counts as paid
/// before it received anything. A payment without an amount has none.
#[must_use]
pub fn payment_tolerance(amount: Option<u64>, asset_scale: u8) -> u64 {
    amount.map_or(0, |amount| {
        tolerance(asset_scale).min(amount / TOLERANCE_SHARE)
    })
}

/// Whether `received` pays an incoming payment of `amount`, in an asset at
/// `asset_scale`, in full: all of it, or all but less than its
/// [`payment_tolerance`]. One without an amount never is.
#[must_use]
pub fn paid_in_full(amount: Option<u64>, received: u64, asset_scale: u8) -> bool {
    let tolerance = payment_tolerance(amount, asset_scale);
    amount.is_some_and(|amount| received.saturating_add(tolerance) >= amount)
}

/// Parses an unsigned decimal: digits only, without a sign or leading
/// zeros, at most `u64::MAX`.
#[must_use]
pub fn parse_u64(input: &str) -> Option<u64> {
    let canonical = !input.is_empty()
        && input.bytes().all(|byte| byte.is_ascii_digit())
        && (input == "0" || !input.starts_with('0'));
    if canonical { input.parse().ok() } else { None }
}

/// Serde for a `u64` written as a decimal string.
pub mod decimal {
    use super::*;

    /// Writes the value as a string.
    ///
    /// # Errors
    ///
    /// Only the serializer's own.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    /// Reads a string of digits.
    ///
    /// # Errors
    ///
    /// If the string is not a canonical unsigned 64-bit decimal.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
        deserializer.deserialize_str(DecimalVisitor)
    }

    struct DecimalVisitor;

    impl Visitor<'_> for DecimalVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an unsigned 64-bit integer as a decimal string")
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<u64, E> {
            parse_u64(value).ok_or_else(|| E::custom("invalid amount value"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_the_spec_form() {
        let json = r#"{"value":"18446744073709551615","assetCode":"USD","assetScale":2}"#;
        let amount: Amount = serde_json::from_str(json).unwrap();
        assert_eq!(amount, Amount::new(u64::MAX, "USD", 2));
        assert_eq!(serde_json::to_string(&amount).unwrap(), json);
    }

    #[test]
    fn a_payment_is_paid_in_full_within_its_tolerance() {
        // A dollar at scale 9: a ten-thousandth of a dollar either way.
        assert_eq!(payment_tolerance(Some(1_000_000_000), 9), 100_000);
        assert!(!paid_in_full(Some(1_000_000_000), 999_899_999, 9));
        assert!(paid_in_full(Some(1_000_000_000), 999_900_000, 9));
        assert!(paid_in_full(Some(1_000_000_000), 1_000_100_000, 9));
        // A tenth of a cent: a thousandth of it.
        assert_eq!(payment_tolerance(Some(1_000_000), 9), 1_000);
        assert!(!paid_in_full(Some(1_000_000), 998_999, 9));
        assert!(paid_in_full(Some(1_000_000), 999_000, 9));
        // Below a thousand units, exactly.
        assert_eq!(payment_tolerance(Some(999), 9), 0);
        assert!(!paid_in_full(Some(100), 99, 9));
        assert!(paid_in_full(Some(100), 100, 9));
        // Without an amount, never.
        assert_eq!(payment_tolerance(None, 9), 0);
        assert!(!paid_in_full(None, u64::MAX, 9));
    }

    #[test]
    fn the_tolerance_is_a_ten_thousandth_of_the_unit() {
        assert_eq!(tolerance(9), 100_000);
        assert_eq!(tolerance(6), 100);
        assert_eq!(tolerance(4), 1);
        assert_eq!(tolerance(2), 0);
        assert_eq!(tolerance(0), 0);
        assert_eq!(tolerance(23), 10_000_000_000_000_000_000);
        assert_eq!(tolerance(u8::MAX), u64::MAX);
    }

    #[test]
    fn refuses_what_is_not_a_uint64_string() {
        for value in [
            "\"\"",
            "\"-1\"",
            "\"+1\"",
            "\"01\"",
            "\"1.5\"",
            "1",
            "\"18446744073709551616\"",
        ] {
            let json = alloc::format!(r#"{{"value":{value},"assetCode":"USD","assetScale":2}}"#);
            assert!(serde_json::from_str::<Amount>(&json).is_err(), "{value}");
        }
        let scale = r#"{"value":"1","assetCode":"USD","assetScale":256}"#;
        assert!(serde_json::from_str::<Amount>(scale).is_err());
        let code = r#"{"value":"1","assetCode":"","assetScale":2}"#;
        assert!(serde_json::from_str::<Amount>(code).is_err());
    }
}

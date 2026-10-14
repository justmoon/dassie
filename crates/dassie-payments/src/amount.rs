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

fn asset_code<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let code = String::deserialize(deserializer)?;
    let printable = code.bytes().all(|byte| byte.is_ascii_graphic());
    if code.is_empty() || code.len() > MAX_ASSET_CODE_BYTES || !printable {
        return Err(de::Error::custom("invalid asset code"));
    }
    Ok(code)
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

//! The ledgers a node knows, and their assets.

use core::fmt;

/// Real or test money. The same as the node's realm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Realm {
    /// Test money.
    Test,
    /// Real money; refused until D8's criteria are met.
    Live,
}

/// An asset: its code and how many decimal places an amount has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asset {
    /// For example `USD`.
    pub code: &'static str,
    /// An amount of 1 is 10^-scale units of the asset.
    pub scale: u8,
}

/// An internal ledger. Named `<settlement family>+<currency>`, as in the
/// original (spec 5 §1.1). The list is closed: a new ledger comes with the
/// settlement module that serves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LedgerId {
    /// The stub settlement module's US dollars.
    StubUsd,
    /// XRP on the XRP Ledger testnet.
    XrplTestnetXrp,
    /// XRP on the XRP Ledger mainnet.
    XrplXrp,
}

impl LedgerId {
    /// Every ledger, in a fixed order.
    pub const ALL: [Self; 3] = [Self::StubUsd, Self::XrplTestnetXrp, Self::XrplXrp];

    /// The ledger's name, for example `stub+usd`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StubUsd => "stub+usd",
            Self::XrplTestnetXrp => "xrpl-testnet+xrp",
            Self::XrplXrp => "xrpl+xrp",
        }
    }

    /// The ledger called `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ledger| ledger.as_str() == name)
    }

    /// The asset the ledger counts. Both currencies use scale 9 internally
    /// (spec 5 §1.2), so 1 USD is 10^9 units.
    #[must_use]
    pub const fn asset(self) -> Asset {
        match self {
            Self::StubUsd => Asset {
                code: "USD",
                scale: 9,
            },
            Self::XrplTestnetXrp | Self::XrplXrp => Asset {
                code: "XRP",
                scale: 9,
            },
        }
    }

    /// Whether the ledger holds real money.
    #[must_use]
    pub const fn realm(self) -> Realm {
        match self {
            Self::StubUsd | Self::XrplTestnetXrp => Realm::Test,
            Self::XrplXrp => Realm::Live,
        }
    }
}

impl fmt::Display for LedgerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for ledger in LedgerId::ALL {
            assert_eq!(LedgerId::parse(ledger.as_str()), Some(ledger));
        }
        assert_eq!(LedgerId::parse("stub+eur"), None);
    }
}

//! The ledgers a node knows, and their assets.

use core::fmt;

/// Real or test money. The same as the node's realm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Realm {
    /// Test money.
    Test,
    /// Real money; refused until the criteria of spec/decisions/real-money.md
    /// are met.
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
/// original (spec/areas/ledger.md#ledgers-and-amounts). The list is closed:
/// a new ledger comes with the settlement module that serves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LedgerId {
    /// The stub settlement module's US dollars.
    StubUsd,
    /// XRP on the XRP Ledger testnet.
    XrplTestnetXrp,
    /// XRP on the XRP Ledger mainnet.
    XrplXrp,
    /// RLUSD on the XRP Ledger testnet, a dollar token booked as `USD` at
    /// par (spec/decisions/issued-assets.md). Its slug, `xrpl-testnet-rlusd`,
    /// differs from the XRP ledger's, since a ledger's slug names its
    /// receiver, its seed path and its wallet address.
    XrplTestnetRlusdUsd,
    /// DSUSD on the XRP Ledger testnet, Dassie's own testnet dollar token,
    /// booked as `USD` at par as RLUSD is, on a ledger of its own.
    XrplTestnetDsusdUsd,
}

impl LedgerId {
    /// Every ledger, in a fixed order.
    pub const ALL: [Self; 5] = [
        Self::StubUsd,
        Self::XrplTestnetXrp,
        Self::XrplXrp,
        Self::XrplTestnetRlusdUsd,
        Self::XrplTestnetDsusdUsd,
    ];

    /// The ledger's name, for example `stub+usd`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StubUsd => "stub+usd",
            Self::XrplTestnetXrp => "xrpl-testnet+xrp",
            Self::XrplXrp => "xrpl+xrp",
            Self::XrplTestnetRlusdUsd => "xrpl-testnet-rlusd+usd",
            Self::XrplTestnetDsusdUsd => "xrpl-testnet-dsusd+usd",
        }
    }

    /// The ledger called `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|ledger| ledger.as_str() == name)
    }

    /// The asset the ledger counts. Both currencies use scale 9 internally
    /// (spec/areas/ledger.md#ledgers-and-amounts), so 1 USD is 10^9 units.
    #[must_use]
    pub const fn asset(self) -> Asset {
        match self {
            Self::StubUsd | Self::XrplTestnetRlusdUsd | Self::XrplTestnetDsusdUsd => Asset {
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
            Self::StubUsd
            | Self::XrplTestnetXrp
            | Self::XrplTestnetRlusdUsd
            | Self::XrplTestnetDsusdUsd => Realm::Test,
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

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
    /// receiver and its seed path; its wallet address is its wallet name. Its scheme is
    /// disabled (spec/decisions/mainnet-schemes.md); the ledger stays, so
    /// the books of a node that ran it keep their name.
    XrplTestnetRlusdUsd,
    /// RLUSD on the XRP Ledger mainnet, booked as `USD` at par as test
    /// RLUSD was: real money.
    XrplRlusdUsd,
    /// DSUSD on the XRP Ledger testnet, Dassie's own testnet dollar token,
    /// booked as `USD` at par as RLUSD is, on a ledger of its own.
    XrplTestnetDsusdUsd,
    /// XRP in the node's settlement hub on the XRP Ledger testnet
    /// (spec/designs/settlement-hubs.md): a ledger of its own, since what
    /// the hub holds is a claim on its validators, not on the XRP Ledger.
    HubXrplTestnetXrp,
    /// DSUSD in a settlement hub on the XRP Ledger testnet, booked as `USD`
    /// at par as the token is (spec/decisions/hub-scheme-names.md). No
    /// scheme books on it yet (spec/roadmap.md#token-hubs).
    HubXrplTestnetDsusdUsd,
}

impl LedgerId {
    /// Every ledger, in a fixed order.
    pub const ALL: [Self; 8] = [
        Self::StubUsd,
        Self::XrplTestnetXrp,
        Self::XrplXrp,
        Self::XrplTestnetRlusdUsd,
        Self::XrplRlusdUsd,
        Self::XrplTestnetDsusdUsd,
        Self::HubXrplTestnetXrp,
        Self::HubXrplTestnetDsusdUsd,
    ];

    /// The ledger's name, for example `stub+usd`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StubUsd => "stub+usd",
            Self::XrplTestnetXrp => "xrpl-testnet-xrp+xrp",
            Self::XrplXrp => "xrpl-xrp+xrp",
            Self::XrplTestnetRlusdUsd => "xrpl-testnet-rlusd+usd",
            Self::XrplRlusdUsd => "xrpl-rlusd+usd",
            Self::XrplTestnetDsusdUsd => "xrpl-testnet-dsusd+usd",
            Self::HubXrplTestnetXrp => "hub-xrpl-testnet-xrp+xrp",
            Self::HubXrplTestnetDsusdUsd => "hub-xrpl-testnet-dsusd+usd",
        }
    }

    /// The ledger's name for people, in its wallet address
    /// (`/pay/<name>`): its asset, short, `hub-` before it for a settlement
    /// hub's. Unique among the ledgers of a realm, which are all a node
    /// runs. Its ID's slug, which names its scheme, its STREAM receiver and
    /// its payer, is plumbing (spec/decisions/scheme-names.md).
    #[must_use]
    pub const fn wallet_name(self) -> &'static str {
        match self {
            Self::StubUsd => "usd",
            Self::XrplTestnetXrp | Self::XrplXrp => "xrp",
            Self::XrplTestnetRlusdUsd | Self::XrplRlusdUsd => "rlusd",
            Self::XrplTestnetDsusdUsd => "dsusd",
            Self::HubXrplTestnetXrp => "hub-xrp",
            Self::HubXrplTestnetDsusdUsd => "hub-dsusd",
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
            Self::StubUsd
            | Self::XrplTestnetRlusdUsd
            | Self::XrplRlusdUsd
            | Self::XrplTestnetDsusdUsd
            | Self::HubXrplTestnetDsusdUsd => Asset {
                code: "USD",
                scale: 9,
            },
            Self::XrplTestnetXrp | Self::XrplXrp | Self::HubXrplTestnetXrp => Asset {
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
            | Self::XrplTestnetDsusdUsd
            | Self::HubXrplTestnetXrp
            | Self::HubXrplTestnetDsusdUsd => Realm::Test,
            Self::XrplXrp | Self::XrplRlusdUsd => Realm::Live,
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

    // spec/decisions/scheme-names.md: a ledger's name for people is unique
    // among its realm's, and names its asset, a hub's with `hub-` before
    // it; its slug names its scheme.
    #[test]
    fn wallet_names_are_unique_within_a_realm() {
        for ledger in LedgerId::ALL {
            let name = ledger.wallet_name();
            let code = ledger.asset().code.to_ascii_lowercase();
            let asset = name.strip_prefix("hub-").unwrap_or(name);
            assert!(
                asset == code || ledger.asset().code == "USD",
                "{ledger}: {name}"
            );
            assert_eq!(
                name.starts_with("hub-"),
                ledger.as_str().starts_with("hub-"),
                "{ledger}"
            );
            for other in LedgerId::ALL {
                if other != ledger && other.realm() == ledger.realm() {
                    assert_ne!(name, other.wallet_name(), "{ledger}, {other}");
                }
            }
        }
    }
}

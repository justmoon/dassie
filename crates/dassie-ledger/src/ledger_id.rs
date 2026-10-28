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
    /// receiver; its wallet address is its wallet name. Its scheme is
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
    /// at par as the token is (spec/decisions/hub-scheme-names.md): the
    /// ledger of the `hub-xrpl-testnet-dsusd` scheme.
    HubXrplTestnetDsusdUsd,
    /// XRP in a settlement hub on the XRP Ledger mainnet: real money, the
    /// ledger of the `hub-xrpl-xrp` scheme.
    HubXrplXrp,
    /// RLUSD in a settlement hub on the XRP Ledger mainnet, booked as `USD`
    /// at par as the token is: real money, the ledger of the
    /// `hub-xrpl-rlusd` scheme.
    HubXrplRlusdUsd,
}

impl LedgerId {
    /// Every ledger, in a fixed order.
    pub const ALL: [Self; 10] = [
        Self::StubUsd,
        Self::XrplTestnetXrp,
        Self::XrplXrp,
        Self::XrplTestnetRlusdUsd,
        Self::XrplRlusdUsd,
        Self::XrplTestnetDsusdUsd,
        Self::HubXrplTestnetXrp,
        Self::HubXrplTestnetDsusdUsd,
        Self::HubXrplXrp,
        Self::HubXrplRlusdUsd,
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
            Self::HubXrplXrp => "hub-xrpl-xrp+xrp",
            Self::HubXrplRlusdUsd => "hub-xrpl-rlusd+usd",
        }
    }

    /// The name of the node's wallet address for the ledger's asset
    /// (`/pay/<name>`), for people: the asset, short. A settlement hub's
    /// ledger shares it with the owner book of its asset, since an asset
    /// has one wallet address, which books what arrives over Interledger
    /// on the hub's ledger where the node runs one ([`Self::is_hub`]). A
    /// realm has at most one ledger of each name besides a hub's, which
    /// are all a node runs. Its ID's slug, which names its scheme, its
    /// STREAM receiver and its payer, is plumbing
    /// (spec/decisions/scheme-names.md).
    #[must_use]
    pub const fn wallet_name(self) -> &'static str {
        match self {
            Self::StubUsd => "usd",
            Self::XrplTestnetXrp | Self::XrplXrp | Self::HubXrplTestnetXrp | Self::HubXrplXrp => {
                "xrp"
            }
            Self::XrplTestnetRlusdUsd | Self::XrplRlusdUsd | Self::HubXrplRlusdUsd => "rlusd",
            Self::XrplTestnetDsusdUsd | Self::HubXrplTestnetDsusdUsd => "dsusd",
        }
    }

    /// Whether the ledger is a settlement hub's: what the node holds in
    /// its account there.
    #[must_use]
    pub const fn is_hub(self) -> bool {
        match self {
            Self::HubXrplTestnetXrp
            | Self::HubXrplTestnetDsusdUsd
            | Self::HubXrplXrp
            | Self::HubXrplRlusdUsd => true,
            Self::StubUsd
            | Self::XrplTestnetXrp
            | Self::XrplXrp
            | Self::XrplTestnetRlusdUsd
            | Self::XrplRlusdUsd
            | Self::XrplTestnetDsusdUsd => false,
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
            | Self::HubXrplTestnetDsusdUsd
            | Self::HubXrplRlusdUsd => Asset {
                code: "USD",
                scale: 9,
            },
            Self::XrplTestnetXrp | Self::XrplXrp | Self::HubXrplTestnetXrp | Self::HubXrplXrp => {
                Asset {
                    code: "XRP",
                    scale: 9,
                }
            }
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
            Self::XrplXrp | Self::XrplRlusdUsd | Self::HubXrplXrp | Self::HubXrplRlusdUsd => {
                Realm::Live
            }
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

    // spec/decisions/scheme-names.md, spec/areas/payments.md#wallet-addresses:
    // a wallet address names an asset, which a hub's ledger shares with the
    // owner book of its asset; within a realm, at most one ledger besides a
    // hub's has each name, and so does at most one hub's.
    #[test]
    fn wallet_names_name_one_asset_within_a_realm() {
        for ledger in LedgerId::ALL {
            let name = ledger.wallet_name();
            let code = ledger.asset().code.to_ascii_lowercase();
            assert!(
                name == code || ledger.asset().code == "USD",
                "{ledger}: {name}"
            );
            assert_eq!(
                ledger.is_hub(),
                ledger.as_str().starts_with("hub-"),
                "{ledger}"
            );
            for other in LedgerId::ALL {
                if other != ledger
                    && other.realm() == ledger.realm()
                    && other.is_hub() == ledger.is_hub()
                {
                    assert_ne!(name, other.wallet_name(), "{ledger}, {other}");
                }
            }
        }
        assert_eq!(LedgerId::HubXrplTestnetXrp.wallet_name(), "xrp");
        assert_eq!(LedgerId::HubXrplRlusdUsd.wallet_name(), "rlusd");
    }
}

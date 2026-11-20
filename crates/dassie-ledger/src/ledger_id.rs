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

/// One value for each ledger: what differs by ledger, such as the largest
/// packet and the default credit (spec/decisions/limits-per-ledger.md).
/// Build one from a `const fn` over [`LedgerId`] with [`per_ledger!`],
/// whose exhaustive match gives a new ledger its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PerLedger<T>([T; LedgerId::ALL.len()]);

impl<T: Copy> PerLedger<T> {
    /// `value` on every ledger.
    #[must_use]
    pub const fn splat(value: T) -> Self {
        Self([value; LedgerId::ALL.len()])
    }

    /// The values of the ledgers in [`LedgerId::ALL`]'s order, as
    /// [`per_ledger!`] lists them.
    #[doc(hidden)]
    #[must_use]
    pub const fn in_order(values: [T; LedgerId::ALL.len()]) -> Self {
        Self(values)
    }

    /// The value on `ledger`.
    #[must_use]
    pub const fn get(&self, ledger: LedgerId) -> T {
        let [
            stub,
            testnet_xrp,
            xrp,
            testnet_rlusd,
            rlusd,
            testnet_dsusd,
            hub_testnet_xrp,
            hub_testnet_dsusd,
            hub_xrp,
            hub_rlusd,
        ] = self.0;
        match ledger {
            LedgerId::StubUsd => stub,
            LedgerId::XrplTestnetXrp => testnet_xrp,
            LedgerId::XrplXrp => xrp,
            LedgerId::XrplTestnetRlusdUsd => testnet_rlusd,
            LedgerId::XrplRlusdUsd => rlusd,
            LedgerId::XrplTestnetDsusdUsd => testnet_dsusd,
            LedgerId::HubXrplTestnetXrp => hub_testnet_xrp,
            LedgerId::HubXrplTestnetDsusdUsd => hub_testnet_dsusd,
            LedgerId::HubXrplXrp => hub_xrp,
            LedgerId::HubXrplRlusdUsd => hub_rlusd,
        }
    }

    /// The same values, but `value` on `ledger`.
    #[must_use]
    pub const fn with(self, ledger: LedgerId, value: T) -> Self {
        let mut values = self.0;
        let [
            stub,
            testnet_xrp,
            xrp,
            testnet_rlusd,
            rlusd,
            testnet_dsusd,
            hub_testnet_xrp,
            hub_testnet_dsusd,
            hub_xrp,
            hub_rlusd,
        ] = &mut values;
        let slot = match ledger {
            LedgerId::StubUsd => stub,
            LedgerId::XrplTestnetXrp => testnet_xrp,
            LedgerId::XrplXrp => xrp,
            LedgerId::XrplTestnetRlusdUsd => testnet_rlusd,
            LedgerId::XrplRlusdUsd => rlusd,
            LedgerId::XrplTestnetDsusdUsd => testnet_dsusd,
            LedgerId::HubXrplTestnetXrp => hub_testnet_xrp,
            LedgerId::HubXrplTestnetDsusdUsd => hub_testnet_dsusd,
            LedgerId::HubXrplXrp => hub_xrp,
            LedgerId::HubXrplRlusdUsd => hub_rlusd,
        };
        *slot = value;
        Self(values)
    }

    /// What `f` makes of each ledger's value.
    #[must_use]
    pub fn map<U: Copy>(&self, f: impl Fn(T) -> U) -> PerLedger<U> {
        PerLedger(self.0.map(f))
    }

    /// Every ledger with its value, in [`LedgerId::ALL`]'s order.
    pub fn iter(&self) -> impl Iterator<Item = (LedgerId, T)> + '_ {
        LedgerId::ALL
            .into_iter()
            .map(|ledger| (ledger, self.get(ledger)))
    }
}

/// A [`PerLedger`] of what a `const fn(LedgerId) -> T` gives each ledger,
/// in a constant.
#[macro_export]
macro_rules! per_ledger {
    ($of:path) => {
        $crate::PerLedger::in_order([
            $of($crate::LedgerId::StubUsd),
            $of($crate::LedgerId::XrplTestnetXrp),
            $of($crate::LedgerId::XrplXrp),
            $of($crate::LedgerId::XrplTestnetRlusdUsd),
            $of($crate::LedgerId::XrplRlusdUsd),
            $of($crate::LedgerId::XrplTestnetDsusdUsd),
            $of($crate::LedgerId::HubXrplTestnetXrp),
            $of($crate::LedgerId::HubXrplTestnetDsusdUsd),
            $of($crate::LedgerId::HubXrplXrp),
            $of($crate::LedgerId::HubXrplRlusdUsd),
        ])
    };
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

    const fn hubs_count_more(ledger: LedgerId) -> u64 {
        if ledger.is_hub() { 2 } else { 1 }
    }

    // `per_ledger!` lists the ledgers, and `get` and `with` name them, in
    // `ALL`'s order: each ledger has a value of its own.
    #[test]
    fn a_table_holds_each_ledgers_value() {
        const TABLE: PerLedger<u64> = crate::per_ledger!(hubs_count_more);
        for (ledger, value) in TABLE.iter() {
            assert_eq!(value, hubs_count_more(ledger), "{ledger}");
        }
        let changed = TABLE.with(LedgerId::StubUsd, 7);
        assert_eq!(changed.get(LedgerId::StubUsd), 7);
        assert_eq!(changed.get(LedgerId::HubXrplXrp), 2);
        assert_eq!(PerLedger::splat(3).get(LedgerId::XrplXrp), 3);
        assert_eq!(TABLE.map(|value| value * 10).get(LedgerId::HubXrplXrp), 20);
        for (index, ledger) in LedgerId::ALL.into_iter().enumerate() {
            let numbered = PerLedger::in_order(core::array::from_fn(|i| i));
            assert_eq!(numbered.get(ledger), index, "{ledger}");
            let marked = PerLedger::splat(0).with(ledger, 1);
            for (other, value) in marked.iter() {
                assert_eq!(value, usize::from(other == ledger), "{ledger}, {other}");
            }
        }
    }
}

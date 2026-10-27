//! The XRP Ledger's networks, as the node's accounts on them see them
//! (spec/decisions/mainnet-schemes.md): one account type, which takes
//! everything that tells one network from another from this table.
//!
//! Each network has its own asset kind, so an account's ID names its
//! network ([`Network::of`]), and the node derives an account's keys at a
//! path that names the network's ID and the account's number
//! (spec/areas/identity.md#seed-paths), so the two networks' accounts have
//! different keys and a Payment signed for one never applies on the other.
//! Mainnet's account is of the `live` realm, so it runs only in a node of
//! that realm, which stays refused (spec/decisions/real-money.md).

use dassie_ledger::{Name, Realm};
use dassie_settlement::{AssetId, AssetKind};

/// An XRP Ledger network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    /// Its name, as the owner reads it.
    pub name: &'static str,
    /// The network ID its servers report: a server that reports another,
    /// or none, stops the account. At most 1024, as a transaction names
    /// its network only above that, and the account names none. The keys
    /// of the node's accounts on it derive at a path that names it,
    /// `node/xrpl/<id>/account/<index>`, which is frozen
    /// (spec/overview.md#frozen-surfaces).
    pub id: u32,
    /// The kind of the node's accounts on it.
    pub asset_kind: AssetKind,
    /// The scheme that settles in its XRP: on an account it settles from,
    /// what other schemes spend of the XRP as gas is the owner's funds on
    /// its ledger (spec/decisions/external-assets.md).
    pub xrp_scheme: Name,
    /// The server an account connects to when its config names none.
    pub server: &'static str,
    /// Its faucet, if it has one: an account without funds asks it, and a
    /// config may name another only where there is one.
    pub faucet: Option<&'static str>,
    /// The realm its ledgers are of.
    pub realm: Realm,
    /// Why the account stops when its server reports another network, or
    /// none.
    pub wrong_network: &'static str,
}

/// The testnet's faucet.
pub(crate) const TESTNET_FAUCET: &str = "https://faucet.altnet.rippletest.net/accounts";

/// The XRP Ledger testnet. Its server is the project's proof server
/// (spec/roadmap.md#proof-servers).
pub const TESTNET: Network = Network {
    name: "testnet",
    id: 1,
    asset_kind: AssetKind::XrplTestnetAccount,
    xrp_scheme: dassie_ledger::name!(crate::scheme::SCHEME_ID),
    server: "wss://testnet.xrpl.dassieland.net",
    faucet: Some(TESTNET_FAUCET),
    realm: Realm::Test,
    wrong_network: "the server is not on the XRP Ledger testnet; remove the scheme and add it with a testnet server",
};

/// The XRP Ledger mainnet: real money. It has no faucet. Its server is the
/// project's proof server (spec/roadmap.md#proof-servers).
pub const MAINNET: Network = Network {
    name: "mainnet",
    id: 0,
    asset_kind: AssetKind::XrplAccount,
    xrp_scheme: dassie_ledger::name!(crate::scheme::MAINNET_SCHEME_ID),
    server: "wss://mainnet.xrpl.dassieland.net",
    faucet: None,
    realm: Realm::Live,
    wrong_network: "the server is not on the XRP Ledger mainnet; remove the scheme and add it with a mainnet server",
};

/// Every network, in a fixed order.
pub const NETWORKS: [&Network; 2] = [&TESTNET, &MAINNET];

impl Network {
    /// The network of the node's accounts of `kind`, if they are XRPL
    /// accounts.
    #[must_use]
    pub const fn of(kind: AssetKind) -> Option<&'static Self> {
        match kind {
            AssetKind::XrplTestnetAccount => Some(&TESTNET),
            AssetKind::XrplAccount => Some(&MAINNET),
            AssetKind::HubTestnetAccount | AssetKind::HubAccount => None,
        }
    }

    /// The node's first account on it, whose keys derive at
    /// `node/xrpl/<id>/account/0` (spec/areas/identity.md#seed-paths).
    #[must_use]
    pub const fn first(&self) -> AssetId {
        AssetId {
            kind: self.asset_kind,
            index: 0,
        }
    }

    /// Whether its chain may start again from a new genesis
    /// ([`resets`](crate::account::resets)): only the testnet's.
    #[must_use]
    pub const fn resets(&self) -> bool {
        crate::account::resets(self.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec/decisions/mainnet-schemes.md: each network's account is told
    // from the other's by its kind, its keys, its servers and its realm,
    // and only the testnet resets or has a faucet.
    #[test]
    fn no_two_networks_share_what_tells_them_apart() {
        for (index, network) in NETWORKS.iter().enumerate() {
            assert_eq!(Network::of(network.asset_kind), Some(*network));
            assert_eq!(network.first().kind, network.asset_kind);
            assert!(network.server.starts_with("wss://"));
            assert!(network.wrong_network.contains(network.name));
            assert!(network.id <= 1024, "the account signs no NetworkID");
            for other in NETWORKS.iter().skip(index + 1) {
                assert_ne!(network.id, other.id);
                assert_ne!(network.asset_kind, other.asset_kind);
                assert_ne!(network.xrp_scheme, other.xrp_scheme);
                assert_ne!(network.server, other.server);
                assert_ne!(network.realm, other.realm);
            }
        }
        assert!(TESTNET.resets() && TESTNET.faucet.is_some());
        assert!(!MAINNET.resets() && MAINNET.faucet.is_none());
        assert_eq!(Network::of(AssetKind::HubTestnetAccount), None);
        assert_eq!(Network::of(AssetKind::HubAccount), None);
    }

    // The network's XRP scheme books on its realm's ledger of XRP, whose
    // slug is the scheme's ID.
    #[test]
    fn the_xrp_ledger_is_of_the_networks_realm() {
        for (network, xrp) in [
            (&TESTNET, crate::scheme::TESTNET_XRP),
            (&MAINNET, crate::scheme::MAINNET_XRP),
        ] {
            assert_eq!(xrp.network, network);
            assert_eq!(xrp.scheme, network.xrp_scheme);
            assert_eq!(xrp.ledger.realm(), network.realm);
            assert_eq!(xrp.ledger.asset().code, "XRP");
            assert_eq!(xrp.ledger.as_str().split('+').next(), Some(xrp.scheme_id));
        }
    }
}

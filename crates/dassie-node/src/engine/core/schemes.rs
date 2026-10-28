//! The settlement schemes the node can run, the external assets they settle
//! through, and the assets' connections to their ledgers
//! (spec/areas/settlement.md#external-assets).
//!
//! An external asset with a ledger to follow does no IO: it hands the core
//! what to send, and the driver carries it over a WebSocket of its own, one
//! per asset however many schemes settle through it, bound to the asset's
//! *epoch*, so that a message from a connection the core has since replaced
//! is dropped. After the asset hears anything, every scheme on it is told
//! what it learnt. So the node starts without waiting for a ledger, and a
//! scheme that fails fails alone, where the original's start waited for
//! every ledger and a module's failure reached them all.

use dassie_identity::NodeKey;
use dassie_ledger::{AccountName, LedgerId, Name, Realm};
use dassie_settlement::{
    AssetEffect, AssetId, AssetKind, Detach, Effect, ExternalAsset, Holding, LedgerInput,
    LedgerStatus, SchemeError, SchemeInfo, SettlementScheme, Stub, Unproven,
};
use dassie_xrpl::{DsusdScheme, RlusdScheme, XrplAccount, XrplMainnetScheme, XrplScheme};

use super::events::SettlementEvent;
use super::ledgers::ActiveScheme;
use super::{Core, EngineError, Output};
use crate::store::Checkpoint;

/// Every scheme the node can run, in a fixed order.
pub const SCHEMES: &[SchemeInfo] = &[
    SchemeInfo {
        id: "stub",
        name: "Stub",
        ledger: LedgerId::StubUsd,
    },
    SchemeInfo {
        id: dassie_xrpl::scheme::SCHEME_ID,
        name: "XRP Ledger Testnet",
        ledger: LedgerId::XrplTestnetXrp,
    },
    SchemeInfo {
        id: dassie_xrpl::scheme::MAINNET_SCHEME_ID,
        name: "XRP Ledger",
        ledger: LedgerId::XrplXrp,
    },
    SchemeInfo {
        id: dassie_xrpl::rlusd::SCHEME_ID,
        name: "RLUSD on the XRP Ledger",
        ledger: LedgerId::XrplRlusdUsd,
    },
    SchemeInfo {
        id: dassie_xrpl::dsusd::SCHEME_ID,
        name: "DSUSD on the XRP Ledger Testnet",
        ledger: LedgerId::XrplTestnetDsusdUsd,
    },
    SchemeInfo {
        id: dassie_hub::member::SCHEME_ID,
        name: "XRP in a settlement hub on the XRP Ledger Testnet",
        ledger: LedgerId::HubXrplTestnetXrp,
    },
    SchemeInfo {
        id: dassie_hub::member::DSUSD_SCHEME_ID,
        name: "DSUSD in a settlement hub on the XRP Ledger Testnet",
        ledger: LedgerId::HubXrplTestnetDsusdUsd,
    },
    SchemeInfo {
        id: dassie_hub::member::MAINNET_SCHEME_ID,
        name: "XRP in a settlement hub on the XRP Ledger",
        ledger: LedgerId::HubXrplXrp,
    },
    SchemeInfo {
        id: dassie_hub::member::RLUSD_SCHEME_ID,
        name: "RLUSD in a settlement hub on the XRP Ledger",
        ledger: LedgerId::HubXrplRlusdUsd,
    },
];

/// The schemes over a settlement hub, one per network and asset
/// (spec/decisions/hub-scheme-names.md).
const HUB_SCHEMES: [&str; 4] = [
    dassie_hub::member::SCHEME_ID,
    dassie_hub::member::DSUSD_SCHEME_ID,
    dassie_hub::member::MAINNET_SCHEME_ID,
    dassie_hub::member::RLUSD_SCHEME_ID,
];

/// The default hub of each scheme of [`HUB_SCHEMES`], in its order: the
/// hex of the rules built in, or `None` while none is
/// (spec/designs/settlement-hubs.md#11-choosing-a-hub). Mainnet's are
/// placeholders until the project's validators found them
/// (spec/roadmap.md#mainnet-hubs): while one is `None`, its scheme is added
/// only with the rules its config names, and is not offered. A scheme
/// added without rules stores the default's ([`config_to_store`]), so a
/// default only ever names the hub a scheme joins when it is added
/// (spec/decisions/hub-named-when-added.md).
pub(super) type DefaultHubs = [Option<&'static str>; HUB_SCHEMES.len()];

/// The default hubs this build carries.
pub(super) const DEFAULT_HUBS: DefaultHubs = [
    dassie_hub::member::DEFAULT_TESTNET_HUB,
    dassie_hub::member::DEFAULT_TESTNET_DSUSD_HUB,
    dassie_hub::member::DEFAULT_MAINNET_HUB,
    dassie_hub::member::DEFAULT_MAINNET_RLUSD_HUB,
];

/// The default hub of scheme `id` among `hubs`, if it is a hub's: the hex
/// of the rules built in, or `None` while none is.
fn default_hub(id: &str, hubs: &DefaultHubs) -> Option<Option<&'static str>> {
    HUB_SCHEMES
        .iter()
        .zip(hubs)
        .find(|(scheme, _)| **scheme == id)
        .map(|(_, hub)| *hub)
}

/// The schemes the node no longer runs: test RLUSD, for which DSUSD stands
/// in on the testnet (spec/decisions/issued-assets.md). None is in
/// [`SCHEMES`], so none is offered, and none can be added. One stored by a
/// node that ran it does not start, and is left as it is: its books, its
/// checkpoint and its line on the ledger, with nothing written off. It
/// still names a scheme of the account it settled through, so what the
/// account kept for it loads, as for any scheme not on the account.
pub const DISABLED: &[&str] = &[dassie_xrpl::rlusd::TESTNET_SCHEME_ID];

/// Whether this build runs the stub scheme: only one with debug
/// assertions, as dev, the end-to-end tests, the tests and the simulations
/// are, never a release (spec/decisions/stub-in-debug-builds.md). A peer on
/// the stub mints what it says it settled, and the stub's dollars convert
/// at par into the dollar tokens', so a release node that ran it could be
/// drained of them.
pub const STUB_RUNS: bool = cfg!(debug_assertions);

/// Why this build does not run scheme `id`, if it does not: a disabled
/// scheme, such as test RLUSD, or the stub in a release build. One stored
/// is left as it is, as a disabled one: its books stay.
#[must_use]
pub fn not_run(id: &str) -> Option<&'static str> {
    not_run_in(id, STUB_RUNS)
}

/// [`not_run`], in a build that runs the stub or not.
fn not_run_in(id: &str, stub_runs: bool) -> Option<&'static str> {
    if DISABLED.contains(&id) {
        Some("the scheme is disabled (spec/decisions/issued-assets.md)")
    } else if id == "stub" && !stub_runs {
        Some(
            "the stub runs only in a build with debug assertions (spec/decisions/stub-in-debug-builds.md)",
        )
    } else {
        None
    }
}

/// Whether the scheme `info` is offered to the owner of a node of `realm`
/// to add: only one this build runs ([`not_run`]), of the node's realm, which is the only realm it runs
/// (spec/decisions/mainnet-schemes.md), and a settlement hub's only once
/// the node knows a hub of its asset to join without being told one, so the
/// wallet never offers what it cannot add with nothing to fill in. One
/// added with a config of its own is listed as any active scheme.
#[must_use]
pub fn offered(info: &SchemeInfo, realm: Realm) -> bool {
    offered_in(info, realm, &DEFAULT_HUBS)
}

/// [`offered`], with the default hubs `hubs` built in.
fn offered_in(info: &SchemeInfo, realm: Realm, hubs: &DefaultHubs) -> bool {
    not_run(info.id).is_none()
        && info.ledger.realm() == realm
        && default_hub(info.id, hubs).is_none_or(|hub| hub.is_some())
}

/// The external asset scheme `id` settles through, if any, and the config
/// the asset is made with: for the XRPL schemes, the server and the faucet
/// their config names, which are the account's. Those of a network settle
/// through the node's first XRPL account on it, so one address holds the
/// XRP of one and the gas of the others (spec/areas/xrpl.md#the-account):
/// the testnet's, or mainnet's for `xrpl-xrp` and `xrpl-rlusd`. Disabled test
/// RLUSD's is the testnet's. A hub's scheme settles through the node's
/// account in that hub, one per asset (`dassie_hub::member::account_id`).
fn asset_of(id: &str) -> Option<AssetId> {
    #[cfg(test)]
    if id == super::tests::WITNESS {
        return Some(dassie_xrpl::account::FIRST);
    }
    if let Some(account) = dassie_hub::member::account_id(id) {
        return Some(account);
    }
    if [
        dassie_xrpl::scheme::MAINNET_SCHEME_ID,
        dassie_xrpl::rlusd::SCHEME_ID,
    ]
    .contains(&id)
    {
        return Some(dassie_xrpl::network::MAINNET.first());
    }
    [
        dassie_xrpl::scheme::SCHEME_ID,
        dassie_xrpl::rlusd::TESTNET_SCHEME_ID,
        dassie_xrpl::dsusd::SCHEME_ID,
    ]
    .contains(&id)
    .then_some(dassie_xrpl::account::FIRST)
}

/// The config hub scheme `id` is stored with when the owner adds it with
/// `config`: with none, the rules of the default hub of its asset among
/// `hubs`, as `{"rules": "…"}`, so that the stored scheme names its hub for
/// good, whatever default a later release carries
/// (spec/decisions/hub-named-when-added.md); any other config, and that of
/// a scheme that is no hub's, as it is.
///
/// # Errors
///
/// [`EngineError::Invalid`] for a hub's scheme added without rules while
/// no default hub of its asset is built in.
pub(super) fn config_to_store(
    id: &str,
    config: String,
    hubs: &DefaultHubs,
) -> Result<String, EngineError> {
    match default_hub(id, hubs) {
        Some(hub) if config.trim().is_empty() => hub
            .map(|hex| serde_json::json!({ "rules": hex.trim() }).to_string())
            .ok_or_else(|| {
                EngineError::Invalid(
                    "no default hub of the scheme's asset is built in: \
                     name the hub's rules in its config"
                        .into(),
                )
            }),
        _ => Ok(config),
    }
}

/// The rules of the hub the config of a hub scheme names: the hex of their
/// encoding as `{"rules": "…"}`. Every stored hub scheme names its rules,
/// since one added without them stores the default hub's
/// ([`config_to_store`]); an empty config names no hub
/// (spec/decisions/hub-named-when-added.md).
fn hub_rules(config: &str) -> Result<dassie_hub::Rules, SchemeError> {
    let value: serde_json::Value =
        serde_json::from_str(config).map_err(|_| SchemeError::InvalidConfig)?;
    value
        .get("rules")
        .and_then(serde_json::Value::as_str)
        .and_then(|hex| decode_rules(hex).ok())
        .ok_or(SchemeError::InvalidConfig)
}

/// Refuses the state the books kept of hub account `asset` if it is of
/// another hub than the one `config` names, or names none: its number,
/// holds and operations are of the hub it was kept in, and the account
/// restores none of them into another (spec/decisions/hub-named-when-added.md).
/// The account would refuse it too; this says why. A config that names no
/// hub is the account's to refuse.
///
/// # Errors
///
/// [`EngineError::Invalid`] for a state of another hub, or of none.
fn check_hub_state(asset: AssetId, config: &str, state: &[u8]) -> Result<(), EngineError> {
    let hub_account = matches!(
        asset.kind(),
        AssetKind::HubXrplTestnetXrp
            | AssetKind::HubXrplTestnetDsusd
            | AssetKind::HubXrplXrp
            | AssetKind::HubXrplRlusd
    );
    if !hub_account || state.is_empty() {
        return Ok(());
    }
    let Ok(rules) = hub_rules(config) else {
        return Ok(());
    };
    let named = rules.id();
    match dassie_hub::member::hub_of_state(state) {
        Some(kept) if kept == named => Ok(()),
        Some(kept) => Err(EngineError::Invalid(format!(
            "the node's account {asset} is in hub {}, not in hub {} that the scheme's config \
             names: a scheme stays in the hub it was added with",
            kept.0, named.0
        ))),
        None => Err(EngineError::Invalid(format!(
            "the state kept of the node's account {asset} names no hub this version reads"
        ))),
    }
}

fn decode_rules(hex: &str) -> Result<dassie_hub::Rules, SchemeError> {
    let bytes = dassie_xrpl::from_hex(hex).ok_or(SchemeError::InvalidConfig)?;
    let mut reader = dassie_oer::Reader::new(&bytes);
    let rules = dassie_hub::Rules::decode(&mut reader).map_err(|_| SchemeError::InvalidConfig)?;
    reader.finish().map_err(|_| SchemeError::InvalidConfig)?;
    Ok(rules)
}

/// The node's hub account `id` in the hub of `config`: its key derived at
/// `node/hub/member/<hub ID>/account`, paying out at the hub's end to the
/// node's own first XRPL account on the hub's network, which holds the
/// owner book of the hub's asset: the testnet's for a testnet hub,
/// mainnet's for a mainnet hub. The account is the one of a hub scheme
/// (`dassie_hub::member::account_id`), and the hub's network and asset
/// must name that scheme (spec/decisions/hub-scheme-names.md): rules of a
/// DSUSD hub given to `hub-xrpl-testnet-xrp`, or the other way round, or
/// of a testnet hub given to a mainnet hub's scheme, are an invalid
/// config, so a hub's terms are always on the network of its scheme's
/// realm. A hub whose legs between members do not fit in the connector's
/// message window is not joined: it is too far apart for a peering over it
/// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
fn create_hub_account(
    id: AssetId,
    config: &str,
    state: &[u8],
    (key, window_ms): (&NodeKey, u64),
) -> Result<Box<dyn ExternalAsset>, SchemeError> {
    let scheme = HUB_SCHEMES
        .into_iter()
        .find(|scheme| dassie_hub::member::account_id(scheme) == Some(id))
        .ok_or(SchemeError::UnknownScheme)?;
    let rules = hub_rules(config)?;
    if dassie_hub::member::scheme_of(&rules.terms).map(|(named, _)| named) != Some(scheme) {
        return Err(SchemeError::InvalidConfig);
    }
    let timers = &rules.terms.timers;
    if timers.margin.saturating_add(timers.allowance) > window_ms {
        return Err(SchemeError::InvalidConfig);
    }
    let network = hub_network(&rules.terms).ok_or(SchemeError::InvalidConfig)?;
    let ledger = SCHEMES
        .iter()
        .find(|info| info.id == scheme)
        .map(|info| info.ledger);
    assert_eq!(
        ledger.map(LedgerId::realm),
        Some(network.realm),
        "a hub scheme's ledger is of its network's realm"
    );
    let member = dassie_hub::member::member_key(key, &rules.id());
    let disbursal = dassie_hub::operation::Destination {
        account: own_xrpl_account(key, network).0,
        tag: None,
    };
    let scheme = Name::new(scheme).map_err(|_| SchemeError::InvalidConfig)?;
    Ok(Box::new(dassie_hub::member::HubAccount::new(
        id,
        rules,
        std::sync::Arc::new(member),
        disbursal,
        scheme,
        state,
    )?))
}

/// The XRP Ledger network of a hub of `terms`, if it is one a node's
/// accounts are on.
pub(super) fn hub_network(
    terms: &dassie_hub::rules::Terms,
) -> Option<&'static dassie_xrpl::network::Network> {
    dassie_xrpl::network::NETWORKS
        .into_iter()
        .find(|network| network.id == terms.network)
}

/// The node's first XRPL account on `network`: where its accounts in that
/// network's hubs pay out, and where it withdraws to, holding the owner
/// book of each such hub's asset.
pub(super) fn own_xrpl_account(
    key: &NodeKey,
    network: &dassie_xrpl::network::Network,
) -> dassie_xrpl::AccountId {
    let entropy = key.xrpl_account_entropy(network.id, 0);
    dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account()
}

/// Whether `asset` is an XRPL account, whose config is its own, set by
/// the owner apart from any scheme's (spec/areas/settlement.md#external-assets).
const fn configured_alone(asset: AssetId) -> bool {
    matches!(
        asset.kind(),
        AssetKind::XrplTestnetAccount | AssetKind::XrplAccount
    )
}

/// The owner's config of `asset`, checked as the asset reads it, and
/// written one way: empty for its network's defaults, otherwise its JSON
/// with its keys in order and no spaces, so that two that say the same are
/// the same. Only the first XRPL account of each network is built.
///
/// # Errors
///
/// [`EngineError::Invalid`] for another asset, or a config it refuses.
fn checked_asset_config(asset: AssetId, config: &str) -> Result<String, EngineError> {
    let network = dassie_xrpl::network::Network::of(asset.kind())
        .filter(|_| asset.index() == Some(0))
        .ok_or_else(|| EngineError::Invalid("no external asset the owner configures".into()))?;
    let invalid = |error: SchemeError| EngineError::Invalid(error.to_string());
    let config = config.trim();
    if config.is_empty() {
        return Ok(String::new());
    }
    dassie_xrpl::account::Config::parse_on(config, network).map_err(invalid)?;
    let value: serde_json::Value =
        serde_json::from_str(config).map_err(|_| invalid(SchemeError::InvalidConfig))?;
    Ok(value.to_string())
}

/// The schemes that may settle through `asset`, disabled ones included.
fn schemes_on(asset: AssetId) -> Vec<Name> {
    let ids = SCHEMES
        .iter()
        .map(|info| info.id)
        .chain(DISABLED.iter().copied());
    #[cfg(test)]
    let ids = ids.chain([super::tests::WITNESS]);
    ids.filter(|id| asset_of(id) == Some(asset))
        .filter_map(|id| Name::new(id).ok())
        .collect()
}

/// Makes the external asset `id` with its config, and the state the books
/// kept for it. An XRPL account derives its keys at the path its ID names,
/// its network's ID and its number, which is frozen
/// (spec/areas/identity.md#seed-paths). Accounts beyond the first are not
/// built.
///
/// # Errors
///
/// If the asset is unknown, or refuses its config or its state.
fn create_asset(
    id: AssetId,
    config: &str,
    state: &[u8],
    (key, window_ms): (&NodeKey, u64),
) -> Result<Box<dyn ExternalAsset>, SchemeError> {
    match (id.kind(), id.index()) {
        (AssetKind::XrplTestnetAccount | AssetKind::XrplAccount, Some(index @ 0)) => {
            let network =
                dassie_xrpl::network::Network::of(id.kind()).ok_or(SchemeError::UnknownScheme)?;
            let entropy = key.xrpl_account_entropy(network.id, index);
            Ok(Box::new(XrplAccount::new(
                id,
                &schemes_on(id),
                config,
                entropy.as_bytes(),
                state,
            )?))
        }
        (
            AssetKind::HubXrplTestnetXrp
            | AssetKind::HubXrplTestnetDsusd
            | AssetKind::HubXrplXrp
            | AssetKind::HubXrplRlusd,
            None,
        ) => create_hub_account(id, config, state, (key, window_ms)),
        _ => Err(SchemeError::UnknownScheme),
    }
}

/// Has `asset`, if it is an XRPL account, check validations by the fake
/// ledger's keyed hash under `secret`, as in a simulation.
#[cfg(feature = "testing")]
pub(super) fn check_validations_with(asset: &mut dyn ExternalAsset, secret: [u8; 32]) {
    let any: &mut dyn core::any::Any = asset;
    if let Some(account) = any.downcast_mut::<XrplAccount>() {
        account.check_validations_with(dassie_xrpl::fake::KeyedSignatures::new(secret));
    }
}

/// Makes the hub scheme `id` on its account in the hub, `asset`.
///
/// # Errors
///
/// If `asset` is no hub account, or one made for another scheme, or the
/// scheme refuses its state.
fn create_hub_scheme(
    id: &str,
    asset: Option<&dyn ExternalAsset>,
    max_packet: u64,
    balance: Option<u64>,
    state: &[u8],
) -> Result<Box<dyn SettlementScheme>, SchemeError> {
    let any: Option<&dyn core::any::Any> = asset.map(|asset| asset as _);
    let account = any
        .and_then(|asset| asset.downcast_ref::<dassie_hub::member::HubAccount>())
        .ok_or(SchemeError::InvalidConfig)?;
    let scheme = dassie_hub::member::HubScheme::new(account, max_packet, balance, state)?;
    // The account was made for this scheme, from rules whose asset names
    // it.
    if scheme.id().as_str() != id {
        return Err(SchemeError::InvalidConfig);
    }
    Ok(Box::new(scheme))
}

/// Makes the scheme `id` with its config, what the books last kept for it,
/// and the asset it settles through; a dollar token's peg guard runs on
/// `peg`. The original's RPC dropped every key of the config, and it
/// never passed on what it stored.
///
/// # Errors
///
/// If the scheme is unknown, or refuses its config or its state.
fn create(
    id: &str,
    config: &str,
    checkpoint: Option<&Checkpoint>,
    asset: Option<&dyn ExternalAsset>,
    max_packet: u64,
    peg: dassie_xrpl::peg::Timings,
) -> Result<Box<dyn SettlementScheme>, SchemeError> {
    let balance = checkpoint.map(|c| c.balance);
    let account = || {
        let any: Option<&dyn core::any::Any> = asset.map(|asset| asset as _);
        any.and_then(|asset| asset.downcast_ref::<XrplAccount>())
            .ok_or(SchemeError::InvalidConfig)
    };
    let state = checkpoint.map_or(&[][..], |c| c.state.as_slice());
    // Every XRPL scheme is an owner book, whose config says nothing
    // (spec/areas/xrpl.md#owner-books).
    let owner_book = || dassie_xrpl::scheme::SchemeConfig::parse(config).map(|_| ());
    match id {
        "stub" => Ok(Box::new(Stub::new(config, balance, state)?)),
        dassie_xrpl::scheme::SCHEME_ID => {
            owner_book()?;
            Ok(Box::new(XrplScheme::new(account()?, balance, state)?))
        }
        dassie_xrpl::scheme::MAINNET_SCHEME_ID => {
            owner_book()?;
            Ok(Box::new(XrplMainnetScheme::new(
                account()?,
                balance,
                state,
            )?))
        }
        dassie_xrpl::rlusd::SCHEME_ID => {
            owner_book()?;
            let mut scheme = RlusdScheme::new(account()?, balance, state)?;
            scheme.set_peg_timings(peg);
            Ok(Box::new(scheme))
        }
        dassie_xrpl::dsusd::SCHEME_ID => {
            owner_book()?;
            let mut scheme = DsusdScheme::new(account()?, balance, state)?;
            scheme.set_peg_timings(peg);
            Ok(Box::new(scheme))
        }
        id if HUB_SCHEMES.contains(&id) => create_hub_scheme(id, asset, max_packet, balance, state),
        #[cfg(test)]
        super::tests::WITNESS => Ok(Box::new(super::tests::Witness)),
        #[cfg(test)]
        super::tests::DSUSD_HUB => Ok(Box::new(super::tests::HubStandIn::DSUSD)),
        #[cfg(test)]
        super::tests::XRP_HUB => Ok(Box::new(super::tests::HubStandIn::XRP)),
        #[cfg(test)]
        id @ (super::tests::STUB_TWO | super::tests::STUB_THREE) => {
            super::tests::StubTwo::boxed(id, config, balance, state)
        }
        _ => Err(SchemeError::UnknownScheme),
    }
}

/// The warnings for what an asset could not prove that grew from `seen` to
/// `now`: one per kind, with its total. Counts only grow while the asset
/// lives.
fn unproven_warnings(
    asset: AssetId,
    schemes: &[Name],
    seen: Unproven,
    now: Unproven,
) -> impl Iterator<Item = SettlementEvent> {
    assert!(
        now.history_gaps >= seen.history_gaps,
        "history gaps only grow"
    );
    assert!(
        now.settlements >= seen.settlements,
        "unproven settlements only grow"
    );
    let gap = (now.history_gaps > seen.history_gaps).then(|| SettlementEvent::HistoryGap {
        asset,
        schemes: schemes.to_vec(),
        total: now.history_gaps,
    });
    let unproven =
        (now.settlements > seen.settlements).then(|| SettlementEvent::SettlementUnproven {
            asset,
            schemes: schemes.to_vec(),
            total: now.settlements,
        });
    gap.into_iter().chain(unproven)
}

/// A scheme just made, and the asset it settles through.
pub(super) type MadeScheme = (Box<dyn SettlementScheme>, Option<AssetId>);

/// An active external asset.
pub(super) struct ActiveAsset {
    pub asset: Box<dyn ExternalAsset>,
    /// The config it was made with ([`asset_config`]): every scheme on it
    /// names the same.
    pub config: String,
    /// Its connection to its ledger, if it follows one; 0 before the
    /// first, and once closed.
    pub epoch: u64,
    /// The state version last written with a checkpoint.
    pub version: u64,
    /// What it could not prove, as last warned of.
    pub unproven: Unproven,
}

/// An external asset as the owner sees it: what it holds, as its ledger
/// says, never as the books add it up (spec/decisions/external-assets.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetView {
    /// Its ID.
    pub id: String,
    /// Where the owner sends money to fund it, if anywhere.
    pub address: Option<String>,
    /// What it holds, as its ledger last said.
    pub holdings: Vec<Holding>,
    /// How it stands with its ledger.
    pub status: LedgerStatus,
    /// What it waits for, if something keeps it from going on, such as a
    /// validator list.
    pub waiting: Option<&'static str>,
    /// The schemes that settle through it.
    pub schemes: Vec<String>,
    /// Whether the node can ask a faucet for test money on it.
    pub faucet: bool,
    /// What it could not prove of its ledger since the node started.
    pub unproven: Unproven,
    /// The config it runs with: for an XRPL account, the owner's, empty
    /// for its network's defaults.
    pub config: String,
    /// Whether the owner set another config since it was made, which it
    /// takes at the node's next start.
    pub config_waits: bool,
}

/// What comes for an asset from its connection or its faucet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerEvent {
    /// The connection is up.
    Connected,
    /// A message came.
    Message(Vec<u8>),
    /// The connection went down; the driver tries again.
    Disconnected,
    /// The faucet answered, or did not.
    Funded(bool),
    /// A fetch of `url` answered with `body`, or failed: `None`.
    Fetched {
        /// What was fetched.
        url: String,
        /// What came.
        body: Option<Vec<u8>>,
    },
    /// A hub's validator at `endpoint` answered.
    From {
        /// Its place in the hub's rules.
        endpoint: usize,
        /// What it said.
        message: Vec<u8>,
    },
}

/// How a scheme stands, as the owner sees it: its asset's status until the
/// asset is synced, then its own.
pub(super) fn merged_status(asset: Option<LedgerStatus>, scheme: LedgerStatus) -> LedgerStatus {
    match asset {
        Some(LedgerStatus::Synced) | None => scheme,
        Some(status) => status,
    }
}

impl Core {
    /// Makes scheme `id` from its config and checkpoint, with the asset it
    /// settles through, made first if it is not active. Returns the scheme
    /// and its asset.
    ///
    /// # Errors
    ///
    /// If either refuses its config or state, or the asset is active with
    /// another config.
    pub(super) fn make_scheme(
        &mut self,
        id: Name,
        config: &str,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<MadeScheme, EngineError> {
        let invalid = |error: SchemeError| EngineError::Invalid(error.to_string());
        let Some(asset) = asset_of(id.as_str()) else {
            let scheme = create(
                id.as_str(),
                config,
                checkpoint,
                None,
                self.settle.max_packet,
                self.settle.peg,
            )
            .map_err(invalid)?;
            return Ok((scheme, None));
        };
        let alone = configured_alone(asset);
        let asset_config = self.config_for(asset, config)?;
        let new = !self.assets.contains_key(&asset);
        if new {
            let state = self
                .asset_checkpoints
                .get(&asset)
                .cloned()
                .unwrap_or_default();
            check_hub_state(asset, &asset_config, &state)?;
            let window_ms = self.connector.config().message_window_ms;
            #[allow(unused_mut)]
            let mut made = create_asset(asset, &asset_config, &state, (&self.key, window_ms))
                .map_err(invalid)?;
            #[cfg(feature = "testing")]
            if let Some(secret) = self.settle.validation_secret {
                check_validations_with(made.as_mut(), secret);
            }
            self.assets.insert(
                asset,
                ActiveAsset {
                    asset: made,
                    config: asset_config.clone(),
                    epoch: 0,
                    version: 0,
                    unproven: Unproven::default(),
                },
            );
        }
        let made = match self.assets.get(&asset) {
            // An XRPL account runs with the config it was made with until
            // the next start, whatever the owner set since.
            Some(active) if alone || active.config == asset_config => create(
                id.as_str(),
                config,
                checkpoint,
                Some(active.asset.as_ref()),
                self.settle.max_packet,
                self.settle.peg,
            )
            .map_err(invalid),
            _ => Err(EngineError::Precondition(
                "the account is in use with another config".into(),
            )),
        };
        match made {
            Ok(scheme) => Ok((scheme, Some(asset))),
            Err(error) => {
                self.release_asset(asset);
                Err(error)
            }
        }
    }

    /// The config `asset` is made with, for a scheme of config `config`: an
    /// XRPL account's is its own, the owner's; a hub account's is its
    /// scheme's, the hub's rules.
    ///
    /// # Errors
    ///
    /// [`EngineError::Invalid`] for an XRPL scheme whose config names the
    /// account's server, faucet or publishers, which are the account's.
    fn config_for(&self, asset: AssetId, config: &str) -> Result<String, EngineError> {
        if !configured_alone(asset) {
            return Ok(config.to_string());
        }
        if dassie_xrpl::scheme::SchemeConfig::parse(config).is_err()
            && dassie_xrpl::scheme::SchemeConfig::names_the_account(config)
        {
            return Err(EngineError::Invalid(
                "the account's server, faucet and publishers are its own config: \
                 set them with SetAssetConfig"
                    .into(),
            ));
        }
        Ok(self.asset_configs.get(&asset).cloned().unwrap_or_default())
    }

    /// Sets the owner's config of external asset `id`: for an XRPL account,
    /// its server, its faucet and its validator list publishers
    /// (spec/areas/settlement.md#external-assets). Empty for its network's
    /// defaults. Kept with the books; an asset made after takes it, and one
    /// active now, with schemes settling through it, at the node's next
    /// start. Returns whether it is in effect now.
    ///
    /// # Errors
    ///
    /// [`EngineError::Invalid`] for an unknown asset, one the owner does not
    /// configure, or a config it refuses; [`EngineError::Precondition`] for
    /// an account of the other realm.
    pub fn set_asset_config(&mut self, id: &str, config: &str) -> Result<bool, EngineError> {
        let asset = AssetId::parse(id)
            .ok_or_else(|| EngineError::Invalid("unknown external asset".into()))?;
        let config = checked_asset_config(asset, config)?;
        let realm = dassie_xrpl::network::Network::of(asset.kind()).map(|network| network.realm);
        if realm != Some(super::ledger_realm(self.realm)) {
            return Err(EngineError::Precondition(
                "the account is of another realm than the node's".into(),
            ));
        }
        let stored = self.asset_configs.get(&asset).cloned().unwrap_or_default();
        if stored != config {
            self.batch
                .asset_configs
                .push((asset.to_string(), config.clone()));
            if config.is_empty() {
                self.asset_configs.remove(&asset);
            } else {
                self.asset_configs.insert(asset, config.clone());
            }
        }
        Ok(self
            .assets
            .get(&asset)
            .is_none_or(|active| active.config == config))
    }

    /// Lets go of `asset` once no scheme settles through it: its connection
    /// is closed, and its checkpoint stays.
    pub(super) fn release_asset(&mut self, asset: AssetId) {
        if self
            .schemes
            .values()
            .any(|active| active.asset == Some(asset))
        {
            return;
        }
        if let Some(active) = self.assets.remove(&asset) {
            self.disconnect_asset(asset, active.epoch);
        }
    }

    /// The schemes on `asset` the books have not stopped.
    fn live_schemes_on(&self, asset: AssetId) -> Vec<Name> {
        self.schemes
            .iter()
            .filter(|(_, active)| active.asset == Some(asset) && active.failed.is_none())
            .map(|(name, _)| *name)
            .collect()
    }

    /// What an asset's connection or faucet says, for the connection of
    /// `epoch`: from an older one, it is dropped.
    pub fn ledger_event(&mut self, asset: &str, epoch: u64, event: &LedgerEvent) {
        let Some(id) = AssetId::parse(asset) else {
            return;
        };
        let now = self.now_ms;
        let Some(active) = self.assets.get_mut(&id) else {
            return;
        };
        if active.epoch != epoch || epoch == 0 {
            return;
        }
        let input = match event {
            LedgerEvent::Connected => LedgerInput::Connected,
            LedgerEvent::Message(bytes) => LedgerInput::Message(bytes),
            LedgerEvent::Disconnected => LedgerInput::Disconnected,
            LedgerEvent::Funded(ok) => LedgerInput::Funded(*ok),
            LedgerEvent::Fetched { url, body } => LedgerInput::Fetched {
                url,
                body: body.as_deref(),
            },
            LedgerEvent::From { endpoint, message } => LedgerInput::From {
                endpoint: *endpoint,
                message,
            },
        };
        active.asset.on_ledger(now, input);
        self.asset_heard(id);
    }

    /// After `asset` heard from its ledger, its timers or its faucet: every
    /// scheme on it is told what it learnt, each booked all or none, and
    /// what the asset asks is carried out, unless no scheme on it took what
    /// it was told ([`carry_out`](Self::carry_out)). A scheme that has just
    /// synced for the first time can be peered on from now
    /// (spec/decisions/synced-before-peering.md).
    fn asset_heard(&mut self, asset: AssetId) {
        for scheme in self.live_schemes_on(asset) {
            self.tell_scheme(scheme, asset);
        }
        if let Some(active) = self.assets.get_mut(&asset) {
            active.asset.told();
        }
        self.warn_unproven(asset);
        self.carry_out(asset);
        self.update_peer_schemes();
    }

    /// Warns of each kind of what `asset` could not prove that grew since
    /// the last warning, with the schemes on it
    /// (spec/areas/xrpl.md#what-it-could-not-prove).
    fn warn_unproven(&mut self, asset: AssetId) {
        let schemes = self.live_schemes_on(asset);
        let Some(active) = self.assets.get_mut(&asset) else {
            return;
        };
        let now = active.asset.unproven();
        let seen = std::mem::replace(&mut active.unproven, now);
        for event in unproven_warnings(asset, &schemes, seen, now) {
            self.changes.ledger = true;
            self.report(event);
        }
    }

    /// Tells `scheme` what `asset` learnt, and books what it says. A
    /// refusal has stopped the scheme, and told the owner.
    pub(super) fn tell_scheme(&mut self, scheme: Name, asset: AssetId) {
        self.tell_owner_funds(asset);
        let now = self.now_ms;
        let (Some(active), Some(on)) = (self.schemes.get_mut(&scheme), self.assets.get_mut(&asset))
        else {
            return;
        };
        if active.failed.is_some() {
            return;
        }
        let mut effects = Vec::new();
        active.scheme.on_asset(now, on.asset.as_mut(), &mut effects);
        let ledger = active.scheme.ledger();
        let _stopped = self.apply_effects(scheme, ledger, &effects);
    }

    /// Tells `asset` what the owner may spend on the ledger of each scheme
    /// on it, by the books, before a call that may sign: what one scheme
    /// spends as gas of a holding another settles in is the owner's there
    /// (spec/decisions/external-assets.md).
    pub(super) fn tell_owner_funds(&mut self, asset: AssetId) {
        let funds: Vec<(Name, u64)> = self
            .live_schemes_on(asset)
            .into_iter()
            .filter_map(|scheme| {
                let ledger = self.schemes.get(&scheme)?.scheme.ledger();
                Some((scheme, self.owner_available(ledger)))
            })
            .collect();
        if let Some(active) = self.assets.get_mut(&asset) {
            for (scheme, available) in funds {
                active.asset.owner_funds(scheme, available);
            }
        }
    }

    /// What the owner may spend on `ledger` now: the owner's group's posted
    /// credits less its debits, posted and pending.
    pub fn owner_available(&self, ledger: LedgerId) -> u64 {
        self.account(ledger, AccountName::EquityOwner)
            .ok()
            .and_then(|owner| self.ledger.group_balance(owner))
            .map_or(0, |balance| {
                let available = balance
                    .credits_posted
                    .saturating_sub(balance.debits_posted)
                    .saturating_sub(balance.debits_pending);
                u64::try_from(available).unwrap_or(u64::MAX)
            })
    }

    /// Carries out what `asset` asks of its ledger, in order: a message
    /// after a reconnection goes on the new connection. All or none with
    /// what its schemes booked: once the books refused what the last scheme
    /// running on it was told, its connection is closed, and nothing the
    /// asset asked meanwhile goes out, no message, connection or request to
    /// a faucet (spec/areas/settlement.md#the-scheme-interface).
    pub(super) fn carry_out(&mut self, asset: AssetId) {
        self.close_if_stopped(asset);
        let mut effects = Vec::new();
        let Some(active) = self.assets.get_mut(&asset) else {
            return;
        };
        active.asset.take_effects(&mut effects);
        if active.epoch == 0 {
            return;
        }
        for effect in effects {
            let Some(epoch) = self.assets.get(&asset).map(|active| active.epoch) else {
                return;
            };
            match effect {
                AssetEffect::Send { message } => self.outputs_peer.push(Output::LedgerSend {
                    asset: asset.to_string(),
                    epoch,
                    message,
                }),
                AssetEffect::SendTo { endpoint, message } => {
                    self.send_to_validator(asset, epoch, endpoint, message);
                }
                AssetEffect::Reconnect => self.connect_asset(asset),
                AssetEffect::Fund { url, body } => self.outputs_peer.push(Output::Fund {
                    asset: asset.to_string(),
                    epoch,
                    url,
                    body,
                }),
                AssetEffect::Fetch { url, max_bytes } => self.outputs_peer.push(Output::Fetch {
                    asset: asset.to_string(),
                    epoch,
                    url,
                    max_bytes,
                }),
            }
        }
    }

    /// Books a scheme's effects, where no one waits for the outcome, and
    /// carries out what its asset was asked meanwhile: a refusal has
    /// stopped the scheme, and told the owner
    /// ([`apply_effects`](Self::apply_effects)).
    pub(super) fn scheme_effects(&mut self, scheme: Name, ledger: LedgerId, effects: &[Effect]) {
        let asset = self.schemes.get(&scheme).and_then(|active| active.asset);
        let booked = self.apply_effects(scheme, ledger, effects);
        if let Some(asset) = asset {
            self.carry_out(asset);
        }
        if booked.is_ok() {
            self.update_peer_schemes();
        }
    }

    /// Stops a scheme whose effects the books refused, which is a bug: it
    /// is fed nothing more, from its asset, peers or the owner, so the
    /// books do not drift further from it, and the owner is told. Its
    /// checkpoint is not written again, so a restart reads what the ledger
    /// said from where the books are; nor is its asset's, once no scheme on
    /// it runs, so the two stay as they were written together.
    pub(super) fn stop_scheme(&mut self, scheme: Name, error: EngineError) {
        self.report(SettlementEvent::SchemeStopped {
            scheme,
            error: error.clone(),
        });
        let mut asset = None;
        if let Some(active) = self.schemes.get_mut(&scheme) {
            active.failed = Some(error.to_string());
            asset = active.asset;
        }
        if let Some(asset) = asset {
            if let Some(active) = self.assets.get_mut(&asset) {
                active.asset.detach(scheme, Detach::Stopped);
            }
            self.close_if_stopped(asset);
        }
        self.update_peer_schemes();
    }

    /// Closes the connection of an asset that stopped by itself, or on
    /// which no scheme runs any more (spec/areas/settlement.md#the-scheme-interface):
    /// the driver would otherwise make it again for ever, to a ledger no
    /// one reads. The asset is told, so it asks nothing of it. A scheme
    /// activated on it, or a restart, connects anew.
    fn close_if_stopped(&mut self, asset: AssetId) {
        let idle = self.live_schemes_on(asset).is_empty();
        let now = self.now_ms;
        let Some(active) = self.assets.get_mut(&asset) else {
            return;
        };
        let stopped = idle || matches!(active.asset.status(), LedgerStatus::Failed(_));
        if stopped && active.epoch != 0 {
            let epoch = std::mem::replace(&mut active.epoch, 0);
            active.asset.on_ledger(now, LedgerInput::Disconnected);
            self.disconnect_asset(asset, epoch);
        }
    }

    /// Connects `asset` for a scheme just activated on it: once its
    /// connection was closed, or before it was made; and anew once it has
    /// heard from its ledger, so that it reads again from the earliest
    /// place of its schemes, the new one's included, and tells them all it
    /// has synced. Not while it waits to connect, which it does for them
    /// all, nor once it stopped by itself.
    pub(super) fn connect_for_scheme(&mut self, asset: AssetId) {
        let Some(active) = self.assets.get(&asset) else {
            return;
        };
        let connect = match active.asset.status() {
            LedgerStatus::Failed(_) => false,
            LedgerStatus::Connecting => active.epoch == 0,
            _ => true,
        };
        if connect {
            self.connect_asset(asset);
        }
    }

    /// Runs the timers of the assets that are due.
    pub(super) fn scheme_timers(&mut self) {
        let now = self.now_ms;
        let due: Vec<AssetId> = self
            .assets
            .iter()
            .filter(|(id, active)| {
                active.asset.deadline().is_some_and(|at| at <= now)
                    && !self.live_schemes_on(**id).is_empty()
            })
            .map(|(id, _)| *id)
            .collect();
        for asset in due {
            if let Some(active) = self.assets.get_mut(&asset) {
                active.asset.on_timer(now);
                self.asset_heard(asset);
            }
        }
    }

    /// The earliest of the assets' timers.
    pub(super) fn schemes_deadline(&self) -> Option<u64> {
        self.assets
            .iter()
            .filter(|(id, _)| !self.live_schemes_on(**id).is_empty())
            .filter_map(|(_, active)| active.asset.deadline())
            .min()
    }

    /// Sends a hub account's message to the validator at `endpoint`,
    /// pinned to its node key (spec/areas/hubs.md#the-members-side).
    fn send_to_validator(&mut self, asset: AssetId, epoch: u64, endpoint: usize, message: Vec<u8>) {
        let Some(active) = self.assets.get(&asset) else {
            return;
        };
        let any: &dyn core::any::Any = active.asset.as_ref();
        let Some(account) = any.downcast_ref::<dassie_hub::member::HubAccount>() else {
            return;
        };
        let (Some(node), Some(address)) = (
            account.validator_node(endpoint),
            active.asset.endpoints().get(endpoint).cloned(),
        ) else {
            return;
        };
        self.outputs_peer.push(Output::HubSend {
            asset: asset.to_string(),
            epoch,
            hub: account.hub(),
            endpoint,
            node: Box::new(node),
            address,
            message,
        });
    }

    /// Opens an asset's connection, anew if it had one: the asset is told
    /// the old one is gone.
    pub(super) fn connect_asset(&mut self, asset: AssetId) {
        self.next_epoch = self.next_epoch.wrapping_add(1);
        let epoch = self.next_epoch;
        let now = self.now_ms;
        let Some(active) = self.assets.get_mut(&asset) else {
            return;
        };
        let old = std::mem::replace(&mut active.epoch, epoch);
        let Some(url) = active.asset.connection().map(str::to_string) else {
            return;
        };
        if old != 0 {
            active.asset.on_ledger(now, LedgerInput::Disconnected);
            self.outputs_peer.push(Output::LedgerClose {
                asset: asset.to_string(),
                epoch: old,
            });
        }
        self.outputs_peer.push(Output::LedgerConnect {
            asset: asset.to_string(),
            epoch,
            url,
        });
    }

    /// Closes an asset's connection, if it has one.
    fn disconnect_asset(&mut self, asset: AssetId, epoch: u64) {
        if epoch != 0 {
            self.outputs_peer.push(Output::LedgerClose {
                asset: asset.to_string(),
                epoch,
            });
        }
    }

    /// Asks the faucet of a scheme's asset for test money.
    ///
    /// # Errors
    ///
    /// [`SchemeError::Unsupported`] for a scheme without an asset or an
    /// asset without a faucet; [`SchemeError::Unavailable`] while the
    /// faucet was asked too recently, or cannot be answered.
    pub(super) fn asset_faucet(&mut self, scheme: Name) -> Result<(), SchemeError> {
        let asset = self
            .schemes
            .get(&scheme)
            .and_then(|active| active.asset)
            .ok_or(SchemeError::Unsupported)?;
        let active = self
            .assets
            .get_mut(&asset)
            .ok_or(SchemeError::Unsupported)?;
        let asked = active.asset.ask_faucet();
        self.carry_out(asset);
        asked
    }

    /// The external assets, as their ledgers say.
    pub fn asset_views(&self) -> Vec<AssetView> {
        self.assets
            .iter()
            .map(|(id, active)| AssetView {
                id: id.to_string(),
                address: active.asset.address(),
                holdings: active.asset.holdings(),
                status: active.asset.status(),
                waiting: active.asset.waiting(),
                schemes: self
                    .schemes
                    .iter()
                    .filter(|(_, scheme)| scheme.asset == Some(*id))
                    .map(|(name, _)| name.as_str().to_string())
                    .collect(),
                faucet: active.asset.has_faucet(),
                unproven: active.asset.unproven(),
                config: active.config.clone(),
                config_waits: configured_alone(*id)
                    && self.asset_configs.get(id).map_or("", String::as_str) != active.config,
            })
            .collect()
    }
}

/// Checkpoints: what the books keep for each scheme and each asset, in the
/// batch that explains it (spec/areas/settlement.md#checkpoints-and-activation).
impl Core {
    /// Keeps a scheme's balance and state with this batch, and its asset's
    /// state: what the scheme signed is kept with its pending transfer.
    pub(super) fn checkpoint(&mut self, scheme: Name) {
        let Some(active) = self.schemes.get_mut(&scheme) else {
            return;
        };
        if active.failed.is_some() {
            return;
        }
        active.version = active.scheme.state_version();
        let checkpoint = Checkpoint {
            balance: active.scheme.balance(),
            state: active.scheme.state(),
        };
        let asset = active.asset;
        self.batch.checkpoints.push((scheme, checkpoint.clone()));
        self.checkpoints.insert(scheme, checkpoint);
        if let Some(asset) = asset {
            self.checkpoint_asset(asset);
        }
    }

    /// Keeps a scheme's balance and state with this batch, if either
    /// changed since last written, and its asset's: a ledger closing
    /// changes neither.
    pub(super) fn checkpoint_if_changed(&mut self, scheme: Name) {
        let Some(active) = self.schemes.get(&scheme) else {
            return;
        };
        let balance = active.scheme.balance();
        let asset = active.asset;
        let changed = active.scheme.state_version() != active.version
            || self
                .checkpoints
                .get(&scheme)
                .is_none_or(|checkpoint| checkpoint.balance != balance);
        if changed {
            self.checkpoint(scheme);
        } else if let Some(asset) = asset {
            self.checkpoint_asset_if_changed(asset);
        }
    }

    /// Keeps an asset's state with this batch, while a scheme on it runs:
    /// once none does, what it learnt since is not what their checkpoints
    /// explain.
    fn checkpoint_asset(&mut self, asset: AssetId) {
        if self.live_schemes_on(asset).is_empty() {
            return;
        }
        let Some(active) = self.assets.get_mut(&asset) else {
            return;
        };
        active.version = active.asset.state_version();
        let state = active.asset.state();
        self.batch
            .asset_checkpoints
            .push((asset.to_string(), state.clone()));
        self.asset_checkpoints.insert(asset, state);
    }

    fn checkpoint_asset_if_changed(&mut self, asset: AssetId) {
        let changed = self.assets.get(&asset).is_some_and(|active| {
            active.asset.state_version() != active.version
                || !self.asset_checkpoints.contains_key(&asset)
        });
        if changed {
            self.checkpoint_asset(asset);
        }
    }

    /// Keeps what changed in any scheme or asset, such as a peer's address
    /// learnt when peering, with this batch.
    pub(super) fn checkpoint_schemes(&mut self) {
        let schemes: Vec<Name> = self.schemes.keys().copied().collect();
        for scheme in schemes {
            self.checkpoint_if_changed(scheme);
        }
        let assets: Vec<AssetId> = self.assets.keys().copied().collect();
        for asset in assets {
            self.checkpoint_asset_if_changed(asset);
        }
    }

    /// `scheme`, if the books have not stopped it, with the asset it
    /// settles through, for a call that may sign, submit or look up.
    pub(super) fn scheme_and_asset(
        &mut self,
        scheme: Name,
    ) -> Option<(&mut ActiveScheme, Option<&mut dyn ExternalAsset>)> {
        let active = self.schemes.get_mut(&scheme)?;
        if active.failed.is_some() {
            return None;
        }
        let asset = match active.asset {
            Some(id) => Some(self.assets.get_mut(&id)?.asset.as_mut()),
            None => None,
        };
        Some((active, asset))
    }

    /// How scheme `active` stands, as the owner sees it.
    pub(super) fn scheme_status(&self, active: &ActiveScheme) -> LedgerStatus {
        let asset = active
            .asset
            .and_then(|id| self.assets.get(&id))
            .map(|asset| asset.asset.status());
        merged_status(asset, active.scheme.status())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // spec/decisions/mainnet-schemes.md: each XRPL scheme settles through
    // the first account on its own network, whose keys are at that
    // network's seed path, mainnet's the original's; only `xrpl-xrp` and
    // `xrpl-rlusd`, the live realm's XRPL schemes, settle through
    // mainnet's.
    #[test]
    fn each_xrpl_scheme_settles_through_its_networks_account() {
        use dassie_xrpl::network::{MAINNET, Network, TESTNET};
        let key = NodeKey::from_bytes(&[7; 32]);
        for network in [&TESTNET, &MAINNET] {
            let asset = create_asset(network.first(), "", &[], (&key, 0)).unwrap();
            let any: &dyn core::any::Any = asset.as_ref();
            let account = any.downcast_ref::<XrplAccount>().unwrap();
            assert_eq!(account.network(), network);
            let entropy = key.xrpl_account_entropy(network.id, 0);
            let wallet = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes());
            assert_eq!(account.account(), wallet.account());
        }
        for info in SCHEMES {
            let network = asset_of(info.id).and_then(|asset| Network::of(asset.kind()));
            // A hub scheme's account is named by the scheme
            // (spec/decisions/hub-scheme-names.md).
            assert_eq!(
                info.id.starts_with("hub-"),
                asset_of(info.id).is_some_and(|asset| asset.to_string() == info.id),
                "{}",
                info.id
            );
            match network {
                Some(network) => assert_eq!(network.realm, info.ledger.realm(), "{}", info.id),
                None => assert!(!info.id.starts_with("xrpl"), "{}", info.id),
            }
            assert_eq!(
                asset_of(info.id) == Some(MAINNET.first()),
                info.ledger.realm() == Realm::Live && info.id.starts_with("xrpl"),
                "{}",
                info.id
            );
        }
        let further = AssetId::numbered(AssetKind::XrplAccount, 1);
        assert!(matches!(
            create_asset(further, "", &[], (&key, 0)),
            Err(SchemeError::UnknownScheme)
        ));
    }

    // spec/areas/xrpl.md#what-it-could-not-prove: each history gap and each
    // settlement proven neither applied nor absent is warned of once, with
    // the asset's schemes and the total; nothing new, nothing warned.
    #[test]
    fn what_an_asset_could_not_prove_is_warned_of_once_as_it_grows() {
        let asset = dassie_xrpl::account::FIRST;
        let schemes = [Name::new("xrpl-testnet-xrp").unwrap()];
        let counts = |history_gaps, settlements| Unproven {
            history_gaps,
            settlements,
        };
        let warned = |seen, now| unproven_warnings(asset, &schemes, seen, now).collect::<Vec<_>>();
        assert!(warned(counts(0, 0), counts(0, 0)).is_empty());
        assert!(warned(counts(2, 1), counts(2, 1)).is_empty());
        assert_eq!(
            warned(counts(0, 0), counts(1, 0)),
            [SettlementEvent::HistoryGap {
                asset,
                schemes: schemes.to_vec(),
                total: 1,
            }]
        );
        assert_eq!(
            warned(counts(1, 0), counts(3, 2)),
            [
                SettlementEvent::HistoryGap {
                    asset,
                    schemes: schemes.to_vec(),
                    total: 3,
                },
                SettlementEvent::SettlementUnproven {
                    asset,
                    schemes: schemes.to_vec(),
                    total: 2,
                },
            ]
        );
        let event = super::super::Event::from(SettlementEvent::SettlementUnproven {
            asset,
            schemes: schemes.to_vec(),
            total: 1,
        });
        assert_eq!(event.severity(), super::super::Severity::Warn);
    }

    /// The config of a DSUSD hub: the rules of the tests' XRP hub, holding
    /// DSUSD, as its validators accept them again.
    fn dsusd_hub_config() -> String {
        use dassie_hub::ledger::TESTNET_NETWORK;
        use dassie_hub::rules::Asset;

        let dsusd = Asset::Token {
            currency: dassie_xrpl::dsusd::CURRENCY,
            issuer: dassie_xrpl::dsusd::ISSUER,
        };
        hub_config_on(TESTNET_NETWORK, dsusd)
    }

    /// The config of a hub of `asset` on the XRP Ledger network `network`:
    /// the rules of the tests' XRP hub, on that network and holding that
    /// asset, as its validators accept them again with their keys for it.
    fn hub_config_on(network: u32, asset: dassie_hub::rules::Asset) -> String {
        use dassie_hub::escrow::ValidatorKeys;
        use dassie_hub::rules::{Asset, LEAST_TOKEN_GAS_DROPS};
        use dassie_oer::Encode;

        let xrp = super::super::tests::hub_config(600, 300);
        let mut rules = hub_rules(&xrp).unwrap();
        rules.terms.network = network;
        rules.terms.asset = asset;
        if asset != Asset::Xrp {
            rules.terms.fees.gas = LEAST_TOKEN_GAS_DROPS;
            rules.terms.fees.ledger = 0;
        }
        let terms = rules.terms.clone();
        for (index, acceptance) in rules.validators.iter_mut().enumerate() {
            let node = NodeKey::from_bytes(&[u8::try_from(index + 1).unwrap(); 32]);
            let endpoint = acceptance.endpoint.clone();
            *acceptance = ValidatorKeys::derive(&node, network, &terms.nonce)
                .unwrap()
                .accept(&node, &terms, endpoint)
                .unwrap();
        }
        rules.check().unwrap();
        let hex: String = rules
            .to_vec()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        serde_json::json!({ "rules": hex }).to_string()
    }

    // spec/decisions/hub-scheme-names.md: each hub scheme settles through
    // an account of its own, which the scheme's ID names, so a node in two
    // hubs has two accounts of distinct IDs, and it joins only a hub whose
    // asset names it: rules of a DSUSD hub given to the XRP hub's scheme, or
    // the other way round, are an invalid config. The DSUSD hub's scheme
    // books on its own ledger.
    #[test]
    fn each_hub_scheme_joins_only_a_hub_of_its_asset() {
        use dassie_hub::member::{DSUSD_SCHEME_ID, SCHEME_ID};

        let key = NodeKey::from_bytes(&[7; 32]);
        let window = u64::MAX;
        let xrp = super::super::tests::hub_config(600, 300);
        let dsusd = dsusd_hub_config();
        let xrp_account = asset_of(SCHEME_ID).unwrap();
        let dsusd_account = asset_of(DSUSD_SCHEME_ID).unwrap();
        assert_eq!(xrp_account, dassie_hub::member::ACCOUNT_ID);
        assert_ne!(xrp_account, dsusd_account, "an account per hub");
        assert_eq!(
            schemes_on(dsusd_account),
            [Name::new(DSUSD_SCHEME_ID).unwrap()]
        );
        assert!(create_hub_account(xrp_account, &xrp, &[], (&key, window)).is_ok());
        let made = create_hub_account(dsusd_account, &dsusd, &[], (&key, window)).unwrap();
        for (account, config) in [(xrp_account, &dsusd), (dsusd_account, &xrp)] {
            assert!(matches!(
                create_hub_account(account, config, &[], (&key, window)),
                Err(SchemeError::InvalidConfig)
            ));
        }
        // An empty config names no hub: a scheme added without rules is
        // stored with the default hub's (spec/decisions/hub-named-when-added.md).
        assert!(matches!(
            create_hub_account(dsusd_account, "", &[], (&key, window)),
            Err(SchemeError::InvalidConfig)
        ));
        // An XRPL account is no hub's.
        assert!(matches!(
            create_hub_account(dassie_xrpl::account::FIRST, &xrp, &[], (&key, window)),
            Err(SchemeError::UnknownScheme)
        ));
        let scheme = create(
            DSUSD_SCHEME_ID,
            &dsusd,
            None,
            Some(made.as_ref()),
            1_000_000,
            dassie_xrpl::peg::Timings::DEFAULT,
        )
        .unwrap();
        assert_eq!(scheme.ledger(), LedgerId::HubXrplTestnetDsusdUsd);
        assert_eq!(scheme.asset(), Some(dsusd_account));
        assert!(matches!(
            create(
                SCHEME_ID,
                &dsusd,
                None,
                Some(made.as_ref()),
                1_000_000,
                dassie_xrpl::peg::Timings::DEFAULT,
            ),
            Err(SchemeError::InvalidConfig)
        ));
        // Offered only with a default hub of its asset built in.
        let info = SCHEMES
            .iter()
            .find(|info| info.id == DSUSD_SCHEME_ID)
            .unwrap();
        assert_eq!(info.ledger, LedgerId::HubXrplTestnetDsusdUsd);
        assert_eq!(
            offered(info, Realm::Test),
            dassie_hub::member::DEFAULT_TESTNET_DSUSD_HUB.is_some()
        );
        assert!(!offered(info, Realm::Live));
    }

    // spec/decisions/hub-scheme-names.md: on mainnet, `hub-xrpl-xrp` joins
    // only a mainnet hub of XRP and `hub-xrpl-rlusd` only one of RLUSD, each
    // through an account of its own that its ID names, and books on
    // its live ledger. A testnet hub's rules given to a mainnet scheme, or a
    // mainnet hub's to a testnet scheme, are an invalid config, so a hub's
    // terms are on the network of its scheme's realm. While no mainnet hub
    // is built in, the schemes run only with rules of their own, and are
    // not offered.
    #[test]
    fn each_mainnet_hub_scheme_joins_only_a_mainnet_hub_of_its_asset() {
        use dassie_hub::member::{MAINNET_SCHEME_ID, RLUSD_SCHEME_ID, SCHEME_ID};
        use dassie_hub::rules::Asset;
        use dassie_xrpl::network::MAINNET;

        let key = NodeKey::from_bytes(&[7; 32]);
        let window = u64::MAX;
        let rlusd = Asset::Token {
            currency: dassie_xrpl::rlusd::CURRENCY,
            issuer: dassie_xrpl::rlusd::ISSUER,
        };
        let xrp = hub_config_on(MAINNET.id, Asset::Xrp);
        let rlusd = hub_config_on(MAINNET.id, rlusd);
        let testnet = super::super::tests::hub_config(600, 300);
        let schemes = [
            (MAINNET_SCHEME_ID, &xrp, &rlusd, LedgerId::HubXrplXrp),
            (RLUSD_SCHEME_ID, &rlusd, &xrp, LedgerId::HubXrplRlusdUsd),
        ];
        let mut accounts = Vec::new();
        for (id, config, other, ledger) in schemes {
            let account = asset_of(id).unwrap();
            assert_eq!(account.to_string(), id);
            assert_eq!(account.index(), None, "{id}");
            assert_eq!(schemes_on(account), [Name::new(id).unwrap()]);
            accounts.push(account);
            let made = create_hub_account(account, config, &[], (&key, window)).unwrap();
            let built_in = default_hub(id, &DEFAULT_HUBS).flatten().is_some();
            for wrong in [other.as_str(), testnet.as_str(), ""] {
                assert!(
                    matches!(
                        create_hub_account(account, wrong, &[], (&key, window)),
                        Err(SchemeError::InvalidConfig)
                    ),
                    "{id}: {wrong}"
                );
            }
            let scheme = create(
                id,
                config,
                None,
                Some(made.as_ref()),
                1_000_000,
                dassie_xrpl::peg::Timings::DEFAULT,
            )
            .unwrap();
            assert_eq!(scheme.ledger(), ledger);
            assert_eq!(ledger.realm(), Realm::Live);
            assert_eq!(scheme.asset(), Some(account));
            let info = SCHEMES.iter().find(|info| info.id == id).unwrap();
            assert_eq!(info.ledger, ledger);
            assert_eq!(offered(info, Realm::Live), built_in);
            assert!(!offered(info, Realm::Test));
        }
        assert_ne!(accounts[0], accounts[1], "an account per hub");
        let testnet_account = asset_of(SCHEME_ID).unwrap();
        assert!(!accounts.contains(&testnet_account));
        assert!(matches!(
            create_hub_account(testnet_account, &xrp, &[], (&key, window)),
            Err(SchemeError::InvalidConfig)
        ));
    }

    // spec/designs/settlement-hubs.md#11-choosing-a-hub: once a default
    // mainnet hub is built in, a node of the live realm is offered its
    // scheme, and the scheme added with an empty config is stored with the
    // default hub's rules, and joins it; while none is, neither. A
    // test-realm node is offered neither either way.
    #[test]
    fn a_built_in_mainnet_hub_is_offered_and_joined_with_an_empty_config() {
        use dassie_hub::member::{MAINNET_SCHEME_ID, RLUSD_SCHEME_ID};
        use dassie_hub::rules::Asset;
        use dassie_xrpl::network::MAINNET;

        let rules_of = |config: String| -> &'static str {
            let value: serde_json::Value = serde_json::from_str(&config).unwrap();
            value["rules"].as_str().unwrap().to_owned().leak()
        };
        let rlusd = Asset::Token {
            currency: dassie_xrpl::rlusd::CURRENCY,
            issuer: dassie_xrpl::rlusd::ISSUER,
        };
        let xrp = rules_of(hub_config_on(MAINNET.id, Asset::Xrp));
        let rlusd = rules_of(hub_config_on(MAINNET.id, rlusd));
        let built_in: DefaultHubs = [None, None, Some(xrp), Some(rlusd)];
        let none: DefaultHubs = [None; HUB_SCHEMES.len()];
        let key = NodeKey::from_bytes(&[7; 32]);
        let window = u64::MAX;
        for (id, hex) in [(MAINNET_SCHEME_ID, xrp), (RLUSD_SCHEME_ID, rlusd)] {
            let info = SCHEMES.iter().find(|info| info.id == id).unwrap();
            assert_eq!(default_hub(id, &built_in), Some(Some(hex)));
            assert!(offered_in(info, Realm::Live, &built_in), "{id}");
            assert!(!offered_in(info, Realm::Test, &built_in), "{id}");
            assert!(!offered_in(info, Realm::Live, &none), "{id}");
            let account = asset_of(id).unwrap();
            let stored = config_to_store(id, String::new(), &built_in).unwrap();
            assert_eq!(stored, serde_json::json!({ "rules": hex }).to_string());
            let made = create_hub_account(account, &stored, &[], (&key, window)).unwrap();
            let any: &dyn core::any::Any = made.as_ref();
            let member = any
                .downcast_ref::<dassie_hub::member::HubAccount>()
                .unwrap();
            assert_eq!(member.hub(), decode_rules(hex).unwrap().id(), "{id}");
            assert!(matches!(
                config_to_store(id, " ".into(), &none),
                Err(EngineError::Invalid(_))
            ));
            // A config of its own is stored as it is.
            let own = hub_config_on(MAINNET.id, Asset::Xrp);
            assert_eq!(config_to_store(id, own.clone(), &built_in).unwrap(), own);
        }
        // A default of one asset is no default of the other.
        let swapped: DefaultHubs = [None, None, Some(rlusd), Some(xrp)];
        let account = asset_of(MAINNET_SCHEME_ID).unwrap();
        let stored = config_to_store(MAINNET_SCHEME_ID, String::new(), &swapped).unwrap();
        assert!(matches!(
            create_hub_account(account, &stored, &[], (&key, window)),
            Err(SchemeError::InvalidConfig)
        ));
        // Not a hub's scheme: its config is stored as it is.
        assert_eq!(
            config_to_store(
                dassie_xrpl::scheme::MAINNET_SCHEME_ID,
                String::new(),
                &built_in
            )
            .unwrap(),
            ""
        );
        // Not a hub's scheme: no default to look up.
        assert_eq!(
            default_hub(dassie_xrpl::scheme::MAINNET_SCHEME_ID, &built_in),
            None
        );
    }

    // A node offers only its realm's schemes.
    #[test]
    fn a_node_offers_only_its_realms_schemes() {
        for realm in [Realm::Test, Realm::Live] {
            for info in SCHEMES {
                if offered(info, realm) {
                    assert_eq!(info.ledger.realm(), realm, "{}", info.id);
                }
            }
        }
        let xrpl = |id: &str| SCHEMES.iter().find(|info| info.id == id).unwrap();
        let mainnet = xrpl(dassie_xrpl::scheme::MAINNET_SCHEME_ID);
        let testnet = xrpl(dassie_xrpl::scheme::SCHEME_ID);
        assert!(offered(mainnet, Realm::Live) && !offered(mainnet, Realm::Test));
        assert!(offered(testnet, Realm::Test) && !offered(testnet, Realm::Live));
    }

    // spec/decisions/stub-in-debug-builds.md: a build with debug
    // assertions, as every test is, runs the stub; a release build neither
    // offers nor runs it, and runs every other scheme as before.
    #[test]
    fn only_a_build_with_debug_assertions_runs_the_stub() {
        // Tests are built with debug assertions.
        assert_eq!(not_run("stub"), None);
        assert!(not_run_in("stub", false).is_some_and(|why| why.contains("debug assertions")));
        assert_eq!(not_run_in("stub", true), None);
        for info in SCHEMES.iter().filter(|info| info.id != "stub") {
            assert_eq!(not_run_in(info.id, false), None, "{}", info.id);
        }
        let disabled = dassie_xrpl::rlusd::TESTNET_SCHEME_ID;
        assert!(not_run_in(disabled, true).is_some_and(|why| why.contains("disabled")));
    }

    // spec/decisions/issued-assets.md: test RLUSD is neither offered nor
    // made, in either realm; it still names a scheme of the testnet's
    // account, so what the account kept for it loads.
    #[test]
    fn test_rlusd_is_disabled() {
        let id = dassie_xrpl::rlusd::TESTNET_SCHEME_ID;
        assert!(DISABLED.contains(&id));
        for disabled in DISABLED {
            assert!(SCHEMES.iter().all(|info| info.id != *disabled));
        }
        let key = NodeKey::from_bytes(&[7; 32]);
        let asset = create_asset(dassie_xrpl::account::FIRST, "", &[], (&key, 0)).unwrap();
        assert!(matches!(
            create(
                id,
                "",
                None,
                Some(asset.as_ref()),
                0,
                dassie_xrpl::peg::Timings::DEFAULT
            ),
            Err(SchemeError::UnknownScheme)
        ));
        assert_eq!(asset_of(id), Some(dassie_xrpl::account::FIRST));
        assert!(
            schemes_on(dassie_xrpl::account::FIRST)
                .iter()
                .any(|scheme| scheme.as_str() == id)
        );
    }
}

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
//! every ledger and a module's failure reached them all (ACC-26).

use dassie_identity::NodeKey;
use dassie_ledger::{AccountName, LedgerId, Name, Realm};
use dassie_settlement::{
    AssetEffect, AssetId, AssetKind, Detach, Effect, ExternalAsset, Holding, LedgerInput,
    LedgerStatus, SchemeError, SchemeInfo, SettlementScheme, Stub,
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
        ledger: LedgerId::HubXrpTestnetXrp,
    },
];

/// The schemes the node no longer runs: test RLUSD, for which DSUSD stands
/// in on the testnet (spec/decisions/issued-assets.md). None is in
/// [`SCHEMES`], so none is offered, and none can be added. One stored by a
/// node that ran it does not start, and is left as it is: its books, its
/// checkpoint and its line on the ledger, with nothing written off. It
/// still names a scheme of the account it settled through, so what the
/// account kept for it loads, as for any scheme not on the account.
pub const DISABLED: &[&str] = &[dassie_xrpl::rlusd::TESTNET_SCHEME_ID];

/// Whether the scheme `info` is offered to the owner of a node of `realm`
/// to add: only one of the node's realm, which is the only realm it runs
/// (spec/decisions/mainnet-schemes.md), and a settlement hub's only once
/// the node knows a hub to join without being told one, so the wallet never
/// offers what it cannot add with nothing to fill in. One added with a
/// config of its own is listed as any active scheme.
#[must_use]
pub fn offered(info: &SchemeInfo, realm: Realm) -> bool {
    info.ledger.realm() == realm
        && (info.id != dassie_hub::member::SCHEME_ID
            || dassie_hub::member::DEFAULT_TESTNET_HUB.is_some())
}

/// The external asset scheme `id` settles through, if any, and the config
/// the asset is made with: for the XRPL schemes, the server and the faucet
/// their config names, which are the account's. Those of a network settle
/// through the node's first XRPL account on it, so one address holds the
/// XRP of one and the gas of the others (spec/areas/xrpl.md#the-account):
/// the testnet's, or mainnet's for `xrpl` and `xrpl-rlusd`. Disabled test
/// RLUSD's is the testnet's.
fn asset_of(id: &str) -> Option<AssetId> {
    #[cfg(test)]
    if id == super::tests::WITNESS {
        return Some(dassie_xrpl::account::FIRST);
    }
    if id == dassie_hub::member::SCHEME_ID {
        return Some(dassie_hub::member::FIRST);
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

/// The rules of the hub a hub scheme's config names: the hex of their
/// encoding as `{"rules": "…"}`, or nothing for the default hub of the
/// testnet (spec/designs/settlement-hubs.md#11-choosing-a-hub).
fn hub_rules(config: &str) -> Result<dassie_hub::Rules, SchemeError> {
    let hex = if config.trim().is_empty() {
        dassie_hub::member::DEFAULT_TESTNET_HUB.ok_or(SchemeError::InvalidConfig)?
    } else {
        let value: serde_json::Value =
            serde_json::from_str(config).map_err(|_| SchemeError::InvalidConfig)?;
        return value
            .get("rules")
            .and_then(serde_json::Value::as_str)
            .and_then(|hex| decode_rules(hex).ok())
            .ok_or(SchemeError::InvalidConfig);
    };
    decode_rules(hex)
}

fn decode_rules(hex: &str) -> Result<dassie_hub::Rules, SchemeError> {
    let bytes = dassie_xrpl::from_hex(hex).ok_or(SchemeError::InvalidConfig)?;
    let mut reader = dassie_oer::Reader::new(&bytes);
    let rules = dassie_hub::Rules::decode(&mut reader).map_err(|_| SchemeError::InvalidConfig)?;
    reader.finish().map_err(|_| SchemeError::InvalidConfig)?;
    Ok(rules)
}

/// The node's hub account in the hub of `config`: its key derived at
/// `node/hub/<hub ID>/account`, paying out at the hub's end to the node's
/// own XRPL testnet account. A hub whose legs between members do not fit
/// in the connector's message window is not joined: it is too far apart
/// for a peering over it (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
fn create_hub_account(
    id: AssetId,
    config: &str,
    state: &[u8],
    (key, window_ms): (&NodeKey, u64),
) -> Result<Box<dyn ExternalAsset>, SchemeError> {
    let rules = hub_rules(config)?;
    let timers = &rules.terms.timers;
    if timers.margin.saturating_add(timers.allowance) > window_ms {
        return Err(SchemeError::InvalidConfig);
    }
    let member =
        dassie_hub::escrow::member_key(key, &rules.id()).ok_or(SchemeError::InvalidConfig)?;
    let disbursal = dassie_hub::operation::Destination {
        account: own_xrpl_account(key)?.0,
        tag: None,
    };
    let scheme =
        Name::new(dassie_hub::member::SCHEME_ID).map_err(|_| SchemeError::InvalidConfig)?;
    Ok(Box::new(dassie_hub::member::HubAccount::new(
        id,
        rules,
        std::sync::Arc::new(member),
        disbursal,
        scheme,
        state,
    )?))
}

/// The node's first XRPL testnet account: where its hub account pays out,
/// and where it withdraws to.
pub(super) fn own_xrpl_account(key: &NodeKey) -> Result<dassie_xrpl::AccountId, SchemeError> {
    let seed = dassie_xrpl::network::TESTNET.seed;
    let entropy = key
        .settlement_entropy(seed.module, seed.path)
        .map_err(|_| SchemeError::InvalidConfig)?;
    Ok(dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account())
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
/// kept for it. The node's first XRPL account on a network derives its keys
/// at its network's seed path, in `dassie_xrpl::network`, which is frozen
/// (spec/areas/identity.md#derivation-chain). Further accounts are not
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
    match (id.kind, id.index) {
        (AssetKind::XrplTestnetAccount | AssetKind::XrplAccount, 0) => {
            let network =
                dassie_xrpl::network::Network::of(id.kind).ok_or(SchemeError::UnknownScheme)?;
            let seed = network.seed;
            let entropy = key
                .settlement_entropy(seed.module, seed.path)
                .map_err(|_| SchemeError::InvalidConfig)?;
            Ok(Box::new(XrplAccount::new(
                id,
                &schemes_on(id),
                config,
                entropy.as_bytes(),
                state,
            )?))
        }
        (AssetKind::HubTestnetAccount, 0) => {
            create_hub_account(id, config, state, (key, window_ms))
        }
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

/// Makes the scheme `id` with its config, what the books last kept for it,
/// and the asset it settles through; a dollar token's peg guard runs on
/// `peg`. The original's RPC dropped every key of the config (RT-21), and it
/// never passed on what it stored (ACC-12).
///
/// # Errors
///
/// If the scheme is unknown, or refuses its config or its state.
fn create(
    id: &str,
    config: &str,
    checkpoint: Option<&Checkpoint>,
    key: &NodeKey,
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
    let node = || Name::new(key.node_id().as_str()).map_err(|_| SchemeError::InvalidData);
    let state = checkpoint.map_or(&[][..], |c| c.state.as_slice());
    match id {
        "stub" => Ok(Box::new(Stub::new(config, balance, state)?)),
        dassie_xrpl::scheme::SCHEME_ID => Ok(Box::new(XrplScheme::new(
            account()?,
            node()?,
            balance,
            state,
        )?)),
        dassie_xrpl::scheme::MAINNET_SCHEME_ID => Ok(Box::new(XrplMainnetScheme::new(
            account()?,
            node()?,
            balance,
            state,
        )?)),
        dassie_xrpl::rlusd::SCHEME_ID => {
            let mut scheme = RlusdScheme::new(account()?, node()?, balance, state)?;
            scheme.set_peg_timings(peg);
            Ok(Box::new(scheme))
        }
        dassie_xrpl::dsusd::SCHEME_ID => {
            let mut scheme = DsusdScheme::new(account()?, node()?, balance, state)?;
            scheme.set_peg_timings(peg);
            Ok(Box::new(scheme))
        }
        dassie_hub::member::SCHEME_ID => {
            let any: Option<&dyn core::any::Any> = asset.map(|asset| asset as _);
            let account = any
                .and_then(|asset| asset.downcast_ref::<dassie_hub::member::HubAccount>())
                .ok_or(SchemeError::InvalidConfig)?;
            Ok(Box::new(dassie_hub::member::HubScheme::new(
                account, max_packet, balance, state,
            )?))
        }
        #[cfg(test)]
        super::tests::WITNESS => Ok(Box::new(super::tests::Witness)),
        _ => Err(SchemeError::UnknownScheme),
    }
}

/// A scheme just made, and the asset it settles through.
pub(super) type MadeScheme = (Box<dyn SettlementScheme>, Option<AssetId>);

/// An active external asset.
pub(super) struct ActiveAsset {
    pub asset: Box<dyn ExternalAsset>,
    /// The config it was made with: every scheme on it names the same.
    pub config: String,
    /// Its connection to its ledger, if it follows one; 0 before the
    /// first, and once closed.
    pub epoch: u64,
    /// The state version last written with a checkpoint.
    pub version: u64,
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
                &self.key,
                None,
                self.settle.max_packet,
                self.settle.peg,
            )
            .map_err(invalid)?;
            return Ok((scheme, None));
        };
        let new = !self.assets.contains_key(&asset);
        if new {
            let state = self
                .asset_checkpoints
                .get(&asset)
                .cloned()
                .unwrap_or_default();
            let window_ms = self.connector.config().message_window_ms;
            #[allow(unused_mut)]
            let mut made =
                create_asset(asset, config, &state, (&self.key, window_ms)).map_err(invalid)?;
            #[cfg(feature = "testing")]
            if let Some(secret) = self.settle.validation_secret {
                check_validations_with(made.as_mut(), secret);
            }
            self.assets.insert(
                asset,
                ActiveAsset {
                    asset: made,
                    config: config.to_string(),
                    epoch: 0,
                    version: 0,
                },
            );
        }
        let made = match self.assets.get(&asset) {
            Some(active) if active.config == config => create(
                id.as_str(),
                config,
                checkpoint,
                &self.key,
                Some(active.asset.as_ref()),
                self.settle.max_packet,
                self.settle.peg,
            )
            .map_err(invalid),
            _ => Err(EngineError::Precondition(
                "the account is in use with another server or faucet".into(),
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
        self.carry_out(asset);
        self.update_peer_schemes();
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
    // network's seed path, mainnet's the original's; only `xrpl` and
    // `xrpl-rlusd`, the live realm's, settle through mainnet's.
    #[test]
    fn each_xrpl_scheme_settles_through_its_networks_account() {
        use dassie_xrpl::network::{MAINNET, Network, TESTNET};
        let key = NodeKey::from_bytes(&[7; 32]);
        for network in [&TESTNET, &MAINNET] {
            let asset = create_asset(network.first(), "", &[], (&key, 0)).unwrap();
            let any: &dyn core::any::Any = asset.as_ref();
            let account = any.downcast_ref::<XrplAccount>().unwrap();
            assert_eq!(account.network(), network);
            let entropy = key
                .settlement_entropy(network.seed.module, network.seed.path)
                .unwrap();
            let wallet = dassie_xrpl::Wallet::from_entropy(entropy.as_bytes());
            assert_eq!(account.account(), wallet.account());
        }
        for info in SCHEMES {
            let network = asset_of(info.id).and_then(|asset| Network::of(asset.kind));
            match network {
                Some(network) => assert_eq!(network.realm, info.ledger.realm(), "{}", info.id),
                None => assert!(!info.id.starts_with("xrpl"), "{}", info.id),
            }
            assert_eq!(
                asset_of(info.id) == Some(MAINNET.first()),
                info.ledger.realm() == Realm::Live,
                "{}",
                info.id
            );
        }
        let further = AssetId {
            kind: AssetKind::XrplAccount,
            index: 1,
        };
        assert!(matches!(
            create_asset(further, "", &[], (&key, 0)),
            Err(SchemeError::UnknownScheme)
        ));
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
                &key,
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

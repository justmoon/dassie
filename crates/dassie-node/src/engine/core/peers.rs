//! The peer protocol in the core: what peering needs from the schemes and
//! the ledger, peers as connector endpoints, routes, and packets over the
//! peer protocol (spec/areas/network.md#peering).

use std::collections::{BTreeMap, BTreeSet};

use dassie_connector::{Connector, EndpointId};
use dassie_identity::{NodeId, PublicKey};
use dassie_ilp::ErrorCode;
use dassie_ledger::{AccountName, AccountPath, Ledger, LedgerId, Name};
use dassie_peer::wire::{RefusalReason, SchemeId, TerminationReason};
use dassie_peer::{
    Agreed, BanError, Declined, Event, InboundId, NameEvent, Output as PeerOutput, PacketAnswer,
    PeeringHooks, Reply,
};

use super::downlinks::Downlinks;
use super::events::{PeerEvent, SettlementEvent};
use super::ledgers::ActiveScheme;
use super::ledgers::OWN_ACCOUNTS;
use super::schemes::{ActiveAsset, merged_status};
use super::{Core, EngineError, Output};
use crate::store::{PeerSettings, Store};
use dassie_settlement::{AssetId, LedgerStatus};

/// What peering needs, over the core's schemes, ledger and connector.
pub(super) struct Hooks<'a> {
    pub schemes: &'a mut BTreeMap<Name, ActiveScheme>,
    pub ledger: &'a mut Ledger,
    pub connector: &'a Connector,
    pub endpoints: &'a BTreeMap<NodeId, u64>,
    /// The external assets the schemes settle through, for their status.
    pub assets: &'a BTreeMap<AssetId, ActiveAsset>,
    /// The database, for what stays of closed settlement channels.
    pub store: &'a Store,
    /// The accounts opened for downlinks, and the downlinks the node keeps
    /// room for (spec/decisions/account-budgets.md).
    pub downlinks: &'a Downlinks,
    /// The owner's peer settings: whether the node offers swaps, and
    /// swaps (spec/decisions/peer-swaps.md).
    pub settings: PeerSettings,
    /// The step's time, which a peer's snapshot must not have expired by.
    pub now_ms: u64,
}

impl Hooks<'_> {
    fn scheme(&mut self, scheme: SchemeId) -> Option<&mut ActiveScheme> {
        let name = Name::new(scheme.as_str()).ok()?;
        self.schemes.get_mut(&name)
    }

    /// The credit we extend to `peer` on `ledger`: what was posted to its
    /// trust account, if it has one from an earlier peering, which is not
    /// posted again; otherwise `start`, what we would extend now.
    fn credit_for(&self, peer: NodeId, ledger: LedgerId, start: u64) -> u64 {
        Name::new(peer.as_str())
            .ok()
            .and_then(|name| {
                self.ledger
                    .find(&AccountPath::new(ledger, AccountName::ContraTrust(name)))
            })
            .and_then(|id| self.ledger.account(id))
            .map_or(start, |account| {
                dassie_settlement::policy::trust(account.balance())
            })
    }

    /// Raises the credit we extend to `peer` on `ledger` by `amount`.
    fn raise(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        ledger: LedgerId,
        amount: u64,
    ) -> Result<(), ()> {
        let name = Name::new(peer.as_str()).map_err(|_| ())?;
        super::trust::raise_trust(self.ledger, now_ms, ledger, name, amount).map_err(|_| ())
    }

    /// Opens a core peer's accounts and extends it `start`, once.
    /// New ones only while the core peers' share of the peers' account
    /// room has some left: they stay when a channel closes, so the node's
    /// ever-growing number of former peers must not take the room its
    /// tokens and ledgers need, nor the downlinks' room, or the reverse
    /// (spec/decisions/account-budgets.md). A downlink's accounts open
    /// only once value moves with it
    /// (spec/decisions/lazy-downlink-accounts.md).
    fn open(&mut self, now_ms: u64, peer: NodeId, ledger: LedgerId, start: u64) -> Result<(), ()> {
        let name = Name::new(peer.as_str()).map_err(|_| ())?;
        if !has_accounts(self.ledger, ledger, name)
            && !room_for_core_peer(self.ledger, &self.downlinks.opened)
        {
            return Err(());
        }
        self.ledger
            .open_peer(now_ms, ledger, name, start)
            .map(|_| ())
            .map_err(|_| ())
    }
}

/// A slot a peer's endpoint should have: one per method of its peering
/// (spec/decisions/settlement-methods.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WantedSlot {
    /// The method's scheme's ledger.
    pub ledger: LedgerId,
    /// The peer's account there, if open.
    pub account: Option<dassie_ledger::AccountId>,
    /// Whether the method is over a hub.
    pub hub: bool,
    /// Whether our packets may take it.
    pub available: bool,
}

/// What an inbound request gave, owned, for after the peer node is done.
enum Inbound {
    Reply(Reply),
    Deferred,
    Packet(PeerPacket),
    Settlement(NodeId, SchemeId, u64, Vec<u8>, u64, Reply),
    SettlementRequest(NodeId, SchemeId, dassie_peer::SettlementAnswer),
}

/// An ILP packet from a peer, owned.
struct PeerPacket {
    from: NodeId,
    request_id: u32,
    packet: Vec<u8>,
    /// The method a Prepare came on; `None` with a Fulfill or Reject.
    scheme: Option<SchemeId>,
    commitment: Option<Vec<u8>>,
    answer: PacketAnswer,
}

/// A node's name in the ledger.
fn name(peer: NodeId) -> Option<Name> {
    Name::new(peer.as_str()).ok()
}

/// The Reject for a Prepare that surely did not reach its peer, or that the
/// peer did not act on, by the status it answered with, if any
/// (spec/areas/connector.md#responses-and-expiry): T05 when it refused us
/// for a rate, so that the sender backs off as from a peer's rate limit, and
/// T01 for everything else.
pub(super) const fn undelivered(status: Option<u16>) -> (ErrorCode, &'static str) {
    match status {
        Some(429) => (
            ErrorCode::T05_RATE_LIMITED,
            "the peer refused the packet for its rate",
        ),
        _ => (
            ErrorCode::T01_PEER_UNREACHABLE,
            "the peer could not be reached",
        ),
    }
}

/// The accounts peers of one kind may take, downlinks or core peers: an
/// even share of what the ledger's accounts leave beside every account the
/// node opens for itself ([`OWN_ACCOUNTS`]), so neither kind's former peers
/// ever take the other's room (spec/decisions/account-budgets.md).
pub(super) fn peer_budget(max_accounts: u32, downlink: bool) -> usize {
    let room = usize::try_from(max_accounts)
        .unwrap_or(usize::MAX)
        .saturating_sub(OWN_ACCOUNTS);
    let downlinks = room / 2;
    if downlink {
        downlinks
    } else {
        room - downlinks
    }
}

/// The peers' accounts on every ledger: two for each peer and ledger.
pub(super) fn peer_accounts(ledger: &Ledger) -> usize {
    ledger
        .accounts()
        .filter(|(_, account)| {
            matches!(
                account.path().name,
                AccountName::AssetsInterledger(_) | AccountName::ContraTrust(_)
            )
        })
        .count()
}

/// Whether `node` has both its accounts on `on`.
pub(super) fn has_accounts(ledger: &Ledger, on: LedgerId, node: Name) -> bool {
    [
        AccountName::AssetsInterledger(node),
        AccountName::ContraTrust(node),
    ]
    .into_iter()
    .all(|name| ledger.find(&AccountPath::new(on, name)).is_some())
}

/// Whether another core peer's two accounts fit in the core peers' share
/// of the peers' room, beside every peer's accounts not opened for a
/// downlink. A downlink's fit as [`Downlinks::fit`] says.
pub(super) fn room_for_core_peer(
    ledger: &Ledger,
    downlink_accounts: &BTreeSet<(LedgerId, Name)>,
) -> bool {
    let taken = peer_accounts(ledger).saturating_sub(2 * downlink_accounts.len());
    taken + 2 <= peer_budget(ledger.config().max_accounts, false)
}

impl PeeringHooks for Hooks<'_> {
    fn speaks(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str())
            .ok()
            .is_some_and(|name| self.schemes.contains_key(&name))
    }

    fn peering_info(&mut self, scheme: SchemeId, out: &mut Vec<u8>) -> Result<(), Declined> {
        let active = self.scheme(scheme).ok_or(Declined)?;
        active.scheme.peering_info(out);
        Ok(())
    }

    fn create_request(
        &mut self,
        peer: NodeId,
        scheme: SchemeId,
        info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), Declined> {
        let peer = name(peer).ok_or(Declined)?;
        let active = self.scheme(scheme).ok_or(Declined)?;
        active
            .scheme
            .create_peering_request(peer, info, out)
            .map_err(|_| Declined)
    }

    fn accept(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        downlink: bool,
        scheme: SchemeId,
        data: &[u8],
    ) -> Result<Agreed, RefusalReason> {
        let peer_name = name(peer).ok_or(RefusalReason::Unspecified)?;
        let active = self
            .scheme(scheme)
            .ok_or(RefusalReason::UnsupportedScheme)?;
        let ledger = active.scheme.ledger();
        let unlimited = active.unlimited;
        // Over a hub the hold is all the credit there is; with unlimited
        // trust, it starts at the unlimited credit
        // (spec/areas/settlement.md#credit). The peer node takes no request
        // on a scheme that would settle directly.
        let start = active.start_credit();
        let credit_out = self.credit_for(peer, ledger, start);
        // A downlink's accounts open once value moves with it, and only if
        // they fit then: the room for them is kept from now on, while its
        // channel is open (spec/decisions/lazy-downlink-accounts.md).
        if downlink
            && !has_accounts(self.ledger, ledger, peer_name)
            && !self.downlinks.fit(self.ledger.config().max_accounts, &peer)
        {
            return Err(RefusalReason::Capacity);
        }
        let active = self
            .scheme(scheme)
            .ok_or(RefusalReason::UnsupportedScheme)?;
        let state = active
            .scheme
            .accept_peering_request(peer_name, data)
            .map_err(|_| RefusalReason::SchemeRefused)?;
        if !downlink {
            self.open(now_ms, peer, ledger, start)
                .map_err(|()| RefusalReason::Capacity)?;
        }
        // With unlimited trust, a trust account from an earlier peering on
        // another kind of stub is raised to it: a peer on it is never asked
        // to settle, so its room must never run out.
        let credit_out =
            if unlimited && credit_out < start && has_accounts(self.ledger, ledger, peer_name) {
                self.raise(now_ms, peer, ledger, start - credit_out)
                    .map_err(|()| RefusalReason::Capacity)?;
                start
            } else {
                credit_out
            };
        // Only now is the peering made: a request refused takes none of
        // the scheme's room.
        let active = self
            .scheme(scheme)
            .ok_or(RefusalReason::UnsupportedScheme)?;
        active.scheme.on_peered(peer_name, &state);
        Ok(Agreed { state, credit_out })
    }

    fn finalize(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        scheme: SchemeId,
        info: &[u8],
    ) -> Result<Agreed, Declined> {
        let peer_name = name(peer).ok_or(Declined)?;
        let active = self.scheme(scheme).ok_or(Declined)?;
        let ledger = active.scheme.ledger();
        // Over a hub the hold is all the credit there is; with unlimited
        // trust, it is the unlimited credit
        // (spec/areas/settlement.md#credit). The peer node asks on no
        // scheme that would settle directly.
        let hub = active.over_hub;
        let unlimited = active.unlimited;
        let start = active.start_credit();
        let credit_out = self.credit_for(peer, ledger, start);
        let active = self.scheme(scheme).ok_or(Declined)?;
        let state = active
            .scheme
            .finalize_peering_request(peer_name, info)
            .map_err(|_| Declined)?;
        // We ask only core nodes: a periphery node is asked by nobody.
        self.open(now_ms, peer, ledger, start)
            .map_err(|()| Declined)?;
        // With unlimited trust, a trust account from an earlier peering on
        // another kind of stub is raised to it: a peer on it is never asked
        // to settle, so its room must never run out.
        let credit_out = if hub {
            0
        } else if unlimited && credit_out < start {
            self.raise(now_ms, peer, ledger, start - credit_out)
                .map_err(|()| Declined)?;
            start
        } else {
            credit_out
        };
        let active = self.scheme(scheme).ok_or(Declined)?;
        active.scheme.on_peered(peer_name, &state);
        Ok(Agreed { state, credit_out })
    }

    /// A peering over a hub carries a commitment with each Prepare and a
    /// claim in each reply to a Fulfill
    /// (spec/designs/settlement-hubs.md#13-how-it-fits-the-node).
    fn uses_commitments(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str())
            .ok()
            .and_then(|name| self.schemes.get(&name))
            .is_some_and(|active| active.over_hub)
    }

    /// Only the stub with unlimited trust's peers trust each other without
    /// limit (spec/areas/settlement.md#the-stub-scheme).
    fn trusts_unlimited(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str())
            .ok()
            .and_then(|name| self.schemes.get(&name))
            .is_some_and(|active| active.unlimited)
    }

    fn forwarded_over_hub(&self, peer: NodeId, request_id: u32) -> bool {
        self.endpoints
            .get(&peer)
            .and_then(|id| {
                self.connector
                    .forwarded_on(EndpointId::Peer(*id), request_id)
            })
            .is_some_and(|ledger| {
                self.schemes
                    .values()
                    .any(|active| active.over_hub && active.scheme.ledger() == ledger)
            })
    }

    /// A method on a scheme the node runs, whose two accounts with the peer
    /// fit as a peering's first method's would: in the core peers' share of
    /// the peers' account room, or the downlinks', unless the peer has them
    /// on the scheme's ledger already, from an earlier peering
    /// (spec/decisions/settlement-methods.md,
    /// spec/decisions/account-budgets.md). Each method is on a ledger of its
    /// own, as each running scheme is: its accounts, credit and position
    /// are its own.
    fn wants_method(&self, peer: NodeId, downlink: bool, scheme: SchemeId) -> bool {
        let Some(active) = Name::new(scheme.as_str())
            .ok()
            .and_then(|name| self.schemes.get(&name))
            .filter(|active| active.failed.is_none() && active.scheme.peers())
        else {
            return false;
        };
        let Some(peer_name) = name(peer) else {
            return false;
        };
        let max_accounts = self.ledger.config().max_accounts;
        has_accounts(self.ledger, active.scheme.ledger(), peer_name)
            || if downlink {
                self.downlinks.fit(max_accounts, &peer)
            } else {
                room_for_core_peer(self.ledger, &self.downlinks.opened)
            }
    }

    fn drained(&self, peer: NodeId) -> bool {
        self.endpoints
            .get(&peer)
            .is_none_or(|id| self.connector.pending_for(EndpointId::Peer(*id)) == 0)
    }

    fn settling_to(&self, peer: NodeId) -> bool {
        self.ledger
            .pending_transfers()
            .any(|transfer| super::settlement::settles_to(self.ledger, transfer, &peer))
    }

    // Ours on `scheme` alone: those pending in its ledger.
    fn settling_on(&self, peer: NodeId, scheme: SchemeId) -> bool {
        let Some(ledger) = Name::new(scheme.as_str())
            .ok()
            .and_then(|name| self.schemes.get(&name))
            .map(|active| active.scheme.ledger())
        else {
            return false;
        };
        self.ledger.pending_transfers().any(|transfer| {
            super::settlement::settles_to(self.ledger, transfer, &peer)
                && self
                    .ledger
                    .account(transfer.debit)
                    .is_some_and(|account| account.path().ledger == ledger)
        })
    }

    fn receives(&self, scheme: SchemeId) -> bool {
        Name::new(scheme.as_str())
            .ok()
            .and_then(|name| self.schemes.get(&name))
            .is_some_and(|active| active.scheme.can_receive())
    }

    // A database that cannot be read leaves the core unable to tell a
    // settlement applied from one that is not: it stops, as when a write
    // fails, and restarts from the disk.
    #[allow(clippy::panic)]
    fn closed_channel(&self, node: NodeId) -> Option<dassie_peer::Closed> {
        match self.store.closed_channel(&node) {
            Ok(closed) => closed,
            Err(error) => panic!("reading a closed settlement channel failed: {error}"),
        }
    }

    fn closable(&self, node: NodeId) -> bool {
        let Some(peer) = name(node) else {
            return false;
        };
        let held = |name: &AccountName| {
            matches!(name, AccountName::AssetsInterledger(of) | AccountName::ContraTrust(of)
                if *of == peer)
        };
        let pending = self.ledger.pending_transfers().any(|transfer| {
            [transfer.debit, transfer.credit].iter().any(|id| {
                self.ledger
                    .account(*id)
                    .is_some_and(|account| held(&account.path().name))
            })
        });
        // A settlement it made before the close is booked as its own only
        // by a scheme that still knows it, and sees its ledger.
        let with_accounts: Vec<LedgerId> = LedgerId::ALL
            .into_iter()
            .filter(|ledger| {
                self.ledger
                    .find(&AccountPath::new(
                        *ledger,
                        AccountName::AssetsInterledger(peer),
                    ))
                    .is_some()
            })
            .collect();
        // A downlink nothing moved with yet may have settled on any scheme
        // we run, which then opens its accounts: every one of them, stopped
        // ones too, must see its ledger
        // (spec/decisions/lazy-downlink-accounts.md).
        let unseen = if with_accounts.is_empty() {
            self.schemes.values().any(|active| !self.synced(active))
        } else {
            with_accounts.into_iter().any(|ledger| !self.sees(ledger))
        };
        // Nor while a scheme has something with it not settled on its
        // ledger yet, as a hold with funds over a hub.
        let kept = self
            .schemes
            .values()
            .any(|active| active.scheme.keeps_channel(peer));
        !pending && !unseen && !kept
    }

    fn offers_swaps(&self) -> bool {
        self.settings.offer_swaps
    }

    fn wants_snapshots(&self, _: NodeId, peering: &dassie_peer::Peering) -> bool {
        self.settings.swapping
            && super::snapshots::spans_two_ledgers(peering.schemes(), self.schemes)
    }

    fn takes_snapshot(
        &self,
        _: NodeId,
        key: &PublicKey,
        snapshot: &[u8],
        held: Option<&[u8]>,
    ) -> bool {
        Core::takes_snapshot(key, snapshot, held, self.now_ms)
    }
}

impl Hooks<'_> {
    /// Whether a scheme on `ledger` runs and sees its ledger now.
    fn sees(&self, ledger: LedgerId) -> bool {
        self.schemes
            .values()
            .any(|active| active.scheme.ledger() == ledger && self.synced(active))
    }

    /// Whether `active` runs and sees its ledger now.
    fn synced(&self, active: &ActiveScheme) -> bool {
        let asset = active
            .asset
            .and_then(|id| self.assets.get(&id))
            .map(|asset| asset.asset.status());
        active.failed.is_none()
            && merged_status(asset, active.scheme.status()) == LedgerStatus::Synced
    }
}

/// A peering as the owner sees it.
#[derive(Clone, Debug)]
pub struct PeerView {
    /// The peer.
    pub id: NodeId,
    /// The peering.
    pub peering: dassie_peer::Peering,
    /// Its termination, while it is being ended.
    pub termination: Option<dassie_peer::Termination>,
    /// Whether it is listed as our neighbor.
    pub advertised: bool,
    /// Whether it was heard from recently enough to count as online.
    pub online: bool,
    /// What we owe the peer net on its peering's first method; negative
    /// when it owes us. Each method's is in its own ledger's units.
    pub owed: Option<i128>,
    /// What we owe the peer net on each of the peering's methods, in the
    /// peering's order.
    pub owed_by_method: Vec<Option<i128>>,
    /// Whether each of the peering's methods is withdrawn for room, in the
    /// peering's order.
    pub withdrawn_by_method: Vec<bool>,
}

/// The network as the owner sees it.
#[derive(Clone, Debug)]
pub struct NetworkView {
    /// Our node ID.
    pub own: NodeId,
    /// `test` or `g`.
    pub allocation_scheme: &'static str,
    /// The node table.
    pub nodes: Vec<dassie_peer::NodeView>,
    /// Our peerings.
    pub peers: Vec<PeerView>,
    /// Our standing with the bootstrap nodes.
    pub registration: Vec<dassie_peer::RegistrationStatus>,
    /// Settlement channels, with nodes peered with now or before.
    pub channels: Vec<(NodeId, dassie_peer::Channel)>,
    /// Banned nodes.
    pub bans: Vec<NodeId>,
    /// Routes to the node's own endpoints other than peers: address,
    /// endpoint and ledger.
    pub local_routes: Vec<(String, EndpointId, dassie_ledger::LedgerId)>,
    /// Whether we are on a majority of the bootstrap nodes' lists.
    pub member: bool,
}

impl Core {
    /// The owner's peer settings.
    pub const fn peer_settings(&self) -> PeerSettings {
        self.peer_settings
    }

    /// Changes the owner's peer settings.
    ///
    /// # Errors
    ///
    /// They are out of range.
    pub fn set_peer_settings(&mut self, settings: PeerSettings) -> Result<(), EngineError> {
        if !settings.is_valid() {
            return Err(EngineError::Invalid(
                "the fee, the spread and the rebalancing must be at most 100 000 ppm, and a swap \
                 target names an asset, not a hub's ledger"
                    .into(),
            ));
        }
        self.peer_settings = settings;
        // Whether the connector takes a peer's swaps follows the owner's
        // word (spec/decisions/peer-swaps.md), and so does whether the link
        // state names the port the node answers requests for its snapshot
        // on.
        self.connector.set_offer_swaps(settings.offer_swaps);
        let own_info = self.own_info_offered(self.peer.own_info().clone());
        let now = self.now_ms;
        self.peer.set_own_info(now, own_info);
        self.prices.due = true;
        self.batch.peer_settings = Some(settings);
        self.changes.network = true;
        Ok(())
    }

    /// Whether a peering began or ended since the last call, or, at the
    /// first, whether any was restored.
    pub fn take_peerings_changed(&mut self) -> bool {
        std::mem::take(&mut self.peerings_changed)
    }

    /// The nodes peered with us, ending it or not, and whether each is our
    /// downlink: what gives their QUIC connections places of their own
    /// (spec/areas/peer-protocol.md#quic).
    pub fn peerings(&self) -> impl Iterator<Item = (NodeId, bool)> + '_ {
        self.peer
            .peers()
            .map(|(node, _)| (*node, self.peer.is_downlink(node)))
    }

    /// What the connector and the peer protocol counted since the engine
    /// started, in one step, so the two agree.
    pub fn counters(&self) -> (dassie_connector::Stats, dassie_peer::Stats) {
        (self.connector.stats(), *self.peer.stats())
    }

    /// What `peer` earned us as a next hop on its method on `ledger`, as of
    /// `at_ms`: its weight in drawing next hops among those with a method on
    /// that ledger (spec/areas/connector.md#routing), for a simulation to
    /// read. None without a slot there.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn next_hop_weight(&self, peer: &NodeId, ledger: LedgerId, at_ms: u64) -> Option<u64> {
        let endpoint = EndpointId::Peer(*self.peer_endpoints.get(peer)?);
        self.connector.earnings(endpoint, ledger, at_ms)
    }

    /// The network as the owner sees it.
    pub fn network(&self) -> NetworkView {
        NetworkView {
            own: *self.peer.own(),
            allocation_scheme: self.realm.ilp_allocation_scheme(),
            nodes: self.peer.nodes(),
            peers: self
                .peer
                .nodes()
                .into_iter()
                .filter_map(|view| {
                    let mut peering = view.peering?;
                    // The credit we extend now on each method, which over
                    // a hub follows the holds (spec/areas/hubs.md#in-the-node);
                    // the peering keeps what it started with.
                    for method in &mut peering.methods {
                        if let Some(credit) = self.credit_out(view.id, method.scheme) {
                            method.credit_out = credit;
                        }
                    }
                    let first = peering.first().scheme;
                    let owed_by_method = (peering.methods.iter())
                        .map(|method| self.owed(view.id, method.scheme))
                        .collect();
                    let grace = self.peer.config().advertise_short_grace_ms;
                    let withdrawn_by_method = (peering.methods.iter())
                        .map(|method| method.withdrawn(grace))
                        .collect();
                    Some(PeerView {
                        id: view.id,
                        owed: self.owed(view.id, first),
                        owed_by_method,
                        withdrawn_by_method,
                        peering,
                        termination: view.termination,
                        advertised: view.advertised,
                        online: view.online,
                    })
                })
                .collect(),
            registration: self.peer.registration_status(),
            channels: self
                .peer
                .channels()
                .map(|(id, channel)| (*id, channel.clone()))
                .collect(),
            bans: self.peer.bans().copied().collect(),
            local_routes: self
                .connector
                .endpoints()
                .filter(|(id, _, _)| !id.is_peer())
                .map(|(id, address, ledger)| (address.to_string(), id, ledger))
                .collect(),
            member: self.peer.majority().contains(self.peer.own()),
        }
    }

    /// Runs `f` on the peer node with the hooks over the core.
    pub(super) fn with_peer<T>(
        &mut self,
        f: impl FnOnce(&mut dassie_peer::PeerNode, &mut Hooks<'_>, u64) -> T,
    ) -> T {
        let now = self.now_ms;
        let mut hooks = Hooks {
            schemes: &mut self.schemes,
            ledger: &mut self.ledger,
            connector: &self.connector,
            endpoints: &self.peer_endpoints,
            assets: &self.assets,
            store: &self.store,
            downlinks: &self.downlinks,
            settings: self.peer_settings,
            now_ms: now,
        };
        f(&mut self.peer, &mut hooks, now)
    }

    /// An inbound peer request, from the node whose key `from` its
    /// connection proved: its reply goes out once the batch is on disk, as
    /// a peer accepting a peering must have recorded it first.
    pub fn peer_inbound(
        &mut self,
        inbound: InboundId,
        from: dassie_identity::NodeIdentity,
        body: &[u8],
        source: Option<std::net::IpAddr>,
    ) {
        let outcome = self.with_peer(|peer, hooks, now| {
            match peer.handle_inbound(now, hooks, inbound, from, body, source) {
                dassie_peer::Inbound::Reply(reply) => Inbound::Reply(reply),
                dassie_peer::Inbound::Deferred => Inbound::Deferred,
                dassie_peer::Inbound::Packet {
                    from,
                    request_id,
                    packet,
                    scheme,
                    commitment,
                    answer,
                } => Inbound::Packet(PeerPacket {
                    from,
                    request_id,
                    packet: packet.to_vec(),
                    scheme,
                    commitment: commitment.map(<[u8]>::to_vec),
                    answer,
                }),
                dassie_peer::Inbound::Settlement {
                    from,
                    scheme,
                    amount,
                    data,
                    sequence,
                    reply,
                } => Inbound::Settlement(from, scheme, amount, data.to_vec(), sequence, reply),
                dassie_peer::Inbound::SettlementRequest {
                    from,
                    scheme,
                    answer,
                } => Inbound::SettlementRequest(from, scheme, answer),
            }
        });
        let reply = match outcome {
            Inbound::Reply(reply) => Some(reply),
            Inbound::Deferred => None,
            Inbound::Packet(packet) => Some(self.packet_inbound(packet)),
            Inbound::Settlement(from, scheme, amount, data, sequence, reply) => {
                Some(self.settlement_from(from, scheme, amount, &data, sequence, reply))
            }
            Inbound::SettlementRequest(from, scheme, answer) => {
                let settling = self.settlement_requested(from, scheme);
                Some(answer.reply(settling))
            }
        };
        if let Some(reply) = reply {
            self.outputs_peer.push(Output::PeerReply { inbound, reply });
        }
        self.after_peer();
    }

    /// An ILP packet from a peer, and the reply to it, on the packet's
    /// settlement method (spec/decisions/settlement-methods.md): a Prepare's
    /// is the one it came on, which the connector books it on, a Fulfill's
    /// or Reject's the one our Prepare went on, the slot the connector
    /// chose for it. Over a hub, a Prepare goes on only if its commitment
    /// covers it, and the reply to a Fulfill carries our claim
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
    fn packet_inbound(&mut self, inbound: PeerPacket) -> Reply {
        let PeerPacket {
            from,
            request_id,
            packet,
            scheme,
            commitment,
            answer,
        } = inbound;
        let now = self.now_ms;
        let is_prepare = packet.first() == Some(&dassie_ilp::TYPE_PREPARE);
        // A method being wound down takes no new Prepares from the peer
        // (spec/areas/network.md#settlement-methods): its slot is
        // unavailable for ours, and the connector books the peer's on any
        // slot it has.
        if let (true, Some(scheme)) = (is_prepare, scheme)
            && self.peer.method_winding(&from, scheme)
        {
            self.refuse_prepare(now, &from, request_id, "the method is being wound down");
            return answer.reply(&[]);
        }
        if let (true, Some(scheme)) = (is_prepare, scheme)
            && !self.check_hub_prepare(
                now,
                &from,
                scheme,
                request_id,
                &packet,
                commitment.as_deref(),
            )
        {
            return answer.reply(&[]);
        }
        // A response's method is its packet's slot, which the connector
        // forgets once the response resolves it: found by its ledger, so a
        // method dropped since still hears of it.
        let forwarded = if is_prepare {
            None
        } else {
            self.forwarded_ledger(&from, request_id)
        };
        let taken = self.peer_packet(from, scheme, request_id, &packet);
        // A Prepare taken lowers our room for the peer on its method: asked
        // at once if that took it to the trigger
        // (spec/decisions/settle-at-the-trigger.md).
        if is_prepare && let Some(scheme) = scheme {
            self.room_fell(from, scheme);
        }
        let Some(ledger) = forwarded else {
            return answer.reply(&[]);
        };
        if answer.takes_claim() {
            let claim = self.hub_claim(now, &from, ledger, request_id, taken);
            return answer.reply(&claim);
        }
        self.hub_rejected(&from, ledger, request_id);
        answer.reply(&[])
    }

    /// The settlement method of `peer`'s peering on `ledger`: each method
    /// is on a ledger of its own, and so is its slot in the peer's
    /// endpoint. `None` if it has none there, as for a method dropped.
    pub(super) fn method_on(&self, peer: &NodeId, ledger: LedgerId) -> Option<SchemeId> {
        self.peer.peering_of(peer)?.schemes().find(|scheme| {
            Name::new(scheme.as_str())
                .ok()
                .and_then(|name| self.schemes.get(&name))
                .is_some_and(|active| active.scheme.ledger() == ledger)
        })
    }

    /// The scheme over a hub on `ledger`, and `peer`'s name in it, for a
    /// packet with `peer` in flight on its slot there: whether or not the
    /// peering still lists the method, since a dropped method's packets
    /// resolve on it, and one that timed out over a hub resolves as the hub
    /// decided (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
    /// `None` if the method is not over a hub.
    pub(super) fn packet_hub(&self, peer: &NodeId, ledger: LedgerId) -> Option<(Name, Name)> {
        let (&scheme, _) = self
            .schemes
            .iter()
            .find(|(_, active)| active.over_hub && active.scheme.ledger() == ledger)?;
        Some((scheme, name(*peer)?))
    }

    /// The ledger of the slot our Prepare in flight to `peer` with
    /// `request_id` went on, which the connector chose for it: its
    /// method's, whether or not the peering still lists it.
    pub(super) fn forwarded_ledger(&self, peer: &NodeId, request_id: u32) -> Option<LedgerId> {
        let endpoint = EndpointId::Peer(*self.peer_endpoints.get(peer)?);
        self.connector.forwarded_on(endpoint, request_id)
    }

    /// Whether our packets may go to `peer` on its method on `scheme`
    /// (spec/decisions/settlement-methods.md): the scheme runs, settles
    /// over a hub or trusts without limit, sees its ledger and can receive
    /// a peer's settlement, and the method is not withdrawn for room. A
    /// method that would settle directly, as one on an owner book that a
    /// peering stored from when the XRPL schemes peered still has, carries
    /// nothing until the peer protocol drops it
    /// (spec/areas/settlement.md#owner-books). Which method a packet takes depends on this and
    /// its incoming ledger, never on the room each has.
    ///
    /// A hub scheme without an account of its own, which the hub agrees it
    /// has none of, sees its hub all the same toward a peer with one: that
    /// peer pays us in, and meanwhile we spend what it owes us through it,
    /// the books alone bounding it (spec/designs/settlement-hubs.md#5-joining).
    ///
    /// A hub scheme whose account only lags in reading the newest blocks it
    /// heard of sees its hub too, unless the hub is frozen: the account is
    /// behind the validators for a moment after nearly every block, and a
    /// packet would otherwise leave its ledger now and then for another
    /// method.
    pub(super) fn method_available(&self, peer: &NodeId, scheme: SchemeId) -> bool {
        let Some(method) = self
            .peer
            .peering_of(peer)
            .and_then(|peering| peering.method(scheme))
        else {
            return false;
        };
        let Some((id, active)) = Name::new(scheme.as_str())
            .ok()
            .and_then(|id| Some((id, self.schemes.get(&id)?)))
        else {
            return false;
        };
        let sees_ledger = match self.scheme_status(active) {
            LedgerStatus::Synced => true,
            LedgerStatus::Unfunded => {
                active.over_hub
                    && name(*peer)
                        .and_then(|peer| active.scheme.peer_hub_account(peer))
                        .is_some()
            }
            LedgerStatus::Syncing => {
                active.over_hub
                    && active.scheme.status() == LedgerStatus::Synced
                    && self
                        .hub_account(id)
                        .is_some_and(|account| account.keeps_up() && account.frozen().is_none())
            }
            _ => false,
        };
        let grace_ms = self.peer.config().advertise_short_grace_ms;
        active.failed.is_none()
            && !active.settles_directly()
            && sees_ledger
            && active.scheme.can_receive()
            && !method.withdrawn(grace_ms)
            && !method.winding_down()
    }

    /// Applies a peer's settlement, once: refused with 400 if the books
    /// cannot take it, which changes nothing, and the peer sends it again
    /// until they can, as once a scheme that stopped runs again.
    ///
    /// One on a scheme the node does not run, as one its owner removed, is
    /// acknowledged and books nothing (spec/areas/settlement.md#incoming-settlements):
    /// it could never be booked, and the peer, which sends the pair's
    /// settlements one at a time, would send it again for ever, the rest
    /// of the channel waiting behind it. What it moved, if anything, stays
    /// on the scheme's ledger, which the scheme reads when it runs again; a
    /// removed scheme ran on until such a settlement had its time
    /// ([`Core::removal_wait_ms`]).
    fn settlement_from(
        &mut self,
        from: NodeId,
        scheme: SchemeId,
        amount: u64,
        data: &[u8],
        sequence: u64,
        reply: Reply,
    ) -> Reply {
        let runs = Name::new(scheme.as_str()).is_ok_and(|name| self.schemes.contains_key(&name));
        if !runs {
            self.report(SettlementEvent::PeerSettlementUnbooked {
                from,
                scheme,
                amount,
            });
            self.peer.settlement_applied(self.now_ms, &from, sequence);
            return reply;
        }
        match self.incoming_settlement(from, scheme, amount, data) {
            Ok(()) => {
                self.peer.settlement_applied(self.now_ms, &from, sequence);
                reply
            }
            Err(error) => {
                self.report(SettlementEvent::PeerSettlementRefused {
                    from,
                    scheme,
                    error: error.clone(),
                });
                Reply::refuse(400, "the settlement cannot be booked")
            }
        }
    }

    /// Ends the peering with `node`, as its owner.
    ///
    /// # Errors
    ///
    /// There is no peering with it.
    pub fn terminate_peering(&mut self, node: NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        if !self.peer.terminate(now, &node, TerminationReason::Owner) {
            return Err(EngineError::NotFound("no peering with that node".into()));
        }
        self.after_peer();
        Ok(())
    }

    /// Registers with the bootstrap node `node` now.
    ///
    /// # Errors
    ///
    /// It is not one of our bootstrap nodes, we have no link state yet, or
    /// a registration with it is on its way.
    pub fn register_with_bootstrap(&mut self, node: NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        self.peer
            .register_now(now, &node)
            .map_err(|error| match error {
                dassie_peer::RegisterError::NotBootstrap => {
                    EngineError::NotFound("not a bootstrap node".into())
                }
                dassie_peer::RegisterError::NoLinkState => EngineError::Precondition(
                    "the node has no link state to register yet: set its hostname".into(),
                ),
                dassie_peer::RegisterError::InFlight => {
                    EngineError::Precondition("a registration with it is on its way".into())
                }
            })?;
        self.changes.network = true;
        self.after_peer();
        Ok(())
    }

    /// Bans `node`, ending a peering with it.
    ///
    /// # Errors
    ///
    /// It is us, or the list is full.
    pub fn ban_node(&mut self, node: NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        self.peer.ban(now, node).map_err(|error| match error {
            BanError::Ourselves => EngineError::Invalid("a node cannot ban itself".into()),
            BanError::Full => EngineError::Exhausted("too many banned nodes".into()),
        })?;
        self.after_peer();
        Ok(())
    }

    /// Lifts a ban.
    ///
    /// # Errors
    ///
    /// The node is not banned.
    pub fn unban_node(&mut self, node: &NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        if !self.peer.unban(now, node) {
            return Err(EngineError::NotFound("the node is not banned".into()));
        }
        self.after_peer();
        Ok(())
    }

    /// The banned nodes.
    pub fn bans(&self) -> Vec<NodeId> {
        self.peer.bans().copied().collect()
    }

    /// Whether `node` is a peer, ending the peering or not.
    #[must_use]
    pub fn is_peer(&self, node: &NodeId) -> bool {
        self.peer.peering_of(node).is_some()
    }

    /// The result of one of our peer requests.
    pub fn peer_response(
        &mut self,
        id: dassie_peer::RequestId,
        result: Result<dassie_peer::Response, dassie_peer::Failure>,
    ) {
        self.with_peer(|peer, hooks, now| peer.handle_response(now, hooks, id, result));
        self.after_peer();
    }

    /// `info` as our link state says it: the port the node answers
    /// requests for its snapshot on only while its owner lets it offer
    /// swaps, since a node that offers none answers no request
    /// (spec/areas/rate-feed.md#finding-a-node). The peer protocol leaves
    /// it out on a periphery node, which offers none either.
    pub(super) fn own_info_offered(&self, info: dassie_peer::OwnInfo) -> dassie_peer::OwnInfo {
        dassie_peer::OwnInfo {
            feed_port: self.feed_port.filter(|_| self.peer_settings.offer_swaps),
            ..info
        }
    }

    /// Changes what our link state says: a new one follows. What it
    /// offers is the core's ([`Self::update_peer_schemes`]).
    pub fn set_own_info(&mut self, info: dassie_peer::OwnInfo) {
        let now = self.now_ms;
        self.feed_port = info.feed_port;
        let info = self.own_info_offered(info);
        self.peer.set_own_info(now, info);
        // Becoming a periphery node, or a core node again, changes how
        // packets from peers are routed.
        self.update_routes();
        self.after_peer();
    }

    /// Runs the peer node's timers, if due.
    pub(super) fn peer_timers(&mut self) {
        let now = self.now_ms;
        if self.peer.poll_timeout().is_some_and(|at| at <= now) {
            self.with_peer(|peer, hooks, now| peer.handle_timeout(now, hooks));
            self.after_peer();
            // The peer node's reviews add, drop and withdraw methods.
            self.sync_peer_slots();
        }
        self.finish_removals();
    }

    /// Tells the peer node which schemes it takes peers on, as its link
    /// state offers them, in a fixed order
    /// (spec/areas/peer-protocol.md#settlement-offers): the active ones that
    /// peer at all, which an owner book does not
    /// (spec/areas/settlement.md#owner-books), that would not settle
    /// directly (spec/decisions/hubs-only.md), that have seen their ledger
    /// (spec/decisions/synced-before-peering.md), have not stopped, and can
    /// receive a peer's settlement. Before a scheme first synced, a peer's
    /// settlement would be booked as the owner's deposit; once it stopped,
    /// it would be refused; while it cannot receive, as once its hub is
    /// shutting down, stalled or ended, it could not be paid
    /// (spec/areas/settlement.md#when-peers-cannot-pay-us). A scheme that
    /// only lost its ledger stays, once it has synced. So the node
    /// neither offers, asks for nor accepts a peering on it; peerings already
    /// made stay. A scheme over a hub is offered only with its membership,
    /// each hub once ([`super::hubs::offers`]). The list is made from the
    /// schemes active now, each time: the original's started from those
    /// active at its start, so of two changes the second undid the first.
    /// The node's hubs change with its schemes, so their funding follows
    /// them here too.
    pub(super) fn update_peer_schemes(&mut self) {
        // Whether each method may carry our packets follows its scheme.
        self.sync_peer_slots();
        let names: Vec<Name> = self
            .schemes
            .iter()
            .filter(|(_, active)| {
                active.failed.is_none()
                    && active.removing.is_none()
                    && active.scheme.peers()
                    && !active.settles_directly()
                    && active.scheme.has_synced()
                    && active.scheme.can_receive()
            })
            .map(|(name, _)| *name)
            .collect();
        let offerings: Vec<(Name, super::hubs::Offering)> = names
            .into_iter()
            .filter_map(|name| Some((name, self.offering(name)?)))
            .collect();
        let offers = super::hubs::offers(offerings);
        let now = self.now_ms;
        self.peer.set_offers(now, offers);
        self.follow_hubs();
    }

    /// Takes in what the peer node did: its requests and replies to hand
    /// over, and its events.
    pub(super) fn after_peer(&mut self) {
        while let Some(output) = self.peer.poll_output() {
            self.outputs_peer.push(match output {
                PeerOutput::Request(request) => Output::PeerRequest(request),
                PeerOutput::Reply { inbound, reply } => Output::PeerReply { inbound, reply },
            });
        }
        while let Some(event) = self.peer.poll_event() {
            self.peer_event(event);
        }
        self.take_journal();
    }

    pub(super) fn peer_event(&mut self, event: Event) {
        self.changes.network = true;
        self.report(event.clone());
        match event {
            Event::Peered { node, .. } => {
                self.peerings_changed = true;
                // A downlink's channel opened: room is kept for its
                // accounts until value moves with it.
                self.refresh_waiting(node);
                if let Err(error) = self.add_peer_endpoint(node) {
                    self.report(PeerEvent::NoEndpoint { node, error });
                }
                self.update_routes();
            }
            Event::RoutesChanged => self.update_routes(),
            // The claim for a Fulfill of ours, of a Prepare the peer sent on
            // one of its methods, which the connector no longer holds and
            // the peering may have dropped since: a claim is signed for one
            // hub's hold and verifies against it alone, so each scheme over
            // a hub is handed it, and the others drop it.
            Event::FulfillReply {
                peer,
                request_id,
                claim,
            } => self.hub_claim_received(self.now_ms, &peer, request_id, &claim),
            // Asked again at the next check, not a minute later: a peer
            // that had not yet taken in our acceptance settles once it has
            // (spec/areas/settlement.md#asking-a-peer-to-settle), and a
            // request that never arrived does not use up the grace before a
            // peer short of room is withdrawn
            // (spec/decisions/peering-lifecycle.md). A refusal
            // for not being peered, or a request that never left, says so
            // of the peering, not of one method: each of its methods may be
            // asked again.
            Event::SettlementRequestRefused { peer }
            | Event::SettlementRequestNotDelivered { peer } => {
                self.settle.asked.retain(|(node, _), _| *node != peer);
            }
            // A method dropped is asked on, and held off, no more, and its
            // slot in the peer's endpoint goes, once what is in flight on
            // it resolved.
            Event::MethodDropped { peer, scheme, .. } => {
                self.settle.asked.remove(&(peer, scheme));
                self.settle.backoff.remove(&(peer, scheme));
                self.resync_slots(peer);
            }
            // While our messages surely do not reach the peer, its new
            // Prepares are refused: a Fulfill of ours would count only in
            // our books (spec/decisions/unreachable-peers.md).
            Event::Reachable { peer, reachable } => {
                if let Some(&endpoint) = self.peer_endpoints.get(&peer) {
                    self.connector
                        .set_unreached(EndpointId::Peer(endpoint), !reachable);
                }
            }
            Event::PacketFailed {
                peer,
                request_id,
                definite,
                status,
            } => {
                // Only a packet that surely did not arrive, or that the
                // peer did not act on, is failed now: one that may have
                // been is left to expire, so a peer that fulfilled it does
                // not bear its loss.
                if definite && let Some(&endpoint) = self.peer_endpoints.get(&peer) {
                    // Over a hub, the scheme of the packet's method forgets
                    // the commitment too, a dropped method's as well.
                    let ledger = self.forwarded_ledger(&peer, request_id);
                    if let Some((scheme, name)) =
                        ledger.and_then(|ledger| self.packet_hub(&peer, ledger))
                    {
                        self.with_commitments(scheme, |c| c.on_answered(name, request_id));
                    }
                    let (code, message) = undelivered(status);
                    self.connector.fail_forwarded(
                        self.now_ms,
                        EndpointId::Peer(endpoint),
                        request_id,
                        code,
                        message,
                    );
                }
            }
            // The names the node answers to follow its view
            // (spec/areas/payments.md#the-nodes-names).
            Event::Name(NameEvent::HoldersChanged { .. }) => {
                self.held_names_changed = true;
            }
            other => self.lifecycle_event(other),
        }
    }

    /// What a peering's lifecycle and its settlements bring about.
    fn lifecycle_event(&mut self, event: Event) {
        match event {
            // Its new Prepares are refused while we collect, ours still go
            // (spec/decisions/ending-a-peering.md).
            Event::Depeering { peer, .. } => {
                if let Some(&endpoint) = self.peer_endpoints.get(&peer) {
                    self.connector
                        .set_refusing(EndpointId::Peer(endpoint), true);
                }
            }
            Event::Terminating { peer, .. } => {
                if let Some(&endpoint) = self.peer_endpoints.get(&peer) {
                    self.connector
                        .set_draining(EndpointId::Peer(endpoint), true);
                }
                self.update_routes();
            }
            Event::PeeringEnded { peer } => {
                self.peerings_changed = true;
                self.remove_peer_endpoint(&peer);
                self.update_routes();
            }
            // A method added gets a slot of its own; one withdrawn, or no
            // longer, or being wound down, is available no more, or again.
            Event::MethodAdded { peer, .. }
            | Event::MethodWinding { peer, .. }
            | Event::Advertised { peer, .. } => {
                self.resync_slots(peer);
            }
            // Its accounts stay with what they show; the schemes forget
            // what they kept of the node for its settlements
            // (spec/decisions/closing-channels.md).
            Event::ChannelClosed { peer } => {
                self.settle.backoff.retain(|(node, _), _| *node != peer);
                // A downlink that never moved value leaves nothing behind
                // (spec/decisions/lazy-downlink-accounts.md).
                self.refresh_waiting(peer);
                if let Some(name) = name(peer) {
                    // A stopped scheme is fed nothing: the channel closed
                    // only while every scheme it has accounts with ran.
                    for active in self.schemes.values_mut().filter(|a| a.failed.is_none()) {
                        active.scheme.on_closed(name);
                    }
                }
            }
            _ => {}
        }
    }

    /// The slots `node`'s endpoint should have: one per method of its
    /// peering whose scheme is active, in the peering's order, on the
    /// scheme's ledger and the peer's account there, which only a downlink
    /// may lack (spec/decisions/lazy-downlink-accounts.md), over a hub if
    /// the scheme is, and available as [`Self::method_available`] says.
    pub(super) fn wanted_slots(&self, node: &NodeId) -> Vec<WantedSlot> {
        let Some(peering) = self.peer.peering_of(node) else {
            return Vec::new();
        };
        let Some(peer) = name(*node) else {
            return Vec::new();
        };
        let downlink = self.peer.is_downlink(node);
        peering
            .schemes()
            .filter_map(|method| {
                let active = self.schemes.get(&Name::new(method.as_str()).ok()?)?;
                let ledger = active.scheme.ledger();
                let account = self.ledger.find(&AccountPath::new(
                    ledger,
                    AccountName::AssetsInterledger(peer),
                ));
                (account.is_some() || downlink).then(|| WantedSlot {
                    ledger,
                    account,
                    hub: active.over_hub,
                    available: self.method_available(node, method),
                })
            })
            .collect()
    }

    /// Brings `node`'s endpoint's slots in line with its peering's methods
    /// ([`Self::wanted_slots`]): a slot for a method added, none for one
    /// dropped, whose packets in flight still resolve on it, and each
    /// slot's hub flag, its availability and an account opened since. A
    /// peer without an endpoint gets one once a method has a slot to make
    /// it with. Returns whether it made one, so the routes reach it.
    pub(super) fn sync_slots_of(&mut self, node: NodeId) -> bool {
        let wanted = self.wanted_slots(&node);
        let Some(&id) = self.peer_endpoints.get(&node) else {
            if wanted.is_empty() || self.peer.peering_of(&node).is_none() {
                return false;
            }
            if let Err(error) = self.add_peer_endpoint(node) {
                self.report(PeerEvent::NoEndpoint { node, error });
                return false;
            }
            self.mark_peer_endpoint(&node);
            return true;
        };
        let endpoint = EndpointId::Peer(id);
        let gone: Vec<LedgerId> = self
            .connector
            .slots(endpoint)
            .filter(|slot| !slot.removed && !wanted.iter().any(|w| w.ledger == slot.ledger))
            .map(|slot| slot.ledger)
            .collect();
        for ledger in gone {
            self.connector.remove_slot(endpoint, ledger);
        }
        for slot in &wanted {
            let has = self
                .connector
                .slots(endpoint)
                .find(|view| view.ledger == slot.ledger && !view.removed);
            match has {
                None => {
                    if let Err(error) = self.connector.add_slot(endpoint, slot.ledger, slot.account)
                    {
                        self.report(PeerEvent::NoEndpoint {
                            node,
                            error: error.to_string(),
                        });
                        continue;
                    }
                }
                Some(view) if view.account.is_none() => {
                    if let Some(account) = slot.account {
                        self.connector.open_account(endpoint, slot.ledger, account);
                    }
                }
                Some(_) => {}
            }
            self.connector.set_hub(endpoint, slot.ledger, slot.hub);
            self.connector
                .set_available(endpoint, slot.ledger, slot.available);
        }
        false
    }

    /// Brings `node`'s slots in line with its methods
    /// ([`Self::sync_slots_of`]), and the routes with an endpoint made.
    fn resync_slots(&mut self, node: NodeId) {
        if self.sync_slots_of(node) {
            self.update_routes();
        }
    }

    /// Brings every peer's slots in line with its methods
    /// ([`Self::sync_slots_of`]), and the routes with the endpoints made.
    pub(super) fn sync_peer_slots(&mut self) {
        let peers: Vec<NodeId> = self.peer.peers().map(|(node, _)| *node).collect();
        let mut made = false;
        for node in peers {
            made |= self.sync_slots_of(node);
        }
        if made {
            self.update_routes();
        }
    }

    /// Marks a peer's new endpoint as its peering's state has it: one
    /// being ended stays drained; one we end privately still refuses the
    /// peer's Prepares (spec/decisions/ending-a-peering.md).
    pub(super) fn mark_peer_endpoint(&mut self, peer: &NodeId) {
        let Some(&endpoint) = self.peer_endpoints.get(peer) else {
            return;
        };
        let endpoint = EndpointId::Peer(endpoint);
        let active = self.peer.active_peers().any(|(id, _)| id == peer);
        let depeering = self
            .peer
            .peering_of(peer)
            .is_some_and(|peering| peering.depeering.is_some());
        if !active {
            self.connector.set_draining(endpoint, true);
        } else if depeering {
            self.connector.set_refusing(endpoint, true);
        }
    }

    /// Takes a peer's endpoint away, at a peering's end: nothing is in
    /// flight with it by now, since the peering ended only once drained.
    /// Were anything left, our Prepares to it would be rejected at once
    /// (T01), a Fulfill for one that comes later dropped, and its Prepares
    /// to us get no reply. A method's drop takes only its slot away, whose
    /// packets resolve on it ([`Self::sync_peer_slots`]).
    fn remove_peer_endpoint(&mut self, peer: &NodeId) {
        if let Some(endpoint) = self.peer_endpoints.remove(peer) {
            self.endpoint_peers.remove(&endpoint);
            let now = self.now_ms;
            // What was in flight to it is due now: the step expires it as
            // it finishes, rather than here, which the expiry could reach
            // again through the peer node (spec/decisions/tigerstyle.md: no
            // recursion).
            self.connector
                .remove_endpoint(now, EndpointId::Peer(endpoint));
        }
    }

    /// Makes a peer a connector endpoint, with a slot per method of its
    /// peering on the method's ledger and the peer's account there
    /// ([`Self::wanted_slots`]), at the address ILDCP will tell it: its own
    /// `<scheme>.das.<id>`, which routes reach as they reach any node's; or,
    /// for a downlink, its child address beneath ours, `<our address>.<id>`,
    /// which reaches it through us
    /// (spec/areas/connector.md#addresses-and-endpoints). Every method
    /// carries packets, each on its own slot
    /// (spec/decisions/settlement-methods.md). The original gave a peer that
    /// was not a member an address beneath its own that nothing routed to,
    /// and stored the addresses its peers gave it without reading them.
    pub(super) fn add_peer_endpoint(&mut self, node: NodeId) -> Result<(), String> {
        if self.peer_endpoints.contains_key(&node) {
            return Ok(());
        }
        self.peer.peering_of(&node).ok_or("not peered")?;
        // A downlink's accounts open with the first packet that moves money
        // to it, or its first settlement
        // (spec/decisions/lazy-downlink-accounts.md); a core peer's are
        // there, or its method gets no slot.
        let first = self
            .wanted_slots(&node)
            .into_iter()
            .next()
            .ok_or("no method of the peer's has a scheme and an account")?;
        let (ledger, account) = (first.ledger, first.account);
        let id = self.next_peer_endpoint;
        self.next_peer_endpoint += 1;
        let address = if self.peer.is_downlink(&node) {
            format!("{}.{node}", self.connector.own_address())
        } else {
            format!("{}.das.{node}", self.realm.ilp_allocation_scheme())
        };
        let first = self.rng.next_u32();
        self.connector
            .add_peer(EndpointId::Peer(id), &address, account, ledger, first)
            .map_err(|e| e.to_string())?;
        self.peer_endpoints.insert(node, id);
        self.endpoint_peers.insert(id, node);
        if !self.peer.reachable(&node) {
            self.connector.set_unreached(EndpointId::Peer(id), true);
        }
        // The further methods, and each slot's flags: a packet on a method
        // over a hub that timed out waits for the hub before it is rejected
        // upstream (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
        self.sync_slots_of(node);
        Ok(())
    }

    /// Gives the connector the peer node's routes, as prefixes and peer
    /// endpoints.
    pub(super) fn update_routes(&mut self) {
        let scheme = self.realm.ilp_allocation_scheme();
        let routes: Vec<(String, Vec<EndpointId>)> = self
            .peer
            .routes()
            .iter()
            .map(|(destination, route)| {
                let endpoints = route
                    .first_hops
                    .iter()
                    .filter_map(|hop| self.peer_endpoints.get(hop))
                    .map(|id| EndpointId::Peer(*id))
                    .collect();
                (format!("{scheme}.das.{destination}"), endpoints)
            })
            .filter(|(_, hops): &(String, Vec<EndpointId>)| !hops.is_empty())
            .collect();
        let routes = routes
            .iter()
            .map(|(prefix, hops)| (prefix.as_str(), hops.as_slice()));
        // The table holds a route per node the peer node knows
        // (`check_bounds`), none of them below our own address.
        let taken = self.connector.set_routes(routes);
        assert!(taken.is_ok(), "routes not taken: {taken:?}");
        let children = self
            .peer
            .is_periphery()
            .then(|| self.child_addresses(false));
        // One per uplink, each beneath another node's address.
        let taken = self.connector.set_periphery(children.as_deref());
        assert!(taken.is_ok(), "child addresses not taken: {taken:?}");
    }

    /// A periphery node's addresses beneath its uplinks, each
    /// `<uplink's address>.<our ID>`, which ILDCP tells it there
    /// (spec/areas/connector.md#a-periphery-node): of every peering, or,
    /// if `connected`, of those whose uplink is confirmed now, best first.
    pub(super) fn child_addresses(&self, connected: bool) -> Vec<String> {
        self.peer
            .core_peers()
            .filter(|(id, _)| !connected || self.peer.is_advertised(id))
            .map(|(id, _)| self.child_address(id))
            .collect()
    }

    /// The address beneath `uplink`, `<uplink's address>.<our ID>`: the
    /// one rule [`Self::child_addresses`] and [`Self::address_beneath`]
    /// share, so the two never disagree.
    fn child_address(&self, uplink: &NodeId) -> String {
        format!(
            "{}.das.{uplink}.{}",
            self.realm.ilp_allocation_scheme(),
            self.peer.own()
        )
    }

    /// Where a periphery node is paid beneath the uplink its owner chose
    /// (spec/areas/payments.md#receiving): only one confirmed now, as
    /// [`Self::receiving_address`] would pick among.
    ///
    /// # Errors
    ///
    /// A core node, which is paid at its own address, or `uplink` is not
    /// an uplink confirmed now.
    pub(super) fn address_beneath(&self, uplink: &NodeId) -> Result<String, EngineError> {
        if !self.peer.is_periphery() {
            return Err(EngineError::Precondition(
                "a core node is paid at its own address, beneath no uplink".into(),
            ));
        }
        if !self.peer.is_advertised(uplink) {
            return Err(EngineError::Precondition(format!(
                "{uplink} is not a confirmed uplink"
            )));
        }
        let address = self.child_address(uplink);
        // A confirmed uplink is a core peering, so the connector holds the
        // address: what a payment's packets reach it at.
        assert!(
            self.child_addresses(false).contains(&address),
            "the address beneath a confirmed uplink is not the node's"
        );
        Ok(address)
    }

    /// Where a periphery node is paid now: beneath an uplink it is
    /// connected to, if any. `None` on a core node, which is paid at its
    /// own address.
    pub(super) fn receiving_address(&self) -> Option<String> {
        if !self.peer.is_periphery() {
            return None;
        }
        let connected = self.child_addresses(true).into_iter().next();
        connected.or_else(|| self.child_addresses(false).into_iter().next())
    }
}

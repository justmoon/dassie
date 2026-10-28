//! The connector: endpoints, packets in flight, and the trace.

mod hops;
mod invariants;
mod prepare;
mod response;
mod swaps;

pub use response::{Posted, Responded};
pub use swaps::{SWAP_PREFIX, swap_scheme};

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use dassie_ilp::{Address, ErrorCode};
use dassie_ledger::{AccountId, Ledger, LedgerId, TraceId, TransferId};

use crate::PacketTransfers;
use crate::cap::{CapError, CapState, CapUsage, CapView, SpendingCap};
use crate::config::Config;
use crate::endpoint::{
    Endpoint, EndpointError, EndpointId, Endpoints, MAX_HELD_SLOTS, MAX_SLOTS, Slot, SlotError,
    SlotView,
};
use crate::trace::{PacketRecord, Trace, TraceEvent};

/// Most next hops a route lists.
pub const MAX_ROUTE_OPTIONS: usize = 4;

/// Most addresses a periphery node has beneath its uplinks: one per
/// uplink, and it has no more peers than this. As many as the uplinks a
/// periphery node may be pinned to, `dassie_peer::MAX_UPLINKS`, which the
/// node checks at start.
pub const MAX_CHILD_ADDRESSES: usize = 128;

/// Why routes could not be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutesError {
    /// More routes than the table holds.
    TooMany,
    /// A prefix is not an ILP address, or lies below our own.
    InvalidPrefix,
}

/// What became of an incoming Prepare. The packet to send is in the buffer
/// the caller passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Send the Prepare in the buffer on.
    Forward(Forwarded),
    /// Answer the source at once, with the request ID it used: the buffer
    /// holds a Fulfill (ILDCP) or a Reject of this many bytes.
    Respond(usize),
    /// The source reused a request ID that is still in flight. Nothing was
    /// done; the transport should report the error its own way.
    Duplicate,
    /// The source is not an endpoint (any more). Nothing was done.
    UnknownSource,
}

/// A Prepare to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forwarded {
    /// The endpoint to send it to.
    pub to: EndpointId,
    /// The ledger of its slot the packet goes on: for a peer, the
    /// settlement method to send it on.
    pub ledger: LedgerId,
    /// The request ID to send it with; the response comes back with it.
    pub request_id: u32,
    /// The packet's length in the buffer.
    pub length: usize,
}

/// A response to send back to where a Prepare came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// The endpoint the Prepare came from.
    pub to: EndpointId,
    /// The ledger of its slot the Prepare came in on: for a peer, the
    /// settlement method it came on.
    pub ledger: LedgerId,
    /// The request ID it came with.
    pub request_id: u32,
    /// The response's length in the buffer.
    pub length: usize,
}

/// Counters, for the owner's status (spec/areas/node.md#the-dassie-command)
/// and simulation checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Prepares received.
    pub prepares: u64,
    /// Prepares forwarded.
    pub forwarded: u64,
    /// Prepares answered at once (ILDCP or a reject).
    pub answered: u64,
    /// Forwarded packets fulfilled.
    pub fulfilled: u64,
    /// Forwarded packets rejected downstream.
    pub rejected: u64,
    /// Forwarded packets that expired, or whose next hop went away.
    pub expired: u64,
    /// Fulfillments that did not match the condition (F05).
    pub wrong_fulfillments: u64,
    /// Responses for no packet in flight: late, duplicate or bogus.
    pub unexpected_responses: u64,
    /// Prepares that reused a request ID in flight.
    pub duplicates: u64,
}

/// A packet in flight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pending {
    pub trace: TraceId,
    pub source: EndpointId,
    pub source_request_id: u32,
    /// The ledger of the source's slot it came in on, and of the
    /// destination's slot it went out on: what it books on, and what the
    /// money in flight on each slot counts it in.
    pub in_ledger: LedgerId,
    pub out_ledger: LedgerId,
    /// The source is gone: the response has nowhere to go, but the
    /// transfers are still settled by it.
    pub source_gone: bool,
    pub condition: [u8; 32],
    /// What it carries in from its source, and out to its destination.
    pub amount_in: u64,
    pub amount_out: u64,
    /// The outgoing packet's expiry.
    pub expires_at_ms: u64,
    pub transfers: PacketTransfers,
    /// Set when the packet cannot reach its next hop: the code and message
    /// it is rejected with at once.
    pub failure: Option<(ErrorCode, &'static str)>,
    /// The account it spends from and the amount, if it moves money out of
    /// its owner's funds: what a spending cap counts.
    pub charge: Option<(AccountId, u64)>,
    /// For a packet to a peer over a hub, how long it may wait for the hub
    /// once its own expiry passed: the incoming packet's expiry, and at
    /// most one message window past its own.
    pub backstop_ms: Option<u64>,
    /// It expired and waits for the hub to decide it.
    pub awaiting_hub: bool,
    /// What its Fulfill earns its next hop toward the draw: our fee, if
    /// the packet kept its ledger (spec/decisions/weighted-next-hops.md).
    pub earns: u64,
}

/// Opens a peer's accounts for the first packet that moves money with it,
/// for a peer added without them: the node's, which keeps the room its
/// ledger has for peers' accounts (spec/decisions/lazy-downlink-accounts.md).
pub trait OpensAccounts {
    /// Opens the accounts of the peer at `endpoint` on `on` in `ledger`
    /// for a packet to it, and returns the account its packets debit and
    /// credit, or nothing if there is no room for them.
    fn open(
        &mut self,
        now_ms: u64,
        ledger: &mut Ledger,
        endpoint: EndpointId,
        on: LedgerId,
    ) -> Option<AccountId>;

    /// Takes back what [`open`](Self::open) just did for `endpoint`: the
    /// ledger refused the packet it opened them for.
    fn take_back(&mut self, ledger: &mut Ledger, endpoint: EndpointId, on: LedgerId);
}

/// For a connector none of whose peers lacks its accounts: opens nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpensNone;

impl OpensAccounts for OpensNone {
    fn open(&mut self, _: u64, _: &mut Ledger, _: EndpointId, _: LedgerId) -> Option<AccountId> {
        None
    }

    // It opens nothing, so it has nothing to take back.
    fn take_back(&mut self, _: &mut Ledger, _: EndpointId, _: LedgerId) {}
}

/// The connector.
#[derive(Debug)]
pub struct Connector {
    config: Config,
    own_address: String,
    endpoints: Endpoints,
    /// Routes beyond our own address: prefix to next hops, best first.
    routes: BTreeMap<String, [Option<EndpointId>; MAX_ROUTE_OPTIONS]>,
    /// By destination and outgoing request ID.
    pending: BTreeMap<(EndpointId, u32), Pending>,
    /// When each packet in flight expires.
    deadlines: BTreeSet<(u64, EndpointId, u32)>,
    /// Incoming request IDs in flight, by source.
    incoming: BTreeSet<(EndpointId, u32)>,
    trace: Trace,
    next_sequence: u64,
    stats: Stats,
    /// Reused for linked transfers, so the packet path does not allocate.
    scratch: Vec<TransferId>,
    /// Spending caps, by the account they limit.
    caps: BTreeMap<AccountId, CapState>,
    /// Caps whose usage changed since it was last polled.
    cap_changes: BTreeSet<AccountId>,
    /// On a periphery node, its addresses beneath its uplinks: what lies
    /// below one of them is routed as if below our own address, and
    /// nothing else from a peer is forwarded
    /// (spec/areas/connector.md#a-periphery-node). `None` on a core node.
    periphery: Option<Vec<String>>,
    /// Reused for a destination written below our own address, so the
    /// packet path does not allocate.
    rewritten: String,
    /// Packets to peers over a hub that expired, for the hub's scheme to
    /// watch, oldest first.
    hub_waits: alloc::collections::VecDeque<HubWait>,
    /// Whether the owner lets the node offer swaps to its peers: on by
    /// default; a periphery node never does
    /// (spec/areas/connector.md#swaps).
    offer_swaps: bool,
    /// The node's swap endpoints, one per ledger at most: what its own
    /// swaps are sent from, and what they reach when they come back.
    swap_endpoints: BTreeMap<LedgerId, EndpointId>,
}

/// A packet to a peer over a hub that expired without a response. Its
/// Reject upstream waits until the hub has a final block stamped at or
/// after its commitment's expiry: by then the payee either won a claim for
/// it, and it is fulfilled, or cannot any more, and it is rejected
/// ([`Connector::settle_hub`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubWait {
    /// The peer it went to.
    pub destination: EndpointId,
    /// The ledger of the method it went on.
    pub ledger: LedgerId,
    /// The request ID it went with.
    pub request_id: u32,
    /// Its condition.
    pub condition: [u8; 32],
    /// The latest it can be answered upstream: after that it is rejected
    /// whatever the hub says.
    pub backstop_ms: u64,
}

impl Connector {
    /// A connector for the node at `own_address`.
    ///
    /// # Panics
    ///
    /// If the config does not validate or the address is not one.
    #[must_use]
    pub fn new(config: Config, own_address: &str) -> Self {
        config.validate();
        assert!(Address::parse(own_address).is_ok(), "not an ILP address");
        Self {
            config,
            own_address: String::from(own_address),
            endpoints: Endpoints::default(),
            routes: BTreeMap::new(),
            pending: BTreeMap::new(),
            deadlines: BTreeSet::new(),
            incoming: BTreeSet::new(),
            trace: Trace::new(config.trace_capacity, config.max_events),
            next_sequence: 0,
            stats: Stats::default(),
            scratch: Vec::with_capacity(3),
            caps: BTreeMap::new(),
            cap_changes: BTreeSet::new(),
            periphery: None,
            rewritten: String::new(),
            hub_waits: alloc::collections::VecDeque::new(),
            offer_swaps: true,
            swap_endpoints: BTreeMap::new(),
        }
    }

    /// Makes the connector a periphery node's, with its addresses beneath
    /// its uplinks (spec/areas/connector.md#a-periphery-node): packets below
    /// one of them reach the node's own endpoints, written below its own
    /// address, and a peer's packet for anywhere else gets F02, since a
    /// periphery node forwards nothing. With `None`, it is a core node's
    /// again.
    ///
    /// # Errors
    ///
    /// Too many addresses, or one that is not an address, lies below our
    /// own, or overlaps another; nothing changes then.
    pub fn set_periphery(&mut self, addresses: Option<&[String]>) -> Result<(), RoutesError> {
        let Some(addresses) = addresses else {
            self.periphery = None;
            return Ok(());
        };
        if addresses.len() > MAX_CHILD_ADDRESSES {
            return Err(RoutesError::TooMany);
        }
        let own = Address::parse(&self.own_address).map_err(|_| RoutesError::InvalidPrefix)?;
        // Each parsed once: with an address beneath each of many uplinks,
        // every pair is compared.
        let parsed = addresses
            .iter()
            .map(|text| Address::parse(text))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| RoutesError::InvalidPrefix)?;
        for (index, address) in parsed.iter().copied().enumerate() {
            let nested = parsed
                .iter()
                .skip(index + 1)
                .any(|other| other.has_prefix(address) || address.has_prefix(*other));
            if address.has_prefix(own) || nested {
                return Err(RoutesError::InvalidPrefix);
            }
        }
        self.periphery = Some(addresses.to_vec());
        Ok(())
    }

    /// The node's addresses beneath its uplinks, if it is a periphery node.
    #[must_use]
    pub fn child_addresses(&self) -> Option<&[String]> {
        self.periphery.as_deref()
    }

    /// What follows a periphery node's address beneath an uplink in
    /// `destination`, if it lies below one: the same as below our own.
    pub(crate) fn below_child_address<'d>(&self, destination: Address<'d>) -> Option<&'d str> {
        self.periphery
            .iter()
            .flatten()
            .filter_map(|text| Address::parse(text).ok())
            .find_map(|address| destination.strip_prefix(address))
    }

    /// The node's ILP address.
    #[must_use]
    pub fn own_address(&self) -> &str {
        &self.own_address
    }

    /// Adds an endpoint at `address`, which must be below the node's own and
    /// not overlap another endpoint's, with the account its packets use.
    ///
    /// # Errors
    ///
    /// If the table is full, the ID is taken, or the address is unusable.
    pub fn add_endpoint(
        &mut self,
        id: EndpointId,
        address: &str,
        account: AccountId,
        ledger: LedgerId,
    ) -> Result<(), EndpointError> {
        if id.is_peer() {
            return Err(EndpointError::InvalidAddress);
        }
        if self.endpoints.len() >= self.config.max_endpoints {
            return Err(EndpointError::Full);
        }
        let own = Address::parse(&self.own_address).map_err(|_| EndpointError::InvalidAddress)?;
        let endpoint = Endpoint::new(address, Slot::new(ledger, Some(account)), 0);
        self.endpoints.insert(id, endpoint, own)
    }

    /// Adds a peer at `address`, which is only what ILDCP tells it: packets
    /// reach peers through the route table. It starts with one slot, its
    /// peering's first settlement method on `ledger`; further methods are
    /// [`add_slot`](Self::add_slot)s. Its first request ID is chosen by the
    /// caller at random, so that after a restart our new packets do not
    /// reuse IDs the peer still holds from before. A slot without `account`
    /// has its accounts opened by the first packet that moves money with it
    /// ([`OpensAccounts`]).
    ///
    /// # Errors
    ///
    /// If the table is full, the ID is taken or not a peer's, or the address
    /// is not one.
    pub fn add_peer(
        &mut self,
        id: EndpointId,
        address: &str,
        account: Option<AccountId>,
        ledger: LedgerId,
        first_request_id: u32,
    ) -> Result<(), EndpointError> {
        if !id.is_peer() {
            return Err(EndpointError::Duplicate);
        }
        if self.endpoints.len() >= self.config.max_endpoints {
            return Err(EndpointError::Full);
        }
        let own = Address::parse(&self.own_address).map_err(|_| EndpointError::InvalidAddress)?;
        let endpoint = Endpoint::new(address, Slot::new(ledger, account), first_request_id);
        self.endpoints.insert(id, endpoint, own)
    }

    /// Adds a slot on `ledger` to a peer: a further settlement method of its
    /// peering, on a ledger none of its slots is on
    /// (spec/decisions/settlement-methods.md). A slot on `ledger` that was
    /// removed and still waits for its packets is taken up again, with
    /// them, and moves last, as a method added again comes last in its
    /// peering. A new slot is available, and not over a hub, until the node
    /// says otherwise; one taken up again is available, and keeps whether
    /// it is over a hub, which its packets in flight still need.
    ///
    /// # Errors
    ///
    /// No such peer, a slot on `ledger` already, [`MAX_SLOTS`] in use, or
    /// [`MAX_HELD_SLOTS`] with the removed ones.
    ///
    /// # Panics
    ///
    /// If a slot taken up again had another account.
    pub fn add_slot(
        &mut self,
        id: EndpointId,
        ledger: LedgerId,
        account: Option<AccountId>,
    ) -> Result<(), SlotError> {
        let endpoint = self
            .endpoints
            .get_mut(id)
            .filter(|_| id.is_peer())
            .ok_or(SlotError::Unknown)?;
        let at = endpoint.slots.iter().position(|slot| slot.ledger == ledger);
        if at.is_some_and(|at| endpoint.slots.get(at).is_some_and(|slot| !slot.removed)) {
            return Err(SlotError::Duplicate);
        }
        let in_use = endpoint.slots.iter().filter(|slot| !slot.removed).count();
        if in_use >= MAX_SLOTS || (at.is_none() && endpoint.slots.len() >= MAX_HELD_SLOTS) {
            return Err(SlotError::Full);
        }
        let Some(at) = at else {
            endpoint.slots.push(Slot::new(ledger, account));
            return Ok(());
        };
        let mut slot = endpoint.slots.remove(at);
        assert!(
            slot.account.is_none() || account.is_none() || slot.account == account,
            "a slot's account never changes"
        );
        slot.removed = false;
        slot.available = true;
        slot.account = slot.account.or(account);
        endpoint.slots.push(slot);
        Ok(())
    }

    /// Removes a peer's slot on `ledger`: no new packet takes it, either
    /// way. Its packets in flight resolve, or expire, as any packet does,
    /// and the slot goes with the last of them, within the longest hold
    /// time. Returns whether it had the slot.
    pub fn remove_slot(&mut self, id: EndpointId, ledger: LedgerId) -> bool {
        let Some(endpoint) = self.endpoints.get_mut(id).filter(|_| id.is_peer()) else {
            return false;
        };
        let Some(slot) = endpoint.slot_mut(ledger).filter(|slot| !slot.removed) else {
            return false;
        };
        slot.removed = true;
        endpoint
            .slots
            .retain(|slot| !slot.removed || slot.pending > 0);
        true
    }

    /// Lets the node send packets on a peer's slot on `ledger`, or not
    /// (spec/decisions/settlement-methods.md): which method a packet takes
    /// depends on the methods available, never on their room. Returns
    /// whether it has the slot.
    pub fn set_available(&mut self, id: EndpointId, ledger: LedgerId, available: bool) -> bool {
        if !id.is_peer() {
            return false;
        }
        self.endpoints
            .slot_mut(id, ledger)
            .map(|slot| slot.available = available)
            .is_some()
    }

    /// An endpoint's slots, in the order they were added, removed ones that
    /// wait for their packets included.
    pub fn slots(&self, id: EndpointId) -> impl Iterator<Item = SlotView> + '_ {
        self.endpoints
            .get(id)
            .into_iter()
            .flat_map(|endpoint| endpoint.slots.iter().map(Slot::view))
    }

    /// Replaces the route table: for each prefix outside our own address,
    /// its next hops, best first. Hops that are not endpoints (any more)
    /// are skipped when routing.
    ///
    /// # Errors
    ///
    /// Too many routes, or a prefix that is not a valid one; the table is
    /// then unchanged.
    pub fn set_routes<'a>(
        &mut self,
        routes: impl IntoIterator<Item = (&'a str, &'a [EndpointId])>,
    ) -> Result<(), RoutesError> {
        let own = Address::parse(&self.own_address).map_err(|_| RoutesError::InvalidPrefix)?;
        let mut table = BTreeMap::new();
        for (prefix, hops) in routes {
            let address = Address::parse(prefix).map_err(|_| RoutesError::InvalidPrefix)?;
            if address.has_prefix(own) {
                return Err(RoutesError::InvalidPrefix);
            }
            if table.len() >= self.config.max_routes {
                return Err(RoutesError::TooMany);
            }
            let mut options = [None; MAX_ROUTE_OPTIONS];
            for (slot, hop) in options.iter_mut().zip(hops) {
                *slot = Some(*hop);
            }
            table.insert(String::from(prefix), options);
        }
        self.routes = table;
        Ok(())
    }

    /// Fails a forwarded packet that surely did not reach its next hop:
    /// it is rejected upstream with `code` at once, through
    /// [`handle_timeout`](Self::handle_timeout), and its transfers voided.
    /// Use it only when the next hop cannot have acted on the packet; when
    /// unsure, let it expire: a next hop that fulfilled a packet we already
    /// failed would bear its loss. Returns whether the packet was in
    /// flight.
    pub fn fail_forwarded(
        &mut self,
        now_ms: u64,
        to: EndpointId,
        request_id: u32,
        code: ErrorCode,
        message: &'static str,
    ) -> bool {
        let Some(entry) = self.pending.get_mut(&(to, request_id)) else {
            return false;
        };
        self.deadlines
            .remove(&(entry.expires_at_ms, to, request_id));
        entry.failure = Some((code, message));
        entry.expires_at_ms = now_ms;
        entry.awaiting_hub = false;
        self.deadlines.insert((now_ms, to, request_id));
        true
    }

    /// Removes an endpoint. Packets in flight to it are failed with T01
    /// at once, through [`handle_timeout`](Self::handle_timeout); their
    /// transfers are voided. A child may already have fulfilled them: the
    /// loss is on the child's own account, since we never got the
    /// fulfillment. Packets it sent keep running, and their responses are
    /// dropped. Returns whether it existed.
    pub fn remove_endpoint(&mut self, now_ms: u64, id: EndpointId) -> bool {
        if self.endpoints.remove(id).is_none() {
            return false;
        }
        self.swap_endpoints.retain(|_, swap| *swap != id);
        let doomed: Vec<(u64, EndpointId, u32)> = self
            .pending
            .iter()
            .filter(|((destination, _), _)| *destination == id)
            .map(|(&(destination, request_id), entry)| {
                (entry.expires_at_ms, destination, request_id)
            })
            .collect();
        for (expires_at_ms, destination, request_id) in doomed {
            self.deadlines
                .remove(&(expires_at_ms, destination, request_id));
            if let Some(entry) = self.pending.get_mut(&(destination, request_id)) {
                entry.failure = Some((ErrorCode::T01_PEER_UNREACHABLE, "next hop went away"));
                entry.expires_at_ms = now_ms;
                entry.awaiting_hub = false;
            }
            self.deadlines.insert((now_ms, destination, request_id));
        }
        for entry in self.pending.values_mut() {
            if entry.source == id {
                entry.source_gone = true;
            }
        }
        self.incoming.retain(|(source, _)| *source != id);
        true
    }

    /// The account the packets on `id`'s first slot debit and credit, if
    /// it has one yet: a child's only one.
    #[must_use]
    pub fn account_of(&self, id: EndpointId) -> Option<AccountId> {
        self.endpoints
            .get(id)
            .and_then(Endpoint::first_slot)
            .and_then(|slot| slot.account)
    }

    /// The account the packets on `id`'s slot on `ledger` debit and
    /// credit, if it has the slot and the account yet.
    #[must_use]
    pub fn account_on(&self, id: EndpointId, ledger: LedgerId) -> Option<AccountId> {
        self.endpoints
            .slot(id, ledger)
            .and_then(|slot| slot.account)
    }

    /// Gives a peer's slot on `ledger` that had none the account its
    /// packets debit and credit, opened by the node outside a packet, as
    /// for a settlement (spec/decisions/lazy-downlink-accounts.md). Returns
    /// whether the slot exists.
    ///
    /// # Panics
    ///
    /// If the slot already has another account.
    pub fn open_account(&mut self, id: EndpointId, ledger: LedgerId, account: AccountId) -> bool {
        self.endpoints
            .slot_mut(id, ledger)
            .map(|slot| {
                assert!(
                    slot.account.is_none_or(|had| had == account),
                    "a slot's account never changes"
                );
                slot.account = Some(account);
            })
            .is_some()
    }

    /// Marks a peer's slot on `ledger` as a method over a settlement hub, or
    /// not. Returns whether it is a peer's slot.
    pub fn set_hub(&mut self, id: EndpointId, ledger: LedgerId, hub: bool) -> bool {
        if !id.is_peer() {
            return false;
        }
        self.endpoints
            .slot_mut(id, ledger)
            .map(|slot| slot.hub = hub)
            .is_some()
    }

    /// The next packet to a peer over a hub that expired and waits for the
    /// hub, oldest first.
    pub fn poll_hub_wait(&mut self) -> Option<HubWait> {
        self.hub_waits.pop_front()
    }

    /// Stops, or resumes, new Prepares to and from an endpoint, whose
    /// peering is ending: packets in flight finish, and their responses
    /// still flow. Returns whether the endpoint exists.
    pub fn set_draining(&mut self, id: EndpointId, draining: bool) -> bool {
        self.endpoints
            .get_mut(id)
            .map(|endpoint| endpoint.draining = draining)
            .is_some()
    }

    /// Stops, or resumes, new Prepares from an endpoint, while Prepares to
    /// it still go: a peering we are ending, and have not told the peer of
    /// yet (spec/decisions/ending-a-peering.md). Its Prepares are refused as
    /// for want of liquidity, which tells it nothing more. Returns whether
    /// the endpoint exists.
    pub fn set_refusing(&mut self, id: EndpointId, refusing: bool) -> bool {
        self.endpoints
            .get_mut(id)
            .map(|endpoint| endpoint.refusing = refusing)
            .is_some()
    }

    /// Stops, or resumes, new Prepares from a peer that our messages
    /// surely do not reach, while Prepares to it still go
    /// (spec/decisions/unreachable-peers.md): a Fulfill of ours could not
    /// count in its books, and would only drift ours from its. Its Prepares
    /// are refused as for want of liquidity, which is temporary. Independent
    /// of [`set_refusing`](Self::set_refusing): either refuses. Returns
    /// whether the endpoint exists.
    pub fn set_unreached(&mut self, id: EndpointId, unreached: bool) -> bool {
        self.endpoints
            .get_mut(id)
            .map(|endpoint| endpoint.unreached = unreached)
            .is_some()
    }

    /// Packets in flight to or from an endpoint: once none are left, a
    /// draining endpoint can go with no packet's outcome left unknown.
    #[must_use]
    pub fn pending_for(&self, id: EndpointId) -> usize {
        let from = self
            .endpoints
            .get(id)
            .map_or(0, |endpoint| endpoint.pending_from);
        let to = self.pending.range((id, 0)..=(id, u32::MAX)).count();
        from + to
    }

    /// The most packets in flight from `id`: a peer that may carry other
    /// nodes' traffic gets a peer's share; a child, or a downlink, which
    /// sends only its own, a child's.
    pub(crate) fn max_pending_from(&self, id: EndpointId) -> usize {
        let transit = id.is_peer() && self.endpoints.get(id).is_none_or(|e| !e.child);
        if transit {
            self.config.max_pending_per_peer
        } else {
            self.config.max_pending_per_endpoint
        }
    }

    /// Packets in flight that came from an endpoint.
    #[must_use]
    pub fn pending_from(&self, id: EndpointId) -> usize {
        self.endpoints
            .get(id)
            .map_or(0, |endpoint| endpoint.pending_from)
    }

    /// Limits what `account` may spend out of its owner's funds, from
    /// `now_ms` on (spec/areas/connector.md#spending-caps), or lifts its
    /// cap. A new cap starts with nothing used; a changed one keeps what was
    /// used, rounded up to the new window. Packets in flight count against
    /// it either way.
    ///
    /// # Errors
    ///
    /// If the cap is invalid, or the table of caps is full.
    pub fn set_cap(
        &mut self,
        now_ms: u64,
        account: AccountId,
        cap: Option<SpendingCap>,
    ) -> Result<(), CapError> {
        let Some(cap) = cap else {
            self.caps.remove(&account);
            self.cap_changes.remove(&account);
            return Ok(());
        };
        let usage = match self.caps.get(&account) {
            Some(state) => state.usage.rescaled(state.cap, cap, now_ms),
            None => CapUsage::new(now_ms),
        };
        self.insert_cap(account, cap, usage)
    }

    /// Restores a cap with the usage persisted for it.
    ///
    /// # Errors
    ///
    /// As [`set_cap`](Self::set_cap).
    pub fn restore_cap(
        &mut self,
        account: AccountId,
        cap: SpendingCap,
        usage: CapUsage,
    ) -> Result<(), CapError> {
        self.insert_cap(account, cap, usage)
    }

    fn insert_cap(
        &mut self,
        account: AccountId,
        cap: SpendingCap,
        usage: CapUsage,
    ) -> Result<(), CapError> {
        if !cap.is_valid() {
            return Err(CapError::Invalid);
        }
        if !self.caps.contains_key(&account) && self.caps.len() >= self.config.max_caps {
            return Err(CapError::Full);
        }
        let reserved = self
            .pending
            .values()
            .filter_map(|entry| entry.charge)
            .filter(|(charged, _)| *charged == account)
            .map(|(_, amount)| u128::from(amount))
            .sum();
        let mut state = CapState {
            cap,
            usage,
            reserved,
            over: false,
        };
        state.over = !state.within();
        self.caps.insert(account, state);
        Ok(())
    }

    /// The cap on `account` as of `now_ms`, if it has one.
    #[must_use]
    pub fn cap(&self, account: AccountId, now_ms: u64) -> Option<CapView> {
        self.caps.get(&account).map(|state| state.view(now_ms))
    }

    /// The next cap whose usage changed, with its cap and usage: what to
    /// persist with the step's journal.
    pub fn poll_cap_change(&mut self) -> Option<(AccountId, SpendingCap, CapUsage)> {
        while let Some(account) = self.cap_changes.pop_first() {
            if let Some(state) = self.caps.get(&account) {
                return Some((account, state.cap, state.usage));
            }
        }
        None
    }

    /// Every endpoint with a slot: its ID, address and its first slot's
    /// ledger, a child's only one.
    pub fn endpoints(&self) -> impl Iterator<Item = (EndpointId, &str, LedgerId)> {
        self.endpoints.iter().filter_map(|(id, endpoint)| {
            let slot = endpoint.first_slot()?;
            Some((*id, endpoint.address.as_str(), slot.ledger))
        })
    }

    /// The ledger of the slot a packet in flight to `to` with
    /// `request_id` went out on: for a peer, its settlement method.
    #[must_use]
    pub fn forwarded_on(&self, to: EndpointId, request_id: u32) -> Option<LedgerId> {
        self.pending
            .get(&(to, request_id))
            .map(|entry| entry.out_ledger)
    }

    /// The address an endpoint was added with.
    #[must_use]
    pub fn endpoint_address(&self, id: EndpointId) -> Option<&str> {
        self.endpoints
            .get(id)
            .map(|endpoint| endpoint.address.as_str())
    }

    /// The earliest deadline of a packet in flight.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<u64> {
        self.deadlines.first().map(|&(at, _, _)| at)
    }

    /// Packets in flight.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// The next trace event for live subscribers.
    pub fn poll_event(&mut self) -> Option<TraceEvent> {
        self.trace.poll_event()
    }

    /// The trace's records after `after`, oldest first.
    pub fn packets_after(&self, after: Option<TraceId>) -> impl Iterator<Item = &PacketRecord> {
        self.trace.records_after(after)
    }

    /// Counters.
    #[must_use]
    pub const fn stats(&self) -> Stats {
        self.stats
    }

    /// The bounds.
    #[must_use]
    pub const fn config(&self) -> Config {
        self.config
    }

    fn next_trace(&mut self) -> TraceId {
        let trace = TraceId {
            epoch: self.config.epoch,
            sequence: self.next_sequence,
        };
        self.next_sequence += 1;
        trace
    }

    /// Where a packet to `destination` from one of the node's own
    /// endpoints would go first, and the ledger of the slot it would take:
    /// what such a packet must be paid from, since the node's own endpoints
    /// never convert. Not drawn: the route's first usable hop, by the order
    /// the routes list them, with a method available on `ledger`, the one
    /// the owner asks to pay from; with no such hop, or none asked for, the
    /// first usable hop and its first available method. So a quote or a
    /// payment picks its ledger once (spec/decisions/weighted-next-hops.md);
    /// each of its packets then draws among the hops with a method on that
    /// ledger.
    #[must_use]
    pub fn first_hop(
        &self,
        destination: &str,
        ledger: Option<LedgerId>,
    ) -> Option<(EndpointId, LedgerId)> {
        let address = Address::parse(destination).ok()?;
        match self.next_hop(0, address, (None, ledger), None) {
            hops::Hop::To(hop, Some(ledger)) => Some((hop, ledger)),
            hops::Hop::To(_, None) | hops::Hop::NoMethod | hops::Hop::Back | hops::Hop::None => {
                None
            }
        }
    }

    /// The routes, for inspection.
    pub fn routes(&self) -> impl Iterator<Item = (&str, impl Iterator<Item = EndpointId> + '_)> {
        self.routes
            .iter()
            .map(|(prefix, options)| (prefix.as_str(), options.iter().flatten().copied()))
    }

    /// A request ID for `destination` that is not in flight to it.
    fn request_id_for(&mut self, destination: EndpointId) -> Option<u32> {
        let endpoint = self.endpoints.get_mut(destination)?;
        // At most `max_pending` IDs are taken, so one of the next
        // `pending + 1` candidates is free.
        for _ in 0..=self.pending.len() {
            let candidate = endpoint.next_request_id;
            endpoint.next_request_id = candidate.wrapping_add(1);
            if !self.pending.contains_key(&(destination, candidate)) {
                return Some(candidate);
            }
        }
        None
    }
}

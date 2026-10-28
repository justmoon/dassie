//! Endpoints, their addresses, their settlement-method slots and accounts,
//! and routing.

use alloc::collections::BTreeMap;
use alloc::string::String;
use core::fmt;

use dassie_ilp::Address;
use dassie_ledger::{AccountId, LedgerId};

/// Most settlement-method slots a peer's endpoint has: one per method of
/// its peering, so as many as a peering has methods, the peer protocol's
/// `MAX_METHODS`, which the node checks at start
/// (spec/areas/connector.md#addresses-and-endpoints).
pub const MAX_SLOTS: usize = 16;

/// Most slots a peer's endpoint holds, removed ones that wait for their
/// packets included: a method dropped and another added in its place
/// before the dropped one's packets resolve leaves a peering at
/// [`MAX_SLOTS`] methods with more slots than that, each removed one gone
/// within the longest hold time.
pub const MAX_HELD_SLOTS: usize = 2 * MAX_SLOTS;

/// Something the connector exchanges packets with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EndpointId {
    /// A BTP connection, by the node's number for it.
    Btp(u64),
    /// An in-process endpoint, such as the STREAM receiver.
    Local(u16),
    /// A peer node, by the node's number for the peering: never reused, so
    /// a late response cannot resolve a later peering's packet.
    Peer(u64),
}

impl EndpointId {
    /// Whether it is a peer, rather than a child of the node.
    #[must_use]
    pub const fn is_peer(self) -> bool {
        matches!(self, Self::Peer(_))
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Btp(id) => write!(f, "btp:{id}"),
            Self::Local(id) => write!(f, "local:{id}"),
            Self::Peer(id) => write!(f, "peer:{id}"),
        }
    }
}

/// Why an endpoint could not be added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointError {
    /// The endpoint table is full.
    Full,
    /// An endpoint with that ID exists.
    Duplicate,
    /// The address is not a valid ILP address below the node's own, or is
    /// taken.
    InvalidAddress,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Full => "too many endpoints",
            Self::Duplicate => "the endpoint exists",
            Self::InvalidAddress => "the address is invalid or taken",
        })
    }
}

/// Why a slot could not be added to a peer's endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotError {
    /// There is no such peer endpoint.
    Unknown,
    /// It has a slot on that ledger.
    Duplicate,
    /// It has [`MAX_SLOTS`] slots in use, or [`MAX_HELD_SLOTS`] with the
    /// removed ones that wait for their packets.
    Full,
}

impl fmt::Display for SlotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unknown => "no such peer endpoint",
            Self::Duplicate => "the peer has a slot on that ledger",
            Self::Full => "too many settlement methods",
        })
    }
}

/// One of an endpoint's settlement methods: a ledger, the account its
/// packets debit and credit there, and what is in flight on it. A child of
/// the node has one; a peer one per method of its peering
/// (spec/decisions/settlement-methods.md), each on a ledger of its own.
#[derive(Clone, Debug)]
pub(crate) struct Slot {
    /// Its ledger.
    pub ledger: LedgerId,
    /// The account its packets debit and credit. Only a peer's may have
    /// none yet: a downlink's, whose accounts open with the first packet
    /// that moves money with it (spec/decisions/lazy-downlink-accounts.md).
    pub account: Option<AccountId>,
    /// Packets in flight on it, either way: a slot removed while it has
    /// some stays until they are resolved.
    pub pending: usize,
    /// If a peer's, the money its packets in flight carry to us on it
    /// (spec/decisions/packets-in-flight.md).
    pub money_from: u64,
    /// If a peer's, the money our packets in flight carry to it on it.
    pub money_to: u64,
    /// It is a method over a settlement hub: a packet on it that times out
    /// is rejected upstream only once the hub has decided it
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
    pub hub: bool,
    /// The node may send packets on it: its scheme runs and sees its
    /// ledger, and the method is not withdrawn
    /// (spec/decisions/settlement-methods.md). Only which method a packet
    /// takes reads it; a Prepare from the peer on it is taken either way.
    pub available: bool,
    /// It was removed, and waits for its packets in flight to resolve: no
    /// new packet takes it.
    pub removed: bool,
    /// If a peer's, the fees we earned on packets fulfilled through it that
    /// kept their ledger, its own, as of `earned_since_ms`, halved for
    /// every half-life since (spec/decisions/weighted-next-hops.md): its
    /// weight in drawing next hops, per peer and ledger. Only in memory.
    pub earned: u64,
    /// When the half-life `earned` is in began.
    pub earned_since_ms: u64,
}

impl Slot {
    /// A slot on `ledger`, booking on `account`, with nothing in flight.
    pub const fn new(ledger: LedgerId, account: Option<AccountId>) -> Self {
        Self {
            ledger,
            account,
            pending: 0,
            money_from: 0,
            money_to: 0,
            hub: false,
            available: true,
            removed: false,
            earned: 0,
            earned_since_ms: 0,
        }
    }

    /// Whether a new packet may go out on it.
    pub const fn usable(&self) -> bool {
        self.available && !self.removed
    }

    /// How the node sees it.
    pub const fn view(&self) -> SlotView {
        SlotView {
            ledger: self.ledger,
            account: self.account,
            hub: self.hub,
            available: self.available,
            removed: self.removed,
            pending: self.pending,
            money_from: self.money_from,
            money_to: self.money_to,
        }
    }
}

/// A slot as the node sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotView {
    /// Its ledger.
    pub ledger: LedgerId,
    /// The account its packets debit and credit, if open.
    pub account: Option<AccountId>,
    /// Whether it is a method over a settlement hub.
    pub hub: bool,
    /// Whether the node may send packets on it.
    pub available: bool,
    /// Whether it was removed, and waits for its packets in flight.
    pub removed: bool,
    /// Its packets in flight, either way.
    pub pending: usize,
    /// The money its packets in flight from the peer carry.
    pub money_from: u64,
    /// The money our packets in flight to the peer on it carry.
    pub money_to: u64,
}

/// An endpoint's registration.
#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    /// Its ILP address: what ILDCP hands out, and the route to it.
    pub address: String,
    /// Its settlement methods, in the order they were added, one taken up
    /// again counting as added then: one for a child, up to [`MAX_SLOTS`]
    /// in use for a peer, and [`MAX_HELD_SLOTS`] with those removed, each
    /// on a ledger of its own.
    pub slots: alloc::vec::Vec<Slot>,
    /// The next outgoing request ID to try: unique per endpoint, whatever
    /// the slot.
    pub next_request_id: u32,
    /// Packets from it in flight.
    pub pending_from: usize,
    /// It takes no new Prepares, either way: its peering is ending.
    pub draining: bool,
    /// It takes no new Prepares from it, but still goes to it: we are
    /// ending its peering, and have not told it yet
    /// (spec/decisions/ending-a-peering.md).
    pub refusing: bool,
    /// It takes no new Prepares from it, but still goes to it: our
    /// messages to it surely do not arrive, so a Fulfill of ours could not
    /// count in its books (spec/decisions/unreachable-peers.md).
    pub unreached: bool,
    /// Its address lies below the node's own, which routes to it: a child
    /// of the node, or a peer that is our downlink
    /// (spec/areas/connector.md#addresses-and-endpoints). Set when added.
    pub child: bool,
    /// Packets reach it by its address. A swap endpoint's are not: only
    /// the node's own swaps coming back reach it
    /// (spec/areas/connector.md#swaps).
    pub routed: bool,
}

impl Endpoint {
    /// An endpoint at `address` with its first slot.
    pub fn new(address: &str, first: Slot, next_request_id: u32) -> Self {
        Self {
            address: String::from(address),
            slots: alloc::vec![first],
            next_request_id,
            pending_from: 0,
            draining: false,
            refusing: false,
            unreached: false,
            child: false,
            routed: true,
        }
    }

    /// Its slot on `ledger`, removed or not.
    pub fn slot(&self, ledger: LedgerId) -> Option<&Slot> {
        self.slots.iter().find(|slot| slot.ledger == ledger)
    }

    /// The same, to change it.
    pub fn slot_mut(&mut self, ledger: LedgerId) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|slot| slot.ledger == ledger)
    }

    /// Its first slot not removed: a child's only one.
    pub fn first_slot(&self) -> Option<&Slot> {
        self.slots.iter().find(|slot| !slot.removed)
    }

    /// Its first slot a new packet may go out on.
    pub fn first_usable(&self) -> Option<&Slot> {
        self.slots.iter().find(|slot| slot.usable())
    }

    /// Its slot on `ledger`, if a new packet may go out on it.
    pub fn usable_on(&self, ledger: LedgerId) -> Option<&Slot> {
        self.slot(ledger).filter(|slot| slot.usable())
    }
}

/// The endpoints, by ID. A child's address, below the node's own, is its
/// route, and so is a downlink's; any other peer's address is only what
/// ILDCP tells it, and routes to such peers come from the route table.
#[derive(Debug, Default)]
pub(crate) struct Endpoints {
    by_id: BTreeMap<EndpointId, Endpoint>,
}

impl Endpoints {
    pub fn get(&self, id: EndpointId) -> Option<&Endpoint> {
        self.by_id.get(&id)
    }

    pub fn get_mut(&mut self, id: EndpointId) -> Option<&mut Endpoint> {
        self.by_id.get_mut(&id)
    }

    /// The slot of `id` on `ledger`, removed or not.
    pub fn slot(&self, id: EndpointId, ledger: LedgerId) -> Option<&Slot> {
        self.get(id).and_then(|endpoint| endpoint.slot(ledger))
    }

    /// The same, to change it.
    pub fn slot_mut(&mut self, id: EndpointId, ledger: LedgerId) -> Option<&mut Slot> {
        self.get_mut(id)
            .and_then(|endpoint| endpoint.slot_mut(ledger))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&EndpointId, &Endpoint)> {
        self.by_id.iter()
    }

    pub fn insert(
        &mut self,
        id: EndpointId,
        mut endpoint: Endpoint,
        own: Address<'_>,
    ) -> Result<(), EndpointError> {
        if self.by_id.contains_key(&id) {
            return Err(EndpointError::Duplicate);
        }
        let address =
            Address::parse(&endpoint.address).map_err(|_| EndpointError::InvalidAddress)?;
        endpoint.child = address.strip_prefix(own).is_some();
        let child_clash = (!id.is_peer() && !endpoint.child)
            || (endpoint.child
                && self
                    .by_id
                    .values()
                    .any(|e| e.child && overlaps(&e.address, &endpoint.address)));
        if child_clash {
            return Err(EndpointError::InvalidAddress);
        }
        self.by_id.insert(id, endpoint);
        Ok(())
    }

    pub fn remove(&mut self, id: EndpointId) -> Option<Endpoint> {
        self.by_id.remove(&id)
    }

    /// The child whose address is a whole-segment prefix of `destination`.
    /// Children's addresses never nest, so at most one fits. A
    /// downlink whose peering is ending takes no new Prepare.
    pub fn route_child(&self, destination: Address<'_>) -> Option<EndpointId> {
        self.by_id
            .iter()
            .filter(|(_, endpoint)| endpoint.child && endpoint.routed && !endpoint.draining)
            .find(|(_, endpoint)| {
                Address::parse(&endpoint.address)
                    .is_ok_and(|address| destination.has_prefix(address))
            })
            .map(|(&id, _)| id)
    }

    /// The child below `own` whose address, after `own`, is a whole-segment
    /// prefix of `below`: what `own.<below>` routes to, without writing it
    /// out.
    pub fn route_child_below(&self, own: &str, below: &str) -> Option<EndpointId> {
        self.by_id
            .iter()
            .filter(|(_, endpoint)| endpoint.child && endpoint.routed && !endpoint.draining)
            .find(|(_, endpoint)| {
                endpoint
                    .address
                    .strip_prefix(own)
                    .and_then(|rest| rest.strip_prefix('.'))
                    .and_then(|segments| below.strip_prefix(segments))
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
            })
            .map(|(&id, _)| id)
    }
}

/// Whether one address is a segment prefix of the other: then routing
/// could not tell them apart.
fn overlaps(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (Address::parse(a), Address::parse(b)) else {
        return true;
    };
    a.has_prefix(b) || b.has_prefix(a)
}

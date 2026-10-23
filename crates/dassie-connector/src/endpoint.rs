//! Endpoints, their addresses and accounts, and routing.

use alloc::collections::BTreeMap;
use alloc::string::String;
use core::fmt;

use dassie_ilp::Address;
use dassie_ledger::{AccountId, LedgerId};

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

/// An endpoint's registration.
#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    /// Its ILP address: what ILDCP hands out, and the route to it.
    pub address: String,
    /// The account its packets debit and credit.
    pub account: AccountId,
    /// That account's ledger.
    pub ledger: LedgerId,
    /// The next outgoing request ID to try.
    pub next_request_id: u32,
    /// Packets from it in flight.
    pub pending_from: usize,
    /// If a peer, the money its packets in flight carry to us
    /// (spec/decisions/packets-in-flight.md).
    pub money_from: u64,
    /// If a peer, the money our packets in flight carry to it.
    pub money_to: u64,
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
    /// It is a peer we peer with over a settlement hub: a packet to it that
    /// times out is rejected upstream only once the hub has decided it
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
    pub hub: bool,
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

    /// The child whose address is a whole-segment prefix of `destination`
    /// (NET-33). Children's addresses never nest, so at most one fits. A
    /// downlink whose peering is ending takes no new Prepare.
    pub fn route_child(&self, destination: Address<'_>) -> Option<EndpointId> {
        self.by_id
            .iter()
            .filter(|(_, endpoint)| endpoint.child && !endpoint.draining)
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
            .filter(|(_, endpoint)| endpoint.child && !endpoint.draining)
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

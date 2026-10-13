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
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Btp(id) => write!(f, "btp:{id}"),
            Self::Local(id) => write!(f, "local:{id}"),
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
}

/// The endpoints, by ID. Every endpoint's address is its route: the
/// connector's endpoints are all children of the node in M3; routes learned
/// from peers come in M4.
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
        endpoint: Endpoint,
        own: Address<'_>,
    ) -> Result<(), EndpointError> {
        if self.by_id.contains_key(&id) {
            return Err(EndpointError::Duplicate);
        }
        let address =
            Address::parse(&endpoint.address).map_err(|_| EndpointError::InvalidAddress)?;
        if address.strip_prefix(own).is_none()
            || self
                .by_id
                .values()
                .any(|other| overlaps(&other.address, &endpoint.address))
        {
            return Err(EndpointError::InvalidAddress);
        }
        self.by_id.insert(id, endpoint);
        Ok(())
    }

    pub fn remove(&mut self, id: EndpointId) -> Option<Endpoint> {
        self.by_id.remove(&id)
    }

    /// The endpoint whose address is the longest whole-segment prefix of
    /// `destination` (NET-33). Addresses never nest, so at most one fits.
    pub fn route(&self, destination: Address<'_>) -> Option<EndpointId> {
        self.by_id
            .iter()
            .find(|(_, endpoint)| {
                Address::parse(&endpoint.address)
                    .is_ok_and(|address| destination.has_prefix(address))
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

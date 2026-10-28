//! The bounds on peer requests in flight
//! (spec/areas/peer-protocol.md#outbound-requests): in total, and to each
//! node, by its ID. They are the peer protocol's alone: requests to servers
//! that are not nodes have bounds of their own ([`crate::outbound`]), so
//! neither crowds the other out.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use dassie_identity::NodeId;
use dassie_peer::{Failure, Request};

use crate::outbound::literal_is_private;

/// Most peer requests in flight, in total and to one node.
const MAX_IN_FLIGHT: usize = 128;
const MAX_PER_NODE: usize = 16;

/// The peer requests in flight.
#[derive(Debug, Default)]
pub(super) struct RequestBounds {
    in_flight: Arc<Mutex<InFlight>>,
}

#[derive(Debug, Default)]
struct InFlight {
    total: usize,
    by_node: BTreeMap<NodeId, usize>,
}

/// A request's place in the bounds, given back when it ends.
pub(super) struct Slot {
    in_flight: Arc<Mutex<InFlight>>,
    node: NodeId,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        in_flight.total = in_flight.total.saturating_sub(1);
        if let Some(count) = in_flight.by_node.get_mut(&self.node) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                in_flight.by_node.remove(&self.node);
            }
        }
    }
}

impl RequestBounds {
    /// Takes `request`'s place in the bounds; `None` for a packet, which
    /// takes none: ILP packets are bounded by the connector's tables, and
    /// their responses must get through.
    ///
    /// # Errors
    ///
    /// [`Failure::NotDelivered`] when the endpoint names an IP literal that
    /// is not public and `allow_private` is not set, or the bounds are full.
    pub(super) fn hold(
        &self,
        request: &Request,
        allow_private: bool,
    ) -> Result<Option<Slot>, Failure> {
        // A periphery node has no endpoint, and so no address to check: it
        // is reached only over the connection it opened to us.
        if let Some(endpoint) = &request.endpoint
            && !allow_private
            && literal_is_private(&endpoint.to_string())
        {
            return Err(Failure::NotDelivered);
        }
        if request.kind.carries_packet() {
            return Ok(None);
        }
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let for_node = in_flight.by_node.get(&request.to).copied().unwrap_or(0);
        if in_flight.total >= MAX_IN_FLIGHT || for_node >= MAX_PER_NODE {
            return Err(Failure::NotDelivered);
        }
        in_flight.total += 1;
        in_flight.by_node.insert(request.to, for_node + 1);
        Ok(Some(Slot {
            in_flight: Arc::clone(&self.in_flight),
            node: request.to,
        }))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn request(seed: u8, url: &str) -> Request {
        Request {
            id: dassie_peer::RequestId(0),
            to: dassie_identity::IdentityKey::from_bytes(&[seed; 32]).node_id(),
            endpoint: dassie_peer::wire::Endpoint::of_url(url),
            kind: dassie_peer::wire::Kind::LinkStateUpdate,
            body: Vec::new(),
            timeout_ms: 1_000,
        }
    }

    // Each node has a bound of its own, by its ID: periphery nodes, which
    // have no endpoint, as much as nodes that share one host. And a node
    // without an endpoint is not refused as a private address.
    #[test]
    fn each_node_has_its_own_bound() {
        let bounds = RequestBounds::default();
        let mut held = Vec::new();
        for seed in 1..=u8::try_from(MAX_PER_NODE).unwrap() + 1 {
            held.push(bounds.hold(&request(seed, ""), false).unwrap());
            held.push(
                bounds
                    .hold(&request(seed, "https://a.example"), false)
                    .unwrap(),
            );
        }
        for _ in 2..MAX_PER_NODE {
            held.push(bounds.hold(&request(1, ""), false).unwrap());
        }
        assert_eq!(
            bounds.hold(&request(1, "https://a.example"), false).err(),
            Some(Failure::NotDelivered)
        );
        // A place given back is free again.
        held.pop();
        assert!(bounds.hold(&request(1, ""), false).is_ok());
    }

    #[test]
    fn a_private_literal_is_refused_unless_allowed() {
        let bounds = RequestBounds::default();
        let private = request(1, "https://127.0.0.1:8443");
        assert_eq!(
            bounds.hold(&private, false).err(),
            Some(Failure::NotDelivered)
        );
        assert!(bounds.hold(&private, true).is_ok());
    }

    #[test]
    fn packets_take_no_place() {
        let bounds = RequestBounds::default();
        let mut packet = request(1, "");
        packet.kind = dassie_peer::wire::Kind::InterledgerPacket;
        for _ in 0..=MAX_IN_FLIGHT {
            assert!(bounds.hold(&packet, false).unwrap().is_none());
        }
    }
}

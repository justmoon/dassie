//! Swaps with peers (spec/decisions/peer-swaps.md,
//! spec/areas/connector.md#swaps).
//!
//! A swap is a Prepare to `peer.swap.<scheme ID>`. A node sends it to a peer
//! on one method of their peering, from its swap endpoint on that method's
//! ledger, as the owner's own funds that never convert. The peer takes it
//! before routing, as every address under `peer.` is the link's: it converts
//! it as transit, by its curve, fee and spread, and sends it back on the
//! method the address names, to the same peer, with its destination,
//! condition and data unchanged. Back at the node, a `peer.swap.<scheme>`
//! that comes on the method it names is one of its own swaps coming back,
//! and goes to its swap endpoint on that ledger, which decides it. The two
//! legs meet only through their condition, so the connector keeps nothing
//! per swap beyond its packets in flight.

use dassie_ilp::{Address, ErrorCode, MAX_PACKET_LENGTH, Prepare};
use dassie_ledger::{AccountId, Ledger, LedgerId};
use dassie_oer::Decode;

use super::prepare::{Refusal, Route, refuse};
use super::{Connector, OpensAccounts, Outcome};
use crate::endpoint::{EndpointError, EndpointId};
use crate::pricing::PriceTable;
use crate::trace::PacketRecord;

/// What a swap's destination starts with; the one segment after it is the
/// ID of the scheme of the method it is to come back on.
pub const SWAP_PREFIX: Address<'static> = dassie_ilp::address!("peer.swap");

/// The scheme a swap's destination names, the method it is to come back
/// on: the one segment after [`SWAP_PREFIX`], or nothing if it is not a
/// swap's.
#[must_use]
pub fn swap_scheme(destination: Address<'_>) -> Option<&str> {
    destination
        .strip_prefix(SWAP_PREFIX)
        .filter(|scheme| !scheme.contains('.'))
}

/// Whether `destination` is under `peer.`, which ILP reserves for the link
/// it is sent on: never routed.
pub(super) fn is_link_local(destination: Address<'_>) -> bool {
    destination.as_str().starts_with("peer.")
}

/// A ledger's slug, the ID of the scheme that serves it, which names its
/// method in a swap's destination (spec/areas/ledger.md#ledgers-and-amounts).
fn scheme_of(ledger: LedgerId) -> &'static str {
    let id = ledger.as_str();
    id.split_once('+').map_or(id, |(slug, _)| slug)
}

impl Connector {
    /// Lets the node offer swaps to its peers, or not: the owner's setting,
    /// on by default. A periphery node never offers them, whatever it says
    /// ([`offers_swaps`](Self::offers_swaps)).
    pub const fn set_offer_swaps(&mut self, offer: bool) {
        self.offer_swaps = offer;
    }

    /// Whether the node takes its peers' swaps: the owner lets it, and it
    /// is a core node, since a periphery node forwards nothing.
    #[must_use]
    pub const fn offers_swaps(&self) -> bool {
        self.offer_swaps && self.periphery.is_none()
    }

    /// Adds the node's swap endpoint on `ledger`, a local endpoint at
    /// `address`, below the node's own, that spends from and is paid into
    /// `account`, an account of the owner's group: the node's own swaps on
    /// `ledger` go out from it, and come back to it. No packet reaches it
    /// by its address; only a swap of the node's coming back on `ledger`
    /// does.
    ///
    /// # Errors
    ///
    /// As [`add_endpoint`](Self::add_endpoint), or `ledger` has a swap
    /// endpoint already.
    pub fn add_swap_endpoint(
        &mut self,
        id: EndpointId,
        address: &str,
        account: AccountId,
        ledger: LedgerId,
    ) -> Result<(), EndpointError> {
        if self.swap_endpoints.contains_key(&ledger) {
            return Err(EndpointError::Duplicate);
        }
        self.add_endpoint(id, address, account, ledger)?;
        if let Some(endpoint) = self.endpoints.get_mut(id) {
            endpoint.routed = false;
        }
        self.swap_endpoints.insert(ledger, id);
        Ok(())
    }

    /// The node's swap endpoint on `ledger`, if it has one.
    #[must_use]
    pub fn swap_endpoint(&self, ledger: LedgerId) -> Option<EndpointId> {
        self.swap_endpoints.get(&ledger).copied()
    }

    /// The ledger whose swap endpoint `id` is, if it is one.
    #[must_use]
    pub fn swap_ledger(&self, id: EndpointId) -> Option<LedgerId> {
        self.swap_endpoints
            .iter()
            .find(|(_, swap)| **swap == id)
            .map(|(ledger, _)| *ledger)
    }

    /// Sends one of the node's own swaps: a Prepare from `source`, its
    /// swap endpoint on a ledger, with the request ID it chose, to the peer
    /// `to` on its method on that ledger, addressed to
    /// `peer.swap.<scheme>`, the scheme of another method of the peering,
    /// which it is to come back on. It is the owner's own funds, booked
    /// from the swap endpoint's account, and never converts here; the peer
    /// converts it. What the connector does with it is as for any Prepare:
    /// forwarded, or answered at once with a Reject.
    ///
    /// The checks are a Prepare's, in the same order: room for it (T03,
    /// T05); the peering has a method on the ledger, and the address names
    /// another of its methods (F02); the peering is not ending (T01), and
    /// its method on the ledger is available (T04); then expiry, the largest packet, the money in
    /// flight with the peer and the books.
    ///
    /// # Panics
    ///
    /// As [`handle_prepare`](Self::handle_prepare).
    #[allow(clippy::too_many_arguments)]
    pub fn send_swap(
        &mut self,
        now_ms: u64,
        (source, request_id): (EndpointId, u32),
        to: EndpointId,
        packet: &[u8],
        ledger: &mut Ledger,
        prices: &PriceTable,
        accounts: &mut dyn OpensAccounts,
        out: &mut [u8],
    ) -> Outcome {
        assert!(
            out.len() >= MAX_PACKET_LENGTH,
            "buffer shorter than a packet"
        );
        let Some(on) = self.swap_ledger(source) else {
            return Outcome::UnknownSource;
        };
        if self.incoming.contains(&(source, request_id)) {
            self.stats.duplicates += 1;
            return Outcome::Duplicate;
        }
        self.stats.prepares += 1;
        let trace = self.next_trace();
        let Ok(prepare) = Prepare::decode_exact(packet) else {
            self.trace
                .push(PacketRecord::new(trace, source, "", 0, now_ms));
            let refusal = refuse(ErrorCode::F01_INVALID_PACKET, "invalid packet");
            return self.answer(now_ms, trace, &refusal, out);
        };
        let destination = prepare.destination.as_str();
        let record = PacketRecord::new(trace, source, destination, prepare.amount, now_ms);
        self.trace.push(record);
        let route = match self.route_own_swap(now_ms, (source, on), to, &prepare, prices, ledger) {
            Ok(route) => route,
            Err(refusal) => return self.answer(now_ms, trace, &refusal, out),
        };
        self.book_and_forward(
            now_ms,
            (trace, source, request_id),
            &prepare,
            &route,
            (ledger, accounts),
            out,
        )
    }

    /// The route of one of the node's own swaps from its swap endpoint
    /// `source` on `on` to the peer `to`, on its method on `on`.
    fn route_own_swap(
        &self,
        now_ms: u64,
        (source, on): (EndpointId, LedgerId),
        to: EndpointId,
        prepare: &Prepare<'_>,
        prices: &PriceTable,
        ledger: &Ledger,
    ) -> Result<Route, Refusal> {
        self.has_room(source)?;
        let peer = self
            .endpoints
            .get(to)
            .filter(|_| to.is_peer())
            .ok_or(refuse(ErrorCode::F02_UNREACHABLE, "no such peer"))?;
        let has = |scheme: &str| {
            peer.slots
                .iter()
                .any(|slot| !slot.removed && scheme_of(slot.ledger) == scheme)
        };
        let back = swap_scheme(prepare.destination);
        let names_another = back.is_some_and(|back| back != scheme_of(on) && has(back));
        if !names_another || !has(scheme_of(on)) {
            return Err(refuse(
                ErrorCode::F02_UNREACHABLE,
                "not another settlement method of the peering",
            ));
        }
        if peer.draining {
            return Err(refuse(
                ErrorCode::T01_PEER_UNREACHABLE,
                "the peering is ending",
            ));
        }
        if peer.usable_on(on).is_none() {
            return Err(refuse(
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "no settlement method available",
            ));
        }
        self.route_onto(
            now_ms,
            (source, on),
            (to, Some(on)),
            prepare,
            prices,
            ledger,
        )
    }

    /// The route of a Prepare from `source` on `in_ledger` addressed under
    /// `peer.`, other than ILDCP's: only a swap, from a peer, goes
    /// anywhere, and only back to that peer.
    ///
    /// - **On the method it names,** it is one of the node's own swaps
    ///   coming back: it goes to the swap endpoint on that ledger, as a
    ///   packet into the owner's own endpoints, unconverted and uncharged.
    /// - **Naming another method of the peering,** it is the peer's swap,
    ///   if the node offers swaps: transit from the method it came on to
    ///   the one it names, converted by the curve and charged the fee and
    ///   spread, as any transit is. It never earns the peer weight in the
    ///   draw, since it converts.
    ///
    /// Every other address under `peer.`, a swap from a child, a scheme
    /// that is not a method of the peering, or a node that offers no swaps
    /// is F02; a method named that is not available T04.
    pub(super) fn route_swap(
        &self,
        now_ms: u64,
        (source, in_ledger): (EndpointId, LedgerId),
        prepare: &Prepare<'_>,
        prices: &PriceTable,
        ledger: &Ledger,
    ) -> Result<Route, Refusal> {
        self.has_room(source)?;
        let from = (source, in_ledger);
        let scheme = swap_scheme(prepare.destination)
            .filter(|_| source.is_peer())
            .ok_or(refuse(ErrorCode::F02_UNREACHABLE, "no such address"))?;
        if scheme == scheme_of(in_ledger) {
            let to = self.swap_endpoint(in_ledger).ok_or(refuse(
                ErrorCode::F02_UNREACHABLE,
                "no swaps on this method",
            ))?;
            return self.route_onto(now_ms, from, (to, Some(in_ledger)), prepare, prices, ledger);
        }
        if !self.offers_swaps() {
            return Err(refuse(ErrorCode::F02_UNREACHABLE, "no swaps offered"));
        }
        let peer = self
            .endpoints
            .get(source)
            .ok_or(refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error"))?;
        let back = peer
            .slots
            .iter()
            .find(|slot| !slot.removed && scheme_of(slot.ledger) == scheme)
            .ok_or(refuse(
                ErrorCode::F02_UNREACHABLE,
                "not a settlement method of the peering",
            ))?;
        if !back.usable() {
            return Err(refuse(
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "no settlement method available",
            ));
        }
        let route = self.route_onto(
            now_ms,
            from,
            (source, Some(back.ledger)),
            prepare,
            prices,
            ledger,
        )?;
        assert!(route.converts(), "a peer's swap changes its ledger");
        Ok(route)
    }

    /// Each swap endpoint is a local endpoint that no packet reaches by its
    /// address, on the ledger it is the swap endpoint of.
    pub(super) fn check_swaps(&self) {
        for (ledger, id) in &self.swap_endpoints {
            assert!(!id.is_peer(), "a swap endpoint is the node's own");
            let endpoint = self.endpoints.get(*id);
            assert!(endpoint.is_some(), "a swap endpoint exists");
            if let Some(endpoint) = endpoint {
                assert!(
                    !endpoint.routed,
                    "no packet reaches a swap endpoint by address"
                );
                assert_eq!(
                    endpoint.first_slot().map(|slot| slot.ledger),
                    Some(*ledger),
                    "a swap endpoint on its ledger"
                );
            }
        }
        assert!(
            self.swap_endpoints.len() <= self.config.max_endpoints,
            "swap endpoints within bounds"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swaps_address_names_one_scheme() {
        let parse = |text| Address::parse(text).unwrap();
        assert_eq!(swap_scheme(parse("peer.swap.stub")), Some("stub"));
        assert_eq!(
            swap_scheme(parse("peer.swap.hub-xrpl-testnet-xrp")),
            Some("hub-xrpl-testnet-xrp")
        );
        assert_eq!(swap_scheme(parse("peer.swap.stub.more")), None);
        assert_eq!(swap_scheme(parse("peer.swapped.stub")), None);
        assert_eq!(swap_scheme(parse("peer.config")), None);
        assert!(is_link_local(parse("peer.anything")));
        assert!(!is_link_local(parse("test.peer.swap")));
    }

    #[test]
    fn a_ledgers_scheme_is_its_slug() {
        assert_eq!(scheme_of(LedgerId::StubUsd), "stub");
        assert_eq!(
            scheme_of(LedgerId::HubXrplTestnetXrp),
            "hub-xrpl-testnet-xrp"
        );
    }
}

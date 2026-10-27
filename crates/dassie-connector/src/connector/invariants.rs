//! What holds after every step (spec/decisions/tigerstyle.md).

use alloc::collections::BTreeMap;

use dassie_ledger::{AccountId, Ledger, LedgerId, Reference};

use super::Connector;
use crate::endpoint::{EndpointId, MAX_HELD_SLOTS, MAX_SLOTS};

/// What the packets in flight count on one slot: packets either way, the
/// money from it, and the money to it with how many packets carry some.
#[derive(Clone, Copy, Debug, Default)]
struct OnSlot {
    pending: usize,
    money_from: u64,
    money_to: u64,
    carrying_to: usize,
}

impl Connector {
    /// Asserts the connector's invariants, and that its packets' transfers
    /// are pending in `ledger` with their trace IDs:
    ///
    /// - the deadline and incoming tables match the packets in flight;
    /// - each endpoint's count of packets in flight from it is right; each
    ///   has a slot per ledger at most, a child one, a peer at most
    ///   [`MAX_SLOTS`] in use and [`MAX_HELD_SLOTS`] with those removed;
    ///   each packet is on a slot of its source and of its
    ///   destination, and each slot's count of its packets in flight is
    ///   right, a removed slot having some; and each peer's money in flight
    ///   either way on each slot, within its limit
    ///   (spec/decisions/packets-in-flight.md,
    ///   spec/decisions/settlement-methods.md);
    /// - only peers earn weight in drawing next hops, and a packet earns
    ///   its next hop at most what came in, only between peers and only on
    ///   the ledger it came in on (spec/decisions/weighted-next-hops.md);
    /// - the bounds hold, and the trace is in order.
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self, ledger: &Ledger) {
        self.check_bounds();
        assert_eq!(self.deadlines.len(), self.pending.len(), "deadlines");
        let mut from: BTreeMap<EndpointId, usize> = BTreeMap::new();
        let mut on_slots: BTreeMap<(EndpointId, LedgerId), OnSlot> = BTreeMap::new();
        let mut live_incoming = 0;
        for (&(destination, request_id), entry) in &self.pending {
            assert!(
                self.deadlines
                    .contains(&(entry.expires_at_ms, destination, request_id)),
                "a packet without its deadline"
            );
            assert!(
                !entry.awaiting_hub || entry.backstop_ms == Some(entry.expires_at_ms),
                "a packet waits for the hub past its backstop"
            );
            if !entry.source_gone {
                live_incoming += 1;
                assert!(
                    self.incoming
                        .contains(&(entry.source, entry.source_request_id)),
                    "a packet without its incoming request ID"
                );
                *from.entry(entry.source).or_default() += 1;
                let on = on_slots.entry((entry.source, entry.in_ledger)).or_default();
                on.pending += 1;
                on.money_from += entry.amount_in;
            }
            let on = on_slots.entry((destination, entry.out_ledger)).or_default();
            on.pending += 1;
            on.money_to += entry.amount_out;
            on.carrying_to += usize::from(entry.amount_out > 0);
            for id in entry.transfers.into_iter().flatten() {
                let transfer = ledger.pending(id);
                assert!(
                    transfer.is_some_and(|t| t.reference == Reference::Packet(entry.trace)),
                    "a packet's transfer is not pending with its trace"
                );
            }
        }
        assert_eq!(self.incoming.len(), live_incoming, "incoming request IDs");
        self.check_endpoints(&from, &on_slots);
        self.check_caps();
        self.check_earnings();
        self.trace.check_invariants();
    }

    /// Each endpoint's count of packets in flight from it, and each of its
    /// slots' packets and money in flight, are what the packets in flight
    /// say, within bounds; and every packet is on a slot of its endpoints.
    fn check_endpoints(
        &self,
        from: &BTreeMap<EndpointId, usize>,
        on_slots: &BTreeMap<(EndpointId, LedgerId), OnSlot>,
    ) {
        let mut slots_seen = 0;
        for (id, endpoint) in self.endpoints.iter() {
            let expected = from.get(id).copied().unwrap_or(0);
            assert_eq!(
                endpoint.pending_from, expected,
                "packets in flight from {id}"
            );
            assert!(endpoint.pending_from <= self.max_pending_from(*id));
            let (in_use, most) = if id.is_peer() {
                (MAX_SLOTS, MAX_HELD_SLOTS)
            } else {
                (1, 1)
            };
            assert!(endpoint.slots.len() <= most, "slots of {id} within bounds");
            assert!(
                endpoint.slots.iter().filter(|slot| !slot.removed).count() <= in_use,
                "slots of {id} in use within bounds"
            );
            for (index, slot) in endpoint.slots.iter().enumerate() {
                assert!(
                    endpoint
                        .slots
                        .iter()
                        .skip(index + 1)
                        .all(|other| other.ledger != slot.ledger),
                    "a slot of {id} per ledger"
                );
                let on = on_slots
                    .get(&(*id, slot.ledger))
                    .copied()
                    .unwrap_or_default();
                slots_seen += usize::from(on_slots.contains_key(&(*id, slot.ledger)));
                assert_eq!(slot.pending, on.pending, "packets on a slot of {id}");
                assert!(!slot.removed || slot.pending > 0, "a removed slot waits");
                assert!(id.is_peer() || slot.usable(), "a child's slot is usable");
                if !id.is_peer() {
                    assert_eq!((slot.money_from, slot.money_to), (0, 0));
                    continue;
                }
                let max = self.config.max_in_flight();
                assert_eq!(slot.money_from, on.money_from, "money from {id}");
                assert_eq!(slot.money_to, on.money_to, "money to {id}");
                assert!(slot.money_from <= max, "money from {id} within bounds");
                assert!(
                    slot.money_to <= max || on.carrying_to == 1,
                    "money to {id} within bounds"
                );
            }
        }
        // Every packet is on a slot of its endpoints, but those of a
        // source that went away.
        let gone = on_slots
            .keys()
            .filter(|(id, _)| self.endpoints.get(*id).is_none())
            .count();
        assert_eq!(slots_seen + gone, on_slots.len(), "a packet on no slot");
    }

    /// The packets in flight, endpoints and routes stay within their
    /// tables' bounds.
    fn check_bounds(&self) {
        assert!(self.pending.len() <= self.config.max_pending);
        assert!(self.endpoints.len() <= self.config.max_endpoints);
        assert!(
            self.routes.len() <= self.config.max_routes,
            "routes within bounds"
        );
    }

    /// Only peers earn weight in drawing next hops, and only packets from
    /// one peer to another earn it, at most what came in: the fee we
    /// charge on transit (spec/decisions/weighted-next-hops.md).
    fn check_earnings(&self) {
        for (id, endpoint) in self.endpoints.iter() {
            assert!(
                id.is_peer() || endpoint.slots.iter().all(|slot| slot.earned == 0),
                "only peers earn weight"
            );
        }
        for (&(destination, _), entry) in &self.pending {
            assert!(
                entry.earns == 0
                    || (entry.source.is_peer()
                        && destination.is_peer()
                        && entry.in_ledger == entry.out_ledger
                        && entry.earns <= entry.amount_in),
                "a packet earns its next hop only its fee on transit"
            );
        }
    }

    /// Each cap holds what its account's packets in flight charge, and
    /// stays within itself unless it was lowered while they were.
    fn check_caps(&self) {
        assert!(
            self.caps.len() <= self.config.max_caps,
            "caps within bounds"
        );
        let mut held: BTreeMap<AccountId, u128> = BTreeMap::new();
        for (account, amount) in self.pending.values().filter_map(|entry| entry.charge) {
            *held.entry(account).or_default() += u128::from(amount);
        }
        for (account, cap) in &self.caps {
            let expected = held.get(account).copied().unwrap_or(0);
            assert_eq!(cap.reserved, expected, "a cap holds its packets in flight");
            assert!(cap.cap.is_valid(), "a cap is valid");
            assert!(cap.over || cap.within(), "a cap holds no more than itself");
        }
        assert!(
            self.cap_changes.iter().all(|id| self.caps.contains_key(id)),
            "changes of caps that exist"
        );
    }
}

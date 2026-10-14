//! What holds after every step (D16).

use alloc::collections::BTreeMap;

use dassie_ledger::{AccountId, Ledger, Reference};

use super::Connector;
use crate::endpoint::EndpointId;

impl Connector {
    /// Asserts the connector's invariants, and that its packets' transfers
    /// are pending in `ledger` with their trace IDs:
    ///
    /// - the deadline and incoming tables match the packets in flight;
    /// - each endpoint's count of packets in flight from it is right;
    /// - the bounds hold, and the trace is in order.
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self, ledger: &Ledger) {
        assert!(self.pending.len() <= self.config.max_pending);
        assert!(self.endpoints.len() <= self.config.max_endpoints);
        assert_eq!(self.deadlines.len(), self.pending.len(), "deadlines");
        let mut from: BTreeMap<EndpointId, usize> = BTreeMap::new();
        let mut live_incoming = 0;
        for (&(destination, request_id), entry) in &self.pending {
            assert!(
                self.deadlines
                    .contains(&(entry.expires_at_ms, destination, request_id)),
                "a packet without its deadline"
            );
            if !entry.source_gone {
                live_incoming += 1;
                assert!(
                    self.incoming
                        .contains(&(entry.source, entry.source_request_id)),
                    "a packet without its incoming request ID"
                );
                *from.entry(entry.source).or_default() += 1;
            }
            for id in entry.transfers.into_iter().flatten() {
                let transfer = ledger.pending(id);
                assert!(
                    transfer.is_some_and(|t| t.reference == Reference::Packet(entry.trace)),
                    "a packet's transfer is not pending with its trace"
                );
            }
        }
        assert_eq!(self.incoming.len(), live_incoming, "incoming request IDs");
        for (id, endpoint) in self.endpoints.iter() {
            let expected = from.get(id).copied().unwrap_or(0);
            assert_eq!(
                endpoint.pending_from, expected,
                "packets in flight from {id}"
            );
            assert!(endpoint.pending_from <= self.config.max_pending_from(*id));
        }
        assert!(
            self.routes.len() <= self.config.max_routes,
            "routes within bounds"
        );
        self.check_caps();
        self.trace.check_invariants();
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

//! The packet trace: a bounded record of recent packets and what became of
//! them, which the journal's packet references point into.

use alloc::collections::VecDeque;
use core::fmt;

use dassie_ilp::ErrorCode;
use dassie_ledger::{LedgerId, TraceId};

use crate::PacketTransfers;
use crate::endpoint::EndpointId;

/// Destination addresses are kept up to this many bytes; longer ones are
/// cut, which a trace for people can afford.
pub const MAX_TRACED_ADDRESS: usize = 96;

/// What became of a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketState {
    /// Forwarded, waiting for the response.
    Pending,
    /// Fulfilled: its transfers are posted.
    Fulfilled,
    /// Rejected, by us or downstream: its transfers are voided.
    Rejected(ErrorCode),
}

/// One packet.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PacketRecord {
    /// Its trace ID, which its transfers carry.
    pub trace: TraceId,
    /// Where it came from.
    pub source: EndpointId,
    /// Where it went, if it was routed.
    pub destination: Option<EndpointId>,
    /// The Prepare's execution condition: a hash already, and the same on
    /// every hop, so that a packet's records on several nodes can be
    /// joined by it. Zero for a packet refused before it decoded.
    pub condition: [u8; 32],
    /// The ledger it came in on: the source's slot, once known.
    pub ledger_in: Option<LedgerId>,
    /// The ledger it went out on, once routed.
    pub ledger_out: Option<LedgerId>,
    destination_address: [u8; MAX_TRACED_ADDRESS],
    destination_length: u8,
    /// The amount that arrived.
    pub amount_in: u64,
    /// The amount sent on.
    pub amount_out: u64,
    /// When it arrived.
    pub received_at_ms: u64,
    /// When it was resolved, if it was.
    pub resolved_at_ms: Option<u64>,
    /// What became of it.
    pub state: PacketState,
    /// Its transfers: none for a zero amount or a packet refused before
    /// the ledger, one on a ledger, two across ledgers, and our fee third.
    pub transfers: PacketTransfers,
}

impl PacketRecord {
    pub(crate) fn new(
        trace: TraceId,
        source: EndpointId,
        destination: &str,
        amount_in: u64,
        now_ms: u64,
    ) -> Self {
        let mut destination_address = [0; MAX_TRACED_ADDRESS];
        let length = destination.len().min(MAX_TRACED_ADDRESS);
        if let (Some(target), Some(source)) = (
            destination_address.get_mut(..length),
            destination.as_bytes().get(..length),
        ) {
            target.copy_from_slice(source);
        }
        Self {
            trace,
            source,
            destination: None,
            condition: [0; 32],
            ledger_in: None,
            ledger_out: None,
            destination_address,
            destination_length: u8::try_from(length).unwrap_or(0),
            amount_in,
            amount_out: 0,
            received_at_ms: now_ms,
            resolved_at_ms: None,
            state: PacketState::Pending,
            transfers: [None; 3],
        }
    }

    /// What every record holds to: one routed went out on a ledger, from
    /// the one it came in on.
    pub fn check(&self) {
        assert_eq!(
            self.destination.is_some(),
            self.ledger_out.is_some(),
            "a routed packet went out on a ledger"
        );
        assert!(
            self.ledger_out.is_none() || self.ledger_in.is_some(),
            "a packet sent on came in on a ledger"
        );
    }

    /// The destination address, cut at [`MAX_TRACED_ADDRESS`] bytes.
    #[must_use]
    pub fn destination_address(&self) -> &str {
        let bytes = self
            .destination_address
            .get(..usize::from(self.destination_length))
            .unwrap_or_default();
        // Addresses are ASCII, so any cut is at a character boundary.
        core::str::from_utf8(bytes).unwrap_or_default()
    }
}

impl fmt::Debug for PacketRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PacketRecord")
            .field("trace", &self.trace)
            .field("source", &self.source)
            .field("destination", &self.destination)
            .field("ledger_in", &self.ledger_in)
            .field("ledger_out", &self.ledger_out)
            .field("address", &self.destination_address())
            .field("amount_in", &self.amount_in)
            .field("amount_out", &self.amount_out)
            .field("state", &self.state)
            .field("transfers", &self.transfers)
            .finish_non_exhaustive()
    }
}

/// A change in the trace, for live subscribers. Records are stored inline
/// rather than boxed, so queueing one does not allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TraceEvent {
    /// A packet arrived, or was resolved: its record as it is now.
    Packet(PacketRecord),
    /// Events were dropped because nobody polled: this many.
    Lagged(u64),
}

/// The ring of records and the queue of events.
#[derive(Debug)]
pub(crate) struct Trace {
    records: VecDeque<PacketRecord>,
    capacity: usize,
    events: VecDeque<TraceEvent>,
    max_events: usize,
    dropped: u64,
}

impl Trace {
    pub fn new(capacity: usize, max_events: usize) -> Self {
        Self {
            records: VecDeque::with_capacity(capacity),
            capacity,
            events: VecDeque::with_capacity(max_events),
            max_events,
            dropped: 0,
        }
    }

    /// Adds a record, evicting the oldest when full. Records arrive in
    /// trace order, one sequence after the other within the run's epoch.
    pub fn push(&mut self, record: PacketRecord) {
        if let Some(last) = self.records.back() {
            assert_eq!(last.trace.epoch, record.trace.epoch, "one epoch per run");
            assert_eq!(
                last.trace.sequence + 1,
                record.trace.sequence,
                "trace order"
            );
        }
        if self.records.len() >= self.capacity {
            self.records.pop_front();
        }
        self.records.push_back(record);
        self.notify(record);
    }

    /// Changes the record of `trace`, if it is still kept.
    pub fn update(&mut self, trace: TraceId, change: impl FnOnce(&mut PacketRecord)) {
        // Sequences are consecutive within an epoch, so the record's place
        // follows from the first one's.
        let Some(first) = self.records.front() else {
            return;
        };
        if first.trace.epoch != trace.epoch || trace.sequence < first.trace.sequence {
            return;
        }
        let Ok(index) = usize::try_from(trace.sequence - first.trace.sequence) else {
            return;
        };
        if let Some(record) = self.records.get_mut(index) {
            assert_eq!(record.trace, trace);
            change(record);
            let record = *record;
            self.notify(record);
        }
    }

    fn notify(&mut self, record: PacketRecord) {
        if self.events.len() >= self.max_events {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(TraceEvent::Packet(record));
    }

    pub fn poll_event(&mut self) -> Option<TraceEvent> {
        if self.dropped > 0 {
            let dropped = self.dropped;
            self.dropped = 0;
            return Some(TraceEvent::Lagged(dropped));
        }
        self.events.pop_front()
    }

    /// Records after `after`, oldest first.
    pub fn records_after(&self, after: Option<TraceId>) -> impl Iterator<Item = &PacketRecord> {
        self.records
            .iter()
            .filter(move |record| after.is_none_or(|after| record.trace > after))
    }

    /// The bounds, and that the records are consecutive: `push` asserts
    /// each one follows the last, so the ends tell for the whole ring.
    pub fn check_invariants(&self) {
        assert!(self.records.len() <= self.capacity);
        assert!(self.events.len() <= self.max_events);
        if let (Some(first), Some(last)) = (self.records.front(), self.records.back()) {
            assert_eq!(first.trace.epoch, last.trace.epoch, "one epoch per run");
            let span = last.trace.sequence.checked_sub(first.trace.sequence);
            let length = u64::try_from(self.records.len() - 1).ok();
            assert_eq!(span, length, "trace order");
        }
        for record in &self.records {
            record.check();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u64) -> PacketRecord {
        let trace = TraceId { epoch: 1, sequence };
        PacketRecord::new(trace, EndpointId::Btp(1), "test.x", 1, 0)
    }

    #[test]
    fn a_full_ring_keeps_the_latest_in_order() {
        let mut trace = Trace::new(4, 4);
        for sequence in 1..=10 {
            trace.push(record(sequence));
            trace.check_invariants();
        }
        let kept: alloc::vec::Vec<u64> = trace
            .records_after(None)
            .map(|r| r.trace.sequence)
            .collect();
        assert_eq!(kept, [7, 8, 9, 10]);
    }

    #[test]
    fn a_record_starts_unrouted_without_a_condition() {
        let record = record(1);
        assert_eq!(record.condition, [0; 32]);
        assert_eq!((record.ledger_in, record.ledger_out), (None, None));
        record.check();
    }

    #[test]
    #[should_panic(expected = "a routed packet went out on a ledger")]
    fn a_routed_record_without_its_ledger_is_caught() {
        let mut record = record(1);
        record.ledger_in = Some(LedgerId::StubUsd);
        record.destination = Some(EndpointId::Btp(2));
        record.check();
    }

    #[test]
    fn an_update_keeps_the_record_inline() {
        let mut trace = Trace::new(4, 4);
        trace.push(record(1));
        let id = TraceId {
            epoch: 1,
            sequence: 1,
        };
        trace.update(id, |record| {
            record.condition = [9; 32];
            record.ledger_in = Some(LedgerId::StubUsd);
            record.ledger_out = Some(LedgerId::StubUsd);
            record.destination = Some(EndpointId::Btp(2));
        });
        trace.check_invariants();
        let _created = trace.poll_event();
        assert!(matches!(
            trace.poll_event(),
            Some(TraceEvent::Packet(updated))
                if updated.condition == [9; 32]
                    && updated.ledger_out == Some(LedgerId::StubUsd)
        ));
    }

    #[test]
    #[should_panic(expected = "trace order")]
    fn a_record_out_of_order_is_refused() {
        let mut trace = Trace::new(4, 4);
        trace.push(record(1));
        trace.push(record(3));
    }
}

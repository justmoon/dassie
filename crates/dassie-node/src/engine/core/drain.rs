//! Draining before the node stops (spec/areas/node.md#shutdown): the
//! connector takes no new packet, from any source, the node's own payments
//! and swaps send none, and what is in flight is answered or expires, so
//! that its Fulfills are booked rather than lost as in a crash.

use super::{Core, EngineError};

impl Core {
    /// Begins draining: closes the connector, and the node's own payments
    /// and swaps send nothing more, though those under way still take
    /// their answers. Returns the latest a packet in flight is answered
    /// by, the drain's bound: now if none is.
    pub fn begin_drain(&mut self) -> u64 {
        self.connector.set_closed(true);
        self.connector
            .latest_deadline(self.now_ms)
            .unwrap_or(self.now_ms)
    }

    /// Ends a drain the owner took back: the connector takes packets again,
    /// and the node's payments and swaps go on.
    pub fn end_drain(&mut self) {
        self.connector.set_closed(false);
        self.swaps_look_again();
    }

    /// Whether the node drains.
    #[must_use]
    pub const fn is_draining(&self) -> bool {
        self.connector.is_closed()
    }

    /// Packets in flight: the node's own, its swaps and those it forwards.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.connector.pending_count()
    }

    /// Whether a drain is done: no packet is in flight.
    #[must_use]
    pub fn drained(&self) -> bool {
        self.in_flight() == 0
    }

    /// Refuses what would start sending while the node drains.
    pub(super) fn refuse_while_draining(&self) -> Result<(), EngineError> {
        if self.is_draining() {
            return Err(EngineError::Precondition("the node is stopping".into()));
        }
        Ok(())
    }
}

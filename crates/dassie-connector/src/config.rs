//! The connector's bounds and timing.

/// The connector's bounds and timing rules. Every table has a limit
/// (D16), and the timing constants are spec 4 §12's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The boot epoch: part of every trace ID, so that references never
    /// repeat across restarts.
    pub epoch: u64,
    /// Most endpoints.
    pub max_endpoints: usize,
    /// Most packets in flight.
    pub max_pending: usize,
    /// Most packets in flight from one child endpoint.
    pub max_pending_per_endpoint: usize,
    /// Most packets in flight from one peer: a peer carries other nodes'
    /// traffic too.
    pub max_pending_per_peer: usize,
    /// Most routes in the route table.
    pub max_routes: usize,
    /// Packet records kept in the trace.
    pub trace_capacity: usize,
    /// Trace events waiting to be polled.
    pub max_events: usize,
    /// How much earlier than the incoming packet the outgoing one expires,
    /// so that the response has time to travel back (`ILP_MESSAGE_WINDOW`).
    pub message_window_ms: u64,
    /// The least time the next hop must get, or R02.
    pub min_timeout_ms: u64,
    /// The longest the next hop gets (`MAXIMUM_HOLD_TIME`).
    pub max_hold_ms: u64,
    /// The largest incoming amount, in the source ledger's units; more gets
    /// F08. Per ledger later (D6). It also sets how much
    /// money may be in flight with a peer: [`IN_FLIGHT_PACKETS`] of it, and
    /// half of it more.
    pub max_packet_amount: u64,
    /// Rates older than this refuse cross-currency packets with T00 (D7).
    pub max_rate_age_ms: u64,
    /// Most accounts with a spending cap.
    pub max_caps: usize,
}

/// How much money may be in flight with one peer each way, in packets of
/// [`Config::max_packet_amount`] (D24), and half a packet more: three of
/// the largest fit, a fourth does not, so half a packet stays free for
/// smaller ones, such as a quote's probes while a payment fills the link.
/// A crash loses the Fulfills of what was in flight, and that is how far
/// two peers' books drift apart for it. Both numbers say how much the
/// owner is prepared to lose on a link, so one follows from the other.
pub const IN_FLIGHT_PACKETS: u64 = 3;

impl Config {
    /// The production values: 0.1 units per packet at scale 9, packets held
    /// up to 30 s.
    pub const DEFAULT: Self = Self {
        epoch: 0,
        max_endpoints: 256,
        max_pending: 4096,
        max_pending_per_endpoint: 512,
        max_pending_per_peer: 512,
        max_routes: 1024,
        trace_capacity: 4096,
        max_events: 1024,
        message_window_ms: 1_000,
        min_timeout_ms: 1_000,
        max_hold_ms: 30_000,
        max_packet_amount: 100_000_000,
        max_rate_age_ms: 60 * 60 * 1_000,
        max_caps: 256,
    };

    /// Checks the bounds make sense.
    ///
    /// # Panics
    ///
    /// If one does not.
    pub fn validate(&self) {
        assert!(self.max_endpoints > 0, "a connector needs endpoints");
        assert!(self.max_pending > 0, "a connector needs room for packets");
        assert!(self.max_pending_per_endpoint > 0);
        assert!(self.max_pending_per_endpoint <= self.max_pending);
        assert!(self.max_pending_per_peer > 0);
        // What is in flight with a peer, and one packet more, fits a u64.
        assert!(self.max_packet_amount <= u64::MAX / (IN_FLIGHT_PACKETS + 1));
        assert!(self.trace_capacity > 0 && self.max_events > 0);
        assert!(self.max_hold_ms >= self.min_timeout_ms);
        // Request IDs are u32s: a counter skipping IDs in use always finds
        // one while fewer than 2^32 are pending.
        assert!(u32::try_from(self.max_pending).is_ok());
    }

    /// The most packets in flight from one endpoint.
    #[must_use]
    pub const fn max_pending_from(&self, endpoint: crate::EndpointId) -> usize {
        if endpoint.is_peer() {
            self.max_pending_per_peer
        } else {
            self.max_pending_per_endpoint
        }
    }

    /// The most money in flight with one peer each way (D24).
    #[must_use]
    pub const fn max_in_flight(&self) -> u64 {
        self.max_packet_amount * IN_FLIGHT_PACKETS + self.max_packet_amount / 2
    }

    /// The pending transfers the ledger must have room for: up to three
    /// per packet, across ledgers with a fee.
    #[must_use]
    pub const fn ledger_pending_needed(&self) -> usize {
        self.max_pending * 3
    }
}

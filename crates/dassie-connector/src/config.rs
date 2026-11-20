//! The connector's bounds and timing.

use dassie_ledger::{LedgerId, PerLedger};

/// The connector's bounds and timing rules. Every table has a limit
/// (spec/decisions/tigerstyle.md), and the timing follows
/// spec/decisions/connector-timing.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The boot epoch: part of every trace ID, so that references never
    /// repeat across restarts.
    pub epoch: u64,
    /// Most endpoints.
    pub max_endpoints: usize,
    /// Most packets in flight.
    pub max_pending: usize,
    /// Most packets in flight from one child endpoint, or one downlink:
    /// each sends only its own.
    pub max_pending_per_endpoint: usize,
    /// Most packets in flight from any other peer: it carries other nodes'
    /// traffic too.
    pub max_pending_per_peer: usize,
    /// Most routes in the route table.
    pub max_routes: usize,
    /// Packet records kept in the trace.
    pub trace_capacity: usize,
    /// Trace events waiting to be polled.
    pub max_events: usize,
    /// How much earlier than the incoming packet the outgoing one expires,
    /// so that the response has time to travel back (`ILP_MESSAGE_WINDOW`),
    /// and a peer over a settlement hub has time for the hub's legs
    /// (spec/decisions/connector-timing.md).
    pub message_window_ms: u64,
    /// The least time the next hop must get, or R02.
    pub min_timeout_ms: u64,
    /// The longest the next hop gets (`MAXIMUM_HOLD_TIME`).
    pub max_hold_ms: u64,
    /// The largest packet on each ledger, in its units: a larger incoming
    /// amount gets F08, and so does one a conversion makes larger than the
    /// outgoing ledger's (spec/areas/connector.md#the-largest-packet). It
    /// also sets how much money may be in flight with a peer on the ledger:
    /// [`IN_FLIGHT_PACKETS`] of it, and half of it more.
    pub max_packet: PerLedger<u64>,
    /// Rates older than this refuse cross-currency packets with T00
    /// (spec/decisions/exchange-rates.md).
    pub max_rate_age_ms: u64,
    /// Most accounts with a spending cap.
    pub max_caps: usize,
    /// How long until what a peer earned us counts half as much in drawing
    /// next hops: it halves once per period
    /// (spec/decisions/weighted-next-hops.md).
    pub earnings_half_life_ms: u64,
    /// The share of next-hop draws, in parts per million, that ignore what
    /// the hops earned us and give each the same chance
    /// (spec/decisions/weighted-next-hops.md).
    pub explore_ppm: u32,
}

/// Parts per million: the whole of the draws.
pub(crate) const PPM: u32 = 1_000_000;

/// How much money may be in flight with one peer each way, in packets of
/// [`Config::max_packet`] (spec/decisions/packets-in-flight.md,
/// spec/decisions/thresholds-in-packets.md), and half a packet more: two of
/// the largest fit, a third does not, so half a packet stays free for smaller
/// ones, such as a quote's probes while a payment fills the link. A crash
/// loses the Fulfills of what was in flight, and that is how far two peers'
/// books drift apart for it. Both numbers say how much the owner is prepared
/// to lose on a link, so one follows from the other.
pub const IN_FLIGHT_PACKETS: u64 = 2;

/// 0.1 XRP: about as much as [`USD_PACKET`] at the time of writing. A value
/// fixed in the build, never converted at a rate
/// (spec/decisions/limits-per-ledger.md).
pub const XRP_PACKET: u64 = 100_000_000;

/// $0.10, on every dollar ledger: they count the same dollar, at par.
pub const USD_PACKET: u64 = 100_000_000;

/// The largest packet on `ledger`, in its units (scale 9). Ledgers that
/// count the same asset share it, testnet and mainnet alike, so the tests
/// run on what real money does.
const fn max_packet(ledger: LedgerId) -> u64 {
    match ledger {
        LedgerId::XrplTestnetXrp
        | LedgerId::XrplXrp
        | LedgerId::HubXrplTestnetXrp
        | LedgerId::HubXrplXrp => XRP_PACKET,
        LedgerId::StubUsd
        | LedgerId::XrplTestnetRlusdUsd
        | LedgerId::XrplRlusdUsd
        | LedgerId::XrplTestnetDsusdUsd
        | LedgerId::HubXrplTestnetDsusdUsd
        | LedgerId::HubXrplRlusdUsd => USD_PACKET,
    }
}

/// The largest packet on each ledger.
pub const MAX_PACKETS: PerLedger<u64> = dassie_ledger::per_ledger!(max_packet);

impl Config {
    /// The production values: [`MAX_PACKETS`], packets held up to 30 s.
    pub const DEFAULT: Self = Self {
        epoch: 0,
        max_endpoints: 256,
        max_pending: 4096,
        max_pending_per_endpoint: 512,
        max_pending_per_peer: 512,
        max_routes: 1024,
        trace_capacity: 4096,
        max_events: 1024,
        message_window_ms: 1_600,
        min_timeout_ms: 1_000,
        max_hold_ms: 30_000,
        max_packet: MAX_PACKETS,
        max_rate_age_ms: 60 * 60 * 1_000,
        max_caps: 256,
        earnings_half_life_ms: 60 * 60 * 1_000,
        explore_ppm: 100_000,
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
        for (_, max_packet) in self.max_packet.iter() {
            assert!(max_packet > 0, "a packet carries money");
            // What is in flight with a peer, and one packet more, fits a u64.
            assert!(max_packet <= u64::MAX / (IN_FLIGHT_PACKETS + 1));
        }
        assert!(self.trace_capacity > 0 && self.max_events > 0);
        assert!(self.max_hold_ms >= self.min_timeout_ms);
        // Request IDs are u32s: a counter skipping IDs in use always finds
        // one while fewer than 2^32 are pending.
        assert!(u32::try_from(self.max_pending).is_ok());
        assert!(self.earnings_half_life_ms > 0, "earnings decay");
        assert!(self.explore_ppm <= PPM, "a share of the draws");
    }

    /// The largest packet on `ledger`.
    #[must_use]
    pub const fn max_packet(&self, ledger: LedgerId) -> u64 {
        self.max_packet.get(ledger)
    }

    /// The most money in flight with one peer each way on `ledger`
    /// (spec/decisions/packets-in-flight.md).
    #[must_use]
    pub const fn max_in_flight(&self, ledger: LedgerId) -> u64 {
        let max_packet = self.max_packet(ledger);
        max_packet * IN_FLIGHT_PACKETS + max_packet / 2
    }

    /// The pending transfers the ledger must have room for: up to three
    /// per packet, across ledgers with a fee.
    #[must_use]
    pub const fn ledger_pending_needed(&self) -> usize {
        self.max_pending * 3
    }
}

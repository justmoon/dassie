/// Limits and tuning of a connection. Every table, queue and buffer a
/// connection has is sized from these when it is created
/// (spec/decisions/tigerstyle.md).
///
/// The defaults follow the original Dassie where it had a value and
/// `ilp-protocol-stream` where it did not, so that both kinds of peer see
/// familiar behaviour (spec/areas/stream.md#policy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// How long a sent packet may take before it expires.
    pub packet_timeout_ms: u64,
    /// Fewest packets in flight that congestion control backs off to.
    pub min_concurrency: u32,
    /// Most packets in flight; also the size of the in-flight table.
    pub max_concurrency: u32,
    /// Smallest money packet worth sending while more is still to come.
    /// The last packet of a payment may be smaller (the original never sent
    /// it).
    pub de_minimis_amount: u64,
    /// Most streams the remote may have open at once. Advertised as
    /// `ConnectionMaxStreamId`.
    pub max_remote_streams: u32,
    /// Most streams this side may have open at once.
    pub max_local_streams: u32,
    /// Bytes each stream buffers for the application to read. Advertised as
    /// `StreamMaxData`.
    pub stream_receive_window_bytes: u32,
    /// Bytes each stream buffers for sending.
    pub stream_send_buffer_bytes: u32,
    /// Bytes buffered across all streams. Advertised as `ConnectionMaxData`.
    pub connection_receive_window_bytes: u64,
    /// How long closing waits for the remote to acknowledge.
    pub close_timeout_ms: u64,
    /// How many times a close is sent before giving up.
    pub close_retries: u32,
    /// How far below a measured exchange rate packets may still be
    /// delivered, in thousandths.
    pub slippage_permille: u16,
    /// Significant digits a rate measurement needs to be trusted.
    pub min_rate_precision_digits: u32,
    /// Volleys of probes a rate measurement may send.
    pub max_probe_volleys: u32,
    /// First delay after a temporary error.
    pub retry_delay_start_ms: u64,
    /// Longest delay after repeated temporary errors.
    pub retry_delay_max_ms: u64,
    /// Largest amount to put in one packet before any F08 says otherwise.
    pub max_packet_amount: u64,
    /// What a stream the remote opens accepts before the application says
    /// otherwise. The application only hears of the stream after the packet
    /// that opened it, so with zero that packet's money is always refused
    /// once; a receiver that takes any payment sets this instead.
    pub remote_stream_receive_max: u64,
    /// How long a connection may go without hearing from the remote before
    /// it closes, as in `ilp-protocol-stream`. Bounds how long abandoned
    /// connections hold a server's table (spec/decisions/tigerstyle.md).
    pub idle_timeout_ms: u64,
}

/// Most streams a connection may have open, both sides together.
pub const MAX_STREAMS: u32 = 100;

impl Policy {
    /// The default policy.
    pub const DEFAULT: Self = Self {
        packet_timeout_ms: 30_000,
        min_concurrency: 1,
        max_concurrency: 100,
        de_minimis_amount: 1000,
        max_remote_streams: 10,
        max_local_streams: 10,
        stream_receive_window_bytes: 16_384,
        stream_send_buffer_bytes: 16_384,
        connection_receive_window_bytes: 65_534,
        close_timeout_ms: 30_000,
        close_retries: 3,
        slippage_permille: 10,
        min_rate_precision_digits: 3,
        max_probe_volleys: 5,
        retry_delay_start_ms: 100,
        retry_delay_max_ms: 30_000,
        max_packet_amount: u64::MAX,
        remote_stream_receive_max: 0,
        idle_timeout_ms: 60_000,
    };

    /// Checks that the limits make sense together.
    ///
    /// # Panics
    ///
    /// If they do not. Policies are configuration, so a bad one is a bug.
    pub fn validate(&self) {
        assert!(self.min_concurrency >= 1, "minimum concurrency below one");
        assert!(
            self.min_concurrency <= self.max_concurrency,
            "minimum concurrency above maximum"
        );
        assert!(
            self.max_concurrency <= 1000,
            "maximum concurrency above 1000"
        );
        assert!(
            self.max_remote_streams >= 1 && self.max_local_streams >= 1,
            "stream limits must allow a stream"
        );
        // Every packet carries the limits of every open stream: with at
        // most 100 streams the largest packet stays within the 512 frames a
        // packet may have and well within ILP's 32 KiB of data.
        assert!(
            self.max_remote_streams + self.max_local_streams <= MAX_STREAMS,
            "more streams than one packet can describe"
        );
        assert!(
            self.stream_receive_window_bytes <= 1 << 24,
            "stream receive window above 16 MiB"
        );
        assert!(
            self.stream_send_buffer_bytes <= 1 << 24,
            "stream send buffer above 16 MiB"
        );
        assert!(self.slippage_permille <= 1000, "slippage above 100%");
        assert!(self.close_retries >= 1, "close needs at least one attempt");
        assert!(
            self.max_probe_volleys >= 1,
            "rate measurement needs a volley"
        );
        assert!(self.max_packet_amount >= 1, "maximum packet amount is zero");
        assert!(
            self.retry_delay_start_ms <= self.retry_delay_max_ms,
            "retry delay start above maximum"
        );
        assert!(self.idle_timeout_ms > 0, "idle timeout is zero");
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

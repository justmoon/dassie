use crate::frame::ErrorCode;
use crate::rate::Rate;

/// Something the application should know about. Events about a stream's
/// money and data are coalesced: one notification stands for any number of
/// changes, and the stream's accessors give the current totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The remote answered for the first time: the connection works.
    Connected,
    /// The remote opened a stream.
    StreamOpened {
        /// The stream.
        stream_id: u64,
    },
    /// A stream received money.
    MoneyReceived {
        /// The stream.
        stream_id: u64,
    },
    /// Money sent on a stream was delivered.
    MoneySent {
        /// The stream.
        stream_id: u64,
    },
    /// A stream has data to read.
    DataReadable {
        /// The stream.
        stream_id: u64,
    },
    /// A stream is closed and gone, with its final totals.
    StreamClosed {
        /// The stream.
        stream_id: u64,
        /// Why, as the side that closed it said.
        code: ErrorCode,
        /// Everything the stream sent.
        total_sent: u64,
        /// Everything the stream received.
        total_received: u64,
    },
    /// A rate measurement finished. The connection now sends at this rate
    /// less the policy's slippage.
    ExchangeRateMeasured {
        /// The measured rate.
        rate: Rate,
    },
    /// A rate measurement could not get a precise enough result.
    ExchangeRateMeasurementFailed,
    /// The receiver got less than the minimum the rate promises, so sending
    /// money stopped until the application sets a new rate.
    ExchangeRateTooLow {
        /// What the sender asked for at least.
        expected: u64,
        /// What arrived.
        received: u64,
    },
    /// The connection is closed. Nothing more happens on it.
    ConnectionClosed {
        /// Why.
        code: ErrorCode,
        /// Whether the remote closed it.
        by_remote: bool,
    },
}

impl Event {
    /// The stream a coalescible event is about, so that a second one for
    /// the same stream is dropped while the first is still queued.
    pub(crate) const fn coalescing_key(&self) -> Option<(u8, u64)> {
        match *self {
            Self::MoneyReceived { stream_id } => Some((0, stream_id)),
            Self::MoneySent { stream_id } => Some((1, stream_id)),
            Self::DataReadable { stream_id } => Some((2, stream_id)),
            _ => None,
        }
    }
}

/// Counters of what a connection did, for instrumentation and simulation
/// checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Prepares sent.
    pub packets_sent: u64,
    /// Of those, fulfilled.
    pub packets_fulfilled: u64,
    /// Of those, rejected, including probes, which are always rejected.
    pub packets_rejected: u64,
    /// Of those, expired without a response.
    pub packets_timed_out: u64,
    /// Prepares received.
    pub packets_received: u64,
    /// Of those, fulfilled.
    pub packets_received_fulfilled: u64,
    /// Source amount of fulfilled packets.
    pub amount_sent: u64,
    /// What the receiver said fulfilled packets delivered.
    pub amount_delivered: u64,
    /// Amount of fulfilled incoming packets.
    pub amount_received: u64,
    /// Stream data bytes sent, counting retransmissions.
    pub bytes_sent: u64,
    /// Stream data bytes accepted from the remote.
    pub bytes_received: u64,
}

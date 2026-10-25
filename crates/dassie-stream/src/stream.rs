use core::cmp::Ordering;

use crate::buffers::{ReceiveBuffer, SendBuffer};
use crate::frame::ErrorCode;

/// The default flow-control window a peer is assumed to grant a stream
/// before it says otherwise, as in `ilp-protocol-stream`.
pub const DEFAULT_REMOTE_STREAM_WINDOW: u64 = 16_384;

/// One stream of a connection: money and data in both directions.
#[derive(Debug)]
pub struct Stream {
    pub(crate) id: u64,
    pub(crate) opened_by_remote: bool,

    // Money out, in source units.
    pub(crate) send_max: u64,
    pub(crate) sent: u64,
    pub(crate) held: u64,
    // The remote's limits, in destination units. `u64::MAX` until it says.
    // `remote_receive_max` and `remote_reported` come from the remote's
    // latest word, and the room is one less the other; `remote_received`
    // is the most it ever said it received.
    pub(crate) remote_receive_max: u64,
    pub(crate) remote_received: u64,
    /// The total the remote's latest word reported, and the sequence the
    /// next request had when it was heard: a reply that reports less is
    /// stale unless its request was sent after
    /// (spec/areas/stream.md#responses).
    pub(crate) remote_reported: u64,
    pub(crate) remote_reported_at: u64,
    /// Whether the remote ever lowered its maximum: what it pays into is
    /// shared, or was cut short.
    pub(crate) remote_max_fell: bool,
    /// What the application says the remote agreed to take in total, as
    /// an incoming payment's amount: while it took less, a stream left
    /// without room asks it again, as if its maximum fell.
    pub(crate) expected_delivery: u64,
    /// The most the application lets the stream deliver in total, in the
    /// remote's units, as it counts: `u64::MAX` for no cap.
    pub(crate) delivery_max: u64,

    // Money in, in our units.
    pub(crate) receive_max: u64,
    pub(crate) received: u64,
    /// The most this stream was told it may receive under the
    /// connection's money limit: what it is told never falls
    /// (spec/decisions/close-rather-than-lower.md). Nothing until told.
    pub(crate) money_told: u64,

    // Data.
    pub(crate) send: SendBuffer,
    pub(crate) receive: ReceiveBuffer,
    pub(crate) remote_max_data: u64,

    // Lifecycle.
    pub(crate) local_close: Option<ErrorCode>,
    pub(crate) local_close_acknowledged: bool,
    pub(crate) remote_close: Option<ErrorCode>,
}

impl Stream {
    pub(crate) const fn new(
        id: u64,
        opened_by_remote: bool,
        send_buffer: usize,
        receive_window: usize,
    ) -> Self {
        Self {
            id,
            opened_by_remote,
            send_max: 0,
            sent: 0,
            held: 0,
            remote_receive_max: u64::MAX,
            remote_received: 0,
            remote_reported: 0,
            remote_reported_at: 0,
            remote_max_fell: false,
            expected_delivery: 0,
            delivery_max: u64::MAX,
            receive_max: 0,
            received: 0,
            money_told: 0,
            send: SendBuffer::new(send_buffer),
            receive: ReceiveBuffer::new(receive_window),
            remote_max_data: DEFAULT_REMOTE_STREAM_WINDOW,
            local_close: None,
            local_close_acknowledged: false,
            remote_close: None,
        }
    }

    /// The stream's ID.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Whether the remote opened it.
    #[must_use]
    pub const fn opened_by_remote(&self) -> bool {
        self.opened_by_remote
    }

    /// The most this stream will send in total, in source units.
    #[must_use]
    pub const fn send_max(&self) -> u64 {
        self.send_max
    }

    /// What it has sent and the remote has acknowledged.
    #[must_use]
    pub const fn total_sent(&self) -> u64 {
        self.sent
    }

    /// What is in flight right now.
    #[must_use]
    pub const fn in_flight(&self) -> u64 {
        self.held
    }

    /// The most this stream will receive in total.
    #[must_use]
    pub const fn receive_max(&self) -> u64 {
        self.receive_max
    }

    /// What it has received.
    #[must_use]
    pub const fn total_received(&self) -> u64 {
        self.received
    }

    /// What the remote says this stream has delivered to it, in its units.
    #[must_use]
    pub const fn remote_received(&self) -> u64 {
        self.remote_received
    }

    /// Bytes ready to read.
    #[must_use]
    pub fn readable(&self) -> usize {
        self.receive.readable()
    }

    /// Whether this side closed the stream.
    #[must_use]
    pub const fn is_closed_locally(&self) -> bool {
        self.local_close.is_some()
    }

    /// Whether the remote closed the stream.
    #[must_use]
    pub const fn is_closed_by_remote(&self) -> bool {
        self.remote_close.is_some()
    }

    /// Whether the stream may still send money or data.
    pub(crate) const fn can_send(&self) -> bool {
        self.local_close.is_none() && self.remote_close.is_none()
    }

    /// Source amount the stream still wants to send beyond what is sent or
    /// in flight.
    pub(crate) const fn unsent_amount(&self) -> u64 {
        self.send_max
            .saturating_sub(self.sent)
            .saturating_sub(self.held)
    }

    /// Destination amount the remote still accepts: unlimited until it
    /// says, and nothing once it has all it takes.
    #[must_use]
    pub const fn remote_room(&self) -> u64 {
        self.remote_receive_max.saturating_sub(self.remote_reported)
    }

    /// Destination amount the application still lets the stream deliver:
    /// its cap less what the remote says it received.
    pub(crate) const fn delivery_room(&self) -> u64 {
        self.delivery_max.saturating_sub(self.remote_received)
    }

    /// Takes a receive maximum and received total the remote reported,
    /// in a reply to request `request` or, with `None`, in a packet of
    /// its own, if they are its latest word: what it takes now
    /// (spec/areas/stream.md#responses). `newest` says the reply answers
    /// the newest request heard back on; `next_sequence` is the sequence
    /// the next request will have.
    ///
    /// A remote's total grows, so one that reports more than its latest
    /// word was written after it, whichever request it answers, and one
    /// that reports less was written before: a receiver can take packets
    /// in another order than they were sent. Taken, its maximum would undo
    /// one that fell since, as when the receiver came within its tolerance
    /// of the total, and report room the receiver no longer has. Unless it
    /// answers a request sent after the latest word was heard: then the
    /// receiver wrote it later, and its count fell, as when it dropped a
    /// connection's count for a new payer
    /// (spec/areas/stream.md#money-limit). Of replies that report the
    /// same, the one to the newest request says what the receiver takes;
    /// another may only raise it.
    pub(crate) fn hear_remote_limit(
        &mut self,
        receive_max: u64,
        total_received: u64,
        request: Option<u64>,
        newest: bool,
        next_sequence: u64,
    ) {
        self.remote_received = self.remote_received.max(total_received);
        let same = total_received == self.remote_reported;
        let current = match total_received.cmp(&self.remote_reported) {
            Ordering::Greater => true,
            Ordering::Equal => newest,
            Ordering::Less => request.is_some_and(|request| request >= self.remote_reported_at),
        };
        let known = self.remote_receive_max != u64::MAX;
        if current {
            if !same {
                self.remote_reported = total_received;
                self.remote_reported_at = next_sequence;
            }
            // What the receiver takes now, even when it fell, as when
            // another payment took from what it pays into: sent what it
            // no longer takes, every packet would be refused
            // (spec/areas/stream.md#money-limit).
            self.remote_max_fell |= known && receive_max < self.remote_receive_max;
            self.remote_receive_max = receive_max;
        } else if same {
            // An unknown limit is the maximum, so the first value always
            // counts.
            self.remote_receive_max = if known {
                self.remote_receive_max.max(receive_max)
            } else {
                receive_max
            };
        }
    }

    /// How much more it accepts.
    pub(crate) const fn receive_capacity(&self) -> u64 {
        if self.local_close.is_some() {
            0
        } else {
            self.receive_max.saturating_sub(self.received)
        }
    }

    /// Whether everything is settled and the stream can be forgotten: one
    /// side closed it (and, if it was us, the remote heard), nothing is in
    /// flight, and the application has read its data or, by closing the
    /// stream itself, said it does not want it.
    pub(crate) fn is_finished(&self) -> bool {
        let closed = self.local_close_acknowledged || self.remote_close.is_some();
        let drained = self.local_close.is_some() || self.receive.readable() == 0;
        closed && self.held == 0 && drained
    }

    pub(crate) fn check_invariants(&self) {
        assert!(
            u128::from(self.sent) + u128::from(self.held) <= u128::from(self.send_max),
            "stream {} sends beyond its maximum",
            self.id
        );
        assert!(
            self.received <= self.receive_max,
            "stream {} received beyond its maximum",
            self.id
        );
        assert!(
            self.send.next_offset() <= self.send.end_offset(),
            "stream {} send offset beyond its data",
            self.id
        );
        self.receive.check_invariants();
    }
}

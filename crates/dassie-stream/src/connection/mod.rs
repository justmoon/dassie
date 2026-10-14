//! A STREAM connection as a sans-IO state machine (spec 7 §4.2).
//!
//! The connection never touches a socket, a clock or the OS RNG. The caller
//! feeds it the current time, randomness and the packets that arrive, and
//! takes from it the packets to send, the next deadline and the events the
//! application cares about:
//!
//! - [`Connection::handle_prepare`] answers an incoming ILP Prepare;
//! - [`Connection::poll_transmit`] produces the next outgoing Prepare;
//! - [`Connection::handle_response`] takes the Fulfill or Reject for it;
//! - [`Connection::handle_timeout`] expires what took too long, at
//!   [`Connection::poll_timeout`];
//! - [`Connection::poll_event`] reports what changed.
//!
//! Every table is allocated when the connection is created, from its
//! [`Policy`], and [`Connection::check_invariants`] verifies the state after
//! any step (D16).

mod closed;
mod receive;
mod response;
mod send;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_ilp::{Address, Timestamp};

use crate::crypto::Keys;
use crate::event::{Event, Stats};
use crate::frame::ErrorCode;
use crate::policy::Policy;
use crate::rate::Rate;
use crate::receipt::{RECEIPT_LENGTH, ReceiptSecret};
use crate::stream::Stream;

/// Length of the buffer that [`Connection::handle_prepare`],
/// [`Connection::poll_transmit`] and [`Connection::handle_response`] work in:
/// room for one ILP packet to write and one to decrypt.
pub const BUFFER_LENGTH: usize = 2 * dassie_ilp::MAX_PACKET_LENGTH;

/// The stream limit a peer is assumed to grant before it says otherwise, as
/// in `ilp-protocol-stream` (ten streams).
const DEFAULT_REMOTE_MAX_STREAM_ID: u64 = 20;

/// Most probes a rate measurement has outstanding.
const MAX_PROBES: usize = 8;

/// Which end of the connection this is. Clients open odd stream IDs,
/// servers even ones (RFC 29 §2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The side that was given the address and shared secret.
    Client,
    /// The side that issued them.
    Server,
}

/// An asset: code and scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetDetails<'a> {
    /// For example `USD`.
    pub code: &'a str,
    /// Decimal places.
    pub scale: u8,
}

/// Everything a connection needs at creation.
#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    /// Client or server.
    pub role: Role,
    /// The shared secret, 32 bytes in practice.
    pub shared_secret: &'a [u8],
    /// This side's ILP address, if it has one. A client without one can
    /// send but not receive.
    pub local_address: Option<Address<'a>>,
    /// The remote's address: a client's destination. A server learns it
    /// from the client.
    pub remote_address: Option<Address<'a>>,
    /// This side's asset, announced to the remote.
    pub local_asset: Option<AssetDetails<'a>>,
    /// Receipt parameters, if this side issues receipts (RFC 39).
    pub receipt: Option<ReceiptSecret>,
    /// Limits and tuning.
    pub policy: Policy,
}

/// Why a stream operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamError {
    /// No open stream has this ID.
    UnknownStream,
    /// The stream is closed.
    StreamClosed,
    /// The remote does not allow another stream yet.
    StreamIdBlocked,
    /// This side has as many streams open as its policy allows.
    TooManyStreams,
    /// The connection is closed or closing.
    ConnectionClosed,
}

/// An outgoing packet that [`Connection::poll_transmit`] wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transmit {
    /// Pass this to [`Connection::handle_response`] with the response.
    pub request_id: u64,
    /// Length of the ILP Prepare at the start of the buffer.
    pub length: usize,
    /// When the Prepare expires.
    pub expires_at: Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Open,
    Closing {
        code: ErrorCode,
        deadline: Timestamp,
    },
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PacketKind {
    Normal,
    Probe,
    Close,
}

/// A packet in flight. Slots are allocated with the connection and reused.
#[derive(Debug)]
struct Slot {
    active: bool,
    sequence: u64,
    kind: PacketKind,
    expires_at: Timestamp,
    condition: [u8; 32],
    source_amount: u64,
    min_destination_amount: u64,
    /// (stream ID, source amount)
    money: Vec<(u64, u64)>,
    /// (stream ID, start offset, end offset)
    data: Vec<(u64, u64, u64)>,
    /// Streams whose `StreamClose` the packet carries.
    closes: Vec<u64>,
    /// Whether it carried this side's address and limits.
    announced: bool,
    /// Which of the application's limit updates it carried, if any. If the
    /// packet is lost they must be sent again, or both sides could wait
    /// forever; if it arrives, only that update is acknowledged.
    window_update: Option<u64>,
}

impl Slot {
    fn new(streams: usize) -> Self {
        Self {
            active: false,
            sequence: 0,
            kind: PacketKind::Normal,
            expires_at: Timestamp::default(),
            condition: [0; 32],
            source_amount: 0,
            min_destination_amount: 0,
            money: Vec::with_capacity(streams),
            data: Vec::with_capacity(streams),
            closes: Vec::with_capacity(streams),
            announced: false,
            window_update: None,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.money.clear();
        self.data.clear();
        self.closes.clear();
        self.announced = false;
        self.window_update = None;
        self.source_amount = 0;
        self.min_destination_amount = 0;
    }
}

/// An exchange-rate measurement in progress.
#[derive(Debug)]
struct Probing {
    queue: [u64; MAX_PROBES],
    queued: usize,
    outstanding: usize,
    /// Significant digits and rate of the best result so far.
    best: Option<(u32, u64, Rate)>,
    /// Packet maxima learned from F08 rejects, for the next volley.
    maxima: [u64; MAX_PROBES],
    maxima_count: usize,
    /// The smallest amount that hit a temporary error.
    smallest_temporary: Option<u64>,
    volleys: u32,
}

/// A STREAM connection.
#[derive(Debug)]
pub struct Connection {
    role: Role,
    keys: Keys,
    policy: Policy,
    local_address: Option<String>,
    remote_address: Option<String>,
    local_asset: Option<(String, u8)>,
    remote_asset: Option<(String, u8)>,
    receipt: Option<ReceiptSecret>,
    /// What the application lets the connection still receive, over all
    /// its streams; `u64::MAX` for no limit beyond the streams' own.
    money_limit: u64,
    /// What the application counts as received already, over earlier
    /// connections too, while it sets a limit.
    money_received: u64,

    state: State,
    close_attempts: u32,
    close_acknowledged: bool,
    remote_knows_us: bool,
    connected: bool,
    announce_pending: bool,
    /// The application raised a receive limit the remote should hear about.
    window_update_pending: bool,
    /// Counts the application's limit updates, to tell them apart.
    window_update_sequence: u64,
    /// Last sign of life from the remote, for the idle timeout.
    last_activity: Option<Timestamp>,

    next_sequence: u64,
    streams: Vec<Stream>,
    next_local_stream_id: u64,
    remote_max_stream_id: u64,
    local_max_stream_id: u64,
    stream_id_blocked: Option<u64>,
    closed_remote_streams: closed::ClosedIds,

    /// The remote's limit on the sum of the highest offsets sent on open
    /// streams. Like `ilp-protocol-stream`, both sides count open streams
    /// only, so closing a stream frees its share.
    remote_connection_max_data: u64,

    slots: Vec<Slot>,
    concurrency_milli: u32,
    /// The least the receiver must get: what packets promise.
    exchange_rate: Option<Rate>,
    /// What the path is expected to deliver: the measured or set rate, or
    /// better if replies show it. Used to convert the receiver's limits,
    /// which the minimum rate would overshoot.
    expected_rate: Option<Rate>,
    rate_too_low: bool,
    max_packet_amount: u64,
    retry_at: Option<Timestamp>,
    retry_delay_ms: u64,
    probing: Option<Probing>,

    events: VecDeque<Event>,
    events_capacity: usize,
    stats: Stats,

    // Scratch space for incoming packets, sized once.
    claims: Vec<crate::allocate::Claim>,
    claim_streams: Vec<u64>,
    allocation: Vec<u64>,
    credited: Vec<(u64, [u8; RECEIPT_LENGTH])>,
    stream_errors: Vec<(u64, ErrorCode)>,
}

fn owned_address(address: Option<Address<'_>>) -> Option<String> {
    address.map(|address| String::from(address.as_str()))
}

impl Connection {
    /// Creates a connection, allocating every table it will use.
    ///
    /// # Panics
    ///
    /// If the policy is inconsistent (see [`Policy::validate`]).
    #[must_use]
    pub fn new(config: &Config<'_>) -> Self {
        let policy = config.policy;
        policy.validate();
        let stream_capacity = (policy.max_local_streams + policy.max_remote_streams) as usize;
        let slot_count = policy.max_concurrency as usize + MAX_PROBES + 1;
        let events_capacity = stream_capacity * 6 + 16;
        let next_local_stream_id = match config.role {
            Role::Client => 1,
            Role::Server => 2,
        };
        Self {
            role: config.role,
            keys: Keys::derive(config.shared_secret),
            policy,
            local_address: owned_address(config.local_address),
            remote_address: owned_address(config.remote_address),
            local_asset: config
                .local_asset
                .map(|asset| (String::from(asset.code), asset.scale)),
            remote_asset: None,
            receipt: config.receipt,
            money_limit: u64::MAX,
            money_received: 0,
            state: State::Open,
            close_attempts: 0,
            close_acknowledged: false,
            // A server's address is how the client found it.
            remote_knows_us: config.role == Role::Server,
            connected: false,
            announce_pending: true,
            window_update_pending: false,
            window_update_sequence: 0,
            last_activity: None,
            next_sequence: 0,
            streams: Vec::with_capacity(stream_capacity),
            next_local_stream_id,
            remote_max_stream_id: DEFAULT_REMOTE_MAX_STREAM_ID,
            local_max_stream_id: 2 * u64::from(policy.max_remote_streams),
            stream_id_blocked: None,
            closed_remote_streams: closed::ClosedIds::default(),
            remote_connection_max_data: policy.connection_receive_window_bytes,
            slots: (0..slot_count)
                .map(|_| Slot::new(stream_capacity))
                .collect(),
            concurrency_milli: policy.min_concurrency * 1000,
            exchange_rate: None,
            expected_rate: None,
            rate_too_low: false,
            max_packet_amount: policy.max_packet_amount,
            retry_at: None,
            retry_delay_ms: policy.retry_delay_start_ms,
            probing: None,
            events: VecDeque::with_capacity(events_capacity),
            events_capacity,
            stats: Stats::default(),
            claims: Vec::with_capacity(stream_capacity),
            claim_streams: Vec::with_capacity(stream_capacity),
            allocation: Vec::with_capacity(stream_capacity),
            credited: Vec::with_capacity(stream_capacity),
            stream_errors: Vec::with_capacity(16),
        }
    }

    /// Client or server.
    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    /// Whether the connection is closed for good.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        matches!(self.state, State::Closed)
    }

    /// Counters for instrumentation.
    #[must_use]
    pub const fn stats(&self) -> &Stats {
        &self.stats
    }

    /// The minimum rate packets are sent at, if one is set.
    #[must_use]
    pub const fn exchange_rate(&self) -> Option<Rate> {
        self.exchange_rate
    }

    /// The largest packet amount the path allows, as far as is known.
    #[must_use]
    pub const fn max_packet_amount(&self) -> u64 {
        self.max_packet_amount
    }

    /// The remote's address, once known.
    #[must_use]
    pub fn remote_address(&self) -> Option<&str> {
        self.remote_address.as_deref()
    }

    /// The remote's asset, once it said.
    #[must_use]
    pub fn remote_asset(&self) -> Option<AssetDetails<'_>> {
        self.remote_asset
            .as_ref()
            .map(|(code, scale)| AssetDetails {
                code,
                scale: *scale,
            })
    }

    /// The open streams, in the order they were opened.
    pub fn streams(&self) -> impl Iterator<Item = &Stream> {
        self.streams.iter()
    }

    /// The open stream `id`.
    #[must_use]
    pub fn stream(&self, id: u64) -> Option<&Stream> {
        self.streams.iter().find(|stream| stream.id == id)
    }

    fn stream_mut(&mut self, id: u64) -> Option<&mut Stream> {
        self.streams.iter_mut().find(|stream| stream.id == id)
    }

    fn is_local_stream_id(&self, id: u64) -> bool {
        match self.role {
            Role::Client => !id.is_multiple_of(2),
            Role::Server => id.is_multiple_of(2),
        }
    }

    /// Opens a stream and returns its ID.
    pub fn open_stream(&mut self) -> Result<u64, StreamError> {
        if !matches!(self.state, State::Open) {
            return Err(StreamError::ConnectionClosed);
        }
        let local_count = self
            .streams
            .iter()
            .filter(|stream| !stream.opened_by_remote)
            .count();
        if local_count >= self.policy.max_local_streams as usize {
            return Err(StreamError::TooManyStreams);
        }
        let id = self.next_local_stream_id;
        if id > self.remote_max_stream_id {
            self.stream_id_blocked = Some(id);
            return Err(StreamError::StreamIdBlocked);
        }
        self.next_local_stream_id += 2;
        self.push_stream(id, false);
        Ok(id)
    }

    fn push_stream(&mut self, id: u64, opened_by_remote: bool) {
        assert!(
            self.streams.len() < self.streams.capacity(),
            "stream table full"
        );
        let mut stream = Stream::new(
            id,
            opened_by_remote,
            self.policy.stream_send_buffer_bytes as usize,
            self.policy.stream_receive_window_bytes as usize,
        );
        if opened_by_remote {
            stream.receive_max = self.policy.remote_stream_receive_max;
        }
        self.streams.push(stream);
    }

    fn open_stream_mut(&mut self, id: u64) -> Result<&mut Stream, StreamError> {
        let open = matches!(self.state, State::Open);
        let stream = self.stream_mut(id).ok_or(StreamError::UnknownStream)?;
        if !open {
            return Err(StreamError::ConnectionClosed);
        }
        if !stream.can_send() {
            return Err(StreamError::StreamClosed);
        }
        Ok(stream)
    }

    /// Sets how much a stream sends in total. Never below what it sent or
    /// has in flight.
    pub fn set_send_max(&mut self, id: u64, amount: u64) -> Result<(), StreamError> {
        let stream = self.open_stream_mut(id)?;
        stream.send_max = amount.max(stream.sent + stream.held);
        Ok(())
    }

    /// Sets how much a stream accepts in total. Never below what it
    /// received.
    pub fn set_receive_max(&mut self, id: u64, amount: u64) -> Result<(), StreamError> {
        let stream = self.stream_mut(id).ok_or(StreamError::UnknownStream)?;
        if stream.local_close.is_some() {
            return Err(StreamError::StreamClosed);
        }
        stream.receive_max = amount.max(stream.received);
        self.note_window_update();
        Ok(())
    }

    /// Sets how much more the connection may receive in total, over all its
    /// streams, and what was received before: a packet that would exceed
    /// the limit is rejected. The application sets both before each packet
    /// when the connection pays into something with a total, such as an
    /// incoming payment; `u64::MAX` removes the limit.
    ///
    /// Under a limit, each stream reports the application's count as what
    /// it received, and that count plus the limit as its maximum. Both only
    /// grow, as senders expect, even when the sender's earlier connection
    /// is forgotten: after a restart, or when the server dropped it.
    pub fn set_money_limit(&mut self, remaining: u64, received: u64) {
        self.money_limit = remaining;
        self.money_received = received;
    }

    /// What the connection may still receive (see
    /// [`set_money_limit`](Self::set_money_limit)).
    #[must_use]
    pub fn money_limit(&self) -> u64 {
        self.money_limit
    }

    /// Queues `data` on a stream and returns how much fit in its buffer.
    pub fn write(&mut self, id: u64, data: &[u8]) -> Result<usize, StreamError> {
        Ok(self.open_stream_mut(id)?.send.write(data))
    }

    /// Reads in-order data from a stream into `output` and returns how much.
    pub fn read(&mut self, id: u64, output: &mut [u8]) -> Result<usize, StreamError> {
        let stream = self.stream_mut(id).ok_or(StreamError::UnknownStream)?;
        let count = stream.receive.read(output);
        if count > 0 {
            self.note_window_update();
        }
        Ok(count)
    }

    fn note_window_update(&mut self) {
        self.window_update_pending = true;
        self.window_update_sequence += 1;
    }

    /// Closes a stream. Money and data not yet sent are dropped; what is in
    /// flight completes.
    pub fn close_stream(&mut self, id: u64, code: ErrorCode) -> Result<(), StreamError> {
        let stream = self.stream_mut(id).ok_or(StreamError::UnknownStream)?;
        if stream.local_close.is_none() {
            stream.local_close = Some(code);
            stream.send_max = stream.sent + stream.held;
        }
        Ok(())
    }

    /// Sets the minimum exchange rate packets are sent at, and resumes
    /// sending money if a too-low rate stopped it.
    pub fn set_exchange_rate(&mut self, rate: Rate) {
        self.exchange_rate = Some(rate);
        self.expected_rate = (!rate.is_zero()).then_some(rate);
        self.rate_too_low = false;
    }

    /// Sends money without caring how much arrives. The receiver's limits
    /// are converted at whatever rate replies show.
    pub fn dangerously_ignore_exchange_rate(&mut self) {
        self.set_exchange_rate(Rate::ZERO);
    }

    /// Starts measuring the exchange rate with unfulfillable probes of
    /// growing size, as the original did (spec 4 §11.10). The result is
    /// reported as an event and, if precise enough, adopted as the rate less
    /// the policy's slippage.
    /// A measurement already in progress continues; calling again does
    /// nothing.
    pub fn measure_exchange_rate(&mut self) {
        if self.probing.is_some() || !matches!(self.state, State::Open) {
            return;
        }
        let mut probing = Probing {
            queue: [0; MAX_PROBES],
            queued: 0,
            outstanding: 0,
            best: None,
            maxima: [0; MAX_PROBES],
            maxima_count: 0,
            smallest_temporary: None,
            volleys: 1,
        };
        for amount in [1, 1_000, 1_000_000, 1_000_000_000, 1_000_000_000_000] {
            probing.enqueue(amount.min(self.max_packet_amount));
        }
        self.probing = Some(probing);
    }

    /// Starts closing the connection. Unsent money and data are dropped.
    pub fn close(&mut self, now: Timestamp, code: ErrorCode) {
        if !matches!(self.state, State::Open) {
            return;
        }
        for stream in &mut self.streams {
            if stream.local_close.is_none() {
                stream.local_close = Some(code);
            }
            stream.send_max = stream.sent + stream.held;
        }
        self.probing = None;
        if self.remote_address.is_none() || !self.connected {
            // Nobody to tell.
            self.finish_close(code, false);
            return;
        }
        self.state = State::Closing {
            code,
            deadline: now.saturating_add_ms(self.policy.close_timeout_ms),
        };
    }

    /// Ends the connection. Nothing new is sent, but packets in flight still
    /// settle when their responses arrive or they expire: money they carried
    /// may have been delivered, and dropping them would under-report it.
    fn finish_close(&mut self, code: ErrorCode, by_remote: bool) {
        if matches!(self.state, State::Closed) {
            return;
        }
        self.state = State::Closed;
        self.retry_at = None;
        self.probing = None;
        self.push_event(Event::ConnectionClosed { code, by_remote });
    }

    /// Whether the connection is closed and nothing is in flight any more.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.is_closed() && self.slots.iter().all(|slot| !slot.active)
    }

    /// Events one incoming packet can cause at most: a stream opened, data
    /// and money for each stream, every stream closed, and connection
    /// events. Packets are refused while the queue lacks this much room, so
    /// a remote cannot overflow it (D16).
    fn event_reserve(&self) -> usize {
        3 * self.streams.capacity() + 4
    }

    fn has_event_room(&self) -> bool {
        self.events.len() + self.event_reserve() <= self.events_capacity
    }

    /// The next event, if any.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// Whether events are waiting to be polled.
    #[must_use]
    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    fn push_event(&mut self, event: Event) {
        if let Some(key) = event.coalescing_key()
            && self
                .events
                .iter()
                .any(|queued| queued.coalescing_key() == Some(key))
        {
            return;
        }
        assert!(
            self.events.len() < self.events_capacity,
            "event queue full: the application must poll events"
        );
        self.events.push_back(event);
    }

    /// When [`handle_timeout`](Self::handle_timeout) should next be called.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<Timestamp> {
        let expiries = self
            .slots
            .iter()
            .filter(|slot| slot.active)
            .map(|slot| slot.expires_at);
        let closing = match self.state {
            State::Closing { deadline, .. } => Some(deadline),
            _ => None,
        };
        expiries
            .chain(closing)
            .chain(self.retry_at)
            .chain(self.idle_deadline())
            .min()
    }

    fn idle_deadline(&self) -> Option<Timestamp> {
        if matches!(self.state, State::Closed) {
            return None;
        }
        self.last_activity
            .map(|at| at.saturating_add_ms(self.policy.idle_timeout_ms))
    }

    fn allowed_concurrency(&self) -> usize {
        (self.concurrency_milli / 1000).max(1) as usize
    }

    fn active_slots(&self, kind: PacketKind) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.active && slot.kind == kind)
            .count()
    }

    fn increase_concurrency(&mut self) {
        let max = self.policy.max_concurrency * 1000;
        self.concurrency_milli = (self.concurrency_milli + 1000).min(max);
    }

    fn decrease_concurrency(&mut self) {
        let min = self.policy.min_concurrency * 1000;
        self.concurrency_milli = (self.concurrency_milli / 2).max(min);
    }

    fn mark_remote_stream_closed(&mut self, id: u64) {
        self.closed_remote_streams.mark(id / 2);
    }

    fn is_remote_stream_closed(&self, id: u64) -> bool {
        self.closed_remote_streams.is_closed(id / 2)
    }

    /// Our connection-level receive limit, advertised as `ConnectionMaxData`:
    /// what the application read on open streams plus the window.
    pub(super) fn connection_max_data(&self) -> u64 {
        self.streams
            .iter()
            .map(|stream| stream.receive.read_offset())
            .fold(
                self.policy.connection_receive_window_bytes,
                u64::saturating_add,
            )
    }

    /// The highest offsets received on open streams, added up.
    fn data_received_total(&self) -> u64 {
        self.streams
            .iter()
            .map(|stream| stream.receive.highest_offset())
            .fold(0, u64::saturating_add)
    }

    /// The highest offsets sent on open streams, added up.
    fn data_sent_total(&self) -> u64 {
        self.streams
            .iter()
            .map(|stream| stream.send.high_offset())
            .fold(0, u64::saturating_add)
    }

    /// Checks the connection's internal consistency. Tests and simulations
    /// call this after every step; it panics on any violation.
    pub fn check_invariants(&self) {
        assert!(
            self.streams.len() <= self.streams.capacity(),
            "stream table overflow"
        );
        assert!(
            self.events.len() <= self.events_capacity,
            "event queue overflow"
        );
        let min = self.policy.min_concurrency * 1000;
        let max = self.policy.max_concurrency * 1000;
        assert!(
            (min..=max).contains(&self.concurrency_milli),
            "concurrency out of bounds"
        );
        for stream in &self.streams {
            stream.check_invariants();
            let held: u64 = self
                .slots
                .iter()
                .filter(|slot| slot.active)
                .flat_map(|slot| slot.money.iter())
                .filter(|(id, _)| *id == stream.id)
                .map(|(_, amount)| amount)
                .sum();
            assert!(held == stream.held, "stream {} hold mismatch", stream.id);
            let parity_ok = self.is_local_stream_id(stream.id) != stream.opened_by_remote;
            assert!(parity_ok, "stream {} has the wrong parity", stream.id);
        }
        let normal = self.active_slots(PacketKind::Normal);
        assert!(
            normal <= self.policy.max_concurrency as usize,
            "more packets in flight than the maximum"
        );
        if !matches!(self.state, State::Closed) {
            assert!(
                self.data_received_total() <= self.connection_max_data(),
                "received beyond the connection window"
            );
        }
    }
}

impl Probing {
    fn enqueue(&mut self, amount: u64) {
        if amount == 0 {
            return;
        }
        let queued = self.queue.get(..self.queued).unwrap_or_default();
        if queued.contains(&amount) {
            return;
        }
        if let Some(slot) = self.queue.get_mut(self.queued) {
            *slot = amount;
            self.queued += 1;
        }
    }

    fn dequeue(&mut self) -> Option<u64> {
        if self.queued == 0 {
            return None;
        }
        let amount = self.queue.first().copied();
        self.queue.copy_within(1.., 0);
        self.queued -= 1;
        amount
    }
}

/// Decimal digits of `value`; zero has none.
const fn digits(mut value: u64) -> u32 {
    let mut count = 0;
    while value > 0 {
        value /= 10;
        count += 1;
    }
    count
}

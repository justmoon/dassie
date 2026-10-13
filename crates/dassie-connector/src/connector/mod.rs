//! The connector: endpoints, packets in flight, and the trace.

mod invariants;
mod prepare;
mod response;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use dassie_ilp::{Address, ErrorCode};
use dassie_ledger::{AccountId, LedgerId, TraceId, TransferId};

use crate::config::Config;
use crate::endpoint::{Endpoint, EndpointError, EndpointId, Endpoints};
use crate::trace::{PacketRecord, Trace, TraceEvent};

/// What became of an incoming Prepare. The packet to send is in the buffer
/// the caller passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Send the Prepare in the buffer on.
    Forward(Forwarded),
    /// Answer the source at once, with the request ID it used: the buffer
    /// holds a Fulfill (ILDCP) or a Reject of this many bytes.
    Respond(usize),
    /// The source reused a request ID that is still in flight. Nothing was
    /// done; the transport should report the error its own way.
    Duplicate,
    /// The source is not an endpoint (any more). Nothing was done.
    UnknownSource,
}

/// A Prepare to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forwarded {
    /// The endpoint to send it to.
    pub to: EndpointId,
    /// The request ID to send it with; the response comes back with it.
    pub request_id: u32,
    /// The packet's length in the buffer.
    pub length: usize,
}

/// A response to send back to where a Prepare came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// The endpoint the Prepare came from.
    pub to: EndpointId,
    /// The request ID it came with.
    pub request_id: u32,
    /// The response's length in the buffer.
    pub length: usize,
}

/// Counters for instrumentation and simulation checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Prepares received.
    pub prepares: u64,
    /// Prepares forwarded.
    pub forwarded: u64,
    /// Prepares answered at once (ILDCP or a reject).
    pub answered: u64,
    /// Forwarded packets fulfilled.
    pub fulfilled: u64,
    /// Forwarded packets rejected downstream.
    pub rejected: u64,
    /// Forwarded packets that expired, or whose next hop went away.
    pub expired: u64,
    /// Fulfillments that did not match the condition (F05).
    pub wrong_fulfillments: u64,
    /// Responses for no packet in flight: late, duplicate or bogus.
    pub unexpected_responses: u64,
    /// Prepares that reused a request ID in flight.
    pub duplicates: u64,
}

/// A packet in flight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pending {
    pub trace: TraceId,
    pub source: EndpointId,
    pub source_request_id: u32,
    /// The source is gone: the response has nowhere to go, but the
    /// transfers are still settled by it.
    pub source_gone: bool,
    pub condition: [u8; 32],
    /// The outgoing packet's expiry.
    pub expires_at_ms: u64,
    pub transfers: [Option<TransferId>; 2],
    /// Set when the next hop went away: the code the packet is rejected
    /// with at once.
    pub failure: Option<ErrorCode>,
}

/// The connector.
#[derive(Debug)]
pub struct Connector {
    config: Config,
    own_address: String,
    endpoints: Endpoints,
    /// By destination and outgoing request ID.
    pending: BTreeMap<(EndpointId, u32), Pending>,
    /// When each packet in flight expires.
    deadlines: BTreeSet<(u64, EndpointId, u32)>,
    /// Incoming request IDs in flight, by source.
    incoming: BTreeSet<(EndpointId, u32)>,
    trace: Trace,
    next_sequence: u64,
    stats: Stats,
    /// Reused for linked transfers, so the packet path does not allocate.
    scratch: Vec<TransferId>,
}

impl Connector {
    /// A connector for the node at `own_address`.
    ///
    /// # Panics
    ///
    /// If the config does not validate or the address is not one.
    #[must_use]
    pub fn new(config: Config, own_address: &str) -> Self {
        config.validate();
        assert!(Address::parse(own_address).is_ok(), "not an ILP address");
        Self {
            config,
            own_address: String::from(own_address),
            endpoints: Endpoints::default(),
            pending: BTreeMap::new(),
            deadlines: BTreeSet::new(),
            incoming: BTreeSet::new(),
            trace: Trace::new(config.trace_capacity, config.max_events),
            next_sequence: 0,
            stats: Stats::default(),
            scratch: Vec::with_capacity(2),
        }
    }

    /// The node's ILP address.
    #[must_use]
    pub fn own_address(&self) -> &str {
        &self.own_address
    }

    /// Adds an endpoint at `address`, which must be below the node's own and
    /// not overlap another endpoint's, with the account its packets use.
    ///
    /// # Errors
    ///
    /// If the table is full, the ID is taken, or the address is unusable.
    pub fn add_endpoint(
        &mut self,
        id: EndpointId,
        address: &str,
        account: AccountId,
        ledger: LedgerId,
    ) -> Result<(), EndpointError> {
        if self.endpoints.len() >= self.config.max_endpoints {
            return Err(EndpointError::Full);
        }
        let own = Address::parse(&self.own_address).map_err(|_| EndpointError::InvalidAddress)?;
        let endpoint = Endpoint {
            address: String::from(address),
            account,
            ledger,
            next_request_id: 0,
            pending_from: 0,
        };
        self.endpoints.insert(id, endpoint, own)
    }

    /// Removes an endpoint. Packets in flight to it are failed with T01
    /// at once, through [`handle_timeout`](Self::handle_timeout); their
    /// transfers are voided. A child may already have fulfilled them: the
    /// loss is on the child's own account, since we never got the
    /// fulfillment. Packets it sent keep running, and their responses are
    /// dropped. Returns whether it existed.
    pub fn remove_endpoint(&mut self, now_ms: u64, id: EndpointId) -> bool {
        if self.endpoints.remove(id).is_none() {
            return false;
        }
        let doomed: Vec<(u64, EndpointId, u32)> = self
            .pending
            .iter()
            .filter(|((destination, _), _)| *destination == id)
            .map(|(&(destination, request_id), entry)| {
                (entry.expires_at_ms, destination, request_id)
            })
            .collect();
        for (expires_at_ms, destination, request_id) in doomed {
            self.deadlines
                .remove(&(expires_at_ms, destination, request_id));
            if let Some(entry) = self.pending.get_mut(&(destination, request_id)) {
                entry.failure = Some(ErrorCode::T01_PEER_UNREACHABLE);
                entry.expires_at_ms = now_ms;
            }
            self.deadlines.insert((now_ms, destination, request_id));
        }
        for entry in self.pending.values_mut() {
            if entry.source == id {
                entry.source_gone = true;
            }
        }
        self.incoming.retain(|(source, _)| *source != id);
        true
    }

    /// The address an endpoint was added with.
    #[must_use]
    pub fn endpoint_address(&self, id: EndpointId) -> Option<&str> {
        self.endpoints
            .get(id)
            .map(|endpoint| endpoint.address.as_str())
    }

    /// The earliest deadline of a packet in flight.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<u64> {
        self.deadlines.first().map(|&(at, _, _)| at)
    }

    /// Packets in flight.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// The next trace event for live subscribers.
    pub fn poll_event(&mut self) -> Option<TraceEvent> {
        self.trace.poll_event()
    }

    /// The trace's records after `after`, oldest first.
    pub fn packets_after(&self, after: Option<TraceId>) -> impl Iterator<Item = &PacketRecord> {
        self.trace.records_after(after)
    }

    /// Counters.
    #[must_use]
    pub const fn stats(&self) -> Stats {
        self.stats
    }

    /// The bounds.
    #[must_use]
    pub const fn config(&self) -> Config {
        self.config
    }

    fn next_trace(&mut self) -> TraceId {
        let trace = TraceId {
            epoch: self.config.epoch,
            sequence: self.next_sequence,
        };
        self.next_sequence += 1;
        trace
    }

    /// A request ID for `destination` that is not in flight to it.
    fn request_id_for(&mut self, destination: EndpointId) -> Option<u32> {
        let endpoint = self.endpoints.get_mut(destination)?;
        // At most `max_pending` IDs are taken, so one of the next
        // `pending + 1` candidates is free.
        for _ in 0..=self.pending.len() {
            let candidate = endpoint.next_request_id;
            endpoint.next_request_id = candidate.wrapping_add(1);
            if !self.pending.contains_key(&(destination, candidate)) {
                return Some(candidate);
            }
        }
        None
    }
}

//! The connector: endpoints, packets in flight, and the trace.

mod invariants;
mod prepare;
mod response;

pub use response::{Posted, Responded};

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use dassie_ilp::{Address, ErrorCode};
use dassie_ledger::{AccountId, LedgerId, TraceId, TransferId};

use crate::PacketTransfers;
use crate::cap::{CapError, CapState, CapUsage, CapView, SpendingCap};
use crate::charges::Charges;
use crate::config::Config;
use crate::endpoint::{Endpoint, EndpointError, EndpointId, Endpoints};
use crate::trace::{PacketRecord, Trace, TraceEvent};

/// Most next hops a route lists.
pub const MAX_ROUTE_OPTIONS: usize = 4;

/// Why routes could not be set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutesError {
    /// More routes than the table holds.
    TooMany,
    /// A prefix is not an ILP address, or lies below our own.
    InvalidPrefix,
}

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
    /// What it carries in from its source, and out to its destination.
    pub amount_in: u64,
    pub amount_out: u64,
    /// The outgoing packet's expiry.
    pub expires_at_ms: u64,
    pub transfers: PacketTransfers,
    /// Set when the packet cannot reach its next hop: the code and message
    /// it is rejected with at once.
    pub failure: Option<(ErrorCode, &'static str)>,
    /// The account it spends from and the amount, if it moves money out of
    /// its owner's funds: what a spending cap counts.
    pub charge: Option<(AccountId, u64)>,
}

/// The connector.
#[derive(Debug)]
pub struct Connector {
    config: Config,
    own_address: String,
    endpoints: Endpoints,
    /// Routes beyond our own address: prefix to next hops, best first.
    routes: BTreeMap<String, [Option<EndpointId>; MAX_ROUTE_OPTIONS]>,
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
    /// Spending caps, by the account they limit.
    caps: BTreeMap<AccountId, CapState>,
    /// Caps whose usage changed since it was last polled.
    cap_changes: BTreeSet<AccountId>,
    /// What transit from one peer to another is charged.
    charges: Charges,
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
            routes: BTreeMap::new(),
            pending: BTreeMap::new(),
            deadlines: BTreeSet::new(),
            incoming: BTreeSet::new(),
            trace: Trace::new(config.trace_capacity, config.max_events),
            next_sequence: 0,
            stats: Stats::default(),
            scratch: Vec::with_capacity(3),
            caps: BTreeMap::new(),
            cap_changes: BTreeSet::new(),
            charges: Charges::NONE,
        }
    }

    /// Sets what transit from one peer to another is charged, from the
    /// next packet on. Returns false, changing nothing, if the charges
    /// are not valid.
    pub fn set_charges(&mut self, charges: Charges) -> bool {
        if !charges.is_valid() {
            return false;
        }
        self.charges = charges;
        true
    }

    /// What transit is charged.
    #[must_use]
    pub const fn charges(&self) -> Charges {
        self.charges
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
        if id.is_peer() {
            return Err(EndpointError::InvalidAddress);
        }
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
            money_from: 0,
            money_to: 0,
            draining: false,
            refusing: false,
        };
        self.endpoints.insert(id, endpoint, own)
    }

    /// Adds a peer at `address`, which is only what ILDCP tells it: packets
    /// reach peers through the route table. Its first request ID is chosen
    /// by the caller at random, so that after a restart our new packets do
    /// not reuse IDs the peer still holds from before.
    ///
    /// # Errors
    ///
    /// If the table is full, the ID is taken or not a peer's, or the address
    /// is not one.
    pub fn add_peer(
        &mut self,
        id: EndpointId,
        address: &str,
        account: AccountId,
        ledger: LedgerId,
        first_request_id: u32,
    ) -> Result<(), EndpointError> {
        if !id.is_peer() {
            return Err(EndpointError::Duplicate);
        }
        if self.endpoints.len() >= self.config.max_endpoints {
            return Err(EndpointError::Full);
        }
        let own = Address::parse(&self.own_address).map_err(|_| EndpointError::InvalidAddress)?;
        let endpoint = Endpoint {
            address: String::from(address),
            account,
            ledger,
            next_request_id: first_request_id,
            pending_from: 0,
            money_from: 0,
            money_to: 0,
            draining: false,
            refusing: false,
        };
        self.endpoints.insert(id, endpoint, own)
    }

    /// Replaces the route table: for each prefix outside our own address,
    /// its next hops, best first. Hops that are not endpoints (any more)
    /// are skipped when routing.
    ///
    /// # Errors
    ///
    /// Too many routes, or a prefix that is not a valid one; the table is
    /// then unchanged.
    pub fn set_routes<'a>(
        &mut self,
        routes: impl IntoIterator<Item = (&'a str, &'a [EndpointId])>,
    ) -> Result<(), RoutesError> {
        let own = Address::parse(&self.own_address).map_err(|_| RoutesError::InvalidPrefix)?;
        let mut table = BTreeMap::new();
        for (prefix, hops) in routes {
            let address = Address::parse(prefix).map_err(|_| RoutesError::InvalidPrefix)?;
            if address.has_prefix(own) {
                return Err(RoutesError::InvalidPrefix);
            }
            if table.len() >= self.config.max_routes {
                return Err(RoutesError::TooMany);
            }
            let mut options = [None; MAX_ROUTE_OPTIONS];
            for (slot, hop) in options.iter_mut().zip(hops) {
                *slot = Some(*hop);
            }
            table.insert(String::from(prefix), options);
        }
        self.routes = table;
        Ok(())
    }

    /// Fails a forwarded packet that surely did not reach its next hop:
    /// it is rejected upstream with `code` at once, through
    /// [`handle_timeout`](Self::handle_timeout), and its transfers voided.
    /// Use it only when the next hop cannot have acted on the packet; when
    /// unsure, let it expire: a next hop that fulfilled a packet we already
    /// failed would bear its loss. Returns whether the packet was in
    /// flight.
    pub fn fail_forwarded(
        &mut self,
        now_ms: u64,
        to: EndpointId,
        request_id: u32,
        code: ErrorCode,
        message: &'static str,
    ) -> bool {
        let Some(entry) = self.pending.get_mut(&(to, request_id)) else {
            return false;
        };
        self.deadlines
            .remove(&(entry.expires_at_ms, to, request_id));
        entry.failure = Some((code, message));
        entry.expires_at_ms = now_ms;
        self.deadlines.insert((now_ms, to, request_id));
        true
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
                entry.failure = Some((ErrorCode::T01_PEER_UNREACHABLE, "next hop went away"));
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

    /// Stops, or resumes, new Prepares to and from an endpoint, whose
    /// peering is ending: packets in flight finish, and their responses
    /// still flow. Returns whether the endpoint exists.
    pub fn set_draining(&mut self, id: EndpointId, draining: bool) -> bool {
        self.endpoints
            .get_mut(id)
            .map(|endpoint| endpoint.draining = draining)
            .is_some()
    }

    /// Stops, or resumes, new Prepares from an endpoint, while Prepares to
    /// it still go: a peering we are ending, and have not told the peer of
    /// yet (D28). Its Prepares are refused as for want of liquidity, which
    /// tells it nothing more. Returns whether the endpoint exists.
    pub fn set_refusing(&mut self, id: EndpointId, refusing: bool) -> bool {
        self.endpoints
            .get_mut(id)
            .map(|endpoint| endpoint.refusing = refusing)
            .is_some()
    }

    /// Packets in flight to or from an endpoint: once none are left, a
    /// draining endpoint can go with no packet's outcome left unknown.
    #[must_use]
    pub fn pending_for(&self, id: EndpointId) -> usize {
        let from = self
            .endpoints
            .get(id)
            .map_or(0, |endpoint| endpoint.pending_from);
        let to = self.pending.range((id, 0)..=(id, u32::MAX)).count();
        from + to
    }

    /// Packets in flight that came from an endpoint.
    #[must_use]
    pub fn pending_from(&self, id: EndpointId) -> usize {
        self.endpoints
            .get(id)
            .map_or(0, |endpoint| endpoint.pending_from)
    }

    /// Limits what `account` may spend out of its owner's funds, from
    /// `now_ms` on (spec/areas/connector.md#spending-caps), or lifts its
    /// cap. A new cap starts with nothing used; a changed one keeps what was
    /// used, rounded up to the new window. Packets in flight count against
    /// it either way.
    ///
    /// # Errors
    ///
    /// If the cap is invalid, or the table of caps is full.
    pub fn set_cap(
        &mut self,
        now_ms: u64,
        account: AccountId,
        cap: Option<SpendingCap>,
    ) -> Result<(), CapError> {
        let Some(cap) = cap else {
            self.caps.remove(&account);
            self.cap_changes.remove(&account);
            return Ok(());
        };
        let usage = match self.caps.get(&account) {
            Some(state) => state.usage.rescaled(state.cap, cap, now_ms),
            None => CapUsage::new(now_ms),
        };
        self.insert_cap(account, cap, usage)
    }

    /// Restores a cap with the usage persisted for it.
    ///
    /// # Errors
    ///
    /// As [`set_cap`](Self::set_cap).
    pub fn restore_cap(
        &mut self,
        account: AccountId,
        cap: SpendingCap,
        usage: CapUsage,
    ) -> Result<(), CapError> {
        self.insert_cap(account, cap, usage)
    }

    fn insert_cap(
        &mut self,
        account: AccountId,
        cap: SpendingCap,
        usage: CapUsage,
    ) -> Result<(), CapError> {
        if !cap.is_valid() {
            return Err(CapError::Invalid);
        }
        if !self.caps.contains_key(&account) && self.caps.len() >= self.config.max_caps {
            return Err(CapError::Full);
        }
        let reserved = self
            .pending
            .values()
            .filter_map(|entry| entry.charge)
            .filter(|(charged, _)| *charged == account)
            .map(|(_, amount)| u128::from(amount))
            .sum();
        let mut state = CapState {
            cap,
            usage,
            reserved,
            over: false,
        };
        state.over = !state.within();
        self.caps.insert(account, state);
        Ok(())
    }

    /// The cap on `account` as of `now_ms`, if it has one.
    #[must_use]
    pub fn cap(&self, account: AccountId, now_ms: u64) -> Option<CapView> {
        self.caps.get(&account).map(|state| state.view(now_ms))
    }

    /// The next cap whose usage changed, with its cap and usage: what to
    /// persist with the step's journal.
    pub fn poll_cap_change(&mut self) -> Option<(AccountId, SpendingCap, CapUsage)> {
        while let Some(account) = self.cap_changes.pop_first() {
            if let Some(state) = self.caps.get(&account) {
                return Some((account, state.cap, state.usage));
            }
        }
        None
    }

    /// Every endpoint: its ID, address and ledger.
    pub fn endpoints(&self) -> impl Iterator<Item = (EndpointId, &str, LedgerId)> {
        self.endpoints
            .iter()
            .map(|(id, endpoint)| (*id, endpoint.address.as_str(), endpoint.ledger))
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

    /// The next hop for `destination`: below our own address only
    /// children (or F02, NET-34); beyond it, the longest whole-segment
    /// prefix in the route table, its first hop that is an endpoint and not
    /// the packet's source, so packets do not bounce between peers.
    pub(crate) fn next_hop(
        &self,
        destination: Address<'_>,
        source: Option<EndpointId>,
    ) -> Option<EndpointId> {
        let own = Address::parse(&self.own_address).ok()?;
        if destination.has_prefix(own) {
            // A child may address itself; only routes between nodes could
            // loop.
            return self.endpoints.route_child(destination);
        }
        let text = destination.as_str();
        let mut end = text.len();
        loop {
            let prefix = text.get(..end)?;
            if let Some(options) = self.routes.get(prefix) {
                let hop = options.iter().flatten().find(|hop| {
                    Some(**hop) != source
                        && self
                            .endpoints
                            .get(**hop)
                            .is_some_and(|endpoint| !endpoint.draining)
                });
                if let Some(hop) = hop {
                    return Some(*hop);
                }
            }
            end = prefix.rfind('.')?;
        }
    }

    /// Where a packet to `destination` from one of the node's own
    /// endpoints would go first, and that endpoint's ledger: what such a
    /// packet must be paid from, since the node's own endpoints never
    /// convert.
    #[must_use]
    pub fn first_hop(&self, destination: &str) -> Option<(EndpointId, LedgerId)> {
        let address = Address::parse(destination).ok()?;
        let hop = self.next_hop(address, None)?;
        self.endpoints
            .get(hop)
            .map(|endpoint| (hop, endpoint.ledger))
    }

    /// The routes, for inspection.
    pub fn routes(&self) -> impl Iterator<Item = (&str, impl Iterator<Item = EndpointId> + '_)> {
        self.routes
            .iter()
            .map(|(prefix, options)| (prefix.as_str(), options.iter().flatten().copied()))
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

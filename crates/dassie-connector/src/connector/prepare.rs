//! Incoming Prepares: ILDCP, routing, expiry, amounts and the ledger
//! (spec/areas/connector.md#handling-a-prepare).

use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode, Fulfill, MAX_PACKET_LENGTH, Prepare, Reject, Timestamp,
    ildcp,
};
use dassie_ledger::{
    AccountId, AccountName, AccountPath, Ledger, Reference, TraceId, TransferError, TransferSpec,
};
use dassie_oer::{Decode, Encode};

use super::{Connector, Forwarded, Outcome, Pending};
use crate::PacketTransfers;
use crate::cap::CapRefusal;
use crate::charges::Charges;
use crate::endpoint::EndpointId;
use crate::fx::{Rates, convert};
use crate::trace::{PacketRecord, PacketState};

/// Room for an ILDCP response: the longest address, a scale and the
/// longest asset code, with their length prefixes.
const ILDCP_RESPONSE_LENGTH: usize = 3 + dassie_ilp::MAX_ADDRESS_LENGTH + 1 + 3 + 255;

/// A refusal before the packet goes anywhere.
struct Refusal {
    code: ErrorCode,
    message: &'static str,
    /// F08 data.
    too_large: Option<AmountTooLarge>,
}

const fn refuse(code: ErrorCode, message: &'static str) -> Refusal {
    Refusal {
        code,
        message,
        too_large: None,
    }
}

/// Where a routed packet goes, and what it books.
struct Route {
    to: EndpointId,
    expires_at: Timestamp,
    /// Our fee, out of the incoming amount: only on transit.
    fee: u64,
    amount_out: u64,
}

/// A packet's pending transfers, and what a spending cap counts of it.
struct Booked {
    transfers: PacketTransfers,
    charge: Option<(AccountId, u64)>,
}

impl Connector {
    /// Handles a Prepare from `source` with the request ID it used.
    ///
    /// # Panics
    ///
    /// If `out` is shorter than [`MAX_PACKET_LENGTH`], or the ledger's
    /// journal is full: the caller persists it after every step.
    #[allow(clippy::too_many_arguments)]
    pub fn handle_prepare(
        &mut self,
        now_ms: u64,
        source: EndpointId,
        request_id: u32,
        packet: &[u8],
        ledger: &mut Ledger,
        rates: &Rates,
        out: &mut [u8],
    ) -> Outcome {
        assert!(
            out.len() >= MAX_PACKET_LENGTH,
            "buffer shorter than a packet"
        );
        if self.endpoints.get(source).is_none() {
            return Outcome::UnknownSource;
        }
        if self.incoming.contains(&(source, request_id)) {
            self.stats.duplicates += 1;
            return Outcome::Duplicate;
        }
        self.stats.prepares += 1;
        let trace = self.next_trace();
        if self
            .endpoints
            .get(source)
            .is_some_and(|endpoint| endpoint.draining)
        {
            self.trace
                .push(PacketRecord::new(trace, source, "", 0, now_ms));
            return self.answer(
                now_ms,
                trace,
                &refuse(ErrorCode::T01_PEER_UNREACHABLE, "the peering is ending"),
                out,
            );
        }
        let Ok(prepare) = Prepare::decode_exact(packet) else {
            self.trace
                .push(PacketRecord::new(trace, source, "", 0, now_ms));
            return self.answer(
                now_ms,
                trace,
                &refuse(ErrorCode::F01_INVALID_PACKET, "invalid packet"),
                out,
            );
        };
        let record = PacketRecord::new(
            trace,
            source,
            prepare.destination.as_str(),
            prepare.amount,
            now_ms,
        );
        self.trace.push(record);
        if prepare.destination.has_prefix(ildcp::ADDRESS) {
            return self.ildcp(now_ms, trace, source, &prepare, out);
        }
        let route = match self.route(now_ms, source, &prepare, rates) {
            Ok(route) => route,
            Err(refusal) => return self.answer(now_ms, trace, &refusal, out),
        };
        let booked = match self.book(now_ms, trace, source, &route, prepare.amount, ledger) {
            Ok(booked) => booked,
            Err(refusal) => return self.answer(now_ms, trace, &refusal, out),
        };
        self.forward(trace, source, request_id, &prepare, &route, booked, out)
    }

    /// Answers at once with a reject from this node.
    fn answer(
        &mut self,
        now_ms: u64,
        trace: TraceId,
        refusal: &Refusal,
        out: &mut [u8],
    ) -> Outcome {
        let data = refusal.too_large.map(AmountTooLarge::to_bytes);
        let reject = Reject {
            code: refusal.code,
            triggered_by: Address::parse(&self.own_address).ok(),
            message: refusal.message,
            data: data.as_ref().map_or(&[], |data| data.as_slice()),
        };
        let length = reject.encode(out);
        self.trace.update(trace, |record| {
            record.state = PacketState::Rejected(refusal.code);
            record.resolved_at_ms = Some(now_ms);
        });
        self.stats.answered += 1;
        Outcome::Respond(length)
    }

    /// Answers an ILDCP request with the source's routed address and its
    /// ledger's asset (ILP-7, NET-25). It must carry no money and the ILDCP
    /// condition, or it is not one.
    fn ildcp(
        &mut self,
        now_ms: u64,
        trace: TraceId,
        source: EndpointId,
        prepare: &Prepare<'_>,
        out: &mut [u8],
    ) -> Outcome {
        if prepare.amount != 0 || !ildcp::is_request(prepare) {
            let refusal = refuse(ErrorCode::F01_INVALID_PACKET, "not an ILDCP request");
            return self.answer(now_ms, trace, &refusal, out);
        }
        if prepare.expires_at <= Timestamp::from_unix_ms(now_ms) {
            let refusal = refuse(ErrorCode::R02_INSUFFICIENT_TIMEOUT, "expired");
            return self.answer(now_ms, trace, &refusal, out);
        }
        let Some(endpoint) = self.endpoints.get(source) else {
            return Outcome::UnknownSource;
        };
        let Ok(address) = Address::parse(&endpoint.address) else {
            let refusal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
            return self.answer(now_ms, trace, &refusal, out);
        };
        let asset = endpoint.ledger.asset();
        let response = ildcp::Response {
            address,
            asset_scale: asset.scale,
            asset_code: asset.code,
        };
        let mut data = [0u8; ILDCP_RESPONSE_LENGTH];
        let data_length = response.encode(&mut data);
        let fulfill = Fulfill {
            fulfillment: ildcp::FULFILLMENT,
            data: data.get(..data_length).unwrap_or_default(),
        };
        let length = fulfill.encode(out);
        self.trace.update(trace, |record| {
            record.state = PacketState::Fulfilled;
            record.resolved_at_ms = Some(now_ms);
        });
        self.stats.answered += 1;
        Outcome::Respond(length)
    }

    /// Capacity, route, expiry, amount and conversion.
    fn route(
        &self,
        now_ms: u64,
        source: EndpointId,
        prepare: &Prepare<'_>,
        rates: &Rates,
    ) -> Result<Route, Refusal> {
        let config = &self.config;
        let from = self
            .endpoints
            .get(source)
            .ok_or(refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error"))?;
        if self.pending.len() >= config.max_pending {
            return Err(refuse(ErrorCode::T03_CONNECTOR_BUSY, "connector busy"));
        }
        if from.pending_from >= config.max_pending_from(source) {
            return Err(refuse(
                ErrorCode::T05_RATE_LIMITED,
                "too many packets in flight",
            ));
        }
        // Unroutable, including anything below our own address that is no
        // endpoint's and our own address itself (ILP-3, NET-34).
        let to = self
            .next_hop(prepare.destination, Some(source))
            .ok_or(refuse(ErrorCode::F02_UNREACHABLE, "no route found"))?;
        let incoming_ms = prepare.expires_at.unix_ms;
        let latest_ms = incoming_ms.saturating_sub(config.message_window_ms);
        if latest_ms < now_ms.saturating_add(config.min_timeout_ms) {
            return Err(refuse(
                ErrorCode::R02_INSUFFICIENT_TIMEOUT,
                "insufficient timeout",
            ));
        }
        let expires_at = Timestamp::from_unix_ms(latest_ms.min(now_ms + config.max_hold_ms));
        if prepare.amount > config.max_packet_amount {
            return Err(Refusal {
                code: ErrorCode::F08_AMOUNT_TOO_LARGE,
                message: "packet amount exceeds maximum allowed amount",
                too_large: Some(AmountTooLarge {
                    received_amount: prepare.amount,
                    maximum_amount: config.max_packet_amount,
                }),
            });
        }
        let (fee, amount_out) = self.amounts(now_ms, source, to, prepare.amount, rates)?;
        self.admits_in_flight(source, to, prepare.amount, amount_out)?;
        Ok(Route {
            to,
            expires_at,
            fee,
            amount_out,
        })
    }

    /// Whether the money in flight with each peer the packet passes stays
    /// within [`Config::max_in_flight`] each way, or T05 (D24). A crash
    /// loses the Fulfills of what was in flight, so this bounds how far
    /// a crash makes two peers' books drift apart. Half a packet of it
    /// stays free for smaller packets while a payment fills the link. A
    /// packet always passes alone, even one a conversion made larger than
    /// the limit.
    fn admits_in_flight(
        &self,
        source: EndpointId,
        to: EndpointId,
        amount_in: u64,
        amount_out: u64,
    ) -> Result<(), Refusal> {
        let max = self.config.max_in_flight();
        let over = |in_flight: u64, amount: u64| {
            amount > 0 && in_flight > 0 && in_flight.saturating_add(amount) > max
        };
        let from = self.endpoints.get(source).map_or(0, |e| e.money_from);
        let onward = self.endpoints.get(to).map_or(0, |e| e.money_to);
        if (source.is_peer() && over(from, amount_in)) || (to.is_peer() && over(onward, amount_out))
        {
            return Err(refuse(
                ErrorCode::T05_RATE_LIMITED,
                "too much money in flight",
            ));
        }
        Ok(())
    }

    /// Our fee on a packet from `source` to `to`, and what goes on: only
    /// transit from one peer to another is charged, and the owner's own
    /// endpoints never convert, so what the owner holds on one ledger is
    /// never spent on another by a packet
    /// (spec/areas/ledger.md#the-owners-accounts). The first hop that
    /// charges for it converts.
    fn amounts(
        &self,
        now_ms: u64,
        source: EndpointId,
        to: EndpointId,
        amount: u64,
        rates: &Rates,
    ) -> Result<(u64, u64), Refusal> {
        let internal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
        let (Some(from), Some(destination)) = (self.endpoints.get(source), self.endpoints.get(to))
        else {
            return Err(internal);
        };
        let charges = if source.is_peer() && to.is_peer() {
            self.charges
        } else {
            Charges::NONE
        };
        let fee = charges.fee(amount);
        let net = amount - fee;
        let converts = from.ledger != destination.ledger && amount > 0;
        if converts && !source.is_peer() {
            return Err(refuse(
                ErrorCode::F02_UNREACHABLE,
                "pay from the route's ledger",
            ));
        }
        let (from_asset, to_asset) = (from.ledger.asset(), destination.ledger.asset());
        if converts
            && !rates.fresh(
                from_asset.code,
                to_asset.code,
                now_ms,
                self.config.max_rate_age_ms,
            )
        {
            return Err(refuse(
                ErrorCode::T00_INTERNAL_ERROR,
                "exchange rates are stale",
            ));
        }
        let amount_out = if converts {
            let converted = convert(rates, from_asset, to_asset, net)
                .ok_or(refuse(ErrorCode::T00_INTERNAL_ERROR, "no exchange rate"))?;
            charges.after_spread(converted)
        } else {
            net
        };
        if amount > 0 && amount_out == 0 {
            return Err(refuse(ErrorCode::F02_UNREACHABLE, "amount too small"));
        }
        Ok((fee, amount_out))
    }

    /// The packet's pending transfers: one on a ledger, two across ledgers
    /// through the FX accounts, and our fee in a third, none for nothing or
    /// between endpoints that share an account. A packet that moves money
    /// out of its owner's funds
    /// must pass the source account's spending cap first, and holds its
    /// amount there while in flight.
    fn book(
        &mut self,
        now_ms: u64,
        trace: TraceId,
        source: EndpointId,
        route: &Route,
        amount_in: u64,
        ledger: &mut Ledger,
    ) -> Result<Booked, Refusal> {
        let internal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
        let (Some(from), Some(to)) = (self.endpoints.get(source), self.endpoints.get(route.to))
        else {
            return Err(internal);
        };
        if amount_in == 0 || from.account == to.account {
            return Ok(Booked {
                transfers: [None; 3],
                charge: None,
            });
        }
        let (from_account, to_account) = (from.account, to.account);
        let counted = from.ledger != to.ledger || !same_group(ledger, from_account, to_account);
        let charge = counted.then_some((from_account, amount_in));
        if let Some(cap) = charge.and_then(|(account, _)| self.caps.get(&account)) {
            cap.admits(now_ms, amount_in)
                .map_err(|refusal| cap_refusal(refusal, amount_in))?;
        }
        let transfers = self.book_transfers(now_ms, trace, source, route, amount_in, ledger)?;
        if let Some(cap) = charge.and_then(|(account, _)| self.caps.get_mut(&account)) {
            cap.reserve(now_ms, amount_in);
        }
        Ok(Booked { transfers, charge })
    }

    /// The pending transfers of a packet that moves money, linked: the
    /// first debits the source's account; the second, across ledgers,
    /// credits the destination's from the FX accounts; the third is our
    /// fee, which debits the source's account too.
    fn book_transfers(
        &mut self,
        now_ms: u64,
        trace: TraceId,
        source: EndpointId,
        route: &Route,
        amount_in: u64,
        ledger: &mut Ledger,
    ) -> Result<PacketTransfers, Refusal> {
        let internal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
        let (Some(from), Some(to)) = (self.endpoints.get(source), self.endpoints.get(route.to))
        else {
            return Err(internal);
        };
        let reference = Reference::Packet(trace);
        let pending = |debit, credit, amount| TransferSpec {
            debit,
            credit,
            amount,
            pending: true,
            reference,
        };
        assert!(route.fee <= amount_in, "a fee above the amount");
        let net = amount_in - route.fee;
        let (first, second) = if from.ledger == to.ledger {
            // Without a conversion, what goes on is what came, less the fee.
            assert_eq!(route.amount_out, net, "what goes on is what was booked");
            (pending(from.account, to.account, net), None)
        } else {
            let revenue = ledger.find(&AccountPath::new(from.ledger, AccountName::RevenueFx));
            let expenses = ledger.find(&AccountPath::new(to.ledger, AccountName::ExpensesFx));
            let (Some(revenue), Some(expenses)) = (revenue, expenses) else {
                return Err(internal);
            };
            (
                pending(from.account, revenue, net),
                Some(pending(expenses, to.account, route.amount_out)),
            )
        };
        let fee = if route.fee > 0 {
            let fees = ledger
                .find(&AccountPath::new(from.ledger, AccountName::RevenueFees))
                .ok_or(internal)?;
            Some(pending(from.account, fees, route.fee))
        } else {
            None
        };
        let slots = [Some(first), second, fee];
        let mut specs = [first; 3];
        let mut count = 0;
        for (target, spec) in specs.iter_mut().zip(slots.iter().flatten()) {
            *target = *spec;
            count += 1;
        }
        self.scratch.clear();
        if let Err((_, error)) = ledger.create_linked(
            now_ms,
            specs.get(..count).unwrap_or_default(),
            &mut self.scratch,
        ) {
            return Err(ledger_refusal(error));
        }
        assert_eq!(self.scratch.len(), count, "one transfer per leg");
        let mut ids = self.scratch.iter().copied();
        Ok(slots.map(|slot| slot.and_then(|_| ids.next())))
    }

    /// Registers the packet as in flight and writes the outgoing Prepare.
    #[allow(clippy::too_many_arguments)]
    fn forward(
        &mut self,
        trace: TraceId,
        source: EndpointId,
        source_request_id: u32,
        prepare: &Prepare<'_>,
        route: &Route,
        booked: Booked,
        out: &mut [u8],
    ) -> Outcome {
        let Booked { transfers, charge } = booked;
        // The route exists and the table has room: an ID is always found.
        let Some(request_id) = self.request_id_for(route.to) else {
            return Outcome::UnknownSource;
        };
        let expires_at_ms = route.expires_at.unix_ms;
        self.pending.insert(
            (route.to, request_id),
            Pending {
                trace,
                source,
                source_request_id,
                source_gone: false,
                condition: prepare.execution_condition,
                amount_in: prepare.amount,
                amount_out: route.amount_out,
                expires_at_ms,
                transfers,
                failure: None,
                charge,
            },
        );
        self.deadlines.insert((expires_at_ms, route.to, request_id));
        self.incoming.insert((source, source_request_id));
        if let Some(endpoint) = self.endpoints.get_mut(source) {
            endpoint.pending_from += 1;
            if source.is_peer() {
                endpoint.money_from += prepare.amount;
            }
        }
        if let Some(endpoint) = self.endpoints.get_mut(route.to)
            && route.to.is_peer()
        {
            endpoint.money_to += route.amount_out;
        }
        let outgoing = Prepare {
            amount: route.amount_out,
            expires_at: route.expires_at,
            ..*prepare
        };
        let length = outgoing.encode(out);
        let to = route.to;
        let amount_out = route.amount_out;
        self.trace.update(trace, |record| {
            record.destination = Some(to);
            record.amount_out = amount_out;
            record.transfers = transfers;
        });
        self.stats.forwarded += 1;
        Outcome::Forward(Forwarded {
            to,
            request_id,
            length,
        })
    }
}

/// Whether two accounts are in the same group: money between them stays
/// in their owner's funds.
fn same_group(ledger: &Ledger, a: AccountId, b: AccountId) -> bool {
    let group = |id| ledger.account(id).and_then(|account| account.group());
    group(a).is_some_and(|root| group(b) == Some(root))
}

/// What a spending cap's refusal means to the sender: a packet larger than
/// the cap never passes, and is told the most that would (F08); otherwise
/// the cap refills, and the sender may try again later (T04).
const fn cap_refusal(refusal: CapRefusal, amount: u64) -> Refusal {
    match refusal {
        CapRefusal::TooLarge(maximum) => Refusal {
            code: ErrorCode::F08_AMOUNT_TOO_LARGE,
            message: "packet amount exceeds the spending cap",
            too_large: Some(AmountTooLarge {
                received_amount: amount,
                maximum_amount: maximum,
            }),
        },
        CapRefusal::Exhausted => refuse(
            ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
            "the spending cap is reached",
        ),
    }
}

/// What a ledger refusal means to the sender.
fn ledger_refusal(error: TransferError) -> Refusal {
    match error {
        TransferError::ExceedsDebits | TransferError::ExceedsCredits | TransferError::Overflow => {
            refuse(
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "insufficient liquidity",
            )
        }
        TransferError::PendingFull | TransferError::JournalFull => {
            refuse(ErrorCode::T03_CONNECTOR_BUSY, "connector busy")
        }
        TransferError::UnknownAccount
        | TransferError::SameAccount
        | TransferError::DifferentLedgers
        | TransferError::ZeroAmount => refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error"),
    }
}

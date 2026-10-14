//! Incoming Prepares: ILDCP, routing, expiry, amounts and the ledger, in
//! the order of spec 4 §4.3.

use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode, Fulfill, MAX_PACKET_LENGTH, Prepare, Reject, Timestamp,
    ildcp,
};
use dassie_ledger::{
    AccountName, AccountPath, Ledger, Reference, TraceId, TransferError, TransferId, TransferSpec,
};
use dassie_oer::{Decode, Encode};

use super::{Connector, Forwarded, Outcome, Pending};
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
    amount_out: u64,
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
        let transfers = match self.book(now_ms, trace, source, &route, prepare.amount, ledger) {
            Ok(transfers) => transfers,
            Err(refusal) => return self.answer(now_ms, trace, &refusal, out),
        };
        self.forward(trace, source, request_id, &prepare, &route, transfers, out)
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
            .next_hop(prepare.destination, source)
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
        let destination = self
            .endpoints
            .get(to)
            .ok_or(refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error"))?;
        let amount_out = if from.ledger == destination.ledger || prepare.amount == 0 {
            prepare.amount
        } else {
            if now_ms.saturating_sub(rates.as_of_ms) > config.max_rate_age_ms {
                return Err(refuse(
                    ErrorCode::T00_INTERNAL_ERROR,
                    "exchange rates are stale",
                ));
            }
            convert(
                rates,
                from.ledger.asset(),
                destination.ledger.asset(),
                prepare.amount,
            )
            .ok_or(refuse(ErrorCode::T00_INTERNAL_ERROR, "no exchange rate"))?
        };
        Ok(Route {
            to,
            expires_at,
            amount_out,
        })
    }

    /// The packet's pending transfers: one on a ledger, two across ledgers
    /// through the FX accounts, none for nothing or between endpoints that
    /// share an account.
    fn book(
        &mut self,
        now_ms: u64,
        trace: TraceId,
        source: EndpointId,
        route: &Route,
        amount_in: u64,
        ledger: &mut Ledger,
    ) -> Result<[Option<TransferId>; 2], Refusal> {
        let internal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
        let (Some(from), Some(to)) = (self.endpoints.get(source), self.endpoints.get(route.to))
        else {
            return Err(internal);
        };
        if amount_in == 0 || from.account == to.account {
            return Ok([None, None]);
        }
        let reference = Reference::Packet(trace);
        let pending = |debit, credit, amount| TransferSpec {
            debit,
            credit,
            amount,
            pending: true,
            reference,
        };
        if from.ledger == to.ledger {
            let spec = pending(from.account, to.account, amount_in);
            return match ledger.create(now_ms, &spec) {
                Ok(id) => Ok([Some(id), None]),
                Err(error) => Err(ledger_refusal(error)),
            };
        }
        if route.amount_out == 0 {
            return Err(refuse(
                ErrorCode::F02_UNREACHABLE,
                "amount too small to convert",
            ));
        }
        let revenue = ledger.find(&AccountPath::new(from.ledger, AccountName::RevenueFx));
        let expenses = ledger.find(&AccountPath::new(to.ledger, AccountName::ExpensesFx));
        let (Some(revenue), Some(expenses)) = (revenue, expenses) else {
            return Err(internal);
        };
        let specs = [
            pending(from.account, revenue, amount_in),
            pending(expenses, to.account, route.amount_out),
        ];
        self.scratch.clear();
        match ledger.create_linked(now_ms, &specs, &mut self.scratch) {
            Ok(()) => Ok([self.scratch.first().copied(), self.scratch.get(1).copied()]),
            Err((_, error)) => Err(ledger_refusal(error)),
        }
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
        transfers: [Option<TransferId>; 2],
        out: &mut [u8],
    ) -> Outcome {
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
                expires_at_ms,
                transfers,
                failure: None,
            },
        );
        self.deadlines.insert((expires_at_ms, route.to, request_id));
        self.incoming.insert((source, source_request_id));
        if let Some(endpoint) = self.endpoints.get_mut(source) {
            endpoint.pending_from += 1;
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

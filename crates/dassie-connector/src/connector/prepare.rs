//! Incoming Prepares: ILDCP, routing, expiry, amounts and the ledger
//! (spec/areas/connector.md#handling-a-prepare).

use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode, Fulfill, MAX_PACKET_LENGTH, Prepare, Reject, Timestamp,
    ildcp,
};
use dassie_ledger::{
    AccountId, AccountName, AccountPath, Ledger, LedgerId, Reference, TraceId, TransferError,
    TransferSpec,
};
use dassie_oer::{Decode, Encode};

use super::hops::Hop;
use super::{Connector, Forwarded, OpensAccounts, OpensNone, Outcome, Pending};
use crate::PacketTransfers;
use crate::cap::CapRefusal;
use crate::charges::Charges;
use crate::endpoint::{EndpointId, MAX_SLOTS};
use crate::pricing::{PriceError, PriceTable};
use crate::trace::{PacketRecord, PacketState};

/// Room for an ILDCP response: the longest address, a scale and the
/// longest asset code, with their length prefixes.
const ILDCP_RESPONSE_LENGTH: usize = 3 + dassie_ilp::MAX_ADDRESS_LENGTH + 1 + 3 + 255;

/// A refusal before the packet goes anywhere.
#[derive(Clone, Copy)]
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
    /// The ledger of the source's slot it came in on, and of the
    /// destination's slot it goes out on.
    in_ledger: LedgerId,
    out_ledger: LedgerId,
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
    /// Handles a Prepare from `source` with the request ID it used, on its
    /// first slot, at `prices`: what transit is charged and converts at, and
    /// the spot rates a packet into the owner's own endpoints converts at.
    /// `draw`, a number from the caller's RNG, draws the packet's next hop
    /// among a route's options (spec/decisions/weighted-next-hops.md).
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
        prices: &PriceTable,
        draw: u64,
        out: &mut [u8],
    ) -> Outcome {
        self.handle_prepare_with(
            now_ms,
            (source, request_id),
            None,
            packet,
            ledger,
            prices,
            &mut OpensNone,
            draw,
            out,
        )
    }

    /// [`handle_prepare`](Self::handle_prepare), on the source's slot on
    /// `on`, the ledger of the settlement method a peer's Prepare came on
    /// (its first slot if `None`), where `accounts` opens the accounts of a
    /// peer's slot that has none yet, for a packet to it that moves money
    /// (spec/decisions/lazy-downlink-accounts.md). A Prepare on a method the
    /// source has no slot for, or one being removed, is refused for want of
    /// liquidity on it (T04): the connector books a packet on the method it
    /// came on, or not at all.
    ///
    /// # Panics
    ///
    /// As [`handle_prepare`](Self::handle_prepare).
    #[allow(clippy::too_many_arguments)]
    pub fn handle_prepare_with(
        &mut self,
        now_ms: u64,
        (source, request_id): (EndpointId, u32),
        on: Option<LedgerId>,
        packet: &[u8],
        ledger: &mut Ledger,
        prices: &PriceTable,
        accounts: &mut dyn OpensAccounts,
        draw: u64,
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
        let in_ledger = match self.source_slot(source, on) {
            Ok(in_ledger) => in_ledger,
            Err(refusal) => {
                self.trace
                    .push(PacketRecord::new(trace, source, "", 0, now_ms));
                return self.answer(now_ms, trace, &refusal, out);
            }
        };
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
            return self.ildcp(now_ms, trace, (source, in_ledger), &prepare, out);
        }
        let from = (source, in_ledger);
        let route = match self.route(now_ms, from, &prepare, prices, ledger, draw) {
            Ok(route) => route,
            Err(refusal) => return self.answer(now_ms, trace, &refusal, out),
        };
        let books = (ledger, accounts);
        let booked = match self.book(now_ms, trace, source, &route, prepare.amount, books) {
            Ok(booked) => booked,
            Err(refusal) => return self.answer(now_ms, trace, &refusal, out),
        };
        self.forward(trace, source, request_id, &prepare, &route, booked, out)
    }

    /// The ledger of `source`'s slot on `on` (its first if `None`) that a
    /// Prepare comes in on, if the source takes new Prepares there. A
    /// peering that ends takes no new Prepares; one we end and have not
    /// told the peer of yet refuses them as for want of liquidity
    /// (spec/decisions/ending-a-peering.md), and so does one our messages
    /// surely do not reach, since a Fulfill of ours would count only in our
    /// books (spec/decisions/unreachable-peers.md). Our Reject is the first
    /// message to try it again. A method with no open slot is refused as
    /// for want of liquidity on it.
    fn source_slot(&self, source: EndpointId, on: Option<LedgerId>) -> Result<LedgerId, Refusal> {
        let endpoint = self
            .endpoints
            .get(source)
            .ok_or(refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error"))?;
        if endpoint.draining {
            return Err(refuse(
                ErrorCode::T01_PEER_UNREACHABLE,
                "the peering is ending",
            ));
        }
        if endpoint.refusing {
            return Err(refuse(
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "insufficient liquidity",
            ));
        }
        if endpoint.unreached {
            return Err(refuse(
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "cannot answer the peer",
            ));
        }
        let slot = match on {
            Some(ledger) => endpoint.slot(ledger).filter(|slot| !slot.removed),
            None => endpoint.first_slot(),
        };
        slot.map(|slot| slot.ledger).ok_or(refuse(
            ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
            "the settlement method is not open",
        ))
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

    /// Answers an ILDCP request with the source's routed address and the
    /// asset of the ledger of its slot the request came on. It must carry
    /// no money and the ILDCP condition, or it is not one.
    fn ildcp(
        &mut self,
        now_ms: u64,
        trace: TraceId,
        (source, on): (EndpointId, LedgerId),
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
        let asset = on.asset();
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
        (source, in_ledger): (EndpointId, LedgerId),
        prepare: &Prepare<'_>,
        prices: &PriceTable,
        ledger: &Ledger,
        draw: u64,
    ) -> Result<Route, Refusal> {
        let config = &self.config;
        let from = self
            .endpoints
            .get(source)
            .ok_or(refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error"))?;
        if self.pending.len() >= config.max_pending {
            return Err(refuse(ErrorCode::T03_CONNECTOR_BUSY, "connector busy"));
        }
        if from.pending_from >= self.max_pending_from(source) {
            return Err(refuse(
                ErrorCode::T05_RATE_LIMITED,
                "too many packets in flight",
            ));
        }
        // Unroutable, including anything below our own address that is no
        // endpoint's and our own address itself. A route whose hops have
        // no method available now is short of liquidity, which passes.
        let (to, method) =
            match self.next_hop(now_ms, prepare.destination, Some((source, in_ledger)), draw) {
                Hop::To(to, method) => (to, method),
                Hop::NoMethod => {
                    return Err(refuse(
                        ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                        "no settlement method available",
                    ));
                }
                Hop::None => return Err(refuse(ErrorCode::F02_UNREACHABLE, "no route found")),
            };
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
        let from = (source, in_ledger);
        let out_ledger = match method {
            Some(method) => method,
            None => self.best_method(now_ms, from, to, prepare.amount, prices, ledger)?,
        };
        let onto = (to, out_ledger);
        let (fee, amount_out) = self.amounts(now_ms, from, onto, prepare.amount, prices, ledger)?;
        self.admits_in_flight(from, onto, prepare.amount, amount_out)?;
        Ok(Route {
            to,
            in_ledger,
            out_ledger,
            expires_at,
            fee,
            amount_out,
        })
    }

    /// The method a peer's packet that converts takes at `to`, its drawn
    /// next hop: of `to`'s available methods, the one the price table gives
    /// the best rate for, rates on different ledgers compared at spot
    /// (`PriceTable::best_output`), so that it pays out what the node holds
    /// above its target (spec/decisions/settlement-methods.md). A hop with
    /// one method takes it; so does a packet into the node's own endpoints,
    /// which have one each. With no rate on any, the first, which then
    /// refuses the packet as a conversion without a rate does.
    fn best_method(
        &self,
        now_ms: u64,
        (source, in_ledger): (EndpointId, LedgerId),
        to: EndpointId,
        amount: u64,
        prices: &PriceTable,
        ledger: &Ledger,
    ) -> Result<LedgerId, Refusal> {
        let internal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
        let endpoint = self.endpoints.get(to).ok_or(internal)?;
        let mut candidates = [in_ledger; MAX_SLOTS];
        let mut count = 0;
        for slot in endpoint.slots.iter().filter(|slot| slot.usable()) {
            if let Some(entry) = candidates.get_mut(count) {
                *entry = slot.ledger;
                count += 1;
            }
        }
        let candidates = candidates.get(..count).unwrap_or_default();
        let first = *candidates.first().ok_or(internal)?;
        if candidates.len() == 1 || !(source.is_peer() && to.is_peer()) {
            return Ok(first);
        }
        let fee = prices.charges().fee(amount);
        let holdings = prices.live_holdings(ledger);
        let best = prices.best_output(
            in_ledger,
            candidates,
            amount,
            amount - fee,
            &holdings,
            now_ms,
            self.config.max_rate_age_ms,
        );
        Ok(best.map_or(first, |best| best.ledger))
    }

    /// Whether the money in flight with each peer the packet passes stays
    /// within [`Config::max_in_flight`] each way, or T05
    /// (spec/decisions/packets-in-flight.md). A crash loses the Fulfills of
    /// what was in flight, so this bounds how far a crash makes two peers'
    /// books drift apart. Half a packet of it stays free for smaller packets
    /// while a payment fills the link. A packet always passes alone, even one
    /// a conversion made larger than the limit.
    fn admits_in_flight(
        &self,
        (source, in_ledger): (EndpointId, LedgerId),
        (to, out_ledger): (EndpointId, LedgerId),
        amount_in: u64,
        amount_out: u64,
    ) -> Result<(), Refusal> {
        let max = self.config.max_in_flight();
        let over = |in_flight: u64, amount: u64| {
            amount > 0 && in_flight > 0 && in_flight.saturating_add(amount) > max
        };
        let from = self
            .endpoints
            .slot(source, in_ledger)
            .map_or(0, |slot| slot.money_from);
        let onward = self
            .endpoints
            .slot(to, out_ledger)
            .map_or(0, |slot| slot.money_to);
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
    /// transit from one peer to another is charged, and converts at the
    /// curve, at what the owner holds on each ledger by `ledger`'s books
    /// now, the packets in flight and the packet's own move counted against
    /// it (spec/decisions/passive-rebalancing.md); a peer's packet into the
    /// owner's own endpoints converts at spot. The owner's own endpoints
    /// never convert, so what the owner holds on one ledger is never spent on
    /// another by a packet (spec/areas/ledger.md#the-owners-accounts). The
    /// first hop that charges for it converts.
    fn amounts(
        &self,
        now_ms: u64,
        (source, in_ledger): (EndpointId, LedgerId),
        (to, out_ledger): (EndpointId, LedgerId),
        amount: u64,
        prices: &PriceTable,
        ledger: &Ledger,
    ) -> Result<(u64, u64), Refusal> {
        let internal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
        if self.endpoints.slot(source, in_ledger).is_none()
            || self.endpoints.slot(to, out_ledger).is_none()
        {
            return Err(internal);
        }
        let transit = source.is_peer() && to.is_peer();
        let charges = if transit {
            prices.charges()
        } else {
            Charges::NONE
        };
        let fee = charges.fee(amount);
        let net = amount - fee;
        let converts = in_ledger != out_ledger && amount > 0;
        if converts && !source.is_peer() {
            return Err(refuse(
                ErrorCode::F02_UNREACHABLE,
                "pay from the route's ledger",
            ));
        }
        let max_age_ms = self.config.max_rate_age_ms;
        let amount_out = if !converts {
            net
        } else if transit {
            let holdings = prices.live_holdings(ledger);
            prices
                .convert(
                    in_ledger, out_ledger, amount, net, &holdings, now_ms, max_age_ms,
                )
                .map_err(price_refusal)?
        } else {
            prices
                .convert_at_spot(in_ledger, out_ledger, net, now_ms, max_age_ms)
                .map_err(price_refusal)?
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
    ///
    /// A peer without accounts yet is one with nothing on them: it has no
    /// credit to send money with, and a packet that moves money to it opens
    /// them, if there is room, with the transfers or not at all
    /// (spec/decisions/lazy-downlink-accounts.md).
    fn book(
        &mut self,
        now_ms: u64,
        trace: TraceId,
        source: EndpointId,
        route: &Route,
        amount_in: u64,
        (ledger, accounts): (&mut Ledger, &mut dyn OpensAccounts),
    ) -> Result<Booked, Refusal> {
        let internal = refuse(ErrorCode::T00_INTERNAL_ERROR, "internal error");
        let (Some(from), Some(to)) = (
            self.endpoints.slot(source, route.in_ledger),
            self.endpoints.slot(route.to, route.out_ledger),
        ) else {
            return Err(internal);
        };
        let nothing = Booked {
            transfers: [None; 3],
            charge: None,
        };
        if amount_in == 0 {
            return Ok(nothing);
        }
        let Some(from_account) = from.account else {
            return Err(refuse(
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "insufficient liquidity",
            ));
        };
        if to.account == Some(from_account) {
            return Ok(nothing);
        }
        // A peer's accounts are in no group, opened or not.
        let counted = from.ledger != to.ledger
            || !to
                .account
                .is_some_and(|to_account| same_group(ledger, from_account, to_account));
        let charge = counted.then_some((from_account, amount_in));
        if let Some(cap) = charge.and_then(|(account, _)| self.caps.get(&account)) {
            cap.admits(now_ms, amount_in)
                .map_err(|refusal| cap_refusal(refusal, amount_in))?;
        }
        let (on, opens) = (to.ledger, to.account.is_none());
        if opens {
            let account = accounts.open(now_ms, ledger, route.to, on).ok_or(refuse(
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "no room for the peer's accounts",
            ))?;
            self.open_account(route.to, on, account);
        }
        let transfers = match self.book_transfers(now_ms, trace, source, route, amount_in, ledger) {
            Ok(transfers) => transfers,
            Err(refusal) => {
                if opens {
                    accounts.take_back(ledger, route.to, on);
                    if let Some(slot) = self.endpoints.slot_mut(route.to, on) {
                        slot.account = None;
                    }
                }
                return Err(refusal);
            }
        };
        if let Some(cap) = charge.and_then(|(account, _)| self.caps.get_mut(&account)) {
            cap.reserve(now_ms, amount_in);
        }
        Ok(Booked { transfers, charge })
    }

    /// The pending transfers of a packet that moves money, linked: the
    /// first debits the source's account; the second, across ledgers,
    /// credits the destination's from the FX accounts; the third is our
    /// fee, which debits the source's account too. The original booked no
    /// fee, and its fee accounts stayed empty.
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
        let (Some(from), Some(to)) = (
            self.endpoints.slot(source, route.in_ledger),
            self.endpoints.slot(route.to, route.out_ledger),
        ) else {
            return Err(internal);
        };
        let (Some(from_account), Some(to_account)) = (from.account, to.account) else {
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
            (pending(from_account, to_account, net), None)
        } else {
            let revenue = ledger.find(&AccountPath::new(from.ledger, AccountName::RevenueFx));
            let expenses = ledger.find(&AccountPath::new(to.ledger, AccountName::ExpensesFx));
            let (Some(revenue), Some(expenses)) = (revenue, expenses) else {
                return Err(internal);
            };
            (
                pending(from_account, revenue, net),
                Some(pending(expenses, to_account, route.amount_out)),
            )
        };
        let fee = if route.fee > 0 {
            let fees = ledger
                .find(&AccountPath::new(from.ledger, AccountName::RevenueFees))
                .ok_or(internal)?;
            Some(pending(from_account, fees, route.fee))
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

    /// How long a packet on `route` may wait for its hub after its expiry,
    /// if it goes to a peer on a method over a hub: until the incoming
    /// expiry, and at most one message window past its own, even when the
    /// incoming expiry is far. The hub decides well within it, and the
    /// packet holds money in flight meanwhile.
    fn backstop(&self, route: &Route, incoming_ms: u64, expires_at_ms: u64) -> Option<u64> {
        self.endpoints
            .slot(route.to, route.out_ledger)
            .is_some_and(|slot| slot.hub)
            .then_some(incoming_ms.min(expires_at_ms.saturating_add(self.config.message_window_ms)))
    }

    /// What a packet on `route` earns its next hop's method toward the
    /// draw once fulfilled: our fee, only if it keeps its ledger, since
    /// fees on different ledgers do not compare
    /// (spec/decisions/weighted-next-hops.md).
    fn earns(route: &Route) -> u64 {
        if route.in_ledger == route.out_ledger {
            route.fee
        } else {
            0
        }
    }

    /// Counts a packet just forwarded on `route` in flight: from its source,
    /// and on the slots it came in and goes out on, with the money a peer's
    /// carry (spec/decisions/packets-in-flight.md).
    fn count_in_flight(&mut self, source: EndpointId, route: &Route, amount_in: u64) {
        if let Some(endpoint) = self.endpoints.get_mut(source) {
            endpoint.pending_from += 1;
        }
        if let Some(slot) = self.endpoints.slot_mut(source, route.in_ledger) {
            slot.pending += 1;
            if source.is_peer() {
                slot.money_from += amount_in;
            }
        }
        if let Some(slot) = self.endpoints.slot_mut(route.to, route.out_ledger) {
            slot.pending += 1;
            if route.to.is_peer() {
                slot.money_to += route.amount_out;
            }
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
                in_ledger: route.in_ledger,
                out_ledger: route.out_ledger,
                source_gone: false,
                condition: prepare.execution_condition,
                amount_in: prepare.amount,
                amount_out: route.amount_out,
                expires_at_ms,
                transfers,
                failure: None,
                charge,
                backstop_ms: self.backstop(route, prepare.expires_at.unix_ms, expires_at_ms),
                awaiting_hub: false,
                earns: Self::earns(route),
            },
        );
        self.deadlines.insert((expires_at_ms, route.to, request_id));
        self.incoming.insert((source, source_request_id));
        self.count_in_flight(source, route, prepare.amount);
        // A packet below a periphery node's address beneath an uplink
        // reaches its endpoint written below the node's own, the address
        // the endpoint knows (spec/areas/connector.md#a-periphery-node).
        let mut rewritten = core::mem::take(&mut self.rewritten);
        rewritten.clear();
        if let Some(below) = self.below_child_address(prepare.destination) {
            rewritten.push_str(&self.own_address);
            rewritten.push('.');
            rewritten.push_str(below);
        }
        let destination = Address::parse(&rewritten).unwrap_or(prepare.destination);
        let outgoing = Prepare {
            amount: route.amount_out,
            expires_at: route.expires_at,
            destination,
            ..*prepare
        };
        let length = outgoing.encode(out);
        self.rewritten = rewritten;
        let (to, ledger) = (route.to, route.out_ledger);
        let amount_out = route.amount_out;
        self.trace.update(trace, |record| {
            record.destination = Some(to);
            record.amount_out = amount_out;
            record.transfers = transfers;
        });
        self.stats.forwarded += 1;
        Outcome::Forward(Forwarded {
            to,
            ledger,
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

/// What a conversion's refusal means to the sender: rates come back, and
/// funds on the way out, so both are temporary. A pair without a rate, or
/// whose result does not fit, is as a missing rate (T00); only a want of
/// funds on the way out is T04.
const fn price_refusal(error: PriceError) -> Refusal {
    match error {
        PriceError::Stale => refuse(ErrorCode::T00_INTERNAL_ERROR, "exchange rates are stale"),
        PriceError::NoFunds => refuse(
            ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
            "insufficient liquidity",
        ),
        PriceError::NoRate | PriceError::TooLarge => {
            refuse(ErrorCode::T00_INTERNAL_ERROR, "no exchange rate")
        }
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

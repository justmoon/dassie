//! Answering incoming packets.

use alloc::string::String;

use dassie_ilp::{Address, ErrorCode as IlpErrorCode, Fulfill, Prepare, Reject, Timestamp};
use dassie_oer::{Encode, Writer};
use rand_core::CryptoRng;

use super::send::{measure, seal};
use super::{BUFFER_LENGTH, Connection, State};
use crate::allocate::{Claim, allocate};
use crate::buffers::Refusal;
use crate::crypto::OVERHEAD;
use crate::event::Event;
use crate::frame::{ErrorCode, Frame};
use crate::packet::{self, Packet, PacketType};
use crate::receipt;

/// What to answer, decided before any money or data is applied.
#[derive(Clone, Copy, Debug)]
struct Decision {
    fulfill: bool,
    fulfillment: [u8; 32],
    /// The connection ends with this packet: (code, closed by the remote).
    close: Option<(ErrorCode, bool)>,
}

/// How a stream-ID-bearing frame resolved.
enum Resolved {
    Open,
    /// The stream existed and is gone.
    Closed,
    /// The remote broke the stream-ID rules.
    Violation(ErrorCode),
}

impl Connection {
    /// Answers an incoming ILP Prepare for this connection, writing the ILP
    /// Fulfill or Reject into `buffer` and returning its length.
    ///
    /// # Panics
    ///
    /// If `buffer` is shorter than [`BUFFER_LENGTH`].
    pub fn handle_prepare(
        &mut self,
        now: Timestamp,
        rng: &mut (impl CryptoRng + ?Sized),
        prepare: &Prepare<'_>,
        buffer: &mut [u8],
    ) -> usize {
        assert!(
            buffer.len() >= BUFFER_LENGTH,
            "buffer shorter than BUFFER_LENGTH"
        );
        self.stats.packets_received = self.stats.packets_received.saturating_add(1);
        let (out, scratch) = buffer.split_at_mut(dassie_ilp::MAX_PACKET_LENGTH);
        let packet = self
            .keys
            .cipher
            .decrypt(prepare.data, scratch)
            .ok()
            .and_then(|plaintext| Packet::parse(plaintext).ok())
            .filter(|packet| packet.packet_type == PacketType::Prepare);
        let Some(packet) = packet else {
            // Not for us, or garbled: RFC 29 §4.3 says F06.
            return self.write_plain_reject(IlpErrorCode::F06_UNEXPECTED_PAYMENT, out);
        };
        if prepare.expires_at <= now {
            return self.write_plain_reject(IlpErrorCode::R00_TRANSFER_TIMED_OUT, out);
        }
        if !self.has_event_room() {
            // The application is not polling; refuse until it does, so that
            // the remote cannot overflow the queue
            // (spec/decisions/tigerstyle.md). Senders retry.
            return self.write_plain_reject(IlpErrorCode::T05_RATE_LIMITED, out);
        }
        self.last_activity = Some(now);
        let decision = self.receive(prepare, &packet);
        let length = self.write_response(rng, prepare, &packet, &decision, out);
        self.note_money_told();
        if let Some((code, by_remote)) = decision.close {
            self.finish_close(code, by_remote);
        }
        self.after_step(now);
        length
    }

    fn write_plain_reject(&self, code: IlpErrorCode, out: &mut [u8]) -> usize {
        let reject = Reject {
            code,
            triggered_by: self
                .local_address
                .as_deref()
                .and_then(|text| Address::parse(text).ok()),
            message: "",
            data: &[],
        };
        reject.encode(out)
    }

    fn receive(&mut self, prepare: &Prepare<'_>, packet: &Packet<'_>) -> Decision {
        self.credited.clear();
        self.stream_errors.clear();
        let fulfillment = self.keys.fulfillment(prepare.data);
        let mut decision = Decision {
            fulfill: false,
            fulfillment,
            close: None,
        };
        if matches!(self.state, State::Closed) {
            return decision;
        }
        if !self.connected {
            self.connected = true;
            self.push_event(Event::Connected);
        }
        let mut acceptable = true;
        for frame in packet.frames.iter() {
            match self.apply_control_frame(&frame) {
                Ok(true) => {}
                Ok(false) => acceptable = false,
                Err(code) => {
                    decision.close = Some((code, false));
                    return decision;
                }
            }
            if let Frame::ConnectionClose { code, .. } = frame {
                decision.close = Some((code, true));
            }
        }
        let offers_money = prepare.amount > 0
            && packet
                .frames
                .iter()
                .any(|frame| matches!(frame, Frame::StreamMoney { .. }));
        if self.money_room() == 0 && offers_money && decision.close.is_none() {
            // What the connection pays into is full, possibly by another
            // connection, or within the tolerance of it, or closed short
            // of it. Its streams' maxima cannot fall (RFC 29 §4.4.4), so
            // a sender would offer money until its deadline. A close they
            // all understand ends it, with the code the application gave
            // for why; the response still reports the streams' totals, so
            // the sender learns what it delivered. Rate probes and packets
            // without money are answered as before, so a sender can still
            // measure and open a stream.
            decision.close = Some((self.money_exhausted, false));
            return decision;
        }
        let fulfillable = dassie_ilp::condition_for(&fulfillment) == prepare.execution_condition;
        let enough = prepare.amount >= packet.prepare_amount;
        // A packet may overshoot what is left by the tolerance, which
        // rounding on the way can put over it. One that overshoots more is
        // refused with F99, and the reply carries what arrived and each
        // stream's room, so the sender can send what is left instead
        // (spec/areas/stream.md#money-limit).
        let within_limit = prepare.amount <= self.money_limit.saturating_add(self.money_tolerance);
        if !(acceptable && fulfillable && enough) {
            return decision;
        }
        if !within_limit {
            // A payment within what one of its streams was told, but over
            // what is left, met money another connection took: the stream
            // cannot be told less (RFC 29 §4.4.4), so the connection
            // closes, with the code the application gave for a total it
            // takes no more of (spec/decisions/close-rather-than-lower.md).
            let told_room = self.told_room(packet);
            if self.money_limit != u64::MAX
                && offers_money
                && decision.close.is_none()
                && prepare.amount <= told_room.saturating_add(self.money_tolerance)
            {
                decision.close = Some((self.money_exhausted, false));
            }
            return decision;
        }
        match self.check_incoming(prepare, packet) {
            Ok(true) => {
                self.apply_incoming(prepare, packet);
                decision.fulfill = true;
            }
            Ok(false) => {}
            Err(code) => decision.close = Some((code, false)),
        }
        decision
    }

    /// The most a packet's streams were told they may still receive
    /// under the money limit. A stream never told more than the room has
    /// the room for a window, so a packet over it is refused and told the
    /// room.
    fn told_room(&self, packet: &Packet<'_>) -> u64 {
        packet
            .frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::StreamMoney { stream_id, .. } => self.stream(stream_id),
                _ => None,
            })
            .map(|stream| self.money_told_total(stream))
            .max()
            .unwrap_or(0)
            .saturating_sub(self.money_received)
    }

    /// Applies a frame that changes connection or stream state whether or
    /// not the packet is fulfilled. Returns whether the packet may still be
    /// fulfilled, or the error that ends the connection.
    pub(super) fn apply_control_frame(&mut self, frame: &Frame<'_>) -> Result<bool, ErrorCode> {
        match *frame {
            Frame::ConnectionNewAddress {
                source_account: Some(address),
            } => self.learn_remote_address(address),
            Frame::ConnectionAssetDetails {
                asset_code,
                asset_scale,
            } => {
                if self
                    .remote_asset
                    .as_ref()
                    .is_none_or(|(code, _)| code != asset_code)
                {
                    self.remote_asset = Some((String::from(asset_code), asset_scale));
                }
            }
            Frame::ConnectionMaxStreamId { max_stream_id } => {
                self.remote_max_stream_id = self.remote_max_stream_id.max(max_stream_id);
            }
            Frame::ConnectionMaxData { max_offset } => {
                self.remote_connection_max_data = self.remote_connection_max_data.max(max_offset);
            }
            Frame::StreamClose {
                stream_id, code, ..
            } => {
                if let Some(stream) = self.stream_mut(stream_id)
                    && stream.remote_close.is_none()
                {
                    stream.remote_close = Some(code);
                }
            }
            Frame::StreamMaxMoney { stream_id, .. }
            | Frame::StreamMaxData { stream_id, .. }
            | Frame::StreamMoney { stream_id, .. }
            | Frame::StreamData { stream_id, .. } => {
                return self.apply_stream_frame(stream_id, frame);
            }
            _ => {}
        }
        Ok(true)
    }

    fn learn_remote_address(&mut self, address: Address<'_>) {
        if self.remote_address.as_deref() != Some(address.as_str()) {
            if self.remote_address.is_some() {
                // A new path: what congestion control learned is stale.
                self.concurrency_milli = self.policy.min_concurrency * 1000;
            }
            self.remote_address = Some(String::from(address.as_str()));
        }
        // The remote wants our asset and limits in return.
        self.announce_pending = true;
    }

    pub(super) fn apply_stream_frame(
        &mut self,
        stream_id: u64,
        frame: &Frame<'_>,
    ) -> Result<bool, ErrorCode> {
        let carries_value = matches!(frame, Frame::StreamMoney { .. } | Frame::StreamData { .. });
        match self.resolve_stream(stream_id) {
            Resolved::Violation(code) => return Err(code),
            Resolved::Closed => {
                if carries_value {
                    self.report_stream_error(stream_id, ErrorCode::STREAM_STATE_ERROR);
                    return Ok(false);
                }
                return Ok(true);
            }
            Resolved::Open => {}
        }
        let next_sequence = self.next_sequence;
        let Some(stream) = self.stream_mut(stream_id) else {
            return Ok(true);
        };
        match *frame {
            Frame::StreamMaxMoney {
                receive_max,
                total_received,
                ..
            } => {
                // A packet the remote sent, not a reply to one of ours: it
                // says what the remote takes now only if it reports more
                // received than any reply heard.
                stream.hear_remote_limit(receive_max, total_received, None, false, next_sequence);
            }
            Frame::StreamMaxData { max_offset, .. } => {
                stream.remote_max_data = stream.remote_max_data.max(max_offset);
            }
            _ => {}
        }
        Ok(true)
    }

    fn report_stream_error(&mut self, stream_id: u64, code: ErrorCode) {
        let known = self.stream_errors.iter().any(|&(id, _)| id == stream_id);
        if !known && self.stream_errors.len() < self.stream_errors.capacity() {
            self.stream_errors.push((stream_id, code));
        }
    }

    /// Finds or opens the stream a remote frame refers to (RFC 29 §2.2).
    fn resolve_stream(&mut self, id: u64) -> Resolved {
        if self.stream(id).is_some() {
            return Resolved::Open;
        }
        if id == 0 {
            return Resolved::Violation(ErrorCode::PROTOCOL_VIOLATION);
        }
        if self.is_local_stream_id(id) {
            // One of ours: closed if we opened it before, otherwise the
            // remote is opening streams with our parity.
            return if id < self.next_local_stream_id {
                Resolved::Closed
            } else {
                Resolved::Violation(ErrorCode::PROTOCOL_VIOLATION)
            };
        }
        if self.is_remote_stream_closed(id) {
            return Resolved::Closed;
        }
        if id > self.local_max_stream_id {
            return Resolved::Violation(ErrorCode::STREAM_ID_ERROR);
        }
        if !matches!(self.state, State::Open) {
            return Resolved::Closed;
        }
        self.push_stream(id, true);
        self.push_event(Event::StreamOpened { stream_id: id });
        Resolved::Open
    }

    /// Checks the money and data of a fulfillable packet. `Ok(false)`
    /// rejects it so the sender retries; an error closes the connection.
    fn check_incoming(
        &mut self,
        prepare: &Prepare<'_>,
        packet: &Packet<'_>,
    ) -> Result<bool, ErrorCode> {
        self.claims.clear();
        self.claim_streams.clear();
        for frame in packet.frames.iter() {
            if let Frame::StreamMoney { stream_id, shares } = frame {
                let Some(stream) = self.stream(stream_id) else {
                    return Ok(false);
                };
                let capacity = stream.receive_capacity();
                if let Some(index) = self.claim_streams.iter().position(|&id| id == stream_id) {
                    if let Some(claim) = self.claims.get_mut(index) {
                        claim.shares = claim.shares.saturating_add(shares);
                    }
                } else if self.claims.len() < self.claims.capacity() {
                    self.claims.push(Claim { shares, capacity });
                    self.claim_streams.push(stream_id);
                } else {
                    return Ok(false);
                }
            }
        }
        if prepare.amount > 0 {
            // Money must be attributed to streams; the original fulfilled
            // such packets and kept the money unassigned
            // (spec/original/04-interledger.md#117-receiving-a-packet-connectionhandle-packetts).
            if self.claims.is_empty() {
                return Ok(false);
            }
            self.allocation.clear();
            self.allocation.resize(self.claims.len(), 0);
            if allocate(prepare.amount, &self.claims, &mut self.allocation).is_err() {
                return Ok(false);
            }
        } else {
            self.allocation.clear();
            self.allocation.resize(self.claims.len(), 0);
        }
        self.check_incoming_data(packet)
    }

    fn check_incoming_data(&self, packet: &Packet<'_>) -> Result<bool, ErrorCode> {
        for (position, frame) in packet.frames.iter().enumerate() {
            let Frame::StreamData {
                stream_id,
                offset,
                data,
            } = frame
            else {
                continue;
            };
            let Some(stream) = self.stream(stream_id) else {
                return Ok(false);
            };
            // Earlier frames of this packet for the same stream may each
            // need a fragment of their own.
            let earlier = packet
                .frames
                .iter()
                .take(position)
                .filter(|other| matches!(other, Frame::StreamData { stream_id: id, .. } if *id == stream_id))
                .count();
            match stream.receive.check(offset, data.len(), earlier) {
                Ok(()) => {}
                Err(Refusal::FlowControl) => return Err(ErrorCode::FLOW_CONTROL_ERROR),
                Err(Refusal::Fragmented) => return Ok(false),
            }
        }
        // The connection limit, counted as upstream does: the highest
        // offsets of open streams after this packet, added up.
        let mut total: u64 = 0;
        for stream in &self.streams {
            let mut highest = stream.receive.highest_offset();
            for frame in packet.frames.iter() {
                if let Frame::StreamData {
                    stream_id,
                    offset,
                    data,
                } = frame
                    && stream_id == stream.id
                    && !data.is_empty()
                {
                    highest = highest.max(offset.saturating_add(data.len() as u64));
                }
            }
            total = total.saturating_add(highest);
        }
        if total > self.connection_max_data() {
            return Err(ErrorCode::FLOW_CONTROL_ERROR);
        }
        Ok(true)
    }

    fn apply_incoming(&mut self, prepare: &Prepare<'_>, packet: &Packet<'_>) {
        for index in 0..self.claim_streams.len() {
            let (Some(&stream_id), Some(&amount)) =
                (self.claim_streams.get(index), self.allocation.get(index))
            else {
                continue;
            };
            let receipt_secret = self.receipt;
            let Some(stream) = self.stream_mut(stream_id) else {
                continue;
            };
            stream.received += amount;
            let total = stream.received;
            if amount > 0 {
                self.push_event(Event::MoneyReceived { stream_id });
            }
            if let (Some(secret), Ok(small_id)) = (receipt_secret, u8::try_from(stream_id)) {
                let receipt = receipt::create(&secret, small_id, total);
                if self.credited.len() < self.credited.capacity() {
                    self.credited.push((stream_id, receipt));
                }
            }
        }
        for frame in packet.frames.iter() {
            if let Frame::StreamData {
                stream_id,
                offset,
                data,
            } = frame
                && let Some(stream) = self
                    .streams
                    .iter_mut()
                    .find(|stream| stream.id == stream_id)
            {
                stream.receive.insert(offset, data);
                self.stats.bytes_received =
                    self.stats.bytes_received.saturating_add(data.len() as u64);
                if stream.receive.readable() > 0 {
                    self.push_event(Event::DataReadable { stream_id });
                }
            }
        }
        self.stats.packets_received_fulfilled =
            self.stats.packets_received_fulfilled.saturating_add(1);
        self.stats.amount_received = self.stats.amount_received.saturating_add(prepare.amount);
        if self.money_limit != u64::MAX {
            // Within the tolerance, a packet may take more than was left.
            self.money_limit = self.money_limit.saturating_sub(prepare.amount);
            self.money_received = self.money_received.saturating_add(prepare.amount);
        }
    }

    /// Produces the frames of a response. Called twice per response, to
    /// measure and to write, so it must not change anything.
    fn visit_response(&self, decision: &Decision, visit: &mut dyn FnMut(Frame<'_>)) {
        if self.announce_pending {
            self.visit_announcement(visit);
        }
        visit(Frame::ConnectionMaxData {
            max_offset: self.connection_max_data(),
        });
        for stream in &self.streams {
            match stream.local_close {
                Some(code) => visit(Frame::StreamClose {
                    stream_id: stream.id,
                    code,
                    message: "",
                }),
                None => self.visit_stream_limits(stream, visit),
            }
        }
        for &(stream_id, code) in &self.stream_errors {
            visit(Frame::StreamClose {
                stream_id,
                code,
                message: "",
            });
        }
        for (stream_id, receipt) in &self.credited {
            visit(Frame::StreamReceipt {
                stream_id: *stream_id,
                receipt,
            });
        }
        let closing = match (decision.close, self.state) {
            (Some((code, false)), _) | (None, State::Closing { code, .. }) => Some(code),
            // Closed by now, it says again why it closed, so a sender
            // that hears this reply first learns the same as from the
            // first (spec/decisions/full-or-stopped-short.md); one the
            // remote closed says no more than that.
            (None, State::Closed) => Some(self.closed_with.unwrap_or(ErrorCode::NO_ERROR)),
            _ => None,
        };
        if let Some(code) = closing {
            visit(Frame::ConnectionClose { code, message: "" });
        }
    }

    fn write_response(
        &mut self,
        rng: &mut (impl CryptoRng + ?Sized),
        prepare: &Prepare<'_>,
        request: &Packet<'_>,
        decision: &Decision,
        out: &mut [u8],
    ) -> usize {
        let packet_type = if decision.fulfill {
            PacketType::Fulfill
        } else {
            PacketType::Reject
        };
        let measured = measure(|visit| self.visit_response(decision, visit));
        let plaintext_len =
            packet::header_len(request.sequence, prepare.amount, measured.0) + measured.1;
        let data_len = OVERHEAD + plaintext_len;
        let triggered_by = self
            .local_address
            .as_deref()
            .and_then(|text| Address::parse(text).ok());
        let fulfill = Fulfill {
            fulfillment: decision.fulfillment,
            data: &[],
        };
        let reject = Reject {
            code: IlpErrorCode::F99_APPLICATION_ERROR,
            triggered_by,
            message: "",
            data: &[],
        };
        let header_len = if decision.fulfill {
            fulfill.header_len(data_len)
        } else {
            reject.header_len(data_len)
        };
        let (head, rest) = out.split_at_mut(header_len);
        let sealed = seal(
            &self.keys,
            rng,
            rest,
            request.sequence,
            packet_type,
            prepare.amount,
            measured,
            |visit| self.visit_response(decision, visit),
        );
        assert!(sealed == data_len, "sealed length mismatch");
        let mut writer = Writer::new(head);
        if decision.fulfill {
            fulfill.encode_header(data_len, &mut writer);
        } else {
            reject.encode_header(data_len, &mut writer);
        }
        // Responses are not acknowledged, so the announcement is repeated
        // in requests until one gets a response.
        header_len + data_len
    }
}

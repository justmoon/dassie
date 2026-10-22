//! Settling sent packets: responses, timeouts, and what follows from them.

use dassie_ilp::{
    AmountTooLarge, ErrorClass, ErrorCode as IlpErrorCode, Reject, Response, Timestamp,
};
use dassie_oer::Decode;

use super::{BUFFER_LENGTH, Connection, PacketKind, Slot, State, digits};
use crate::event::Event;
use crate::frame::{ErrorCode, Frame};
use crate::packet::{Packet, PacketType};
use crate::rate::Rate;
use crate::receipt::RECEIPT_LENGTH;

/// How a rejection or loss affects sending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Failure {
    /// The receiver rejected the packet but processed it.
    Receiver,
    /// The path said the packet was too large.
    TooLarge,
    /// Worth retrying later.
    Temporary,
    /// The path will not carry this connection's packets.
    Final,
}

impl Connection {
    /// Takes the ILP response to the request `request_id`. Responses to
    /// unknown or already settled requests are ignored.
    ///
    /// # Panics
    ///
    /// If `buffer` is shorter than [`BUFFER_LENGTH`].
    pub fn handle_response(
        &mut self,
        now: Timestamp,
        request_id: u64,
        response: &[u8],
        buffer: &mut [u8],
    ) {
        assert!(
            buffer.len() >= BUFFER_LENGTH,
            "buffer shorter than BUFFER_LENGTH"
        );
        let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.active && slot.sequence == request_id)
        else {
            return;
        };
        let Some(target) = self.slots.get_mut(index) else {
            return;
        };
        let mut slot = core::mem::replace(target, Slot::new(0));
        self.last_activity = Some(now);
        match Response::decode_exact(response) {
            Ok(Response::Fulfill(fulfill))
                if slot.kind != PacketKind::Probe && fulfill.satisfies(&slot.condition) =>
            {
                let reply = self.open_reply(&slot, fulfill.data, PacketType::Fulfill, buffer);
                self.settle_fulfilled(&slot, reply);
            }
            Ok(Response::Reject(reject)) => {
                let reply = if reject.code == IlpErrorCode::F99_APPLICATION_ERROR {
                    self.open_reply(&slot, reject.data, PacketType::Reject, buffer)
                } else {
                    None
                };
                self.settle_rejected(now, &slot, &reject, reply);
            }
            // Malformed responses and fulfillments that do not match the
            // condition are a broken path; treat them as lost (RFC 27).
            _ => self.settle_lost(now, &slot),
        }
        slot.reset();
        if let Some(target) = self.slots.get_mut(index) {
            *target = slot;
        }
        self.after_step(now);
    }

    /// Expires packets and deadlines that have passed.
    pub fn handle_timeout(&mut self, now: Timestamp) {
        for index in 0..self.slots.len() {
            let expired = self
                .slots
                .get(index)
                .is_some_and(|slot| slot.active && slot.expires_at <= now);
            if !expired {
                continue;
            }
            let Some(target) = self.slots.get_mut(index) else {
                continue;
            };
            let mut slot = core::mem::replace(target, Slot::new(0));
            self.stats.packets_timed_out = self.stats.packets_timed_out.saturating_add(1);
            self.settle_lost(now, &slot);
            slot.reset();
            if let Some(target) = self.slots.get_mut(index) {
                *target = slot;
            }
        }
        if let State::Closing { code, deadline } = self.state
            && now >= deadline
        {
            self.finish_close(code, false);
        }
        if self.idle_deadline().is_some_and(|deadline| now >= deadline) {
            // Nothing from the remote for the idle timeout: give up, as
            // upstream does, so that abandoned connections end.
            self.finish_close(ErrorCode::NO_ERROR, false);
        }
        self.after_step(now);
    }

    /// Decrypts and checks the STREAM reply carried by a response, applies
    /// its frames, and returns what the receiver says arrived.
    fn open_reply(
        &mut self,
        slot: &Slot,
        data: &[u8],
        expected: PacketType,
        buffer: &mut [u8],
    ) -> Option<u64> {
        if data.is_empty() {
            return None;
        }
        let plaintext = self.keys.cipher.decrypt(data, buffer).ok()?;
        let packet = Packet::parse(plaintext).ok()?;
        // A reply for another request, or claiming another ILP type, is not
        // trustworthy; the original did not check
        // (spec/original/04-interledger.md#1110-sending-connectionsend-ts-streamsend-ts).
        if packet.sequence != slot.sequence || packet.packet_type != expected {
            return None;
        }
        self.remote_knows_us = true;
        if slot.announced {
            self.announce_pending = false;
        }
        if !self.connected {
            self.connected = true;
            self.push_event(Event::Connected);
        }
        // Replies may arrive out of order, and the receiver may take
        // packets in another order than they were sent: which reply says
        // what the receiver takes now is read from the totals they report
        // (`take_remote_maximum`).
        let newest = self
            .newest_reply
            .is_none_or(|newest| slot.sequence > newest);
        if newest {
            self.newest_reply = Some(slot.sequence);
        }
        let mut remote_close = None;
        for frame in packet.frames.iter() {
            if let Frame::ConnectionClose { code, .. } = frame {
                remote_close = Some(code);
            }
            self.apply_reply_frame(
                &frame,
                expected == PacketType::Fulfill,
                slot.sequence,
                newest,
            );
        }
        if let Some(code) = remote_close {
            self.finish_close(code, true);
        }
        Some(packet.prepare_amount)
    }

    fn apply_reply_frame(
        &mut self,
        frame: &Frame<'_>,
        fulfilled: bool,
        sequence: u64,
        newest: bool,
    ) {
        match *frame {
            Frame::StreamMaxMoney {
                stream_id,
                receive_max,
                total_received,
            } => self.take_remote_maximum(stream_id, receive_max, total_received, sequence, newest),
            Frame::StreamReceipt { stream_id, receipt } if fulfilled => {
                if let (Some(stream), Ok(receipt)) = (
                    self.stream_mut(stream_id),
                    <[u8; RECEIPT_LENGTH]>::try_from(receipt),
                ) {
                    stream.last_receipt = Some(receipt);
                }
            }
            Frame::StreamMaxData { stream_id, .. }
                // Replies update limits of known streams but never open one.
                if self.stream(stream_id).is_some() =>
            {
                let _ = self.apply_stream_frame(stream_id, frame);
            }
            Frame::StreamClose { .. }
            | Frame::ConnectionMaxStreamId { .. }
            | Frame::ConnectionMaxData { .. }
            | Frame::ConnectionAssetDetails { .. } => {
                let _ = self.apply_control_frame(frame);
            }
            _ => {}
        }
    }

    /// Takes the receive maximum a reply to request `sequence` reports for
    /// a known stream, if the reply says what the receiver takes now
    /// (`Stream::hear_remote_limit`).
    fn take_remote_maximum(
        &mut self,
        stream_id: u64,
        receive_max: u64,
        total_received: u64,
        sequence: u64,
        newest: bool,
    ) {
        let next_sequence = self.next_sequence;
        // Replies never open a stream.
        if let Some(stream) = self.stream_mut(stream_id) {
            stream.hear_remote_limit(
                receive_max,
                total_received,
                Some(sequence),
                newest,
                next_sequence,
            );
        }
    }

    fn settle_fulfilled(&mut self, slot: &Slot, reply: Option<u64>) {
        for &(id, amount) in &slot.money {
            if let Some(stream) = self.stream_mut(id) {
                stream.held -= amount;
                stream.sent += amount;
            }
            if amount > 0 {
                self.push_event(Event::MoneySent { stream_id: id });
            }
        }
        self.acknowledge_closes(slot);
        self.stats.packets_fulfilled = self.stats.packets_fulfilled.saturating_add(1);
        self.stats.amount_sent = self.stats.amount_sent.saturating_add(slot.source_amount);
        self.stats.amount_delivered = self
            .stats
            .amount_delivered
            .saturating_add(reply.unwrap_or(slot.min_destination_amount));
        self.increase_concurrency();
        self.retry_delay_ms = self.policy.retry_delay_start_ms;
    }

    fn acknowledge_closes(&mut self, slot: &Slot) {
        for &id in &slot.closes {
            if let Some(stream) = self.stream_mut(id) {
                stream.local_close_acknowledged = true;
            }
        }
        if slot.kind == PacketKind::Close {
            self.close_acknowledged = true;
        }
    }

    /// Returns a packet's money, data and limit updates to be sent again.
    fn undo(&mut self, slot: &Slot) {
        if slot.window_update.is_some() {
            self.window_update_pending = true;
        }
        for &(id, amount) in &slot.money {
            if let Some(stream) = self.stream_mut(id) {
                stream.held -= amount;
            }
        }
        for &(id, start, _) in &slot.data {
            if let Some(stream) = self.stream_mut(id) {
                stream.send.rewind(start);
            }
        }
    }

    fn settle_rejected(
        &mut self,
        now: Timestamp,
        slot: &Slot,
        reject: &Reject<'_>,
        reply: Option<u64>,
    ) {
        self.undo(slot);
        self.stats.packets_rejected = self.stats.packets_rejected.saturating_add(1);
        if reply.is_some() {
            // The receiver processed the frames: closes and limits included.
            self.acknowledge_closes(slot);
            // Only if the application raised nothing since this packet left.
            if slot.window_update == Some(self.window_update_sequence) {
                self.window_update_pending = false;
            }
        }
        let failure = match (reject.code, reply) {
            (IlpErrorCode::F99_APPLICATION_ERROR, Some(_)) => Failure::Receiver,
            (IlpErrorCode::F08_AMOUNT_TOO_LARGE, _) => Failure::TooLarge,
            (code, _) => match code.class() {
                ErrorClass::Temporary | ErrorClass::Relative => Failure::Temporary,
                ErrorClass::Final | ErrorClass::Unknown => Failure::Final,
            },
        };
        if failure == Failure::TooLarge {
            self.learn_max_packet_amount(slot.source_amount, reject);
        }
        if slot.kind == PacketKind::Probe {
            self.probe_settled(now, slot.source_amount, failure, reply);
            return;
        }
        match failure {
            Failure::Receiver => {
                if let Some(received) = reply
                    && slot.source_amount > 0
                {
                    if received < slot.min_destination_amount {
                        self.rate_too_low = true;
                        self.push_event(Event::ExchangeRateTooLow {
                            expected: slot.min_destination_amount,
                            received,
                        });
                    } else {
                        self.learn_rate(received, slot.source_amount);
                    }
                }
                // The receiver refused; its new limits are applied, but
                // sending again at once could repeat the refusal.
                self.back_off_for(now, slot);
            }
            Failure::TooLarge => {}
            Failure::Temporary => self.back_off_for(now, slot),
            // As upstream does: a final error means this connection's
            // packets cannot get through (the original kept retrying).
            Failure::Final => self.finish_close(ErrorCode::INTERNAL_ERROR, false),
        }
    }

    fn settle_lost(&mut self, now: Timestamp, slot: &Slot) {
        self.undo(slot);
        if slot.kind == PacketKind::Probe {
            self.probe_settled(now, slot.source_amount, Failure::Temporary, None);
        } else {
            self.back_off_for(now, slot);
        }
    }

    /// Backs off for a packet that was refused or lost. Packets in flight
    /// together are usually refused together, by the same full credit
    /// limit: that is one back-off, or nine packets would wait 25.6 s.
    fn back_off_for(&mut self, now: Timestamp, slot: &Slot) {
        if slot.sequence >= self.back_off_sequence {
            self.back_off(now);
        }
    }

    fn back_off(&mut self, now: Timestamp) {
        self.back_off_sequence = self.next_sequence;
        self.decrease_concurrency();
        if matches!(self.state, State::Closed) {
            return;
        }
        self.retry_at = Some(now.saturating_add_ms(self.retry_delay_ms));
        self.retry_delay_ms = self
            .retry_delay_ms
            .saturating_mul(2)
            .min(self.policy.retry_delay_max_ms);
    }

    /// A reply showed the path delivering `received` for `source`: if that
    /// is better than expected, expect it from now on, so the receiver's
    /// limits are converted at what actually arrives.
    fn learn_rate(&mut self, received: u64, source: u64) {
        let Some(observed) = Rate::new(received, source) else {
            return;
        };
        let better = self
            .expected_rate
            .is_none_or(|expected| observed.at_least(expected) && observed != expected);
        if better {
            self.expected_rate = Some(observed);
        }
    }

    /// Lowers the packet limit after an F08: the packet's amount scaled by
    /// what the rejecting hop accepts over what it received.
    fn learn_max_packet_amount(&mut self, source_amount: u64, reject: &Reject<'_>) {
        // An F08 for a packet without money says nothing about amounts.
        if source_amount == 0 {
            return;
        }
        let limit = match AmountTooLarge::from_reject(reject) {
            Some(data) if data.received_amount > 0 => {
                let scaled = u128::from(source_amount) * u128::from(data.maximum_amount)
                    / u128::from(data.received_amount);
                u64::try_from(scaled).unwrap_or(u64::MAX)
            }
            // Without data (ILP-4) the best guess is half.
            _ => source_amount / 2,
        };
        self.max_packet_amount = self
            .max_packet_amount
            .min(limit.min(source_amount.saturating_sub(1)));
        if self.max_packet_amount == 0 {
            // The path carries no money at all: give up, as upstream does.
            self.finish_close(ErrorCode::INTERNAL_ERROR, false);
        }
    }

    fn probe_settled(&mut self, now: Timestamp, amount: u64, failure: Failure, reply: Option<u64>) {
        let max_packet = self.max_packet_amount;
        let Some(probing) = self.probing.as_mut() else {
            return;
        };
        probing.outstanding = probing.outstanding.saturating_sub(1);
        if let Some(received) = reply
            && received > 0
        {
            let candidate = (digits(received), amount);
            let better = probing
                .best
                .is_none_or(|(best_digits, best_amount, _)| candidate > (best_digits, best_amount));
            if better && let Some(rate) = Rate::new(received, amount) {
                probing.best = Some((candidate.0, amount, rate));
            }
        }
        match failure {
            Failure::TooLarge if max_packet > 0 => probing.retry(max_packet),
            // Every probe lost or refused for the moment is tried again,
            // smaller in case the amount was what failed, as with too
            // little liquidity. Retrying only the smallest of them would
            // retry a 1-unit probe, which can never be precise, and forget
            // the larger ones: on a lossy path, two unlucky volleys would
            // end the measurement.
            Failure::Temporary => {
                probing.temporary = true;
                probing.retry(amount - amount / 3);
            }
            _ => {}
        }
        self.finish_volley(now);
    }

    fn finish_volley(&mut self, now: Timestamp) {
        let Some(probing) = self.probing.as_mut() else {
            return;
        };
        if probing.outstanding > 0 || probing.queued > 0 {
            return;
        }
        if let Some((precision, _, rate)) = probing.best
            && precision >= self.policy.min_rate_precision_digits
        {
            self.probing = None;
            self.set_exchange_rate(rate.with_slippage(self.policy.slippage_permille));
            self.push_event(Event::ExchangeRateMeasured { rate });
            return;
        }
        if probing.volleys >= self.policy.max_probe_volleys {
            self.probing = None;
            self.push_event(Event::ExchangeRateMeasurementFailed);
            return;
        }
        probing.volleys += 1;
        // A probe no larger than the best result's cannot get more digits
        // than it did, so it is not sent again.
        let best_amount = probing.best.map_or(0, |(_, amount, _)| amount);
        let retries = probing.retries;
        for &amount in retries.get(..probing.retry_count).unwrap_or_default() {
            let amount = amount.min(self.max_packet_amount);
            if amount > best_amount {
                probing.enqueue(amount);
            }
        }
        probing.retry_count = 0;
        let temporary = core::mem::take(&mut probing.temporary);
        if probing.queued == 0 {
            self.probing = None;
            self.push_event(Event::ExchangeRateMeasurementFailed);
        } else if temporary {
            self.back_off(now);
        }
    }

    /// Housekeeping after anything settled: free acknowledged data, forget
    /// finished streams, and finish closing.
    pub(super) fn after_step(&mut self, _now: Timestamp) {
        self.release_send_buffers();
        self.sweep_streams();
        if let State::Closing { code, .. } = self.state {
            let idle = self.slots.iter().all(|slot| !slot.active);
            let attempts_spent = self.close_attempts >= self.policy.close_retries;
            if idle && (self.close_acknowledged || attempts_spent) {
                self.finish_close(code, false);
            }
        }
    }

    fn release_send_buffers(&mut self) {
        for stream in &mut self.streams {
            if stream.send.is_empty() {
                continue;
            }
            let in_flight_start = self
                .slots
                .iter()
                .filter(|slot| slot.active)
                .flat_map(|slot| slot.data.iter())
                .filter(|(id, _, _)| *id == stream.id)
                .map(|&(_, start, _)| start)
                .min();
            let delivered = in_flight_start.map_or(stream.send.next_offset(), |start| {
                start.min(stream.send.next_offset())
            });
            stream.send.release(delivered);
        }
    }

    fn sweep_streams(&mut self) {
        let mut index = 0;
        while let Some(stream) = self.streams.get(index) {
            if !stream.is_finished() {
                index += 1;
                continue;
            }
            let stream = self.streams.remove(index);
            let code = stream
                .local_close
                .or(stream.remote_close)
                .unwrap_or(ErrorCode::NO_ERROR);
            if stream.opened_by_remote {
                self.mark_remote_stream_closed(stream.id);
                // Let the remote open another in its place (as upstream does).
                self.local_max_stream_id += 2;
                self.announce_pending = true;
            }
            self.push_event(Event::StreamClosed {
                stream_id: stream.id,
                code,
                total_sent: stream.sent,
                total_received: stream.received,
            });
        }
    }
}

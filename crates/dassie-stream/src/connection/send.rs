//! Building outgoing packets.

use dassie_ilp::{Address, MAX_DATA_LENGTH, Prepare, Timestamp};
use dassie_oer::Writer;
use rand_core::CryptoRng;

use super::{BUFFER_LENGTH, Connection, PacketKind, Role, Slot, State, Transmit};
use crate::crypto::{Keys, OVERHEAD};
use crate::frame::{ErrorCode, Frame};
use crate::packet::{self, PacketType};
use crate::rate::Rate;
use crate::stream::Stream;

/// Largest STREAM plaintext that fits an ILP packet's data once encrypted.
pub(super) const MAX_PLAINTEXT: usize = MAX_DATA_LENGTH - OVERHEAD;

/// Most bytes a `StreamData` frame needs besides its data: type, length
/// determinant, two 9-byte integers and the data's length determinant.
const DATA_FRAME_OVERHEAD: usize = 1 + 3 + 9 + 9 + 3;

/// The least a stream with a delivery cap offers in a packet: what
/// delivers a unit at the minimum `rate`, or one unit when it accepts
/// anything.
fn least_under_cap(rate: Rate) -> u64 {
    rate.min_source_for_one_unit().unwrap_or(1)
}

/// Counts and sizes the frames `visit` produces.
pub(super) fn measure(visit: impl Fn(&mut dyn FnMut(Frame<'_>))) -> (usize, usize) {
    let mut count = 0;
    let mut length = 0;
    visit(&mut |frame| {
        count += 1;
        length += frame.encoded_len();
    });
    (count, length)
}

/// Writes the STREAM packet made of `visit`'s frames into `buffer` after
/// room for the encryption overhead, then encrypts it in place. Returns the
/// ciphertext length.
#[allow(clippy::too_many_arguments)]
pub(super) fn seal(
    keys: &Keys,
    rng: &mut (impl CryptoRng + ?Sized),
    buffer: &mut [u8],
    sequence: u64,
    packet_type: PacketType,
    prepare_amount: u64,
    (count, frames_length): (usize, usize),
    visit: impl Fn(&mut dyn FnMut(Frame<'_>)),
) -> usize {
    let plaintext_len = packet::header_len(sequence, prepare_amount, count) + frames_length;
    assert!(plaintext_len <= MAX_PLAINTEXT, "STREAM packet too large");
    assert!(
        buffer.len() >= OVERHEAD + plaintext_len,
        "STREAM buffer too small"
    );
    let (region, _) = buffer.split_at_mut(OVERHEAD + plaintext_len);
    let (_, body) = region.split_at_mut(OVERHEAD);
    let mut writer = Writer::new(body);
    packet::encode_header(&mut writer, sequence, packet_type, prepare_amount, count);
    visit(&mut |frame| frame.encode_to(&mut writer));
    assert!(
        writer.position() == plaintext_len,
        "frames changed between passes"
    );
    keys.cipher.encrypt(rng, plaintext_len, region)
}

impl Connection {
    /// Writes the next packet to send into `buffer`, if there is one to
    /// send now.
    ///
    /// # Panics
    ///
    /// If `buffer` is shorter than [`BUFFER_LENGTH`].
    pub fn poll_transmit(
        &mut self,
        now: Timestamp,
        rng: &mut (impl CryptoRng + ?Sized),
        buffer: &mut [u8],
    ) -> Option<Transmit> {
        assert!(
            buffer.len() >= BUFFER_LENGTH,
            "buffer shorter than BUFFER_LENGTH"
        );
        if matches!(self.state, State::Closed) || self.remote_address.is_none() {
            return None;
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return None;
        }
        // The idle clock starts with the first chance to send.
        self.last_activity.get_or_insert(now);
        self.retry_at = None;
        let index = self.slots.iter().position(|slot| !slot.active)?;
        let mut slot = core::mem::replace(self.slots.get_mut(index)?, Slot::new(0));
        let planned = match self.state {
            State::Closing { code, .. } => self.plan_close(&mut slot, code),
            _ => self.plan_probe(&mut slot) || self.plan_normal(&mut slot),
        };
        let transmit = planned.then(|| {
            let transmit = self.encode_request(now, rng, &mut slot, buffer);
            self.commit_request(&mut slot);
            transmit
        });
        if let Some(target) = self.slots.get_mut(index) {
            *target = slot;
        }
        transmit
    }

    fn plan_close(&mut self, slot: &mut Slot, _code: ErrorCode) -> bool {
        let close_in_flight = self.active_slots(PacketKind::Close) > 0;
        if close_in_flight || self.close_attempts >= self.policy.close_retries {
            return false;
        }
        self.close_attempts += 1;
        slot.kind = PacketKind::Close;
        true
    }

    fn plan_probe(&mut self, slot: &mut Slot) -> bool {
        let Some(probing) = self.probing.as_mut() else {
            return false;
        };
        let Some(amount) = probing.dequeue() else {
            return false;
        };
        probing.outstanding += 1;
        slot.kind = PacketKind::Probe;
        slot.source_amount = amount;
        slot.announced = !self.remote_knows_us || self.announce_pending;
        true
    }

    fn plan_normal(&mut self, slot: &mut Slot) -> bool {
        if self.active_slots(PacketKind::Normal) >= self.allowed_concurrency() {
            return false;
        }
        slot.kind = PacketKind::Normal;
        slot.announced = !self.remote_knows_us || self.announce_pending;
        self.plan_money(slot);
        for stream in &self.streams {
            if stream.local_close.is_some() && !stream.local_close_acknowledged {
                slot.closes.push(stream.id);
            }
        }
        self.plan_data(slot);
        let worthwhile = slot.source_amount > 0
            || !slot.data.is_empty()
            || !slot.closes.is_empty()
            || (slot.announced && self.role == Role::Client && !self.remote_knows_us)
            || self.stream_id_blocked.is_some()
            || self.window_update_pending;
        if !worthwhile {
            slot.reset();
        }
        worthwhile
    }

    fn plan_money(&self, slot: &mut Slot) {
        let (Some(rate), false) = (self.exchange_rate, self.money_stopped) else {
            return;
        };
        let mut budget = self.max_packet_amount;
        // The receiver's room is converted at the rate the path is expected
        // to deliver, not the minimum: at the minimum a packet that fits
        // would arrive too large whenever the real rate is better, and be
        // refused forever.
        let room_rate = self.expected_rate.unwrap_or(rate);
        for stream in self.streams.iter().filter(|stream| stream.can_send()) {
            // What is in flight takes from that room already, so packets
            // sent together never offer more than the receiver takes, or
            // than the application lets the stream deliver.
            let capped = stream.delivery_max != u64::MAX;
            let cap = self.delivery_cap(stream, rate);
            let room = room_rate
                .max_source_amount(stream.remote_room())
                .min(cap)
                .saturating_sub(stream.held);
            let unsent = stream.unsent_amount();
            let mut sendable = unsent.min(room);
            let no_room = stream.remote_room() == 0;
            let short = stream.remote_received < stream.expected_delivery;
            let ask = stream.remote_max_fell || short;
            if no_room && unsent > 0 && stream.held == 0 && ask {
                // No room for what is left, after the receiver lowered its
                // maximum, or while it took less than the application
                // expects it to, as when another payment took from what it
                // pays into before this stream first heard its maximum.
                // Offer it the least that delivers a unit anyway, once per
                // back-off, so that it can say why: a reply with the room
                // it has now, or a close with the reason it takes no more
                // (spec/areas/stream.md#money-limit). Waiting instead
                // would wait for a word it may never send. A receiver that
                // never lowered its maximum and took what it was expected
                // to is simply full. One that still has room, less than a
                // source unit delivers, is not offered more than it takes.
                sendable = rate
                    .min_source_for_one_unit()
                    .filter(|&least| least <= unsent && least <= cap)
                    .unwrap_or(0);
            }
            let mut amount = sendable.min(budget);
            // Under a cap, a packet must deliver a unit at the minimum rate:
            // one that may deliver nothing would pay for nothing, again and
            // again, as the cap's last units come near.
            if capped && amount < least_under_cap(rate) {
                amount = 0;
            }
            if amount > 0 {
                slot.money.push((stream.id, amount));
                budget -= amount;
                slot.source_amount += amount;
            }
        }
        // Dust waits while other money is in flight: those packets may fail
        // and free a larger amount. Alone, even a small amount goes, so a
        // payment below the threshold completes (the original never sent
        // it, ILP-11).
        let money_in_flight = self
            .slots
            .iter()
            .any(|other| other.active && other.source_amount > 0);
        if slot.source_amount < self.policy.de_minimis_amount && money_in_flight {
            slot.money.clear();
            slot.source_amount = 0;
        }
        if slot.source_amount > 0 {
            slot.min_destination_amount = rate.destination_amount(slot.source_amount);
        }
    }

    /// Whether a stream delivered all its cap lets it: no packet that
    /// delivers a unit at the minimum rate fits what is left, at the
    /// expected rate, and none of its money is in flight. False without a
    /// cap or a rate.
    pub(super) fn cap_reached(&self, stream: &Stream) -> bool {
        let Some(rate) = self.exchange_rate else {
            return false;
        };
        let room_rate = self.expected_rate.unwrap_or(rate);
        stream.delivery_max != u64::MAX
            && stream.held == 0
            && room_rate.exact_max_source_amount(stream.delivery_room()) < least_under_cap(rate)
    }

    /// The most source units a stream may offer under its delivery cap, at
    /// the expected rate: unlimited without one. What is left of the cap is
    /// converted exactly, since hops only round down, so the last unit can
    /// go. Until a reply showed the rate the path delivers, only half of
    /// it, rounded up: sized at the minimum rate, a packet that fit the
    /// whole would overshoot by however much better the path is. If half
    /// fits no packet that delivers a unit at the minimum `rate`, but the
    /// whole does, the least such packet goes, or the stream would never
    /// send.
    fn delivery_cap(&self, stream: &Stream, rate: Rate) -> u64 {
        if stream.delivery_max == u64::MAX {
            return u64::MAX;
        }
        let room_rate = self.expected_rate.unwrap_or(rate);
        let left = stream.delivery_room();
        let whole = room_rate.exact_max_source_amount(left);
        if self.rate_seen {
            return whole;
        }
        let half = room_rate.exact_max_source_amount(left - left / 2);
        let least = least_under_cap(rate);
        if half < least && whole >= least {
            least
        } else {
            half
        }
    }

    fn plan_data(&self, slot: &mut Slot) {
        let (_, control_length) = measure(|visit| self.visit_request(slot, visit));
        let fixed = packet::header_len(u64::MAX, u64::MAX, 512) + control_length;
        let mut budget = MAX_PLAINTEXT.saturating_sub(fixed);
        let connection_room = self
            .remote_connection_max_data
            .saturating_sub(self.data_sent_total());
        let mut connection_budget = connection_room;
        for stream in self.streams.iter().filter(|stream| stream.can_send()) {
            if budget <= DATA_FRAME_OVERHEAD || !stream.send.has_unsent() {
                continue;
            }
            // New bytes beyond the stream's high-water mark count against
            // the connection's limit; retransmissions do not.
            let high = stream.send.high_offset();
            let limit = stream
                .remote_max_data
                .min(high.saturating_add(connection_budget));
            let (offset, bytes) = stream.send.unsent(budget - DATA_FRAME_OVERHEAD, limit);
            if bytes.is_empty() {
                continue;
            }
            let end = offset + bytes.len() as u64;
            connection_budget = connection_budget.saturating_sub(end.saturating_sub(high));
            budget -= DATA_FRAME_OVERHEAD + bytes.len();
            slot.data.push((stream.id, offset, end));
        }
    }

    /// Produces the frames of the request planned in `slot`. Called twice
    /// per packet, to measure and to write, so it must not change anything.
    pub(super) fn visit_request(&self, slot: &Slot, visit: &mut dyn FnMut(Frame<'_>)) {
        if slot.announced {
            self.visit_announcement(visit);
        }
        match slot.kind {
            PacketKind::Probe => return,
            PacketKind::Close => {
                let code = match self.state {
                    State::Closing { code, .. } => code,
                    _ => ErrorCode::NO_ERROR,
                };
                visit(Frame::ConnectionClose { code, message: "" });
                return;
            }
            PacketKind::Normal => {}
        }
        if let Some(max_stream_id) = self.stream_id_blocked {
            visit(Frame::ConnectionStreamIdBlocked { max_stream_id });
        }
        visit(Frame::ConnectionMaxData {
            max_offset: self.connection_max_data(),
        });
        for stream in &self.streams {
            if let Some(code) = stream.local_close {
                if slot.closes.contains(&stream.id) {
                    visit(Frame::StreamClose {
                        stream_id: stream.id,
                        code,
                        message: "",
                    });
                }
            } else {
                self.visit_stream_limits(stream, visit);
            }
        }
        for &(stream_id, shares) in &slot.money {
            visit(Frame::StreamMoney { stream_id, shares });
        }
        for &(stream_id, start, end) in &slot.data {
            if let Some(stream) = self.stream(stream_id) {
                visit(Frame::StreamData {
                    stream_id,
                    offset: start,
                    data: stream.send.slice(start, end),
                });
            }
        }
    }

    /// This side's address, asset and limits.
    pub(super) fn visit_announcement(&self, visit: &mut dyn FnMut(Frame<'_>)) {
        if self.role == Role::Client
            && let Some(address) = self.local_address.as_deref()
        {
            visit(Frame::ConnectionNewAddress {
                source_account: Address::parse(address).ok(),
            });
        }
        if let Some((code, scale)) = &self.local_asset {
            visit(Frame::ConnectionAssetDetails {
                asset_code: code,
                asset_scale: *scale,
            });
        }
        visit(Frame::ConnectionMaxStreamId {
            max_stream_id: self.local_max_stream_id,
        });
    }

    pub(super) fn visit_stream_limits(
        &self,
        stream: &crate::stream::Stream,
        visit: &mut dyn FnMut(Frame<'_>),
    ) {
        // Under a connection limit, a stream reports the application's
        // count, which survives reconnections and leaves out what the
        // application does not count yet, and as its maximum that count
        // plus what is left of the limit, or the most it was told before:
        // a maximum never falls (spec/decisions/close-rather-than-lower.md).
        let (receive_max, total_received) = if self.money_limit == u64::MAX {
            (stream.receive_max, stream.received)
        } else {
            (self.money_told_total(stream), self.money_received)
        };
        visit(Frame::StreamMaxMoney {
            stream_id: stream.id,
            receive_max,
            total_received,
        });
        visit(Frame::StreamMaxData {
            stream_id: stream.id,
            max_offset: stream.receive.max_offset(),
        });
    }

    // Panics only on a broken invariant: addresses are checked when stored.
    #[allow(clippy::unreachable)]
    fn encode_request(
        &self,
        now: Timestamp,
        rng: &mut (impl CryptoRng + ?Sized),
        slot: &mut Slot,
        buffer: &mut [u8],
    ) -> Transmit {
        let Some(destination) = self
            .remote_address
            .as_deref()
            .and_then(|text| Address::parse(text).ok())
        else {
            unreachable!("remote addresses are validated when stored");
        };
        let sequence = self.next_sequence;
        let measured = measure(|visit| self.visit_request(slot, visit));
        let plaintext_len =
            packet::header_len(sequence, slot.min_destination_amount, measured.0) + measured.1;
        let data_len = OVERHEAD + plaintext_len;
        let expires_at = now.saturating_add_ms(self.policy.packet_timeout_ms);
        let mut prepare = Prepare {
            amount: slot.source_amount,
            expires_at,
            execution_condition: [0; 32],
            destination,
            data: &[],
        };
        let header_len = prepare.header_len(data_len);
        let (head, rest) = buffer.split_at_mut(header_len);
        let sealed = seal(
            &self.keys,
            rng,
            rest,
            sequence,
            PacketType::Prepare,
            slot.min_destination_amount,
            measured,
            |visit| self.visit_request(slot, visit),
        );
        assert!(sealed == data_len, "sealed length mismatch");
        let condition = if slot.kind == PacketKind::Probe {
            let mut random = [0; 32];
            rng.fill_bytes(&mut random);
            random
        } else {
            self.keys
                .condition(rest.get(..data_len).unwrap_or_default())
        };
        prepare.execution_condition = condition;
        prepare.encode_header(data_len, &mut Writer::new(head));
        slot.sequence = sequence;
        slot.expires_at = expires_at;
        slot.condition = condition;
        Transmit {
            request_id: sequence,
            length: header_len + data_len,
            expires_at,
        }
    }

    fn commit_request(&mut self, slot: &mut Slot) {
        slot.active = true;
        self.next_sequence += 1;
        self.stats.packets_sent = self.stats.packets_sent.saturating_add(1);
        for &(id, amount) in &slot.money {
            if let Some(stream) = self.streams.iter_mut().find(|stream| stream.id == id) {
                stream.held += amount;
            }
        }
        for &(id, start, end) in &slot.data {
            if let Some(stream) = self.streams.iter_mut().find(|stream| stream.id == id) {
                let length = usize::try_from(end - start).unwrap_or(0);
                stream.send.mark_sent(length);
                self.stats.bytes_sent = self.stats.bytes_sent.saturating_add(end - start);
            }
        }
        if slot.kind == PacketKind::Normal {
            self.note_money_told();
            self.stream_id_blocked = None;
            slot.window_update = self
                .window_update_pending
                .then_some(self.window_update_sequence);
            self.window_update_pending = false;
        }
    }
}

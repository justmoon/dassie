//! Responses from downstream, and packets that expire.

use dassie_ilp::{Address, ErrorCode, MAX_PACKET_LENGTH, Reject, Response, condition_for};
use dassie_ledger::{Ledger, LedgerError};
use dassie_oer::{Decode, Encode};

use super::{Connector, Pending, Upstream};
use crate::endpoint::EndpointId;
use crate::trace::PacketState;

/// How a packet in flight ended.
enum Resolution<'a> {
    /// Fulfilled with a matching fulfillment: pass these bytes on.
    Fulfilled(&'a [u8]),
    /// Rejected downstream: pass these bytes on.
    Rejected(&'a [u8], ErrorCode),
    /// Ended here: send our own reject.
    Failed(ErrorCode, &'static str),
}

impl Connector {
    /// Handles a response from `from` to the Prepare sent to it with
    /// `request_id`. Returns what to send upstream, written into `out`, if
    /// anything.
    ///
    /// A response after the outgoing expiry counts as the timeout it is;
    /// one that matches no packet in flight is dropped.
    ///
    /// # Panics
    ///
    /// If `out` is shorter than [`MAX_PACKET_LENGTH`], or the ledger's
    /// journal is full: the caller persists it after every step.
    pub fn handle_response(
        &mut self,
        now_ms: u64,
        from: EndpointId,
        request_id: u32,
        packet: &[u8],
        ledger: &mut Ledger,
        out: &mut [u8],
    ) -> Option<Upstream> {
        assert!(
            out.len() >= MAX_PACKET_LENGTH,
            "buffer shorter than a packet"
        );
        let Some(entry) = self.take(from, request_id) else {
            self.stats.unexpected_responses += 1;
            return None;
        };
        let resolution = if now_ms >= entry.expires_at_ms {
            // The timer just has not fired yet (ILP-1's other half).
            Resolution::Failed(ErrorCode::R00_TRANSFER_TIMED_OUT, "packet timed out")
        } else {
            match Response::decode_exact(packet) {
                Ok(Response::Fulfill(fulfill)) => {
                    if condition_for(&fulfill.fulfillment) == entry.condition {
                        Resolution::Fulfilled(packet)
                    } else {
                        // ILP-2: the original dropped it and timed out.
                        self.stats.wrong_fulfillments += 1;
                        Resolution::Failed(ErrorCode::F05_WRONG_CONDITION, "invalid fulfillment")
                    }
                }
                Ok(Response::Reject(reject)) => Resolution::Rejected(packet, reject.code),
                Err(_) => Resolution::Failed(
                    ErrorCode::T00_INTERNAL_ERROR,
                    "invalid response from the next hop",
                ),
            }
        };
        self.resolve(now_ms, &entry, &resolution, ledger, out)
    }

    /// Fails the next packet whose deadline has come: R00 when it expired,
    /// T01 when its next hop went away. Call until it returns `None`; a
    /// packet whose source is gone is resolved without output.
    ///
    /// # Panics
    ///
    /// As [`handle_response`](Self::handle_response).
    pub fn handle_timeout(
        &mut self,
        now_ms: u64,
        ledger: &mut Ledger,
        out: &mut [u8],
    ) -> Option<Upstream> {
        assert!(
            out.len() >= MAX_PACKET_LENGTH,
            "buffer shorter than a packet"
        );
        while let Some(&(at, destination, request_id)) = self.deadlines.first() {
            if at > now_ms {
                return None;
            }
            // Every deadline belongs to a packet in flight (an invariant);
            // without one, this would loop for ever.
            let entry = self.take(destination, request_id);
            assert!(entry.is_some(), "a deadline without its packet");
            let entry = entry?;
            let resolution = match entry.failure {
                Some((code, message)) => Resolution::Failed(code, message),
                None => Resolution::Failed(ErrorCode::R00_TRANSFER_TIMED_OUT, "packet timed out"),
            };
            if let Some(upstream) = self.resolve(now_ms, &entry, &resolution, ledger, out) {
                return Some(upstream);
            }
        }
        None
    }

    /// Removes a packet from flight.
    fn take(&mut self, destination: EndpointId, request_id: u32) -> Option<Pending> {
        let entry = self.pending.remove(&(destination, request_id))?;
        self.deadlines
            .remove(&(entry.expires_at_ms, destination, request_id));
        if !entry.source_gone {
            self.incoming
                .remove(&(entry.source, entry.source_request_id));
            if let Some(source) = self.endpoints.get_mut(entry.source) {
                source.pending_from -= 1;
            }
        }
        Some(entry)
    }

    /// Settles the transfers and writes the response for upstream.
    fn resolve(
        &mut self,
        now_ms: u64,
        entry: &Pending,
        resolution: &Resolution<'_>,
        ledger: &mut Ledger,
        out: &mut [u8],
    ) -> Option<Upstream> {
        let fulfilled = matches!(resolution, Resolution::Fulfilled(_));
        self.charge(now_ms, entry, fulfilled);
        for id in entry.transfers.into_iter().flatten() {
            let settled = if fulfilled {
                ledger.post(now_ms, id)
            } else {
                ledger.void(now_ms, id)
            };
            // The transfers are the packet's own and the caller persists the
            // journal after every step: failing here would lose money.
            assert!(
                !matches!(settled, Err(LedgerError::JournalFull)),
                "the journal is full; it must be persisted after every step"
            );
            assert!(settled.is_ok(), "a packet's transfer is gone: {settled:?}");
        }
        let (state, length) = match *resolution {
            Resolution::Fulfilled(bytes) => {
                self.stats.fulfilled += 1;
                (PacketState::Fulfilled, copy(bytes, out))
            }
            Resolution::Rejected(bytes, code) => {
                self.stats.rejected += 1;
                (PacketState::Rejected(code), copy(bytes, out))
            }
            Resolution::Failed(code, message) => {
                if code == ErrorCode::F05_WRONG_CONDITION {
                    self.stats.rejected += 1;
                } else {
                    self.stats.expired += 1;
                }
                let reject = Reject {
                    code,
                    triggered_by: Address::parse(&self.own_address).ok(),
                    message,
                    data: &[],
                };
                (PacketState::Rejected(code), reject.encode(out))
            }
        };
        self.trace.update(entry.trace, |record| {
            record.state = state;
            record.resolved_at_ms = Some(now_ms);
        });
        (!entry.source_gone).then_some(Upstream {
            to: entry.source,
            request_id: entry.source_request_id,
            length,
        })
    }
}

impl Connector {
    /// Settles a resolved packet with its account's spending cap: a
    /// fulfilled one is spent, either way it is no longer in flight.
    fn charge(&mut self, now_ms: u64, entry: &Pending, fulfilled: bool) {
        let Some((account, amount)) = entry.charge else {
            return;
        };
        let Some(cap) = self.caps.get_mut(&account) else {
            return;
        };
        if fulfilled {
            cap.post(now_ms, amount);
            self.cap_changes.insert(account);
        } else {
            cap.release(amount);
        }
    }
}

/// Copies a response that decoded, and so fits a packet, into `out`.
fn copy(bytes: &[u8], out: &mut [u8]) -> usize {
    match out.get_mut(..bytes.len()) {
        Some(target) => {
            target.copy_from_slice(bytes);
            bytes.len()
        }
        None => 0,
    }
}

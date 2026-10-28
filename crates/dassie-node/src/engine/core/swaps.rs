//! The node's own swaps with its peers (spec/decisions/peer-swaps.md,
//! spec/areas/connector.md#swaps): each ledger's swap endpoint, a local
//! endpoint on the owner's `local/swap` account, sends a swap out to a peer
//! on the method of its ledger, and takes it back when it returns on the
//! method its address names.
//!
//! A swap carries data of the node's own, and its condition is derived
//! from that data: the fulfillment is an HMAC under a secret the node holds
//! in memory, so a swap coming back is matched to the one that went out by
//! its condition alone, and the node keeps nothing per swap in flight. The
//! peer never reads the data; a peer that changed it, or sent the swap back
//! on another method, gets nothing for it.
//!
//! What to swap, with whom and when is the swap service's; it is not built
//! yet ([roadmap](spec/roadmap.md#peer-swaps)). This is what it sends with
//! ([`Core::send_swap`]) and hears back from ([`Core::poll_swap_outcome`]).

use std::collections::VecDeque;

use dassie_connector::{EndpointId, Forwarded, OpensNone, Outcome};
use dassie_identity::NodeId;
use dassie_ilp::{
    Address, ErrorCode, Fulfill, Prepare, Reject, Response, Timestamp, condition_for,
};
use dassie_ledger::{AccountName, LedgerId, OwnerSub};
use dassie_oer::{Decode, Encode};
use ring::hmac;
use subtle::ConstantTimeEq;

use super::packets::ledger_slug;
use super::{Core, EngineError};

/// The prefix of a swap endpoint's address segment below the node's own;
/// the ledger's slug follows. No packet reaches it by that address.
pub(super) const SWAP_SEGMENT: &str = "swap-";

/// The version of the data the node's swaps carry.
pub const SWAP_DATA_VERSION: u8 = 1;

/// The length of the data the node's swaps carry: its version, the least
/// it takes back, and a nonce that makes each swap's condition its own.
pub const SWAP_DATA_LENGTH: usize = 1 + 8 + NONCE_LENGTH;

const NONCE_LENGTH: usize = 16;

/// Most outcomes of the node's swaps kept until the swap service takes
/// them; past it, the oldest go.
pub const MAX_SWAP_OUTCOMES: usize = 1_024;

/// The owner's account a ledger's swap endpoint pays from and is paid into.
fn swap_account() -> AccountName {
    AccountName::EquityOwnerSub(OwnerSub::Local(dassie_ledger::name!("swap")))
}

/// A ledger's swap endpoint address.
fn swap_address(own_address: &str, ledger: LedgerId) -> String {
    format!("{own_address}.{SWAP_SEGMENT}{}", ledger_slug(ledger))
}

/// A swap to send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewSwap {
    /// The peer to swap with.
    pub peer: NodeId,
    /// The ledger of the method it is paid on: the swap endpoint there pays
    /// it out of the owner's funds, unconverted.
    pub pay_on: LedgerId,
    /// The ledger of the method it is to come back on, another of the
    /// peering's.
    pub back_on: LedgerId,
    /// What it pays, on `pay_on`.
    pub amount: u64,
    /// The least it takes back, on `back_on`: less is rejected (F04).
    pub minimum: u64,
    /// How long it may take, both ways: its expiry is this long from now.
    pub timeout_ms: u64,
}

/// A swap sent: what its outcome will name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapSent {
    /// The request ID its outcome comes with.
    pub request_id: u32,
    /// Its condition, which its way back carries too.
    pub condition: [u8; 32],
}

/// How one of the node's swaps ended: once for each swap sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapOutcome {
    /// The request ID it was sent with.
    pub request_id: u32,
    /// The ledger it paid on.
    pub ledger: LedgerId,
    /// Fulfilled, or the code it was rejected with, by this node, the
    /// peer, or this node's swap endpoint on the way back. T04 and T05 say
    /// to back off and try again.
    pub result: Result<(), ErrorCode>,
}

/// The node's swaps: the secret their fulfillments derive from, the next
/// request ID, and the outcomes the swap service has yet to take.
#[derive(Default)]
pub(super) struct Swaps {
    /// Drawn with the node's first swap, held in memory only: a swap in
    /// flight at a restart is lost as any packet in flight is.
    secret: Option<[u8; 32]>,
    next_request: u32,
    outcomes: VecDeque<SwapOutcome>,
    /// Outcomes dropped for want of room.
    dropped: u64,
}

/// What the node's swaps carry, decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SwapData {
    minimum: u64,
    nonce: [u8; NONCE_LENGTH],
}

impl SwapData {
    fn encode(self) -> [u8; SWAP_DATA_LENGTH] {
        let mut data = [0; SWAP_DATA_LENGTH];
        let (version, rest) = data.split_at_mut(1);
        let (minimum, nonce) = rest.split_at_mut(8);
        version.copy_from_slice(&[SWAP_DATA_VERSION]);
        minimum.copy_from_slice(&self.minimum.to_be_bytes());
        nonce.copy_from_slice(&self.nonce);
        data
    }

    /// The data of a swap of this version, or nothing.
    fn decode(data: &[u8]) -> Option<Self> {
        let data: &[u8; SWAP_DATA_LENGTH] = data.try_into().ok()?;
        let (version, rest) = data.split_first()?;
        let (minimum, nonce) = rest.split_at(8);
        (*version == SWAP_DATA_VERSION).then_some(Self {
            minimum: u64::from_be_bytes(minimum.try_into().ok()?),
            nonce: nonce.try_into().ok()?,
        })
    }
}

/// The fulfillment of a swap with `data` that is to come back on `back_on`:
/// an HMAC under `secret` over the ledger and the data, so a swap sent
/// back on another method than it named has another, and fails.
fn fulfillment(secret: &[u8; 32], back_on: LedgerId, data: &[u8]) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
    let mut context = hmac::Context::with_key(&key);
    // A ledger's name has no NUL: the two parts never run together.
    context.update(back_on.as_str().as_bytes());
    context.update(&[0]);
    context.update(data);
    let mut out = [0; 32];
    out.copy_from_slice(context.sign().as_ref());
    out
}

/// Whether the node fulfills a swap of its own coming back on `on`, and
/// with what: its condition must be the one its data derives, else F05,
/// and it must bring at least the minimum its data names, else F04.
/// Without a secret, the node sent no swap, and it is not one of ours.
fn decide(
    secret: Option<&[u8; 32]>,
    on: LedgerId,
    prepare: &Prepare<'_>,
) -> Result<[u8; 32], ErrorCode> {
    let (Some(secret), Some(data)) = (secret, SwapData::decode(prepare.data)) else {
        return Err(ErrorCode::F05_WRONG_CONDITION);
    };
    let fulfillment = fulfillment(secret, on, prepare.data);
    let condition = condition_for(&fulfillment);
    if !bool::from(condition.ct_eq(&prepare.execution_condition)) {
        return Err(ErrorCode::F05_WRONG_CONDITION);
    }
    if prepare.amount < data.minimum {
        return Err(ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT);
    }
    Ok(fulfillment)
}

impl Core {
    /// Gives a ledger its swap endpoint, a local endpoint on the owner's
    /// `local/swap` account.
    pub(super) fn add_swap_endpoint(&mut self, ledger: LedgerId) -> Result<(), EngineError> {
        if self.connector.swap_endpoint(ledger).is_some() {
            return Ok(());
        }
        let endpoint = self.free_local();
        let account = self
            .ledger
            .open(dassie_ledger::AccountPath::new(ledger, swap_account()))
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        let own = self.connector.own_address().to_string();
        self.connector
            .add_swap_endpoint(endpoint, &swap_address(&own, ledger), account, ledger)
            .map_err(|e| EngineError::Internal(e.to_string()))
    }

    /// Sends a swap to a peer: from the swap endpoint on `pay_on`, out of
    /// the owner's funds, to the peer on its method there, addressed to
    /// `peer.swap.<scheme>` of `back_on`'s method, with data of the node's
    /// own that its condition derives from. Every swap sent has one
    /// outcome, which [`poll_swap_outcome`](Self::poll_swap_outcome) gives:
    /// at once, if the connector refuses it here, or when it is answered;
    /// none if the scheme of `pay_on` goes while it is in flight, which
    /// takes its swap endpoint.
    ///
    /// # Errors
    ///
    /// No swap endpoint on `pay_on`, or the node is not peered with `peer`.
    pub fn send_swap(&mut self, swap: NewSwap) -> Result<SwapSent, EngineError> {
        let now = self.now_ms();
        let source = self
            .connector
            .swap_endpoint(swap.pay_on)
            .ok_or_else(|| EngineError::Precondition("no swaps on that ledger".into()))?;
        let to = self
            .peer_endpoints
            .get(&swap.peer)
            .map(|endpoint| EndpointId::Peer(*endpoint))
            .ok_or_else(|| EngineError::NotFound("no such peer".into()))?;
        let destination = format!("peer.swap.{}", ledger_slug(swap.back_on));
        let destination = Address::parse(&destination)
            .map_err(|_| EngineError::Internal("a swap's address".into()))?;
        let rng = &mut self.rng;
        let secret = *self.swaps.secret.get_or_insert_with(|| {
            let mut secret = [0; 32];
            rng.fill_bytes(&mut secret);
            secret
        });
        let mut nonce = [0; NONCE_LENGTH];
        self.rng.fill_bytes(&mut nonce);
        let data = SwapData {
            minimum: swap.minimum,
            nonce,
        }
        .encode();
        let condition = condition_for(&fulfillment(&secret, swap.back_on, &data));
        let packet = Prepare {
            amount: swap.amount,
            expires_at: Timestamp::from_unix_ms(now.saturating_add(swap.timeout_ms)),
            execution_condition: condition,
            destination,
            data: &data,
        }
        .to_vec();
        let request_id = self.swaps.next_request;
        self.swaps.next_request = request_id.wrapping_add(1);
        let outcome = self.connector.send_swap(
            now,
            (source, request_id),
            to,
            &packet,
            &mut self.ledger,
            &self.prices.table,
            &mut OpensNone,
            &mut self.out,
        );
        match outcome {
            Outcome::Forward(forwarded) => self.forward_out(now, forwarded),
            Outcome::Respond(length) => {
                let response = self.out.get(..length).unwrap_or_default();
                let result = result_of(response);
                self.swap_outcome(request_id, swap.pay_on, result);
            }
            // A request ID comes back only after 2^32 swaps, long after its
            // swap resolved; the source is the swap endpoint just found.
            Outcome::Duplicate | Outcome::UnknownSource => {
                return Err(EngineError::Internal("a swap's request ID".into()));
            }
        }
        self.take_journal();
        Ok(SwapSent {
            request_id,
            condition,
        })
    }

    /// The next outcome of the node's swaps, oldest first.
    pub fn poll_swap_outcome(&mut self) -> Option<SwapOutcome> {
        self.swaps.outcomes.pop_front()
    }

    fn swap_outcome(&mut self, request_id: u32, ledger: LedgerId, result: Result<(), ErrorCode>) {
        if self.swaps.outcomes.len() >= MAX_SWAP_OUTCOMES {
            self.swaps.outcomes.pop_front();
            self.swaps.dropped = self.swaps.dropped.saturating_add(1);
        }
        self.swaps.outcomes.push_back(SwapOutcome {
            request_id,
            ledger,
            result,
        });
    }

    /// The answer to one of the node's swaps, come back to its swap
    /// endpoint on `ledger`: its outcome.
    pub(super) fn swap_answered(&mut self, ledger: LedgerId, request_id: u32, length: usize) {
        let result = result_of(self.up.get(..length).unwrap_or_default());
        self.swap_outcome(request_id, ledger, result);
    }

    /// One of the node's own swaps come back to its swap endpoint on
    /// `ledger`, the method its address names: fulfilled if its condition
    /// is the node's and it brings the minimum its data names, rejected
    /// otherwise, at once.
    pub(super) fn swap_arrived(&mut self, now: u64, ledger: LedgerId, forwarded: Forwarded) {
        let packet = self.out.get(..forwarded.length).unwrap_or_default();
        let decided = Prepare::decode_exact(packet)
            .map_err(|_| ErrorCode::F01_INVALID_PACKET)
            .and_then(|prepare| {
                let named = dassie_connector::swap_scheme(prepare.destination);
                if named == Some(ledger_slug(ledger)) {
                    decide(self.swaps.secret.as_ref(), ledger, &prepare)
                } else {
                    Err(ErrorCode::F02_UNREACHABLE)
                }
            });
        let length = match decided {
            Ok(fulfillment) => Fulfill {
                fulfillment,
                data: &[],
            }
            .encode(&mut self.scratch),
            Err(code) => Reject {
                code,
                triggered_by: Address::parse(self.connector.own_address()).ok(),
                message: "not a swap of ours",
                data: &[],
            }
            .encode(&mut self.scratch),
        };
        let responded = self.connector.handle_response(
            now,
            forwarded.to,
            forwarded.request_id,
            self.scratch.get(..length).unwrap_or_default(),
            &mut self.ledger,
            &mut self.up,
        );
        self.responded(now, responded, None);
    }

    /// The swap endpoints are those of the active schemes' ledgers, and
    /// the outcomes kept within their bound.
    pub(super) fn check_swaps(&self) {
        assert!(self.swaps.outcomes.len() <= MAX_SWAP_OUTCOMES);
        for active in self.schemes.values() {
            let ledger = active.scheme.ledger();
            let endpoint = self.connector.swap_endpoint(ledger);
            assert!(endpoint.is_some(), "an active ledger has its swap endpoint");
            assert_eq!(
                endpoint.and_then(|endpoint| self.connector.swap_ledger(endpoint)),
                Some(ledger),
                "a swap endpoint on its own ledger"
            );
        }
    }
}

/// Fulfilled, or the code of the Reject; a response that does not decode
/// is as an internal error's.
fn result_of(response: &[u8]) -> Result<(), ErrorCode> {
    match Response::decode_exact(response) {
        Ok(Response::Fulfill(_)) => Ok(()),
        Ok(Response::Reject(reject)) => Err(reject.code),
        Err(_) => Err(ErrorCode::T00_INTERNAL_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swap_with(data: &[u8], amount: u64, condition: [u8; 32]) -> Prepare<'_> {
        Prepare {
            amount,
            expires_at: Timestamp::from_unix_ms(0),
            execution_condition: condition,
            destination: dassie_ilp::address!("peer.swap.stub"),
            data,
        }
    }

    #[test]
    fn swap_data_round_trips_and_is_versioned() {
        let data = SwapData {
            minimum: 0x0102_0304_0506_0708,
            nonce: [7; NONCE_LENGTH],
        };
        let bytes = data.encode();
        assert_eq!(bytes[0], SWAP_DATA_VERSION);
        assert_eq!(bytes[1..9], [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(SwapData::decode(&bytes), Some(data));
        let mut other = bytes;
        other[0] = 2;
        assert_eq!(SwapData::decode(&other), None);
        assert_eq!(SwapData::decode(&bytes[..SWAP_DATA_LENGTH - 1]), None);
        assert_eq!(SwapData::decode(&[]), None);
    }

    #[test]
    fn the_node_fulfills_only_its_own_swaps_with_their_minimum() {
        let secret = [3; 32];
        let ledger = LedgerId::StubUsd;
        let data = SwapData {
            minimum: 1_000,
            nonce: [1; NONCE_LENGTH],
        }
        .encode();
        let fulfilled = fulfillment(&secret, ledger, &data);
        let condition = condition_for(&fulfilled);
        let decide_on = |on, prepare: &Prepare<'_>| decide(Some(&secret), on, prepare);
        assert_eq!(
            decide_on(ledger, &swap_with(&data, 1_000, condition)),
            Ok(fulfilled)
        );
        assert_eq!(
            decide_on(ledger, &swap_with(&data, 5_000, condition)),
            Ok(fulfilled)
        );
        assert_eq!(
            decide_on(ledger, &swap_with(&data, 999, condition)),
            Err(ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT)
        );
        // Another condition, data changed, another ledger, no secret.
        let wrong = ErrorCode::F05_WRONG_CONDITION;
        assert_eq!(
            decide_on(ledger, &swap_with(&data, 1_000, [0; 32])),
            Err(wrong)
        );
        let mut changed = data;
        changed[SWAP_DATA_LENGTH - 1] ^= 1;
        assert_eq!(
            decide_on(ledger, &swap_with(&changed, 1_000, condition)),
            Err(wrong)
        );
        let lowered = SwapData {
            minimum: 1,
            nonce: [1; NONCE_LENGTH],
        }
        .encode();
        assert_eq!(
            decide_on(ledger, &swap_with(&lowered, 1_000, condition)),
            Err(wrong)
        );
        let other = LedgerId::XrplTestnetXrp;
        assert_eq!(
            decide_on(other, &swap_with(&data, 1_000, condition)),
            Err(wrong)
        );
        assert_eq!(
            decide(None, ledger, &swap_with(&data, 1_000, condition)),
            Err(wrong)
        );
        assert_eq!(
            decide_on(ledger, &swap_with(b"short", 1_000, condition)),
            Err(wrong)
        );
    }
}

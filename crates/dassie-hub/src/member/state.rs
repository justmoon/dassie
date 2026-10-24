//! What the account and the scheme keep across restarts, in canonical OER
//! with fixed-width integers, as the hub's own messages are. Each starts
//! with a version byte.
//!
//! - **The account** keeps its number, its cursor, the holds it mirrors and
//!   the operations waiting or on their way: one on its way is signed again
//!   alike (Ed25519 signs deterministically), so its outcome is still known
//!   as ours.
//! - **The scheme** keeps its peers and the records of their holds: the next
//!   sequence, W and T of our claims and the packets the hub may still pay,
//!   and what we fulfilled that no claim of the payer's covers. Packets in
//!   flight at a restart are gone with the node's own: one of ours counts as
//!   given up, one of the peer's as never taken.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_ledger::Name;
use dassie_oer::{Encode, Reader};
use dassie_settlement::OwnerPaymentState;

use super::account::{HubAccount, Purpose, Queued};
use super::mirror::HoldMirror;
use super::payee::{Claiming, Done, Payee, Taken};
use super::payer::{Packet, PaidOnHub, Payer, State};
use super::scheme::{HubScheme, Peer, Settling};
use crate::codec::{read_key, write_key};
use crate::digest::Digest;
use crate::operation::{AccountNumber, MemberOperation, SignedMemberOperation};
use crate::promise::{Claim, Commitment, HoldNumber, Signed};

const VERSION: u8 = 1;

/// Bytes being written.
struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    /// As canonical OER writes it: `FF` for true.
    fn bool(&mut self, value: bool) {
        self.u8(if value { 0xff } else { 0 });
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn u128(&mut self, value: u128) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn count(&mut self, count: usize) {
        self.u32(u32::try_from(count).unwrap_or(u32::MAX));
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn name(&mut self, name: &Name) {
        let text = name.as_str().as_bytes();
        self.u8(u8::try_from(text.len()).unwrap_or(0));
        self.bytes(text);
    }

    fn encoded<T: Encode>(&mut self, value: &T) {
        self.bytes(&value.to_vec());
    }

    fn option<T>(&mut self, value: Option<&T>, write: impl FnOnce(&mut Self, &T)) {
        self.bool(value.is_some());
        if let Some(value) = value {
            write(self, value);
        }
    }
}

fn read_u128(reader: &mut Reader<'_>) -> Option<u128> {
    Some(u128::from_be_bytes(*reader.read_array::<16>().ok()?))
}

fn read_name(reader: &mut Reader<'_>) -> Option<Name> {
    let length = usize::from(reader.read_u8().ok()?);
    let mut text = Vec::with_capacity(length);
    for _ in 0..length {
        text.push(reader.read_u8().ok()?);
    }
    Name::new(core::str::from_utf8(&text).ok()?).ok()
}

fn read_option<T>(
    reader: &mut Reader<'_>,
    read: impl FnOnce(&mut Reader<'_>) -> Option<T>,
) -> Option<Option<T>> {
    if reader.read_bool().ok()? {
        Some(Some(read(reader)?))
    } else {
        Some(None)
    }
}

/// A count of entries, each at least `least` bytes, that the rest of the
/// input can hold: a count from no state of ours allocates nothing.
fn read_count(reader: &mut Reader<'_>, least: usize, most: usize) -> Option<usize> {
    let count = usize::try_from(reader.read_u32().ok()?).ok()?;
    (count <= most && count.saturating_mul(least) <= reader.remaining()).then_some(count)
}

/// A number that changes whenever `state` does.
#[must_use]
pub(super) fn fingerprint(state: &[u8]) -> u64 {
    state.iter().fold(0xcbf2_9ce4_8422_2325, |mix, byte| {
        (mix ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The bytes a lowercase hex string spells.
#[must_use]
pub(super) fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs
        .iter()
        .map(|[high, low]| Some(digit(*high)? << 4 | digit(*low)?))
        .collect()
}

// The account.

/// The account's state.
pub(super) fn account_state(account: &HubAccount) -> Vec<u8> {
    let mut out = Out(Vec::new());
    out.u8(VERSION);
    out.option(account.number.as_ref(), |out, number| out.u32(number.0));
    out.u64(account.cursor);
    out.u64(account.last_touch);
    out.count(account.holds.len());
    for (number, hold) in &account.holds {
        out.u64(number.0);
        write_hold(&mut out, hold);
        out.bool(account.strangers.contains(number));
    }
    out.count(account.queue.len());
    for queued in &account.queue {
        write_purpose(&mut out, queued.purpose);
        out.bool(queued.ready);
        out.option(queued.sent.as_ref(), |out, (signed, _)| {
            out.u64(signed.sequence)
        });
        out.encoded(&queued.operation);
    }
    out.0
}

fn write_hold(out: &mut Out, hold: &HoldMirror) {
    out.u32(hold.payer.0);
    out.u32(hold.payee.0);
    out.u64(hold.amount);
    out.option(hold.reduction.as_ref(), |out, (amount, at)| {
        out.u64(*amount);
        out.u64(*at);
    });
    out.u64(hold.total);
    out.u64(hold.through);
    out.count(hold.claimed.len());
    for (sequence, paid) in &hold.claimed {
        out.u64(*sequence);
        out.u64(*paid);
    }
}

fn read_hold(reader: &mut Reader<'_>) -> Option<HoldMirror> {
    let mut hold = HoldMirror::opened(
        AccountNumber(reader.read_u32().ok()?),
        AccountNumber(reader.read_u32().ok()?),
        reader.read_u64().ok()?,
    );
    hold.reduction = read_option(reader, |r| Some((r.read_u64().ok()?, r.read_u64().ok()?)))?;
    hold.total = reader.read_u64().ok()?;
    hold.through = reader.read_u64().ok()?;
    let count = read_count(reader, 16, usize::from(u16::MAX))?;
    for _ in 0..count {
        hold.claimed
            .insert(reader.read_u64().ok()?, reader.read_u64().ok()?);
    }
    Some(hold)
}

fn write_purpose(out: &mut Out, purpose: Purpose) {
    match purpose {
        Purpose::Settle(settlement) => {
            out.u8(0);
            out.name(&settlement);
        }
        Purpose::Redeem(hold) => {
            out.u8(1);
            out.u64(hold.0);
        }
        Purpose::Claim(hold) => {
            out.u8(2);
            out.u64(hold.0);
        }
        Purpose::Disbursal => out.u8(3),
        Purpose::Withdraw => out.u8(4),
    }
}

fn read_purpose(reader: &mut Reader<'_>) -> Option<Purpose> {
    Some(match reader.read_u8().ok()? {
        0 => Purpose::Settle(read_name(reader)?),
        1 => Purpose::Redeem(HoldNumber(reader.read_u64().ok()?)),
        2 => Purpose::Claim(HoldNumber(reader.read_u64().ok()?)),
        3 => Purpose::Disbursal,
        4 => Purpose::Withdraw,
        _ => return None,
    })
}

/// Restores the account's state, refusing one it did not write.
pub(super) fn restore_account(account: &mut HubAccount, state: &[u8]) -> Option<()> {
    let mut reader = Reader::new(state);
    if reader.read_u8().ok()? != VERSION {
        return None;
    }
    account.number = read_option(&mut reader, |r| Some(AccountNumber(r.read_u32().ok()?)))?;
    account.cursor = reader.read_u64().ok()?;
    account.last_touch = reader.read_u64().ok()?;
    let most = super::account::MAX_HOLDS + super::account::MAX_STRANGER_HOLDS;
    let holds = read_count(&mut reader, 8, most)?;
    for _ in 0..holds {
        let number = HoldNumber(reader.read_u64().ok()?);
        account.holds.insert(number, read_hold(&mut reader)?);
        if reader.read_bool().ok()? {
            account.strangers.insert(number);
        }
    }
    let queued = read_count(&mut reader, 3, super::account::MAX_QUEUED)?;
    for _ in 0..queued {
        let entry = read_queued(account, &mut reader)?;
        if account.queue.iter().any(|q| q.purpose == entry.purpose) {
            return None;
        }
        account.queue.push_back(entry);
    }
    reader.finish().ok()?;
    let sent = account.queue.iter().filter(|q| q.sent.is_some()).count();
    let bounded = account.strangers.len() <= super::account::MAX_STRANGER_HOLDS
        && account.holds.len().saturating_sub(account.strangers.len()) <= super::account::MAX_HOLDS;
    (sent <= 1 && bounded && account.last_touch <= account.cursor).then_some(())
}

fn read_queued(account: &HubAccount, reader: &mut Reader<'_>) -> Option<Queued> {
    let purpose = read_purpose(reader)?;
    let ready = reader.read_bool().ok()?;
    let sequence = read_option(reader, |r| r.read_u64().ok())?;
    let operation = MemberOperation::decode(reader).ok()?;
    let sent = match (sequence, account.number) {
        (Some(sequence), Some(number)) => Some((
            SignedMemberOperation::sign(
                &account.hub,
                number,
                sequence,
                account.rules.terms.fees.operation,
                operation.clone(),
                &account.key,
            ),
            0,
        )),
        (Some(_), None) => return None,
        (None, _) => None,
    };
    Some(Queued {
        purpose,
        operation,
        ready,
        sent,
    })
}

// The scheme.

/// The scheme's state.
pub(super) fn scheme_state(scheme: &HubScheme) -> Vec<u8> {
    let mut out = Out(Vec::new());
    out.u8(VERSION);
    out.bool(scheme.synced);
    out.u64(scheme.cursor);
    out.option(scheme.account.as_ref(), |out, number| out.u32(number.0));
    out.count(scheme.peers.len());
    for (name, peer) in &scheme.peers {
        out.name(name);
        out.u32(peer.account.0);
        write_key_to(&mut out, &peer.key);
        write_payer(&mut out, &peer.payer);
        write_payee(&mut out, &peer.payee);
    }
    out.count(scheme.settling.len());
    for (id, settling) in &scheme.settling {
        out.name(id);
        out.name(&settling.peer);
        out.u64(settling.drops);
    }
    out.option(
        scheme.counters.as_ref(),
        |out, (deposited, withdrawn, returned)| {
            out.u64(*deposited);
            out.u64(*withdrawn);
            out.u64(*returned);
        },
    );
    out.u64(scheme.withdrawals_made);
    out.option(scheme.withdrawal.as_ref(), |out, (number, outcome)| {
        out.u64(*number);
        out.u8(match outcome {
            OwnerPaymentState::Unknown => 0,
            OwnerPaymentState::InFlight => 1,
            OwnerPaymentState::Applied => 2,
            OwnerPaymentState::Failed => 3,
        });
    });
    out.u64(scheme.booked_ahead);
    out.0
}

fn write_key_to(out: &mut Out, key: &dassie_identity::PublicKey) {
    let mut bytes = [0; 32];
    let mut writer = dassie_oer::Writer::new(&mut bytes);
    write_key(&mut writer, key);
    out.bytes(&bytes);
}

fn write_payer(out: &mut Out, payer: &Payer) {
    out.option(payer.hold.as_ref(), |out, hold| out.u64(hold.0));
    out.u64(payer.next_sequence);
    out.u64(payer.through);
    out.u128(payer.through_units);
    out.count(payer.packets.len());
    for (sequence, packet) in &payer.packets {
        out.u64(*sequence);
        out.u64(packet.units);
        out.encoded(&packet.commitment);
        out.u8(match packet.state {
            State::InFlight | State::Waiting | State::GaveUp => 0,
            State::Fulfilled { taken: true } => 1,
            State::Fulfilled { taken: false } => 2,
            State::Rejected => 3,
            State::Decided => 4,
        });
        out.option(packet.paid.as_ref(), |out, paid| {
            out.bytes(&paid.fulfillment);
            out.u64(paid.drops);
            out.bytes(paid.operation.as_bytes());
        });
    }
}

fn read_payer(reader: &mut Reader<'_>) -> Option<Payer> {
    let mut payer = Payer::new();
    payer.hold = read_option(reader, |r| Some(HoldNumber(r.read_u64().ok()?)))?;
    payer.next_sequence = reader.read_u64().ok()?;
    payer.through = reader.read_u64().ok()?;
    payer.through_units = read_u128(reader)?;
    let count = read_count(reader, 8, super::payer::MAX_PACKETS)?;
    for _ in 0..count {
        let sequence = reader.read_u64().ok()?;
        let units = reader.read_u64().ok()?;
        let commitment = Commitment::decode(reader).ok()?;
        // In flight or waiting at a restart: the node rejected it upstream.
        let state = match reader.read_u8().ok()? {
            0 => State::GaveUp,
            1 => State::Fulfilled { taken: true },
            2 => State::Fulfilled { taken: false },
            3 => State::Rejected,
            4 => State::Decided,
            _ => return None,
        };
        let paid = read_option(reader, |r| {
            Some(PaidOnHub {
                fulfillment: *r.read_array::<32>().ok()?,
                drops: r.read_u64().ok()?,
                operation: Digest(*r.read_array::<32>().ok()?),
            })
        })?;
        let packet = Packet {
            request_id: None,
            units,
            commitment,
            state,
            paid,
        };
        let fits = sequence < payer.next_sequence && sequence == commitment.sequence;
        if !fits || payer.packets.insert(sequence, packet).is_some() {
            return None;
        }
    }
    (payer.next_sequence >= 1 && payer.through < payer.next_sequence).then_some(payer)
}

fn write_payee(out: &mut Out, payee: &Payee) {
    out.option(payee.hold.as_ref(), |out, hold| out.u64(hold.0));
    out.bool(payee.key_confirmed);
    out.option(payee.accepted.as_ref(), |out, claim| out.encoded(claim));
    out.u128(payee.base_units);
    let done: Vec<(&u64, &Taken, Done)> = payee
        .taken
        .iter()
        .filter_map(|(sequence, taken)| Some((sequence, taken, taken.done?)))
        .collect();
    out.count(done.len());
    for (sequence, taken, done) in done {
        out.u64(*sequence);
        out.encoded(&taken.commitment);
        out.u64(taken.units);
        out.bytes(&done.fulfillment);
        out.option(done.ready_ms.as_ref(), |out, ready| out.u64(*ready));
        out.u8(match done.claiming {
            Claiming::No => 0,
            Claiming::Submitted => 1,
            Claiming::Paid => 2,
            Claiming::Lost => 3,
        });
        out.u8(done.tries);
    }
}

fn read_payee(reader: &mut Reader<'_>) -> Option<Payee> {
    let mut payee = Payee {
        hold: read_option(reader, |r| Some(HoldNumber(r.read_u64().ok()?)))?,
        key_confirmed: reader.read_bool().ok()?,
        accepted: read_option(reader, |r| Signed::<Claim>::decode(r).ok())?,
        base_units: read_u128(reader)?,
        taken: BTreeMap::new(),
        ..Payee::default()
    };
    let count = read_count(reader, 8, super::payee::MAX_MISSING)?;
    for _ in 0..count {
        let sequence = reader.read_u64().ok()?;
        let commitment = Signed::<Commitment>::decode(reader).ok()?;
        let units = reader.read_u64().ok()?;
        let fulfillment = *reader.read_array::<32>().ok()?;
        let ready_ms = read_option(reader, |r| r.read_u64().ok())?;
        let claiming = match reader.read_u8().ok()? {
            0 => Claiming::No,
            1 => Claiming::Submitted,
            2 => Claiming::Paid,
            3 => Claiming::Lost,
            _ => return None,
        };
        let tries = reader.read_u8().ok()?;
        let taken = Taken {
            request_id: None,
            commitment,
            units,
            done: Some(Done {
                fulfillment,
                ready_ms,
                claiming,
                tries,
            }),
        };
        if sequence != commitment.value.sequence || payee.taken.insert(sequence, taken).is_some() {
            return None;
        }
    }
    Some(payee)
}

/// Restores the scheme's state, refusing one it did not write.
pub(super) fn restore_scheme(scheme: &mut HubScheme, state: &[u8]) -> Option<()> {
    let mut reader = Reader::new(state);
    if reader.read_u8().ok()? != VERSION {
        return None;
    }
    scheme.synced = reader.read_bool().ok()?;
    scheme.cursor = reader.read_u64().ok()?;
    scheme.account = read_option(&mut reader, |r| Some(AccountNumber(r.read_u32().ok()?)))?;
    let peers = read_count(&mut reader, 40, super::scheme::MAX_PEERS)?;
    for _ in 0..peers {
        let name = read_name(&mut reader)?;
        let account = AccountNumber(reader.read_u32().ok()?);
        let key = read_key(&mut reader).ok()?;
        let peer = Peer {
            account,
            key,
            payer: read_payer(&mut reader)?,
            payee: read_payee(&mut reader)?,
        };
        if scheme.peers.insert(name, peer).is_some() {
            return None;
        }
    }
    let settling = read_count(&mut reader, 10, 64)?;
    for _ in 0..settling {
        let id = read_name(&mut reader)?;
        let peer = read_name(&mut reader)?;
        let drops = reader.read_u64().ok()?;
        if drops == 0
            || scheme
                .settling
                .insert(id, Settling { peer, drops })
                .is_some()
        {
            return None;
        }
    }
    scheme.counters = read_option(&mut reader, |r| {
        Some((r.read_u64().ok()?, r.read_u64().ok()?, r.read_u64().ok()?))
    })?;
    scheme.withdrawals_made = reader.read_u64().ok()?;
    scheme.withdrawal = read_option(&mut reader, |r| {
        let number = r.read_u64().ok()?;
        let outcome = match r.read_u8().ok()? {
            0 => OwnerPaymentState::Unknown,
            1 => OwnerPaymentState::InFlight,
            2 => OwnerPaymentState::Applied,
            3 => OwnerPaymentState::Failed,
            _ => return None,
        };
        (number <= scheme.withdrawals_made).then_some((number, outcome))
    })?;
    scheme.booked_ahead = reader.read_u64().ok()?;
    reader.finish().ok()?;
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_reads_what_names_write() {
        let bytes = [0x01, 0xab, 0xff];
        let name = Name::hex(&bytes, false);
        assert_eq!(hex_bytes(name.as_str()), Some(bytes.to_vec()));
        assert_eq!(hex_bytes("abc"), None);
        assert_eq!(hex_bytes("AB"), None);
    }

    #[test]
    fn a_count_past_the_input_is_refused() {
        let bytes = [0, 0, 0, 9, 1, 2];
        let mut reader = Reader::new(&bytes);
        assert_eq!(read_count(&mut reader, 1, 100), None);
    }
}

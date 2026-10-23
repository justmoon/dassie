//! The stub scheme: test money without an underlying ledger
//! (spec/areas/settlement.md#the-stub-scheme).

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
use rand_core::CryptoRng;

use crate::asset::ExternalAsset;
use crate::commitments::{Commitments, Decided, Held, HoldsWith, Membership, Promised};
use crate::scheme::{Effect, PreparedSettlement, SchemeError, SettlementScheme};

/// The balance the stub starts with on its first activation: 100 USD at
/// scale 9, as in the original. Later activations start from the
/// checkpoint instead (ACC-6).
pub const STUB_INITIAL_BALANCE: u64 = 100_000_000_000;

/// Most settlements prepared and not executed.
const MAX_PREPARED: usize = 64;

/// The most packets a stub with commitments holds in flight per peer each
/// way: the most its config may ask for.
pub const MAX_STUB_COMMITMENTS: usize = 64;

/// The most peers a stub with commitments holds packets for, and holds
/// with.
const MAX_COMMITMENT_PEERS: usize = 1_024;

/// The most fulfillments it keeps per peer that no claim covers: past it,
/// the oldest is taken as claimed on the hub, as a payee whose payer's
/// claim never came claims it there.
const MAX_UNCLAIMED: usize = 64;

/// The most waits it has decided that the node has not taken.
const MAX_DECIDED: usize = 1_024;

/// What a stub commitment holds: the promise's amount, then its condition.
const COMMITMENT_BYTES: usize = 8 + 32;

/// The claim a stub payer writes after every verdict on a Fulfill.
const CLAIM: &[u8] = b"stub claim";

/// The pretend hub that every stub with commitments is a member of.
const STUB_HUB: [u8; 32] = *b"the stub with commitments' hub..";

/// The version of a stub with commitments' state.
const STATE_VERSION: u8 = 1;

/// The stub scheme. Its "underlying ledger" is a number, which the node's
/// checkpoint keeps across restarts.
///
/// **Test realm only:** a peer's settlement claim is believed as stated
/// (ACC-23). Anyone peered over the stub can mint credit, which is why it
/// only ever holds test money.
///
/// **With commitments,** for tests of the packet path over a settlement hub
/// without a hub: its peerings carry a commitment with each Prepare and a
/// claim in each reply to a Fulfill, as a hub's do, and it keeps the books
/// a hub's member keeps of them, in memory, which a restart forgets along
/// with the connector's packets. It emulates the holds between it and its
/// peers too, which its state keeps across restarts
/// (spec/areas/settlement.md#the-stub-scheme).
#[derive(Debug)]
pub struct Stub {
    balance: u64,
    prepared: BTreeMap<Name, (Name, u64)>,
    hub: Option<StubHub>,
}

impl Stub {
    /// The stub, with the balance last checkpointed, or the initial balance
    /// on first activation, and the state last checkpointed. Its config is
    /// empty or `{}`, or `{"commitments": N}` for a stub with commitments
    /// that holds at most `N` packets in flight per peer each way, `N` from
    /// 1 to [`MAX_STUB_COMMITMENTS`]. Only a stub with commitments has a
    /// state: its holds.
    ///
    /// # Errors
    ///
    /// If `config` is anything else, or the state is not one the stub
    /// wrote with that config.
    pub fn new(config: &str, checkpoint: Option<u64>, state: &[u8]) -> Result<Self, SchemeError> {
        let limit = commitments_config(config).ok_or(SchemeError::InvalidConfig)?;
        let hub = match limit {
            Some(limit) => Some(StubHub::new(limit, state)?),
            None if state.is_empty() => None,
            None => return Err(SchemeError::InvalidConfig),
        };
        Ok(Self {
            balance: checkpoint.unwrap_or(STUB_INITIAL_BALANCE),
            prepared: BTreeMap::new(),
            hub,
        })
    }
}

/// The limit of a stub with commitments that `config` asks for: `Some(None)`
/// for a stub without; `None` for a config the stub does not take.
fn commitments_config(config: &str) -> Option<Option<usize>> {
    let config = config.trim();
    if config.is_empty() {
        return Some(None);
    }
    let inner = config.strip_prefix('{')?.strip_suffix('}')?.trim();
    if inner.is_empty() {
        return Some(None);
    }
    let (key, value) = inner.split_once(':')?;
    let value = value.trim();
    if key.trim() != "\"commitments\"" || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let limit: usize = value.parse().ok()?;
    (1..=MAX_STUB_COMMITMENTS)
        .contains(&limit)
        .then_some(Some(limit))
}

/// What a stub with commitments keeps of one peer's packets.
#[derive(Debug, Default)]
struct Packets {
    /// Our Prepares to the peer, committed to and neither answered nor
    /// decided, by request ID.
    sent: BTreeMap<u32, Promised>,
    /// The peer's Prepares whose commitments we took, neither fulfilled nor
    /// released.
    taken: BTreeMap<u32, Promised>,
    /// The peer's Prepares we fulfilled that no claim covers yet, by
    /// request ID with their amounts, oldest first.
    unclaimed: VecDeque<(u32, u64)>,
}

impl Packets {
    fn is_empty(&self) -> bool {
        self.sent.is_empty() && self.taken.is_empty() && self.unclaimed.is_empty()
    }
}

/// The side of a stub with commitments that emulates a hub's member: a
/// commitment is the promise's amount and condition, a claim is fixed, and
/// a packet that waits for the hub is decided at once without a
/// fulfillment, as by a hub whose final block passed the commitment's
/// expiry with no claim for it.
///
/// It emulates the holds as each side sees them: our settlement to a peer
/// funds our hold toward it once it executes, and its settlement to us
/// funds its hold toward us; a fulfilled packet is taken from the payer's
/// hold, as a redeem takes it, on the payer's side when it writes its claim
/// for a Fulfill its books took, and on the payee's side when that claim
/// arrives, or once the payee takes it as claimed on the hub. There is no
/// hub, so the two sides' views of a hold may differ, as two nodes' books
/// may.
#[derive(Debug)]
struct StubHub {
    /// The most packets in flight per peer each way.
    limit: usize,
    peers: BTreeMap<Name, Packets>,
    decided: VecDeque<Decided>,
    /// The holds with each peer, none at nothing either way.
    holds: BTreeMap<Name, HoldsWith>,
    /// Changes whenever the holds do: its state.
    version: u64,
}

impl StubHub {
    fn new(limit: usize, state: &[u8]) -> Result<Self, SchemeError> {
        assert!((1..=MAX_STUB_COMMITMENTS).contains(&limit));
        let hub = Self {
            limit,
            peers: BTreeMap::new(),
            decided: VecDeque::new(),
            holds: decode_holds(state).ok_or(SchemeError::InvalidData)?,
            version: 0,
        };
        hub.check_invariants();
        Ok(hub)
    }

    /// The packets of `peer`, made if there is room for another peer.
    fn packets(&mut self, peer: Name) -> Option<&mut Packets> {
        if !self.peers.contains_key(&peer) && self.peers.len() >= MAX_COMMITMENT_PEERS {
            return None;
        }
        Some(self.peers.entry(peer).or_default())
    }

    /// Forgets `peer` once nothing of it is held, so that the peers held
    /// are those with packets.
    fn tidy(&mut self, peer: Name) {
        if self.peers.get(&peer).is_some_and(Packets::is_empty) {
            self.peers.remove(&peer);
        }
    }

    /// Runs `f` on what is held of `peer`, if anything is.
    fn with<R>(&mut self, peer: Name, f: impl FnOnce(&mut Packets) -> R) -> Option<R> {
        let packets = self.peers.get_mut(&peer)?;
        let result = f(packets);
        self.tidy(peer);
        Some(result)
    }

    /// Whether there is room for the holds of `peer`.
    fn has_room_for(&self, peer: Name) -> bool {
        self.holds.contains_key(&peer) || self.holds.len() < MAX_COMMITMENT_PEERS
    }

    /// Changes the holds with `peer` by `f`, if there is room for them.
    fn change_holds(&mut self, peer: Name, f: impl FnOnce(&mut HoldsWith)) {
        if !self.has_room_for(peer) {
            return;
        }
        let holds = self.holds.entry(peer).or_default();
        let before = *holds;
        f(holds);
        assert!(holds.toward <= MAX_TOTAL && holds.from <= MAX_TOTAL);
        if *holds == HoldsWith::default() {
            self.holds.remove(&peer);
        }
        if self.holds.get(&peer).copied().unwrap_or_default() != before {
            self.version = self.version.wrapping_add(1);
        }
    }

    /// Our hold toward `peer` grew by what we settled.
    fn funded_toward(&mut self, peer: Name, amount: u64) {
        self.change_holds(peer, |holds| {
            holds.toward = holds.toward.saturating_add(amount).min(MAX_TOTAL);
        });
    }

    /// The hold toward us from `peer` grew by what it settled.
    fn funded_from(&mut self, peer: Name, amount: u64) {
        self.change_holds(peer, |holds| {
            holds.from = holds.from.saturating_add(amount).min(MAX_TOTAL);
        });
    }

    /// The payee took a packet of `amount` from our hold toward it.
    fn taken_toward(&mut self, peer: Name, amount: u64) {
        self.change_holds(peer, |holds| {
            holds.toward = holds.toward.saturating_sub(amount);
        });
    }

    /// We took a packet of `amount` from the hold toward us.
    fn taken_from(&mut self, peer: Name, amount: u64) {
        self.change_holds(peer, |holds| {
            holds.from = holds.from.saturating_sub(amount);
        });
    }

    fn check_invariants(&self) {
        assert!((1..=MAX_STUB_COMMITMENTS).contains(&self.limit));
        assert!(self.peers.len() <= MAX_COMMITMENT_PEERS);
        assert!(self.holds.len() <= MAX_COMMITMENT_PEERS);
        assert!(self.decided.len() <= MAX_DECIDED);
        for packets in self.peers.values() {
            assert!(!packets.is_empty(), "a peer held with nothing of it");
            assert!(packets.sent.len() <= self.limit);
            assert!(packets.taken.len() <= self.limit);
            assert!(packets.unclaimed.len() <= MAX_UNCLAIMED);
        }
        for holds in self.holds.values() {
            assert!(*holds != HoldsWith::default(), "holds of nothing kept");
            assert!(holds.toward <= MAX_TOTAL && holds.from <= MAX_TOTAL);
        }
    }
}

/// A stub commitment: the promise's amount, big-endian, then its condition.
fn commitment(promised: &Promised) -> [u8; COMMITMENT_BYTES] {
    let mut bytes = [0u8; COMMITMENT_BYTES];
    let (amount, condition) = bytes.split_at_mut(8);
    amount.copy_from_slice(&promised.amount.to_be_bytes());
    condition.copy_from_slice(&promised.condition);
    bytes
}

/// The holds as the state keeps them: nothing for none, else the version,
/// then each peer in order, its name's length and bytes, and what is left
/// in the holds toward it and from it, big-endian.
fn encode_holds(holds: &BTreeMap<Name, HoldsWith>) -> Vec<u8> {
    let mut out = Vec::new();
    if holds.is_empty() {
        return out;
    }
    out.push(STATE_VERSION);
    for (peer, holds) in holds {
        let name = peer.as_str().as_bytes();
        // A name is far shorter than 256 bytes.
        out.push(u8::try_from(name.len()).unwrap_or(u8::MAX));
        out.extend_from_slice(name);
        out.extend_from_slice(&holds.toward.to_be_bytes());
        out.extend_from_slice(&holds.from.to_be_bytes());
    }
    out
}

/// The holds a state keeps, if it is one the stub wrote.
fn decode_holds(state: &[u8]) -> Option<BTreeMap<Name, HoldsWith>> {
    let mut holds = BTreeMap::new();
    let Some((&version, mut rest)) = state.split_first() else {
        return Some(holds);
    };
    if version != STATE_VERSION {
        return None;
    }
    while let Some((&length, after)) = rest.split_first() {
        let (name, after) = after.split_at_checked(usize::from(length))?;
        let name = Name::new(core::str::from_utf8(name).ok()?).ok()?;
        let (toward, after) = after.split_first_chunk::<8>()?;
        let (from, after) = after.split_first_chunk::<8>()?;
        let entry = HoldsWith {
            toward: u64::from_be_bytes(*toward),
            from: u64::from_be_bytes(*from),
        };
        let valid = entry != HoldsWith::default()
            && entry.toward <= MAX_TOTAL
            && entry.from <= MAX_TOTAL
            && holds.last_key_value().is_none_or(|(last, _)| *last < name)
            && holds.len() < MAX_COMMITMENT_PEERS;
        if !valid {
            return None;
        }
        holds.insert(name, entry);
        rest = after;
    }
    (!holds.is_empty()).then_some(holds)
}

impl Commitments for StubHub {
    fn commit(
        &mut self,
        _now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        let limit = self.limit;
        let packets = self.packets(peer).ok_or(SchemeError::Unavailable)?;
        let full = packets.sent.len() >= limit && !packets.sent.contains_key(&request_id);
        if !full {
            packets.sent.insert(request_id, promised);
        }
        self.tidy(peer);
        if full {
            return Err(SchemeError::Unavailable);
        }
        out.clear();
        out.extend_from_slice(&commitment(&promised));
        Ok(())
    }

    fn check_commitment(
        &mut self,
        _now_ms: u64,
        peer: Name,
        request_id: u32,
        promised: Promised,
        bytes: &[u8],
    ) -> Result<(), SchemeError> {
        if bytes != commitment(&promised) {
            return Err(SchemeError::InvalidData);
        }
        let limit = self.limit;
        let packets = self.packets(peer).ok_or(SchemeError::Unavailable)?;
        // A copy of a Prepare we hold is taken as the Prepare was.
        let full = packets.taken.len() >= limit && !packets.taken.contains_key(&request_id);
        if !full {
            packets.taken.insert(request_id, promised);
        }
        self.tidy(peer);
        if full {
            return Err(SchemeError::Unavailable);
        }
        Ok(())
    }

    fn on_fulfilled(&mut self, _now_ms: u64, peer: Name, request_id: u32, _fulfillment: [u8; 32]) {
        let evicted = self.with(peer, |packets| {
            let promised = packets.taken.remove(&request_id)?;
            let evicted = if packets.unclaimed.len() >= MAX_UNCLAIMED {
                packets.unclaimed.pop_front()
            } else {
                None
            };
            packets.unclaimed.push_back((request_id, promised.amount));
            evicted
        });
        // The oldest fulfillment no claim covered is taken as claimed on the
        // hub: from the hold, as any take.
        if let Some(Some((_, amount))) = evicted {
            self.taken_from(peer, amount);
        }
    }

    fn on_released(&mut self, peer: Name, request_id: u32) {
        self.with(peer, |packets| {
            packets.taken.remove(&request_id);
        });
    }

    fn claim(&mut self, _now_ms: u64, peer: Name, request_id: u32, taken: bool, out: &mut Vec<u8>) {
        let promised = self
            .with(peer, |packets| packets.sent.remove(&request_id))
            .flatten();
        // What our books took, the payee takes from our hold: what they did
        // not, the payee claims on the hub, which the stub has not.
        if let Some(promised) = promised.filter(|_| taken) {
            self.taken_toward(peer, promised.amount);
        }
        out.clear();
        out.extend_from_slice(CLAIM);
    }

    fn on_claim(&mut self, _now_ms: u64, peer: Name, request_id: u32, claim: &[u8]) {
        if claim != CLAIM {
            return;
        }
        let claimed = self
            .with(peer, |packets| {
                let at = packets
                    .unclaimed
                    .iter()
                    .position(|(id, _)| *id == request_id)?;
                packets.unclaimed.remove(at)
            })
            .flatten();
        if let Some((_, amount)) = claimed {
            self.taken_from(peer, amount);
        }
    }

    fn on_hub_wait(
        &mut self,
        _now_ms: u64,
        peer: Name,
        request_id: u32,
        condition: [u8; 32],
        _backstop_ms: u64,
    ) {
        let waits = self
            .with(peer, |packets| {
                let matches = packets
                    .sent
                    .get(&request_id)
                    .is_some_and(|promised| promised.condition == condition);
                if matches {
                    packets.sent.remove(&request_id);
                }
                matches
            })
            .unwrap_or(false);
        // One past the bound waits out its backstop, and is rejected then.
        if waits && self.decided.len() < MAX_DECIDED {
            self.decided.push_back(Decided {
                peer,
                request_id,
                fulfillment: None,
            });
        }
    }

    fn on_answered(&mut self, peer: Name, request_id: u32) {
        self.with(peer, |packets| {
            packets.sent.remove(&request_id);
        });
    }

    fn take_decided(&mut self, out: &mut Vec<Decided>) {
        out.extend(self.decided.drain(..));
    }

    fn holds(&self, peer: Name) -> HoldsWith {
        self.holds.get(&peer).copied().unwrap_or_default()
    }

    fn membership(&self) -> Option<Membership> {
        Some(Membership {
            hub: STUB_HUB,
            account: 0,
        })
    }

    fn held(&self, out: &mut Vec<(Name, Held)>) -> bool {
        out.extend(self.peers.iter().map(|(peer, packets)| {
            (
                *peer,
                Held {
                    ours: packets.sent.len(),
                    theirs: packets.taken.len(),
                    unclaimed: packets.unclaimed.len(),
                },
            )
        }));
        true
    }
}

impl SettlementScheme for Stub {
    fn id(&self) -> Name {
        dassie_ledger::name!("stub")
    }

    fn ledger(&self) -> LedgerId {
        LedgerId::StubUsd
    }

    fn supported_versions(&self) -> &'static [u16] {
        &[1]
    }

    fn balance(&self) -> u64 {
        self.balance
    }

    fn state(&self) -> Vec<u8> {
        self.hub
            .as_ref()
            .map_or_else(Vec::new, |hub| encode_holds(&hub.holds))
    }

    fn state_version(&self) -> u64 {
        self.hub.as_ref().map_or(0, |hub| hub.version)
    }

    fn peering_info(&self, out: &mut Vec<u8>) {
        out.clear();
    }

    fn create_peering_request(
        &mut self,
        _peer: Name,
        _peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        out.clear();
        Ok(())
    }

    fn accept_peering_request(
        &mut self,
        _peer: Name,
        _data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError> {
        response.clear();
        Ok(Vec::new())
    }

    fn finalize_peering_request(
        &mut self,
        _peer: Name,
        _peering_info: &[u8],
        _data: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        Ok(Vec::new())
    }

    /// With commitments, a settlement funds our hold toward the peer, so it
    /// is at most what the stub holds, as a hub's member funds a hold with
    /// at most what its account can spend.
    fn prepare_settlement(
        &mut self,
        rng: &mut dyn CryptoRng,
        _asset: Option<&mut dyn ExternalAsset>,
        peer: Name,
        amount: u64,
        _peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        if self.prepared.len() >= MAX_PREPARED {
            return Err(SchemeError::TooManyPrepared);
        }
        let amount = match &self.hub {
            Some(hub) => {
                let spendable = self.balance.saturating_sub(
                    self.prepared
                        .values()
                        .fold(0, |sum, (_, amount)| sum.saturating_add(*amount)),
                );
                let amount = amount.min(spendable);
                if amount == 0 || !hub.has_room_for(peer) {
                    return Err(SchemeError::Unavailable);
                }
                amount
            }
            None => amount,
        };
        let id = random_id(rng);
        self.prepared.insert(id, (peer, amount));
        Ok(PreparedSettlement {
            id,
            amount,
            message: Vec::new(),
            peer_state: None,
        })
    }

    fn execute_settlement(
        &mut self,
        _asset: Option<&mut dyn ExternalAsset>,
        settlement: Name,
        effects: &mut Vec<Effect>,
    ) {
        let Some((peer, amount)) = self.prepared.remove(&settlement) else {
            return;
        };
        match self.balance.checked_sub(amount) {
            Some(balance) => {
                self.balance = balance;
                if let Some(hub) = &mut self.hub {
                    hub.funded_toward(peer, amount);
                }
                effects.push(Effect::Finalize { settlement });
            }
            // The original threw and left the transfer pending (ACC-16).
            None => effects.push(Effect::Cancel { settlement }),
        }
    }

    fn reconcile(
        &mut self,
        _asset: Option<&mut dyn ExternalAsset>,
        pending: &[(Name, u64)],
        effects: &mut Vec<Effect>,
    ) {
        // The stub executes right after the pending transfer is on disk,
        // and its balance is checkpointed with the result. A settlement
        // pending at a restart was never executed: its money never moved.
        for (settlement, _) in pending {
            if !self.prepared.contains_key(settlement) {
                effects.push(Effect::Cancel {
                    settlement: *settlement,
                });
            }
        }
    }

    fn handle_settlement(
        &mut self,
        peer: Name,
        amount: u64,
        _data: &[u8],
        _peer_state: &[u8],
        effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        self.balance = self
            .balance
            .checked_add(amount)
            .ok_or(SchemeError::Overflow)?;
        if let Some(hub) = &mut self.hub {
            hub.funded_from(peer, amount);
        }
        effects.push(Effect::IncomingSettlement {
            peer,
            amount,
            id: None,
        });
        Ok(())
    }

    fn handle_message(&mut self, _peer: Name, _message: &[u8], _effects: &mut Vec<Effect>) {}

    fn deposit(&mut self, amount: u64, effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        if amount == 0 {
            return Err(SchemeError::InvalidData);
        }
        self.balance = self
            .balance
            .checked_add(amount)
            .ok_or(SchemeError::Overflow)?;
        effects.push(Effect::Deposit { amount });
        Ok(())
    }

    fn discard_settlement(&mut self, _asset: Option<&mut dyn ExternalAsset>, settlement: Name) {
        self.prepared.remove(&settlement);
    }

    fn commitments(&mut self) -> Option<&mut dyn Commitments> {
        self.hub.as_mut().map(|hub| hub as &mut dyn Commitments)
    }

    fn check_invariants(&self) {
        assert!(self.prepared.len() <= MAX_PREPARED);
        if let Some(hub) = &self.hub {
            hub.check_invariants();
        }
    }
}

/// 16 random bytes in lowercase hex, as the original's settlement IDs.
fn random_id(rng: &mut dyn CryptoRng) -> Name {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    Name::hex(&bytes, false)
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use dassie_node_core::testing::SeededRng;

    use super::*;

    fn peer() -> Name {
        Name::new("dpeer").unwrap()
    }

    #[test]
    fn starts_from_the_checkpoint() {
        assert_eq!(
            Stub::new("", None, &[]).unwrap().balance(),
            STUB_INITIAL_BALANCE
        );
        assert_eq!(Stub::new("{}", Some(7), &[]).unwrap().balance(), 7);
        assert_eq!(
            Stub::new("{\"x\":1}", None, &[]).unwrap_err(),
            SchemeError::InvalidConfig
        );
    }

    #[test]
    fn deposits_and_incoming_settlements_grow_the_balance() {
        let mut stub = Stub::new("", Some(10), &[]).unwrap();
        let mut effects = Vec::new();
        stub.deposit(5, &mut effects).unwrap();
        stub.handle_settlement(peer(), 3, &[], &[], &mut effects)
            .unwrap();
        assert_eq!(stub.balance(), 18);
        assert_eq!(
            effects,
            [
                Effect::Deposit { amount: 5 },
                Effect::IncomingSettlement {
                    peer: peer(),
                    amount: 3,
                    id: None,
                }
            ]
        );
        assert_eq!(stub.deposit(0, &mut effects), Err(SchemeError::InvalidData));
        assert_eq!(
            stub.deposit(u64::MAX, &mut effects),
            Err(SchemeError::Overflow)
        );
    }

    // ACC-16: a failed execution left the pending transfer for ever.
    #[test]
    fn a_settlement_beyond_the_balance_is_cancelled() {
        let mut stub = Stub::new("", Some(10), &[]).unwrap();
        let mut rng = SeededRng::new(1);
        let small = stub
            .prepare_settlement(&mut rng, None, peer(), 4, &[])
            .unwrap();
        let large = stub
            .prepare_settlement(&mut rng, None, peer(), 40, &[])
            .unwrap();
        assert_ne!(small.id, large.id);
        assert_eq!(small.id.as_str().len(), 32);
        let mut effects = Vec::new();
        stub.execute_settlement(None, small.id, &mut effects);
        stub.execute_settlement(None, large.id, &mut effects);
        // Executing twice does nothing.
        stub.execute_settlement(None, small.id, &mut effects);
        assert_eq!(
            effects,
            [
                Effect::Finalize {
                    settlement: small.id
                },
                Effect::Cancel {
                    settlement: large.id
                }
            ]
        );
        assert_eq!(stub.balance(), 6);
    }

    // ACC-16: a settlement pending across a restart stayed pending.
    #[test]
    fn settlements_pending_at_a_restart_are_cancelled() {
        let mut stub = Stub::new("", Some(10), &[]).unwrap();
        let mut rng = SeededRng::new(3);
        let known = stub
            .prepare_settlement(&mut rng, None, peer(), 4, &[])
            .unwrap();
        let lost = Name::new("0123456789abcdef0123456789abcdef").unwrap();
        let mut effects = Vec::new();
        stub.reconcile(None, &[(known.id, 4), (lost, 7)], &mut effects);
        assert_eq!(effects, [Effect::Cancel { settlement: lost }]);
        assert_eq!(stub.balance(), 10);
    }

    #[test]
    fn preparations_are_bounded() {
        let mut stub = Stub::new("", None, &[]).unwrap();
        let mut rng = SeededRng::new(2);
        for _ in 0..MAX_PREPARED {
            stub.prepare_settlement(&mut rng, None, peer(), 1, &[])
                .unwrap();
        }
        assert_eq!(
            stub.prepare_settlement(&mut rng, None, peer(), 1, &[]),
            Err(SchemeError::TooManyPrepared)
        );
        stub.check_invariants();
    }

    // Found by the invariants (spec/decisions/tigerstyle.md): a settlement
    // the node could not book stayed prepared, so after 64 of them the stub
    // settled nothing more.
    #[test]
    fn a_discarded_settlement_frees_its_slot() {
        let mut stub = Stub::new("", None, &[]).unwrap();
        let mut rng = SeededRng::new(2);
        for _ in 0..MAX_PREPARED * 2 {
            let prepared = stub
                .prepare_settlement(&mut rng, None, peer(), 1, &[])
                .unwrap();
            stub.discard_settlement(None, prepared.id);
            stub.check_invariants();
        }
        assert!(stub.prepared.is_empty());
    }

    #[test]
    fn the_stub_takes_no_config_but_its_commitments() {
        assert!(Stub::new("", None, &[]).unwrap().commitments().is_none());
        assert!(Stub::new("{}", None, &[]).unwrap().commitments().is_none());
        assert!(
            Stub::new(r#" { "commitments" : 3 } "#, None, &[])
                .unwrap()
                .commitments()
                .is_some()
        );
        assert!(
            Stub::new(r#"{"commitments":64}"#, None, &[])
                .unwrap()
                .commitments()
                .is_some()
        );
        for refused in [
            "{\"x\":1}",
            r#"{"commitments":0}"#,
            r#"{"commitments":65}"#,
            r#"{"commitments":-1}"#,
            r#"{"commitments":"1"}"#,
            r#"{"commitments":}"#,
            r#"{"commitments":1,"x":2}"#,
            r#"{"commitments":99999999999999999999999}"#,
            "commitments",
        ] {
            assert_eq!(
                Stub::new(refused, None, &[]).err(),
                Some(SchemeError::InvalidConfig),
                "{refused}"
            );
        }
    }

    fn promised(amount: u64, condition: u8) -> Promised {
        Promised {
            amount,
            condition: [condition; 32],
            expires_at_ms: 1_000,
        }
    }

    fn held(stub: &mut Stub) -> Vec<(Name, Held)> {
        let mut out = Vec::new();
        assert!(stub.commitments().unwrap().held(&mut out));
        stub.check_invariants();
        out
    }

    fn one(ours: usize, theirs: usize, unclaimed: usize) -> Vec<(Name, Held)> {
        vec![(
            peer(),
            Held {
                ours,
                theirs,
                unclaimed,
            },
        )]
    }

    // As the payer: at most its limit of our packets to a peer are
    // committed to at once; a claim, an answer or the hub's decision
    // resolves one, and frees its place.
    #[test]
    fn a_payer_commits_to_at_most_its_limit() {
        let mut stub = Stub::new(r#"{"commitments":2}"#, None, &[]).unwrap();
        let mut out = Vec::new();
        let c = stub.commitments().unwrap();
        c.commit(0, peer(), 1, promised(5, 1), &mut out).unwrap();
        assert_eq!(out.len(), COMMITMENT_BYTES);
        assert_eq!(out[..8], 5u64.to_be_bytes());
        c.commit(0, peer(), 2, promised(5, 2), &mut out).unwrap();
        assert_eq!(
            c.commit(0, peer(), 3, promised(5, 3), &mut out),
            Err(SchemeError::Unavailable)
        );
        // Another peer has places of its own.
        let other = Name::new("dother").unwrap();
        c.commit(0, other, 3, promised(5, 3), &mut out).unwrap();
        c.on_answered(other, 3);
        assert_eq!(held(&mut stub), one(2, 0, 0));
        let c = stub.commitments().unwrap();
        c.claim(0, peer(), 1, true, &mut out);
        assert_eq!(out, CLAIM);
        c.commit(0, peer(), 3, promised(5, 3), &mut out).unwrap();
        c.on_answered(peer(), 2);
        // A wait for another packet's condition is not this one's.
        c.on_hub_wait(0, peer(), 3, [9; 32], 2_000);
        let mut decided = Vec::new();
        c.take_decided(&mut decided);
        assert!(decided.is_empty());
        c.on_hub_wait(0, peer(), 3, [3; 32], 2_000);
        c.take_decided(&mut decided);
        assert_eq!(
            decided,
            [Decided {
                peer: peer(),
                request_id: 3,
                fulfillment: None,
            }]
        );
        assert_eq!(held(&mut stub), []);
    }

    // As the payee: a commitment is taken only as the stub encodes the
    // Prepare it comes with, and at most the limit of a peer's packets at
    // once, a copy of one it holds taken again. A fulfilled packet is kept
    // until the payer's claim covers it.
    #[test]
    fn a_payee_takes_commitments_within_its_limit() {
        let mut stub = Stub::new(r#"{"commitments":1}"#, None, &[]).unwrap();
        let c = stub.commitments().unwrap();
        let good = commitment(&promised(5, 1));
        assert_eq!(
            c.check_commitment(0, peer(), 1, promised(6, 1), &good),
            Err(SchemeError::InvalidData)
        );
        assert_eq!(
            c.check_commitment(0, peer(), 1, promised(5, 2), &good),
            Err(SchemeError::InvalidData)
        );
        assert_eq!(
            c.check_commitment(0, peer(), 1, promised(5, 1), &good[1..]),
            Err(SchemeError::InvalidData)
        );
        c.check_commitment(0, peer(), 1, promised(5, 1), &good)
            .unwrap();
        c.check_commitment(0, peer(), 1, promised(5, 1), &good)
            .unwrap();
        let next = commitment(&promised(5, 2));
        assert_eq!(
            c.check_commitment(0, peer(), 2, promised(5, 2), &next),
            Err(SchemeError::Unavailable)
        );
        assert_eq!(held(&mut stub), one(0, 1, 0));
        let c = stub.commitments().unwrap();
        c.on_fulfilled(0, peer(), 1, [0; 32]);
        assert_eq!(held(&mut stub), one(0, 0, 1));
        let c = stub.commitments().unwrap();
        c.check_commitment(0, peer(), 2, promised(5, 2), &next)
            .unwrap();
        c.on_released(peer(), 2);
        // A claim that is not the stub's covers nothing.
        c.on_claim(0, peer(), 1, b"not a claim");
        assert_eq!(held(&mut stub), one(0, 0, 1));
        let c = stub.commitments().unwrap();
        c.on_claim(0, peer(), 1, CLAIM);
        assert_eq!(held(&mut stub), []);
    }

    // Fulfillments that no claim covers are bounded: past the bound, the
    // oldest is taken as claimed on the hub.
    #[test]
    fn unclaimed_fulfillments_are_bounded() {
        let mut stub = Stub::new(r#"{"commitments":1}"#, None, &[]).unwrap();
        let c = stub.commitments().unwrap();
        for id in 0..u32::try_from(MAX_UNCLAIMED).unwrap() + 3 {
            let promise = promised(1, 1);
            c.check_commitment(0, peer(), id, promise, &commitment(&promise))
                .unwrap();
            c.on_fulfilled(0, peer(), id, [0; 32]);
        }
        assert_eq!(held(&mut stub), one(0, 0, MAX_UNCLAIMED));
    }

    // A stub without commitments holds nothing per packet, and is not over
    // a hub.
    #[test]
    fn a_plain_stub_has_no_commitments() {
        let mut stub = Stub::new("", None, &[]).unwrap();
        assert!(stub.commitments().is_none());
        stub.check_invariants();
    }

    fn holds(stub: &mut Stub) -> HoldsWith {
        stub.check_invariants();
        stub.commitments().unwrap().holds(peer())
    }

    /// Settles `amount` to `peer()`, as the node does: prepared, then
    /// executed.
    fn settle(stub: &mut Stub, amount: u64) -> Vec<Effect> {
        let mut rng = SeededRng::new(4);
        let prepared = stub
            .prepare_settlement(&mut rng, None, peer(), amount, &[])
            .unwrap();
        let mut effects = Vec::new();
        stub.execute_settlement(None, prepared.id, &mut effects);
        effects
    }

    // A stub with commitments emulates the holds: our settlement funds our
    // hold toward the peer once it executes, the peer's funds its hold
    // toward us; a fulfilled packet is taken from the payer's hold when it
    // writes its claim for a Fulfill its books took, and from the payee's
    // view of it when the claim arrives.
    #[test]
    fn holds_grow_by_settlements_and_shrink_by_what_is_taken() {
        let mut payer = Stub::new(r#"{"commitments":2}"#, None, &[]).unwrap();
        let mut payee = Stub::new(r#"{"commitments":2}"#, None, &[]).unwrap();
        let version = payer.state_version();
        settle(&mut payer, 10);
        assert_ne!(payer.state_version(), version);
        payee
            .handle_settlement(peer(), 10, &[], &[], &mut Vec::new())
            .unwrap();
        assert_eq!(
            holds(&mut payer),
            HoldsWith {
                toward: 10,
                from: 0
            }
        );
        assert_eq!(
            holds(&mut payee),
            HoldsWith {
                toward: 0,
                from: 10
            }
        );
        let mut out = Vec::new();
        for (id, taken) in [(1, true), (2, false)] {
            let promise = promised(3, 1);
            payer
                .commitments()
                .unwrap()
                .commit(0, peer(), id, promise, &mut out)
                .unwrap();
            let payee = payee.commitments().unwrap();
            payee
                .check_commitment(0, peer(), id, promise, &out)
                .unwrap();
            payee.on_fulfilled(0, peer(), id, [0; 32]);
            payer
                .commitments()
                .unwrap()
                .claim(0, peer(), id, taken, &mut out);
            payee.on_claim(0, peer(), id, &out);
        }
        // The payer's books did not take the second Fulfill: only the hub
        // would take it from its hold, and the stub has none.
        assert_eq!(holds(&mut payer), HoldsWith { toward: 7, from: 0 });
        assert_eq!(holds(&mut payee), HoldsWith { toward: 0, from: 4 });
        // A claim for a packet already taken, or one we never took, takes
        // nothing more.
        payer
            .commitments()
            .unwrap()
            .claim(0, peer(), 1, true, &mut out);
        payee.commitments().unwrap().on_claim(0, peer(), 1, &out);
        assert_eq!(holds(&mut payer).toward, 7);
        assert_eq!(holds(&mut payee).from, 4);
    }

    // What the payee took as claimed on the hub, once too many claims were
    // missing, it took from the hold too.
    #[test]
    fn a_fulfillment_taken_as_claimed_on_the_hub_is_taken_from_the_hold() {
        let mut stub = Stub::new(r#"{"commitments":1}"#, None, &[]).unwrap();
        let total = u64::try_from(MAX_UNCLAIMED).unwrap() + 3;
        stub.handle_settlement(peer(), total * 2, &[], &[], &mut Vec::new())
            .unwrap();
        let c = stub.commitments().unwrap();
        for id in 0..u32::try_from(total).unwrap() {
            let promise = promised(2, 1);
            c.check_commitment(0, peer(), id, promise, &commitment(&promise))
                .unwrap();
            c.on_fulfilled(0, peer(), id, [0; 32]);
        }
        assert_eq!(holds(&mut stub).from, total * 2 - 3 * 2);
    }

    // The holds outlast a restart in the stub's state, and a state the stub
    // did not write is refused, as is one for a stub without commitments.
    #[test]
    fn holds_outlast_a_restart() {
        let config = r#"{"commitments":1}"#;
        let mut stub = Stub::new(config, None, &[]).unwrap();
        assert!(stub.state().is_empty());
        settle(&mut stub, 10);
        stub.handle_settlement(peer(), 4, &[], &[], &mut Vec::new())
            .unwrap();
        let state = stub.state();
        let mut again = Stub::new(config, Some(stub.balance()), &state).unwrap();
        assert_eq!(
            holds(&mut again),
            HoldsWith {
                toward: 10,
                from: 4
            }
        );
        assert_eq!(again.state(), state);
        assert_eq!(
            Stub::new("", None, &state).err(),
            Some(SchemeError::InvalidConfig)
        );
        for broken in [&state[1..], &state[..state.len() - 1], &[1][..], &[2][..]] {
            assert_eq!(
                Stub::new(config, None, broken).err(),
                Some(SchemeError::InvalidData),
                "{broken:?}"
            );
        }
    }

    // With commitments, a settlement funds a hold with at most what the
    // stub can spend, what earlier settlements prepared set aside; with
    // nothing to spend, it funds none.
    #[test]
    fn a_hold_is_funded_with_at_most_what_the_stub_can_spend() {
        let mut stub = Stub::new(r#"{"commitments":1}"#, Some(10), &[]).unwrap();
        let mut rng = SeededRng::new(5);
        let first = stub
            .prepare_settlement(&mut rng, None, peer(), 7, &[])
            .unwrap();
        assert_eq!(first.amount, 7);
        let second = stub
            .prepare_settlement(&mut rng, None, peer(), 7, &[])
            .unwrap();
        assert_eq!(second.amount, 3);
        assert_eq!(
            stub.prepare_settlement(&mut rng, None, peer(), 7, &[]),
            Err(SchemeError::Unavailable)
        );
        let mut effects = Vec::new();
        stub.execute_settlement(None, first.id, &mut effects);
        stub.execute_settlement(None, second.id, &mut effects);
        assert_eq!(holds(&mut stub).toward, 10);
        assert_eq!(stub.balance(), 0);
        // A plain stub prepares what it is asked, and cancels what it cannot
        // pay when it executes, as before.
        let mut plain = Stub::new("", Some(1), &[]).unwrap();
        assert_eq!(
            plain
                .prepare_settlement(&mut rng, None, peer(), 7, &[])
                .unwrap()
                .amount,
            7
        );
    }

    // Every stub with commitments is a member of the one pretend hub.
    #[test]
    fn a_stub_with_commitments_is_a_member_of_its_hub() {
        let mut stub = Stub::new(r#"{"commitments":1}"#, None, &[]).unwrap();
        let membership = stub.commitments().unwrap().membership().unwrap();
        assert_eq!(membership.hub, STUB_HUB);
    }
}

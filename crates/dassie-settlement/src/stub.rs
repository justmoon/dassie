//! The stub scheme: test money without an underlying ledger
//! (spec/areas/settlement.md#the-stub-scheme).

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use dassie_ledger::{LedgerId, Name};
use rand_core::CryptoRng;

use crate::asset::ExternalAsset;
use crate::commitments::{Commitments, Decided, Held, Promised};
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

/// The most peers a stub with commitments holds packets for.
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
/// a hub's member keeps of them, in memory only, which a restart forgets
/// along with the connector's packets
/// (spec/areas/settlement.md#the-stub-scheme).
#[derive(Debug)]
pub struct Stub {
    balance: u64,
    prepared: BTreeMap<Name, (Name, u64)>,
    hub: Option<StubHub>,
}

impl Stub {
    /// The stub, with the balance last checkpointed, or the initial balance
    /// on first activation. Its config is empty or `{}`, or
    /// `{"commitments": N}` for a stub with commitments that holds at most
    /// `N` packets in flight per peer each way, `N` from 1 to
    /// [`MAX_STUB_COMMITMENTS`].
    ///
    /// # Errors
    ///
    /// If `config` is anything else.
    pub fn new(config: &str, checkpoint: Option<u64>) -> Result<Self, SchemeError> {
        let limit = commitments_config(config).ok_or(SchemeError::InvalidConfig)?;
        Ok(Self {
            balance: checkpoint.unwrap_or(STUB_INITIAL_BALANCE),
            prepared: BTreeMap::new(),
            hub: limit.map(StubHub::new),
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
    /// The peer's Prepares we fulfilled that no claim covers yet, oldest
    /// first.
    unclaimed: VecDeque<u32>,
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
#[derive(Debug)]
struct StubHub {
    /// The most packets in flight per peer each way.
    limit: usize,
    peers: BTreeMap<Name, Packets>,
    decided: VecDeque<Decided>,
}

impl StubHub {
    fn new(limit: usize) -> Self {
        assert!((1..=MAX_STUB_COMMITMENTS).contains(&limit));
        Self {
            limit,
            peers: BTreeMap::new(),
            decided: VecDeque::new(),
        }
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
    fn with(&mut self, peer: Name, f: impl FnOnce(&mut Packets)) {
        if let Some(packets) = self.peers.get_mut(&peer) {
            f(packets);
            self.tidy(peer);
        }
    }

    fn check_invariants(&self) {
        assert!((1..=MAX_STUB_COMMITMENTS).contains(&self.limit));
        assert!(self.peers.len() <= MAX_COMMITMENT_PEERS);
        assert!(self.decided.len() <= MAX_DECIDED);
        for packets in self.peers.values() {
            assert!(!packets.is_empty(), "a peer held with nothing of it");
            assert!(packets.sent.len() <= self.limit);
            assert!(packets.taken.len() <= self.limit);
            assert!(packets.unclaimed.len() <= MAX_UNCLAIMED);
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
        self.with(peer, |packets| {
            if packets.taken.remove(&request_id).is_none() {
                return;
            }
            if packets.unclaimed.len() >= MAX_UNCLAIMED {
                packets.unclaimed.pop_front();
            }
            packets.unclaimed.push_back(request_id);
        });
    }

    fn on_released(&mut self, peer: Name, request_id: u32) {
        self.with(peer, |packets| {
            packets.taken.remove(&request_id);
        });
    }

    fn claim(
        &mut self,
        _now_ms: u64,
        peer: Name,
        request_id: u32,
        _taken: bool,
        out: &mut Vec<u8>,
    ) {
        self.with(peer, |packets| {
            packets.sent.remove(&request_id);
        });
        out.clear();
        out.extend_from_slice(CLAIM);
    }

    fn on_claim(&mut self, _now_ms: u64, peer: Name, request_id: u32, claim: &[u8]) {
        if claim != CLAIM {
            return;
        }
        self.with(peer, |packets| {
            packets
                .unclaimed
                .retain(|unclaimed| *unclaimed != request_id);
        });
    }

    fn on_hub_wait(
        &mut self,
        _now_ms: u64,
        peer: Name,
        request_id: u32,
        condition: [u8; 32],
        _backstop_ms: u64,
    ) {
        let mut waits = false;
        self.with(peer, |packets| {
            if packets
                .sent
                .get(&request_id)
                .is_some_and(|promised| promised.condition == condition)
            {
                packets.sent.remove(&request_id);
                waits = true;
            }
        });
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
        let Some((_, amount)) = self.prepared.remove(&settlement) else {
            return;
        };
        match self.balance.checked_sub(amount) {
            Some(balance) => {
                self.balance = balance;
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
        assert_eq!(Stub::new("", None).unwrap().balance(), STUB_INITIAL_BALANCE);
        assert_eq!(Stub::new("{}", Some(7)).unwrap().balance(), 7);
        assert_eq!(
            Stub::new("{\"x\":1}", None).unwrap_err(),
            SchemeError::InvalidConfig
        );
    }

    #[test]
    fn deposits_and_incoming_settlements_grow_the_balance() {
        let mut stub = Stub::new("", Some(10)).unwrap();
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
        let mut stub = Stub::new("", Some(10)).unwrap();
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
        let mut stub = Stub::new("", Some(10)).unwrap();
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
        let mut stub = Stub::new("", None).unwrap();
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
        let mut stub = Stub::new("", None).unwrap();
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
        assert!(Stub::new("", None).unwrap().commitments().is_none());
        assert!(Stub::new("{}", None).unwrap().commitments().is_none());
        assert!(
            Stub::new(r#" { "commitments" : 3 } "#, None)
                .unwrap()
                .commitments()
                .is_some()
        );
        assert!(
            Stub::new(r#"{"commitments":64}"#, None)
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
                Stub::new(refused, None).err(),
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
        let mut stub = Stub::new(r#"{"commitments":2}"#, None).unwrap();
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
        let mut stub = Stub::new(r#"{"commitments":1}"#, None).unwrap();
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
        let mut stub = Stub::new(r#"{"commitments":1}"#, None).unwrap();
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
        let mut stub = Stub::new("", None).unwrap();
        assert!(stub.commitments().is_none());
        stub.check_invariants();
    }
}

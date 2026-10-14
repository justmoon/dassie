//! The stub scheme: test money without an underlying ledger (spec 5 §6.1).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use dassie_ledger::{LedgerId, Name};
use rand_core::CryptoRng;

use crate::scheme::{Effect, PreparedSettlement, SchemeError, SettlementScheme};

/// The balance the stub starts with on its first activation: 100 USD at
/// scale 9, as in the original. Later activations start from the
/// checkpoint instead (ACC-6).
pub const STUB_INITIAL_BALANCE: u64 = 100_000_000_000;

/// Most settlements prepared and not executed.
const MAX_PREPARED: usize = 64;

/// The stub scheme. Its "underlying ledger" is a number, which the node's
/// checkpoint keeps across restarts.
///
/// **Test realm only:** a peer's settlement claim is believed as stated
/// (ACC-23). Anyone peered over the stub can mint credit, which is why it
/// only ever holds test money.
#[derive(Debug)]
pub struct Stub {
    balance: u64,
    prepared: BTreeMap<Name, (Name, u64)>,
}

impl Stub {
    /// The stub, with the balance last checkpointed, or the initial balance
    /// on first activation. It takes no config: empty or `{}`.
    ///
    /// # Errors
    ///
    /// If `config` is anything else.
    pub fn new(config: &str, checkpoint: Option<u64>) -> Result<Self, SchemeError> {
        if !matches!(config.trim(), "" | "{}") {
            return Err(SchemeError::InvalidConfig);
        }
        Ok(Self {
            balance: checkpoint.unwrap_or(STUB_INITIAL_BALANCE),
            prepared: BTreeMap::new(),
        })
    }
}

impl SettlementScheme for Stub {
    fn id(&self) -> Name {
        // A constant that is a valid name.
        Name::new("stub").unwrap_or_else(|_| unreachable!())
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
            message: Vec::new(),
            peer_state: None,
        })
    }

    fn execute_settlement(&mut self, settlement: Name, effects: &mut Vec<Effect>) {
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

    fn reconcile(&mut self, pending: &[(Name, u64)], effects: &mut Vec<Effect>) {
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
        effects.push(Effect::IncomingSettlement { peer, amount });
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
}

/// 16 random bytes in lowercase hex, as the original's settlement IDs.
fn random_id(rng: &mut dyn CryptoRng) -> Name {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes);
    let mut text = [0u8; 32];
    let digits = b"0123456789abcdef";
    for ([high, low], byte) in text.as_chunks_mut::<2>().0.iter_mut().zip(bytes) {
        *high = digits.get(usize::from(byte >> 4)).copied().unwrap_or(b'0');
        *low = digits.get(usize::from(byte & 15)).copied().unwrap_or(b'0');
    }
    let text = core::str::from_utf8(&text).unwrap_or("0");
    Name::new(text).unwrap_or_else(|_| unreachable!())
}

#[cfg(test)]
mod tests {
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
                    amount: 3
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
        let small = stub.prepare_settlement(&mut rng, peer(), 4, &[]).unwrap();
        let large = stub.prepare_settlement(&mut rng, peer(), 40, &[]).unwrap();
        assert_ne!(small.id, large.id);
        assert_eq!(small.id.as_str().len(), 32);
        let mut effects = Vec::new();
        stub.execute_settlement(small.id, &mut effects);
        stub.execute_settlement(large.id, &mut effects);
        // Executing twice does nothing.
        stub.execute_settlement(small.id, &mut effects);
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
        let known = stub.prepare_settlement(&mut rng, peer(), 4, &[]).unwrap();
        let lost = Name::new("0123456789abcdef0123456789abcdef").unwrap();
        let mut effects = Vec::new();
        stub.reconcile(&[(known.id, 4), (lost, 7)], &mut effects);
        assert_eq!(effects, [Effect::Cancel { settlement: lost }]);
        assert_eq!(stub.balance(), 10);
    }

    #[test]
    fn preparations_are_bounded() {
        let mut stub = Stub::new("", None).unwrap();
        let mut rng = SeededRng::new(2);
        for _ in 0..MAX_PREPARED {
            stub.prepare_settlement(&mut rng, peer(), 1, &[]).unwrap();
        }
        assert_eq!(
            stub.prepare_settlement(&mut rng, peer(), 1, &[]),
            Err(SchemeError::TooManyPrepared)
        );
    }

    #[test]
    fn only_the_stub_exists_so_far() {
        assert!(crate::create("stub", "", None).is_ok());
        assert!(matches!(
            crate::create("xrpl-testnet", "", None),
            Err(SchemeError::UnknownScheme)
        ));
    }
}

//! Settling with peers (spec 5 §4.6, spec 7 §4.7): when what we owe a peer
//! passes the threshold, a settlement is prepared, booked as a durable
//! pending transfer, executed once that is on disk, and announced to the
//! peer once it executed. Incoming settlements are booked before the
//! scheme sees them.

use std::collections::BTreeMap;

use dassie_identity::NodeId;
use dassie_ledger::{
    AccountName, AccountPath, LedgerId, Name, Reference, TransferId, TransferSpec,
};
use dassie_peer::wire::SchemeId;
use dassie_settlement::policy::{self, Terms};

use super::{Core, EngineError};
use crate::log::target;
use crate::node::Policies;

/// The first wait after a cancelled settlement; it doubles with each one
/// in a row, up to an hour, so a node without funds does not journal a
/// pair of rows every few seconds.
const CANCEL_BACKOFF_MIN_MS: u64 = 1_000;
const CANCEL_BACKOFF_MAX_MS: u64 = 3_600_000;

/// The settling side of the core.
pub(super) struct Settlements {
    /// When what is owed to peers is checked next.
    pub at_ms: u64,
    pub interval_ms: u64,
    pub ratio_ppm: u32,
    /// Our settlements prepared and not yet final, by scheme and ID.
    pub settling: BTreeMap<(Name, Name), Settling>,
    /// Settlements to execute once their pending transfers are on disk.
    pub to_execute: Vec<(Name, Name)>,
    /// Peers not settled with before a time, after cancelled settlements,
    /// and how many in a row.
    pub backoff: BTreeMap<NodeId, (u64, u32)>,
    /// Ended peerings still to be settled with: their scheme and state.
    pub finals: BTreeMap<NodeId, (SchemeId, Vec<u8>)>,
}

impl Settlements {
    pub(super) const fn new(now_ms: u64, policies: &Policies) -> Self {
        Self {
            at_ms: now_ms.saturating_add(policies.settlement_interval_ms),
            interval_ms: policies.settlement_interval_ms,
            ratio_ppm: policies.settlement_ratio_ppm,
            settling: BTreeMap::new(),
            to_execute: Vec::new(),
            backoff: BTreeMap::new(),
            finals: BTreeMap::new(),
        }
    }
}

/// One of our settlements, prepared and not yet final.
#[derive(Clone, Debug)]
pub(super) struct Settling {
    /// The peer it pays.
    pub peer: NodeId,
    /// Its pending transfer.
    pub transfer: TransferId,
    /// How much.
    pub amount: u64,
    /// What the scheme gave to send the peer.
    pub message: Vec<u8>,
}

/// A peer's position, as the settlement policy needs it.
struct Position {
    scheme: Name,
    ledger: LedgerId,
    position: u128,
    credit_out: u64,
}

impl Core {
    /// Checks every peer, and every ended peering still owed its final
    /// settlement, and prepares what is due.
    pub(super) fn settlement_timers(&mut self) {
        let now = self.now_ms;
        if self.settle.at_ms > now {
            return;
        }
        self.settle.at_ms = now.saturating_add(self.settle.interval_ms);
        let peers: Vec<(NodeId, SchemeId, u64, Vec<u8>)> = self
            .peer
            .active_peers()
            .map(|(id, peering)| {
                (
                    *id,
                    peering.scheme,
                    peering.credit_in,
                    peering.state.clone(),
                )
            })
            .collect();
        for (node, scheme, credit_in, state) in peers {
            if !self.can_settle(&node) {
                continue;
            }
            let Some(at) = self.position(node, scheme) else {
                continue;
            };
            let terms = Terms {
                credit_in,
                credit_out: at.credit_out,
                ratio_ppm: self.settle.ratio_ppm,
            };
            let amount = policy::amount(at.position, terms);
            if amount > 0 {
                self.prepare_settlement(node, &at, amount, &state);
            }
        }
        let finals: Vec<NodeId> = self.settle.finals.keys().copied().collect();
        for node in finals {
            if !self.can_settle(&node) {
                continue;
            }
            let Some((scheme, state)) = self.settle.finals.remove(&node) else {
                continue;
            };
            let Some(at) = self.position(node, scheme) else {
                continue;
            };
            let amount = policy::final_amount(at.position, at.credit_out);
            if amount > 0 {
                tracing::info!(target: target::SETTLEMENT, %node, amount, "settling what is owed at the end of a peering");
                self.prepare_settlement(node, &at, amount, &state);
            }
        }
    }

    /// Whether nothing is on its way to `node`, and no cancelled
    /// settlement holds it off (ACC-17).
    fn can_settle(&self, node: &NodeId) -> bool {
        !self.peer.settling(node)
            && !self
                .settle
                .settling
                .values()
                .any(|settling| settling.peer == *node)
            && self
                .settle
                .backoff
                .get(node)
                .is_none_or(|(until, _)| *until <= self.now_ms)
    }

    /// Where we stand with `node` on `scheme`'s ledger.
    fn position(&self, node: NodeId, scheme: SchemeId) -> Option<Position> {
        let scheme = Name::new(scheme.as_str()).ok()?;
        let ledger = self.schemes.get(&scheme)?.scheme.ledger();
        let name = Name::new(node.as_str()).ok()?;
        let interledger = self.ledger.find(&AccountPath::new(
            ledger,
            AccountName::AssetsInterledger(name),
        ))?;
        let trust = self
            .ledger
            .find(&AccountPath::new(ledger, AccountName::ContraTrust(name)))?;
        Some(Position {
            scheme,
            ledger,
            position: policy::position(self.ledger.account(interledger)?.balance()),
            credit_out: policy::trust(self.ledger.account(trust)?.balance()),
        })
    }

    /// What we owe `node` net on `scheme`'s ledger: negative when it owes
    /// us.
    pub(super) fn owed(&self, node: NodeId, scheme: SchemeId) -> Option<i128> {
        let at = self.position(node, scheme)?;
        let position = i128::try_from(at.position).ok()?;
        Some(position - i128::from(at.credit_out))
    }

    /// Prepares a settlement and books its pending transfer; it executes
    /// once that is on disk (ACC-16).
    fn prepare_settlement(&mut self, node: NodeId, at: &Position, amount: u64, state: &[u8]) {
        let Ok(name) = Name::new(node.as_str()) else {
            return;
        };
        let (Ok(interledger), Ok(settlement)) = (
            self.account(at.ledger, AccountName::AssetsInterledger(name)),
            self.account(at.ledger, AccountName::AssetsSettlement),
        ) else {
            return;
        };
        let mut spec = TransferSpec {
            debit: interledger,
            credit: settlement,
            amount,
            pending: true,
            reference: Reference::Settlement {
                scheme: at.scheme,
                id: at.scheme,
            },
        };
        // Checked before the scheme is asked, so a refusal leaves nothing
        // prepared behind.
        if let Err(error) = self.ledger.validate(&spec) {
            tracing::warn!(target: target::SETTLEMENT, %node, %error, "the books cannot take a settlement now");
            return;
        }
        let Self { schemes, rng, .. } = self;
        let Some(active) = schemes.get_mut(&at.scheme) else {
            return;
        };
        let prepared = match active
            .scheme
            .prepare_settlement(rng.as_mut(), name, amount, state)
        {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(target: target::SETTLEMENT, %node, %error, "the scheme could not prepare a settlement");
                return;
            }
        };
        spec.reference = Reference::Settlement {
            scheme: at.scheme,
            id: prepared.id,
        };
        let transfer = match self.ledger.create(self.now_ms, &spec) {
            Ok(transfer) => transfer,
            Err(error) => {
                tracing::error!(target: target::SETTLEMENT, %node, %error, "a settlement's transfer was refused after it checked out");
                return;
            }
        };
        if let Some(state) = prepared.peer_state {
            self.peer.set_scheme_state(&node, state);
        }
        tracing::info!(target: target::SETTLEMENT, %node, amount, id = %prepared.id, "settling");
        self.settle.settling.insert(
            (at.scheme, prepared.id),
            Settling {
                peer: node,
                transfer,
                amount,
                message: prepared.message,
            },
        );
        self.settle.to_execute.push((at.scheme, prepared.id));
        self.take_journal();
    }

    /// Executes the settlements whose pending transfers are now on disk.
    /// Returns whether there were any, whose outcome is to be persisted.
    pub(super) fn execute_settlements(&mut self) -> bool {
        if self.settle.to_execute.is_empty() {
            return false;
        }
        for (scheme, id) in std::mem::take(&mut self.settle.to_execute) {
            // A scheme removed since stays pending: its next activation
            // reconciles it.
            let Some(active) = self.schemes.get_mut(&scheme) else {
                continue;
            };
            let mut effects = Vec::new();
            active.scheme.execute_settlement(id, &mut effects);
            let ledger = active.scheme.ledger();
            if let Err(error) = self.apply_effects(scheme, ledger, &effects) {
                tracing::error!(target: target::SETTLEMENT, %error, "a settlement's outcome was not booked");
            }
        }
        true
    }

    /// Hands each scheme its settlements still pending from before a
    /// restart (ACC-16).
    pub(super) fn reconcile_settlements(&mut self) {
        let schemes: Vec<Name> = self.schemes.keys().copied().collect();
        for scheme in schemes {
            let pending: Vec<(Name, u64)> = self
                .ledger
                .pending_transfers()
                .filter_map(|transfer| match transfer.reference {
                    Reference::Settlement { scheme: s, id } if s == scheme => {
                        Some((id, transfer.amount))
                    }
                    _ => None,
                })
                .collect();
            if pending.is_empty() {
                continue;
            }
            let Some(active) = self.schemes.get_mut(&scheme) else {
                continue;
            };
            let mut effects = Vec::new();
            active.scheme.reconcile(&pending, &mut effects);
            let ledger = active.scheme.ledger();
            tracing::info!(target: target::SETTLEMENT, %scheme, pending = pending.len(), "reconciling settlements from before the restart");
            if let Err(error) = self.apply_effects(scheme, ledger, &effects) {
                tracing::error!(target: target::SETTLEMENT, %error, "a reconciliation was not booked");
            }
        }
    }

    /// One of our settlements, by scheme and ID: in progress, or pending in
    /// the ledger from before a restart.
    pub(super) fn take_settling(&mut self, scheme: Name, id: Name) -> Option<Settling> {
        if let Some(settling) = self.settle.settling.remove(&(scheme, id)) {
            return Some(settling);
        }
        let transfer = self
            .ledger
            .pending_transfers()
            .find(|transfer| transfer.reference == Reference::Settlement { scheme, id })?;
        let path = self.ledger.account(transfer.debit)?.path();
        let AccountName::AssetsInterledger(peer) = path.name else {
            return None;
        };
        Some(Settling {
            peer: NodeId::parse(peer.as_str()).ok()?,
            transfer: transfer.id,
            amount: transfer.amount,
            message: Vec::new(),
        })
    }

    /// Our settlement executed: its transfer is posted, and the peer is
    /// told, until it has heard.
    pub(super) fn settlement_done(&mut self, scheme: Name, id: Name) {
        let Some(settling) = self.take_settling(scheme, id) else {
            tracing::warn!(target: target::SETTLEMENT, %scheme, %id, "an unknown settlement executed");
            return;
        };
        if let Err(error) = self.ledger.post(self.now_ms, settling.transfer) {
            tracing::error!(target: target::SETTLEMENT, %error, "a settlement's transfer could not be posted");
            return;
        }
        self.settle.backoff.remove(&settling.peer);
        let Some(scheme_id) = SchemeId::parse(scheme.as_str()) else {
            return;
        };
        let peer = settling.peer;
        if let Err(error) = self.peer.send_settlement(
            self.now_ms,
            &peer,
            scheme_id,
            settling.amount,
            settling.message,
        ) {
            tracing::error!(target: target::SETTLEMENT, %peer, ?error, "a settlement could not be announced");
        }
    }

    /// Our settlement failed for good: its transfer is voided, and the
    /// peer is not settled with again for a while.
    pub(super) fn settlement_cancelled(&mut self, scheme: Name, id: Name) {
        let Some(settling) = self.take_settling(scheme, id) else {
            tracing::warn!(target: target::SETTLEMENT, %scheme, %id, "an unknown settlement was cancelled");
            return;
        };
        if let Err(error) = self.ledger.void(self.now_ms, settling.transfer) {
            tracing::error!(target: target::SETTLEMENT, %error, "a settlement's transfer could not be voided");
        }
        let now = self.now_ms;
        let (_, failures) = self
            .settle
            .backoff
            .get(&settling.peer)
            .copied()
            .unwrap_or((0, 0));
        let wait = CANCEL_BACKOFF_MIN_MS
            .saturating_mul(1u64.checked_shl(failures.min(32)).unwrap_or(u64::MAX))
            .min(CANCEL_BACKOFF_MAX_MS);
        self.settle.backoff.insert(
            settling.peer,
            (now.saturating_add(wait), failures.saturating_add(1)),
        );
        tracing::warn!(target: target::SETTLEMENT, peer = %settling.peer, amount = settling.amount, "a settlement failed");
    }

    /// A peer's settlement: booked first, so the scheme's balance never
    /// moves without the books. Returns whether it was applied.
    pub(super) fn incoming_settlement(
        &mut self,
        from: NodeId,
        scheme: SchemeId,
        amount: u64,
        data: &[u8],
    ) -> Result<(), EngineError> {
        let refused = |why: &str| EngineError::Invalid(why.into());
        let scheme = Name::new(scheme.as_str()).map_err(|_| refused("unknown scheme"))?;
        let ledger = self
            .schemes
            .get(&scheme)
            .map(|active| active.scheme.ledger())
            .ok_or_else(|| refused("the scheme is not active"))?;
        let name = Name::new(from.as_str()).map_err(|_| refused("not a node"))?;
        let interledger = self
            .ledger
            .find(&AccountPath::new(
                ledger,
                AccountName::AssetsInterledger(name),
            ))
            .ok_or_else(|| refused("no accounts on that scheme's ledger"))?;
        let spec = TransferSpec {
            debit: self.account(ledger, AccountName::AssetsSettlement)?,
            credit: interledger,
            amount,
            pending: false,
            reference: Reference::Settlement { scheme, id: scheme },
        };
        self.ledger
            .validate(&spec)
            .map_err(|error| refused(&error.to_string()))?;
        let state = self
            .peer
            .peering_of(&from)
            .map(|peering| peering.state.clone())
            .unwrap_or_default();
        let mut effects = Vec::new();
        self.schemes
            .get_mut(&scheme)
            .ok_or_else(|| refused("the scheme is not active"))?
            .scheme
            .handle_settlement(name, amount, data, &state, &mut effects)
            .map_err(|error| refused(&error.to_string()))?;
        tracing::info!(target: target::SETTLEMENT, %from, amount, "a peer settled");
        self.apply_effects(scheme, ledger, &effects)
    }

    /// A scheme's message from a peer, for the scheme.
    pub(super) fn scheme_message(&mut self, from: NodeId, scheme: SchemeId, message: &[u8]) {
        let (Ok(scheme), Ok(name)) = (Name::new(scheme.as_str()), Name::new(from.as_str())) else {
            return;
        };
        let Some(active) = self.schemes.get_mut(&scheme) else {
            return;
        };
        let mut effects = Vec::new();
        active.scheme.handle_message(name, message, &mut effects);
        let ledger = active.scheme.ledger();
        if let Err(error) = self.apply_effects(scheme, ledger, &effects) {
            tracing::error!(target: target::SETTLEMENT, %error, "a scheme message's effects were not booked");
        }
    }

    /// A reference for an incoming settlement: unique, as the next
    /// transfer's ID is.
    pub(super) fn incoming_reference(&self, scheme: Name) -> Reference {
        let id = Name::new(&format!("in{}", self.ledger.next_transfer_id().0)).unwrap_or(scheme);
        Reference::Settlement { scheme, id }
    }
}

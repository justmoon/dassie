//! The settlement schemes the node can run, and the connections of those that
//! follow an underlying ledger
//! (spec/areas/settlement.md#schemes-that-follow-a-ledger).
//!
//! A scheme with a ledger to follow does no IO: it hands the core what to
//! send, and the driver carries it over a WebSocket of its own, bound to the
//! scheme's *epoch*, so that a message from a connection the core has since
//! replaced is dropped. So the node starts without waiting for a ledger,
//! and a scheme that fails fails alone, where the original's start waited
//! for every ledger and a module's failure reached them all (ACC-26).

use dassie_identity::NodeKey;
use dassie_ledger::{LedgerId, Name};
use dassie_settlement::{
    Effect, LedgerInput, LedgerStatus, SchemeError, SchemeInfo, SettlementScheme, Stub,
};
use dassie_xrpl::XrplScheme;

use super::events::SettlementEvent;
use super::{Core, Output};
use crate::store::Checkpoint;

/// Every scheme the node can run, in a fixed order.
pub const SCHEMES: &[SchemeInfo] = &[
    SchemeInfo {
        id: "stub",
        name: "Stub",
        ledger: LedgerId::StubUsd,
    },
    SchemeInfo {
        id: dassie_xrpl::scheme::SCHEME_ID,
        name: "XRP Ledger Testnet",
        ledger: LedgerId::XrplTestnetXrp,
    },
];

/// Makes the scheme `id` with its config, and what the books last kept
/// for it. The original's RPC dropped every key of the config (RT-21), and
/// it never passed on what it stored (ACC-12).
///
/// # Errors
///
/// If the scheme is unknown, or refuses its config or its state.
pub(super) fn create(
    id: &str,
    config: &str,
    checkpoint: Option<&Checkpoint>,
    key: &NodeKey,
) -> Result<Box<dyn SettlementScheme>, SchemeError> {
    let balance = checkpoint.map(|c| c.balance);
    match id {
        "stub" => Ok(Box::new(Stub::new(config, balance)?)),
        dassie_xrpl::scheme::SCHEME_ID => {
            let entropy = key
                .settlement_entropy(id, "seed-testnet")
                .map_err(|_| SchemeError::InvalidConfig)?;
            let node = Name::new(key.node_id().as_str()).map_err(|_| SchemeError::InvalidData)?;
            let state = checkpoint.map_or(&[][..], |c| c.state.as_slice());
            Ok(Box::new(XrplScheme::new(
                config,
                entropy.as_bytes(),
                node,
                balance,
                state,
            )?))
        }
        _ => Err(SchemeError::UnknownScheme),
    }
}

/// What comes for a scheme from its connection or its faucet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerEvent {
    /// The connection is up.
    Connected,
    /// A message came.
    Message(Vec<u8>),
    /// The connection went down; the driver tries again.
    Disconnected,
    /// The faucet answered, or did not.
    Funded(bool),
}

impl Core {
    /// What a scheme's connection or faucet says, for the connection of
    /// `epoch`: from an older one, it is dropped.
    pub fn ledger_event(&mut self, scheme: &str, epoch: u64, event: &LedgerEvent) {
        let Ok(scheme) = Name::new(scheme) else {
            return;
        };
        let now = self.now_ms;
        let Some(active) = self.schemes.get_mut(&scheme) else {
            return;
        };
        if active.epoch != epoch || active.failed.is_some() {
            return;
        }
        let input = match event {
            LedgerEvent::Connected => LedgerInput::Connected,
            LedgerEvent::Message(bytes) => LedgerInput::Message(bytes),
            LedgerEvent::Disconnected => LedgerInput::Disconnected,
            LedgerEvent::Funded(ok) => LedgerInput::Funded(*ok),
        };
        let mut effects = Vec::new();
        active.scheme.on_ledger(now, input, &mut effects);
        let ledger = active.scheme.ledger();
        self.scheme_effects(scheme, ledger, &effects);
    }

    /// Books a scheme's effects. A failure is a bug: the scheme is fed
    /// nothing more, so the books do not drift further from it, and the
    /// owner is told. Its state is not written, so a restart reads what
    /// the ledger said again.
    ///
    /// A scheme that has just synced for the first time can be peered on
    /// from now (D31).
    fn scheme_effects(&mut self, scheme: Name, ledger: LedgerId, effects: &[Effect]) {
        if let Err(error) = self.apply_effects(scheme, ledger, effects) {
            self.report(SettlementEvent::SchemeStopped {
                scheme,
                error: error.clone(),
            });
            if let Some(active) = self.schemes.get_mut(&scheme) {
                active.failed = Some(error.to_string());
            }
        }
        self.close_if_stopped(scheme);
        self.update_peer_schemes();
    }

    /// Closes the connection of a scheme that stopped, by the books or by
    /// itself (spec/areas/settlement.md#the-scheme-interface): the driver
    /// would otherwise make it again for ever, to a ledger the scheme no
    /// longer reads. Removing the scheme and adding it again, or a restart,
    /// connects anew.
    fn close_if_stopped(&mut self, scheme: Name) {
        let Some(active) = self.schemes.get_mut(&scheme) else {
            return;
        };
        let stopped =
            active.failed.is_some() || matches!(active.scheme.status(), LedgerStatus::Failed(_));
        if stopped && active.epoch != 0 {
            let epoch = std::mem::replace(&mut active.epoch, 0);
            self.disconnect_scheme(scheme, epoch);
        }
    }

    /// Runs the schemes' timers that are due.
    pub(super) fn scheme_timers(&mut self) {
        let now = self.now_ms;
        let due: Vec<Name> = self
            .schemes
            .iter()
            .filter(|(_, active)| {
                active.failed.is_none() && active.scheme.deadline().is_some_and(|at| at <= now)
            })
            .map(|(name, _)| *name)
            .collect();
        for scheme in due {
            let Some(active) = self.schemes.get_mut(&scheme) else {
                continue;
            };
            let mut effects = Vec::new();
            active.scheme.on_timer(now, &mut effects);
            let ledger = active.scheme.ledger();
            self.scheme_effects(scheme, ledger, &effects);
        }
    }

    /// The earliest of the schemes' timers.
    pub(super) fn schemes_deadline(&self) -> Option<u64> {
        self.schemes
            .values()
            .filter(|active| active.failed.is_none())
            .filter_map(|active| active.scheme.deadline())
            .min()
    }

    /// Opens a scheme's connection, anew if it had one.
    pub(super) fn connect_scheme(&mut self, scheme: Name) {
        self.next_epoch = self.next_epoch.wrapping_add(1);
        let epoch = self.next_epoch;
        let Some(active) = self.schemes.get_mut(&scheme) else {
            return;
        };
        let old = std::mem::replace(&mut active.epoch, epoch);
        let Some(url) = active.scheme.connection().map(str::to_string) else {
            return;
        };
        if old != 0 {
            self.outputs_peer.push(Output::LedgerClose {
                scheme: scheme.as_str().into(),
                epoch: old,
            });
        }
        self.outputs_peer.push(Output::LedgerConnect {
            scheme: scheme.as_str().into(),
            epoch,
            url,
        });
    }

    /// Closes a scheme's connection, if it has one.
    pub(super) fn disconnect_scheme(&mut self, scheme: Name, epoch: u64) {
        if epoch != 0 {
            self.outputs_peer.push(Output::LedgerClose {
                scheme: scheme.as_str().into(),
                epoch,
            });
        }
    }

    /// Sends a message over a scheme's connection.
    pub(super) fn ledger_send(&mut self, scheme: Name, message: Vec<u8>) {
        let Some(active) = self.schemes.get(&scheme) else {
            return;
        };
        self.outputs_peer.push(Output::LedgerSend {
            scheme: scheme.as_str().into(),
            epoch: active.epoch,
            message,
        });
    }

    /// Asks a faucet for a scheme.
    pub(super) fn ledger_fund(&mut self, scheme: Name, url: String, body: Vec<u8>) {
        let Some(active) = self.schemes.get(&scheme) else {
            return;
        };
        self.outputs_peer.push(Output::Fund {
            scheme: scheme.as_str().into(),
            epoch: active.epoch,
            url,
            body,
        });
    }

    /// Keeps a scheme's balance and state with this batch.
    pub(super) fn checkpoint(&mut self, scheme: Name) {
        let Some(active) = self.schemes.get_mut(&scheme) else {
            return;
        };
        if active.failed.is_some() {
            return;
        }
        active.version = active.scheme.state_version();
        let checkpoint = Checkpoint {
            balance: active.scheme.balance(),
            state: active.scheme.state(),
        };
        self.batch.checkpoints.push((scheme, checkpoint.clone()));
        self.checkpoints.insert(scheme, checkpoint);
    }

    /// Keeps a scheme's balance and state with this batch, if either
    /// changed since last written: a ledger closing changes neither.
    pub(super) fn checkpoint_if_changed(&mut self, scheme: Name) {
        let Some(active) = self.schemes.get(&scheme) else {
            return;
        };
        let balance = active.scheme.balance();
        let changed = active.scheme.state_version() != active.version
            || self
                .checkpoints
                .get(&scheme)
                .is_none_or(|checkpoint| checkpoint.balance != balance);
        if changed {
            self.checkpoint(scheme);
        }
    }

    /// Keeps what changed in any scheme, such as a peer's address learnt
    /// when peering, with this batch.
    pub(super) fn checkpoint_schemes(&mut self) {
        let schemes: Vec<Name> = self.schemes.keys().copied().collect();
        for scheme in schemes {
            self.checkpoint_if_changed(scheme);
        }
    }
}

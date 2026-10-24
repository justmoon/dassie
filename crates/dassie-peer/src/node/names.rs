//! Names at a registrar (spec/areas/network.md#names): a node that is one
//! of its own bootstrap nodes logs commitments, takes their unsealings and
//! renewals, grants names as windows close, and serves its name list. The
//! rule is [`Registrar`]'s; here is who may ask what, what each answer is,
//! the replay state of the nodes a registrar involves, and the events.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::Encode;

use super::PeerNode;
use super::inbound::ok;
use crate::auth::Accepted;
use crate::io::{Event, NameEvent, Reply};
use crate::names::registrar::{
    CommitError, Committed, Happened, RenewError, UnsealError, Unsealed,
};
use crate::names::{Commitment, Name, Registrar, Salt};
use crate::table::Network;
use crate::wire::encode_expiry;

/// What a node that is no registrar answers to a name message, as to a
/// registration.
const NOT_REGISTRAR: &str = "not a bootstrap node";

impl PeerNode {
    /// The registrar's state, if this node is one: one of its own bootstrap
    /// nodes.
    #[must_use]
    pub const fn registrar(&self) -> Option<&Registrar> {
        self.registrar.as_ref()
    }

    /// A `nameCommit`: logged, and answered with its receipt only once it is
    /// on disk, which the driver sees to. A commitment in the log already
    /// gets its receipt again, at no cost.
    pub(super) fn name_commit(
        &mut self,
        accepted: &Accepted,
        commitment: Commitment,
        network: Option<Network>,
    ) -> Reply {
        let (sender, now) = (accepted.sender, self.now_ms);
        let Some(registrar) = self.registrar.as_mut() else {
            return Reply::refuse(409, NOT_REGISTRAR);
        };
        let receipt = match registrar.commit(commitment, sender, network, now) {
            Ok(Committed::Again(receipt)) => receipt,
            Ok(Committed::New(receipt)) => {
                // Its unsealing must get through however many strangers
                // write to us, and its floor is persisted with the
                // commitment (spec/areas/peer-protocol.md#replay-protection).
                self.auth.pin(sender);
                self.stats.commitments_logged += 1;
                self.event(Event::Name(NameEvent::Committed {
                    committer: sender,
                    position: receipt.position,
                }));
                receipt
            }
            Err(error) => return commit_refusal(error),
        };
        ok(accepted, &receipt.to_vec())
    }

    /// A `nameUnseal`: the sender opens its own commitment. A valid one is
    /// answered alike whether it contends for the name or has lost, so the
    /// answer tells nothing of commitments still sealed.
    pub(super) fn name_unseal(&mut self, accepted: &Accepted, name: Name, salt: &Salt) -> Reply {
        let (sender, now) = (accepted.sender, self.now_ms);
        let Some(registrar) = self.registrar.as_mut() else {
            return Reply::refuse(409, NOT_REGISTRAR);
        };
        match registrar.unseal(sender, name, salt, now) {
            Ok(Unsealed::Again) => {}
            Ok(Unsealed::New { contends }) => {
                // A copied commitment may have been logged by another node:
                // the holder has a part of its own now.
                self.auth.pin(sender);
                self.event(Event::Name(NameEvent::Unsealed {
                    name,
                    holder: sender,
                    contends,
                }));
            }
            Err(error) => return Reply::refuse(409, unseal_refusal(error)),
        }
        ok(accepted, &[])
    }

    /// A `nameRenew`, from the name's holder: answered with its expiry.
    pub(super) fn name_renew(&mut self, accepted: &Accepted, name: Name) -> Reply {
        let (sender, now) = (accepted.sender, self.now_ms);
        let Some(registrar) = self.registrar.as_mut() else {
            return Reply::refuse(409, NOT_REGISTRAR);
        };
        let renewal = match registrar.renew(&sender, &name, now) {
            Ok(renewal) => renewal,
            Err(RenewError::NotHeld) => return Reply::refuse(409, "the name is not held here"),
            Err(RenewError::NotHolder) => return Reply::refuse(409, "another node holds the name"),
        };
        if renewal.extended {
            self.stats.names_renewed += 1;
            self.event(Event::Name(NameEvent::Renewed {
                name,
                holder: sender,
                expires_ms: renewal.expires_ms,
            }));
        }
        ok(accepted, &encode_expiry(renewal.expires_ms))
    }

    /// The name list we serve, or its hash: a registrar's names listed now.
    /// A node that is no registrar serves none.
    pub(super) fn name_list(&mut self, accepted: &Accepted, hash_only: bool) -> Reply {
        let now = self.now_ms;
        let Some(registrar) = self.registrar.as_mut() else {
            return Reply::refuse(409, NOT_REGISTRAR);
        };
        let (served, hash) = registrar.served(now);
        if hash_only {
            ok(accepted, &hash)
        } else {
            ok(accepted, served)
        }
    }

    /// Takes in the passing of time at the registrar: grants, losses,
    /// lapses and names freed. A node left with no part in it lets its
    /// replay state go, unless it is kept for another reason.
    pub(super) fn tend_names(&mut self) {
        let now = self.now_ms;
        let Some(registrar) = self.registrar.as_mut() else {
            return;
        };
        if registrar.next_due().is_none_or(|due| due > now) {
            return;
        }
        let before: BTreeSet<NodeId> = registrar.parties().collect();
        let mut happened = Vec::new();
        registrar.on_time(now, &mut happened);
        let after: BTreeSet<NodeId> = registrar.parties().collect();
        for happening in happened {
            self.name_event(happening);
        }
        for node in before.difference(&after) {
            if !self.keeps_replay_state(node) {
                self.auth.unpin(now, node);
            }
        }
    }

    fn name_event(&mut self, happened: Happened) {
        let event = match happened {
            Happened::Granted { name, holder } => {
                self.stats.names_granted += 1;
                NameEvent::Granted { name, holder }
            }
            Happened::Lost { name, holder } => NameEvent::Lost { name, holder },
            Happened::Lapsed {
                committer,
                position,
            } => {
                self.stats.commitments_lapsed += 1;
                NameEvent::Lapsed {
                    committer,
                    position,
                }
            }
            Happened::Freed { name, holder } => {
                self.stats.names_freed += 1;
                NameEvent::Freed { name, holder }
            }
        };
        self.event(Event::Name(event));
    }

    /// The registrar's invariants, and that every node it involves keeps
    /// its replay state.
    pub(super) fn check_names(&self) {
        let Some(registrar) = &self.registrar else {
            return;
        };
        registrar.check_invariants();
        for party in registrar.parties() {
            assert!(
                party == self.own || self.auth.is_pinned(&party),
                "a node a registrar involves keeps its replay state"
            );
        }
    }
}

/// The answer to a commitment that is not logged, with the statuses a
/// registration's refusals use: 503 for a table full, 409 for a cap, 429
/// for the rate.
fn commit_refusal(error: CommitError) -> Reply {
    match error {
        CommitError::LogFull => Reply::refuse(503, "commitments are full"),
        CommitError::NamesFull => Reply::refuse(503, "names are full"),
        CommitError::Network => Reply::refuse(409, "too many commitments from that network"),
        CommitError::Commitments => Reply::refuse(409, "too many commitments from that node"),
        CommitError::Names => Reply::refuse(409, "the node holds too many names"),
        CommitError::Rate => Reply::refuse(429, "too many new commitments"),
    }
}

const fn unseal_refusal(error: UnsealError) -> &'static str {
    match error {
        UnsealError::Reserved => "the name is reserved",
        UnsealError::Unknown => "no commitment of the sender's opens with that name and salt",
        UnsealError::Closed => "the commitment's window has closed",
        UnsealError::Taken => "the name was taken when the commitment was logged",
    }
}

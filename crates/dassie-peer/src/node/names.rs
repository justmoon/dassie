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

/// Most events a step of the registrar's timer queues, however many names
/// and commitments it decides: the queue is bounded and drops its oldest,
/// and the peer events queued before it in the same tick must reach the
/// driver.
const MAX_REGISTRAR_EVENTS: u64 = 1;

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
    /// lapses and names freed, reported together in one event, however many
    /// there were (spec/areas/network.md#the-registrar). A node left with no
    /// part in it lets its replay state go, unless it is kept for another
    /// reason.
    pub(super) fn tend_names(&mut self) {
        let (now, queued) = (self.now_ms, self.events_queued());
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
        self.report_tended(&happened);
        for node in before.difference(&after) {
            if !self.keeps_replay_state(node) {
                self.auth.unpin(now, node);
            }
        }
        assert!(
            self.events_since(queued) <= MAX_REGISTRAR_EVENTS,
            "a step of the registrar's timer queues a bounded number of events"
        );
    }

    /// Counts what a step of the registrar's timer did, and reports it in
    /// one event if it did anything.
    fn report_tended(&mut self, happened: &[Happened]) {
        let (mut granted, mut lost, mut lapsed, mut freed) = (0, 0, 0, 0);
        for happening in happened {
            match happening {
                Happened::Granted { .. } => granted += 1,
                Happened::Lost { .. } => lost += 1,
                Happened::Lapsed { .. } => lapsed += 1,
                Happened::Freed { .. } => freed += 1,
            }
        }
        if happened.is_empty() {
            return;
        }
        let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
        self.stats.names_granted += count(granted);
        self.stats.commitments_lapsed += count(lapsed);
        self.stats.names_freed += count(freed);
        self.event(Event::Name(NameEvent::RegistrarChanged {
            granted,
            lost,
            lapsed,
            freed,
        }));
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
        UnsealError::Names => "the node holds too many names",
    }
}

#[cfg(all(test, feature = "sim"))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use alloc::format;
    use alloc::vec::Vec;

    use dassie_identity::{NodeId, NodeKey};

    use super::super::MAX_EVENTS;
    use crate::config::Rate;
    use crate::names::{Commitment, Name, Salt};
    use crate::sim::{Network, key};
    use crate::{Config, Event, NameEvent};

    /// More commitments than the event queue holds.
    const COMMITMENTS: usize = MAX_EVENTS + 76;

    /// The `index`th commitment's committer: each sends as many
    /// commitments as one node may have logged.
    fn committer(index: usize, per_node: usize) -> NodeId {
        let mut seed = [9; 32];
        let [high, low] = u16::try_from(index / per_node).unwrap().to_be_bytes();
        (seed[0], seed[1]) = (high, low);
        NodeKey::from_bytes(&seed).node_id()
    }

    // A step of the registrar's timer that decides more commitments than
    // the event queue holds, as one back after a long downtime does,
    // reports one event, so a peer's event queued before it in the same
    // tick still reaches the driver.
    #[test]
    fn a_registrar_step_deciding_every_commitment_queues_one_event() {
        let mut config = Config::DEFAULT;
        config.names.max_commitments = COMMITMENTS;
        config.names.commit_rate = Rate {
            burst: u32::try_from(COMMITMENTS).unwrap(),
            refill_ms: 1,
        };
        let per_node = config.names.max_commitments_per_node;
        let mut network = Network::new(33, 2, 1, config);
        network.run_until(network.now + 30_000);
        let peer = key(1).node_id();
        network.act(0, |node, now| {
            for index in 0..COMMITMENTS {
                let committer = committer(index, per_node);
                let name = Name::parse(&format!("n{index:04}")).unwrap();
                let commitment = Commitment::seal(&name, &committer, &Salt([0; 32]));
                let registrar = node.registrar.as_mut().unwrap();
                assert!(registrar.commit(commitment, committer, None, now).is_ok());
                node.auth.pin(committer);
            }
        });
        let before = network.events.len();
        let window_ms = config.names.window_ms;
        network.act(0, |node, now| {
            node.event(Event::PeeringEnded { peer });
            // Past every window at once.
            node.advance(now + window_ms + u64::try_from(COMMITMENTS).unwrap());
            node.tend_names();
        });
        let events: Vec<&Event> = network.events[before..]
            .iter()
            .map(|(index, event)| {
                assert_eq!(*index, 0);
                event
            })
            .collect();
        assert_eq!(
            events,
            [
                &Event::PeeringEnded { peer },
                &Event::Name(NameEvent::RegistrarChanged {
                    granted: 0,
                    lost: 0,
                    lapsed: COMMITMENTS,
                    freed: 0,
                })
            ]
        );
        let stats = network.node(0).stats();
        assert_eq!(
            stats.commitments_lapsed,
            u64::try_from(COMMITMENTS).unwrap()
        );
    }
}

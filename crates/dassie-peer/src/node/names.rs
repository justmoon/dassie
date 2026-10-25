//! Names (spec/areas/network.md#names). At a registrar, a node that is one
//! of its own bootstrap nodes: it logs commitments, takes their unsealings
//! and renewals, grants names as windows close, serves its name list, and
//! releases the names the other registrars' lists show split. The rule is
//! [`Registrar`]'s; here is who may ask what, what each answer is, the
//! replay state of the nodes a registrar involves, and the events. At every
//! node: polling the registrars' name lists, and taking from them who holds
//! each name, which is [`Holders`]'.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};
use sha2::{Digest, Sha256};

use super::inbound::ok;
use super::{Bootstrap, PeerNode, Problem, Purpose};
use crate::auth::Accepted;
use crate::config::Config;
use crate::io::{Event, NameEvent, Reply};
use crate::names::registrar::{
    CommitError, Committed, Happened, RenewError, UnsealError, Unsealed,
};
use crate::names::{
    Commitment, HeldName, HolderChange, Holders, HoldersConfig, Name, Registrar, Salt,
};
use crate::table::Network;
use crate::wire::{Content, NameListResponse, decode_hash, encode_expiry};

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
                // Only `release_splits` releases.
                Happened::Released { .. } => {}
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

    /// Who holds each name, in our view (spec/areas/network.md#holders).
    #[must_use]
    pub const fn holders(&self) -> &Holders {
        &self.holders
    }

    /// Polls every registrar's name list hash, with the node lists: our own
    /// list, if we are one, is ours without asking. Then takes the holders
    /// again, which time alone can freeze.
    pub(super) fn poll_names(&mut self) {
        self.take_own_names();
        let own = self.own;
        let targets: Vec<NodeId> = self
            .bootstrap
            .iter()
            .map(|node| node.id)
            .filter(|id| *id != own && !self.name_polling.contains(id))
            .collect();
        let timeout = self.config.poll_timeout_ms;
        for target in targets {
            let sent = self.send(
                target,
                &Content::NameListHashRequest,
                Purpose::NameHashPoll,
                timeout,
            );
            if sent.is_some() {
                self.name_polling.insert(target);
            }
        }
        self.update_holders();
    }

    /// Our own name list, as a registrar, into our view.
    fn take_own_names(&mut self) {
        let (own, now) = (self.own, self.now_ms);
        let Some(registrar) = self.registrar.as_mut() else {
            return;
        };
        let (served, hash) = registrar.served(now);
        if self.holders.hash(&own) == Some(hash) {
            self.holders.confirm(&own, now);
        } else if let Ok(list) = NameListResponse::decode_exact(served) {
            self.holders.take_list(&own, list, hash, now);
        }
    }

    pub(super) fn on_name_hash(&mut self, from: NodeId, result: Result<Vec<u8>, Problem>) {
        self.name_polling.remove(&from);
        let Ok(hash) = result.and_then(|body| decode_hash(&body).map_err(|_| Problem::Invalid))
        else {
            return;
        };
        if self.holders.hash(&from) == Some(hash) {
            self.holders.confirm(&from, self.now_ms);
            self.update_holders();
            return;
        }
        let timeout = self.config.request_timeout_ms;
        let sent = self.send(
            from,
            &Content::NameListRequest,
            Purpose::NameListDownload,
            timeout,
        );
        if sent.is_some() {
            self.name_polling.insert(from);
        }
    }

    /// A downloaded name list. Its authenticity comes from the response MAC
    /// under the registrar's configured key, as a node list's; its hash is
    /// that of what was received.
    pub(super) fn on_name_list(&mut self, from: NodeId, result: Result<Vec<u8>, Problem>) {
        self.name_polling.remove(&from);
        let Ok(body) = result else {
            return;
        };
        let Ok(list) = NameListResponse::decode_exact(&body) else {
            return;
        };
        let hash = Sha256::digest(&body).into();
        if self.holders.take_list(&from, list, hash, self.now_ms) {
            self.update_holders();
        }
    }

    /// Takes the holders again, and, at a registrar, releases the names the
    /// other registrars' lists show split. However many names change, it
    /// reports at most two events, one for the view and one for the
    /// releases: the queue is bounded and drops its oldest, and the peer
    /// events queued before this in the same tick must reach the driver.
    fn update_holders(&mut self) {
        let mut changes = Vec::new();
        self.holders.recompute(self.now_ms, &mut changes);
        if !changes.is_empty() {
            let unheld = changes
                .iter()
                .filter(|change| matches!(change, HolderChange::Unheld { .. }))
                .count();
            let held = changes.len() - unheld;
            self.event(Event::Name(NameEvent::HoldersChanged { held, unheld }));
        }
        self.release_splits();
    }

    /// Releases the names we list that more than f of the other registrars'
    /// fresh lists have lacked the holder of for long enough
    /// (spec/areas/network.md#releasing-a-split), reported together. A
    /// holder left with no part here lets its replay state go, unless it is
    /// kept for another reason.
    fn release_splits(&mut self) {
        let (own, now) = (self.own, self.now_ms);
        let holders = &self.holders;
        let Some(registrar) = self.registrar.as_mut() else {
            return;
        };
        let mut happened = Vec::new();
        let is_split = |name: &Name, holder: &NodeId| holders.is_split(&own, name, holder, now);
        registrar.release_splits(now, is_split, &mut happened);
        let mut released = 0;
        for happening in happened {
            let Happened::Released { holder, .. } = happening else {
                continue;
            };
            released += 1;
            if !self.keeps_replay_state(&holder) {
                self.auth.unpin(now, &holder);
            }
        }
        if released > 0 {
            self.stats.names_released += u64::try_from(released).unwrap_or(u64::MAX);
            self.event(Event::Name(NameEvent::Released { names: released }));
        }
    }

    /// The view's invariants, the registrar's, and that every node it
    /// involves keeps its replay state.
    pub(super) fn check_names(&self) {
        self.holders.check_invariants();
        assert!(
            self.name_polling
                .iter()
                .all(|id| self.bootstrap.iter().any(|node| node.id == *id)),
            "only registrars' name lists are polled"
        );
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

/// A view of the lists of the registrars, the bootstrap nodes, starting from
/// `held`, as persisted (spec/areas/network.md#holders). Lists are fresh as
/// long as node lists are, and vote only while they are.
pub(super) fn holders(config: &Config, bootstrap: &[Bootstrap], held: Vec<HeldName>) -> Holders {
    let lists = HoldersConfig {
        list_fresh_ms: config.list_fresh_ms,
    };
    Holders::new(lists, bootstrap.iter().map(|node| node.id), held)
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
    use crate::wire::{MAX_NAME_LIST, NameHolding, NameListResponse};
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

    /// More names than the event queue holds.
    const NAMES: usize = MAX_EVENTS + 76;
    const _: () = assert!(NAMES <= MAX_NAME_LIST);

    /// A list that names `holder` for each of [`NAMES`] names, or none.
    fn list(holder: Option<NodeId>) -> NameListResponse {
        let mut list = NameListResponse::default();
        if let Some(holder) = holder {
            for index in 0..NAMES {
                let name = Name::parse(&format!("n{index:04}")).unwrap();
                let holding = NameHolding {
                    holder,
                    expires_at_ms: u64::MAX,
                };
                list.0.insert(name, holding);
            }
        }
        list
    }

    // A taking of the view that changes more names than the event queue
    // holds reports one event, so a peer's event queued before it in the
    // same tick still reaches the driver.
    #[test]
    fn a_view_changing_every_name_queues_one_event() {
        let mut network = Network::new(31, 5, 4, Config::DEV);
        network.run_until(network.now + 30_000);
        let registrars: Vec<NodeId> = (0..4).map(|index| key(index).node_id()).collect();
        let (holder, peer) = (key(1).node_id(), key(2).node_id());
        for (shown, held, unheld) in [(Some(holder), NAMES, 0), (None, 0, NAMES)] {
            let before = network.events.len();
            // The step's events all reach the driver, which polls them
            // after it.
            network.act(4, |node, now| {
                node.event(Event::PeeringEnded { peer });
                for (registrar, hash) in registrars.iter().zip(1u8..) {
                    assert!(
                        node.holders
                            .take_list(registrar, list(shown), [hash; 32], now)
                    );
                }
                node.update_holders();
            });
            let events: Vec<&Event> = network.events[before..]
                .iter()
                .map(|(index, event)| {
                    assert_eq!(*index, 4);
                    event
                })
                .collect();
            assert_eq!(
                events,
                [
                    &Event::PeeringEnded { peer },
                    &Event::Name(NameEvent::HoldersChanged { held, unheld })
                ]
            );
            assert_eq!(network.node(4).holders().held().count(), NAMES - unheld);
        }
    }
}

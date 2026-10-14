//! Membership (spec 3 §11, D4, spec 7 §4.7): polling the bootstrap nodes'
//! lists, the majority with its freeze, the list we serve, and registering
//! with the bootstrap nodes.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};
use sha2::{Digest, Sha256};

use super::{Bootstrap, PeerNode, Problem, Purpose};
use crate::io::Event;
use crate::table::{Sources, backoff};
use crate::wire::{Content, NodeListResponse, SignedNodeInfo, decode_expiry, decode_hash};

/// A bootstrap node's list, and our registration with it.
#[derive(Clone, Debug, Default)]
struct ListState {
    hash: Option<[u8; 32]>,
    ids: Vec<NodeId>,
    fetched_ms: Option<u64>,
    polling: bool,
    registered_ms: Option<u64>,
    register_at_ms: u64,
    register_failures: u32,
    registering: bool,
    /// The owner asked to register now, whatever the timers say.
    forced: bool,
}

/// Our standing with one bootstrap node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrationStatus {
    /// The bootstrap node.
    pub bootstrap: NodeId,
    /// Whether its list, as last fetched, has us.
    pub listed: bool,
    /// When it last took our registration.
    pub registered_ms: Option<u64>,
    /// When its list was last fetched.
    pub list_fetched_ms: Option<u64>,
}

/// Why registering now was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// The node is not one of our bootstrap nodes.
    NotBootstrap,
    /// We have no link state to register yet: no URL is configured.
    NoLinkState,
    /// A registration with it is on its way.
    InFlight,
}

/// The membership side of the node's state.
pub(crate) struct Lists {
    states: BTreeMap<NodeId, ListState>,
    poll_at_ms: u64,
    /// The nodes on a majority of the lists.
    pub majority: BTreeSet<NodeId>,
    /// Whether the list we serve must be rebuilt.
    pub served_changed: bool,
    served: Vec<u8>,
    served_hash: [u8; 32],
}

impl Lists {
    pub(crate) fn new(bootstrap: &[Bootstrap]) -> Self {
        Self {
            states: bootstrap
                .iter()
                .map(|node| (node.id, ListState::default()))
                .collect(),
            poll_at_ms: 0,
            majority: BTreeSet::new(),
            served_changed: true,
            served: Vec::new(),
            served_hash: [0; 32],
        }
    }

    pub(crate) fn next_due(&self) -> Option<u64> {
        (!self.states.is_empty()).then_some(self.poll_at_ms)
    }

    /// The list we serve, encoded, and its hash.
    pub(crate) fn served(&self) -> (&[u8], [u8; 32]) {
        (&self.served, self.served_hash)
    }
}

impl PeerNode {
    /// The list we serve: registrations that hold, and ourselves (D4).
    pub(crate) fn rebuild_served(&mut self) {
        if !self.lists.served_changed {
            return;
        }
        self.lists.served_changed = false;
        let mut ids: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| entry.registration.is_some())
            .map(|(id, _)| *id)
            .chain(core::iter::once(self.own))
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.truncate(crate::wire::MAX_NODE_LIST);
        let served = NodeListResponse(ids);
        self.lists.served = served.to_vec();
        self.lists.served_hash = Sha256::digest(&self.lists.served).into();
        if let Some(state) = self.lists.states.get_mut(&self.own) {
            state.ids = served.0;
        }
    }

    /// Polls every bootstrap node's list hash; our own list, if we are
    /// one, is ours without asking.
    pub(super) fn poll_lists(&mut self) {
        self.rebuild_served();
        if self.lists.states.is_empty() || self.lists.poll_at_ms > self.now_ms {
            return;
        }
        self.lists.poll_at_ms = self
            .now_ms
            .saturating_add(self.config.list_poll_interval_ms);
        if let Some(state) = self.lists.states.get_mut(&self.own) {
            state.fetched_ms = Some(self.now_ms);
        }
        let targets: Vec<NodeId> = self
            .lists
            .states
            .iter()
            .filter(|(id, state)| **id != self.own && !state.polling)
            .map(|(id, _)| *id)
            .collect();
        let timeout = self.config.poll_timeout_ms;
        for target in targets {
            let sent = self.send(
                target,
                &Content::NodeListHashRequest,
                Purpose::HashPoll,
                timeout,
            );
            if sent.is_some()
                && let Some(state) = self.lists.states.get_mut(&target)
            {
                state.polling = true;
            }
        }
        self.recompute_majority();
    }

    pub(super) fn on_hash(&mut self, from: NodeId, result: Result<Vec<u8>, Problem>) {
        let now = self.now_ms;
        let Some(state) = self.lists.states.get_mut(&from) else {
            return;
        };
        state.polling = false;
        let Ok(hash) = result.and_then(|body| decode_hash(&body).map_err(|_| Problem::Invalid))
        else {
            return;
        };
        if state.hash == Some(hash) {
            state.fetched_ms = Some(now);
            self.recompute_majority();
            return;
        }
        let timeout = self.config.request_timeout_ms;
        if self
            .send(
                from,
                &Content::NodeListRequest,
                Purpose::ListDownload,
                timeout,
            )
            .is_some()
            && let Some(state) = self.lists.states.get_mut(&from)
        {
            state.polling = true;
        }
    }

    /// A downloaded list. Its authenticity comes from the response MAC
    /// under the configured key; its hash is that of what was received
    /// (NET-15).
    pub(super) fn on_list(&mut self, from: NodeId, result: Result<Vec<u8>, Problem>) {
        let now = self.now_ms;
        let Some(state) = self.lists.states.get_mut(&from) else {
            return;
        };
        state.polling = false;
        let Ok(body) = result else {
            return;
        };
        let Ok(list) = NodeListResponse::decode_exact(&body) else {
            return;
        };
        let mut ids = list.0;
        ids.sort_unstable();
        ids.dedup();
        state.ids = ids;
        state.hash = Some(Sha256::digest(&body).into());
        state.fetched_ms = Some(now);
        self.recompute_majority();
    }

    /// The majority: nodes on more than half of the configured lists
    /// (NET-16). Lists not fetched for a day no longer vote; nodes leave
    /// the majority only while more than half of the lists are fresh, so
    /// bootstrap nodes going down do not empty every table (D4).
    pub(crate) fn recompute_majority(&mut self) {
        let total = self.bootstrap.len();
        if total == 0 {
            return;
        }
        let (now, expiry, fresh_ms) = (
            self.now_ms,
            self.config.list_expiry_ms,
            self.config.list_fresh_ms,
        );
        let mut votes: BTreeMap<NodeId, usize> = BTreeMap::new();
        let mut fresh = 0;
        for state in self.lists.states.values() {
            let Some(fetched) = state.fetched_ms else {
                continue;
            };
            if fetched.saturating_add(fresh_ms) >= now {
                fresh += 1;
            }
            if fetched.saturating_add(expiry) >= now {
                for id in &state.ids {
                    *votes.entry(*id).or_default() += 1;
                }
            }
        }
        let majority: BTreeSet<NodeId> = votes
            .into_iter()
            .filter(|(_, count)| count * 2 > total)
            .map(|(id, _)| id)
            .collect();
        let added: Vec<NodeId> = majority.difference(&self.lists.majority).copied().collect();
        for id in added {
            if self.ensure(id, Sources::MAJORITY) {
                self.lists.majority.insert(id);
            }
        }
        if fresh * 2 > total {
            let lost: Vec<NodeId> = self.lists.majority.difference(&majority).copied().collect();
            for id in lost {
                self.drop_source(&id, Sources::MAJORITY);
                self.lists.majority.remove(&id);
            }
        }
    }

    /// Registers with the bootstrap node `node` now, whatever the timers
    /// say: the owner asked.
    pub fn register_now(&mut self, now_ms: u64, node: &NodeId) -> Result<(), RegisterError> {
        self.advance(now_ms);
        if *node == self.own || !self.lists.states.contains_key(node) {
            return Err(RegisterError::NotBootstrap);
        }
        if self.own_link_bytes().is_none() {
            return Err(RegisterError::NoLinkState);
        }
        let state = self
            .lists
            .states
            .get_mut(node)
            .ok_or(RegisterError::NotBootstrap)?;
        if state.registering {
            return Err(RegisterError::InFlight);
        }
        state.forced = true;
        self.register_with_bootstrap();
        Ok(())
    }

    /// Registers with every bootstrap node that should hear from us: once
    /// at start, again before the registration lapses, and again if its
    /// list has not had us for a while (D4).
    pub(super) fn register_with_bootstrap(&mut self) {
        let Some(bytes) = self.own_link_bytes() else {
            return;
        };
        let now = self.now_ms;
        let (renew, recheck) = (
            self.config.registration_renew_ms,
            self.config.list_poll_interval_ms.saturating_mul(5),
        );
        let own = self.own;
        let due: Vec<NodeId> = self
            .lists
            .states
            .iter()
            .filter(|(id, state)| {
                **id != own
                    && !state.registering
                    && (state.forced
                        || state.register_at_ms <= now
                            && state.registered_ms.is_none_or(|at| {
                                at.saturating_add(renew) <= now
                                    || (state.ids.binary_search(&own).is_err()
                                        && at.saturating_add(recheck) <= now)
                            }))
            })
            .map(|(id, _)| *id)
            .collect();
        let Ok(signed) = SignedNodeInfo::decode_exact(&bytes) else {
            return;
        };
        let content = Content::Registration(signed);
        let timeout = self.config.request_timeout_ms;
        let (now, min, max) = (
            self.now_ms,
            self.config.min_backoff_ms,
            self.config.max_backoff_ms,
        );
        for target in due {
            let sent = self
                .send(target, &content, Purpose::Registration, timeout)
                .is_some();
            if let Some(state) = self.lists.states.get_mut(&target) {
                state.registering = sent;
                state.forced = false;
                if !sent {
                    state.register_failures = state.register_failures.saturating_add(1);
                    state.register_at_ms =
                        now.saturating_add(backoff(min, max, state.register_failures));
                }
            }
        }
    }

    pub(super) fn on_registered(&mut self, with: NodeId, result: Result<Vec<u8>, Problem>) {
        let (now, renew) = (self.now_ms, self.config.registration_renew_ms);
        let recheck = self.config.list_poll_interval_ms.saturating_mul(5);
        let (min, max) = (self.config.min_backoff_ms, self.config.max_backoff_ms);
        let Some(state) = self.lists.states.get_mut(&with) else {
            return;
        };
        state.registering = false;
        if result
            .and_then(|body| decode_expiry(&body).map_err(|_| Problem::Invalid))
            .is_ok()
        {
            state.registered_ms = Some(now);
            state.register_failures = 0;
            state.register_at_ms = now.saturating_add(recheck.min(renew));
            self.event(Event::RegisteredWith(with));
        } else {
            state.register_failures = state.register_failures.saturating_add(1);
            state.register_at_ms = now.saturating_add(backoff(min, max, state.register_failures));
        }
    }

    /// Our registration with each bootstrap node.
    #[must_use]
    pub fn registration_status(&self) -> Vec<RegistrationStatus> {
        self.lists
            .states
            .iter()
            .map(|(id, state)| RegistrationStatus {
                bootstrap: *id,
                listed: state.ids.binary_search(&self.own).is_ok(),
                registered_ms: state.registered_ms,
                list_fetched_ms: state.fetched_ms,
            })
            .collect()
    }

    /// The nodes on a majority of the bootstrap nodes' lists.
    #[must_use]
    pub const fn majority(&self) -> &BTreeSet<NodeId> {
        &self.lists.majority
    }

    /// Our own link state's bytes, once issued.
    pub(crate) fn own_link_bytes(&self) -> Option<Vec<u8>> {
        self.table
            .get(&self.own)
            .and_then(|entry| entry.link.as_ref())
            .map(|link| link.bytes.clone())
    }

    /// When registering next has something to do: only once we have a
    /// link state to register with.
    pub(crate) fn registration_due(&self) -> Option<u64> {
        self.own_link_bytes()?;
        self.lists
            .states
            .iter()
            .filter(|(id, state)| **id != self.own && !state.registering)
            .map(|(_, state)| {
                if state.forced {
                    return self.now_ms;
                }
                state.registered_ms.map_or(state.register_at_ms, |at| {
                    let renew = at.saturating_add(self.config.registration_renew_ms);
                    let listed = state.ids.binary_search(&self.own).is_ok();
                    let recheck =
                        at.saturating_add(self.config.list_poll_interval_ms.saturating_mul(5));
                    let due = if listed { renew } else { renew.min(recheck) };
                    due.max(state.register_at_ms)
                })
            })
            .min()
    }
}

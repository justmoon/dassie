//! Names (spec/areas/network.md#names). At a registrar, a node that is one
//! of its own bootstrap nodes: it logs commitments, takes their unsealings
//! and renewals, grants names as windows close, serves its name list, and
//! releases the names the other registrars' lists show split. The rule is
//! [`Registrar`]'s; here is who may ask what, what each answer is, the
//! replay state of the nodes a registrar involves, and the events. At every
//! node: polling the registrars' name lists, and taking from them who holds
//! each name, which is [`Holders`]'; and registering names for the node and
//! renewing them, which is [`Registrant`]'s: here they are sent, to our own
//! registrar too, if we are one.

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
use crate::names::registrant::{
    MAX_REGISTRATIONS, Message, NameRequest, Progress, RegisterError, RenewError as RenewNowError,
};
use crate::names::registrar::{
    CommitError, Committed, Happened, RenewError, UnsealError, Unsealed,
};
use crate::names::{
    Commitment, HeldName, HolderChange, Holders, HoldersConfig, Name, NameRegistration, NameStatus,
    Registrant, RegistrantConfig, Registrar, Salt,
};
use crate::table::Network;
use crate::wire::{
    Content, NameListResponse, NameReceipt, decode_expiry, decode_hash, encode_expiry,
};

/// What a node that is no registrar answers to a name message, as to a
/// registration.
const NOT_REGISTRAR: &str = "not a bootstrap node";

/// Most events a step of the registrar's timer queues, however many names
/// and commitments it decides: the queue is bounded and drops its oldest,
/// and the peer events queued before it in the same tick must reach the
/// driver.
const MAX_REGISTRAR_EVENTS: u64 = 1;

/// Most events a taking of the view queues, however many names change: one
/// for the view and one for a registrar's releases.
const MAX_HOLDERS_EVENTS: u64 = 2;

/// Most rounds of sending what our registrations have due in one step:
/// commitments, then, once our own registrar's receipt completed them,
/// unsealings, and their answers from our own registrar.
const SEND_ROUNDS: usize = 3;

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
        match self.commit_here(accepted.sender, commitment, network) {
            Ok(receipt) => ok(accepted, &receipt.to_vec()),
            Err(refusal) => refusal,
        }
    }

    /// Logs `commitment` from `sender` at our registrar: its receipt, or the
    /// answer that refuses it.
    fn commit_here(
        &mut self,
        sender: NodeId,
        commitment: Commitment,
        network: Option<Network>,
    ) -> Result<NameReceipt, Reply> {
        let (own, now) = (self.own, self.now_ms);
        let Some(registrar) = self.registrar.as_mut() else {
            return Err(Reply::refuse(409, NOT_REGISTRAR));
        };
        match registrar.commit(commitment, sender, network, now) {
            Ok(Committed::Again(receipt)) => Ok(receipt),
            Ok(Committed::New(receipt)) => {
                // Its unsealing must get through however many strangers
                // write to us, and its floor is persisted with the
                // commitment (spec/areas/peer-protocol.md#replay-protection).
                if sender != own {
                    self.auth.pin(sender);
                }
                self.stats.commitments_logged += 1;
                self.event(Event::Name(NameEvent::Committed {
                    committer: sender,
                    position: receipt.position,
                }));
                Ok(receipt)
            }
            Err(error) => Err(commit_refusal(error)),
        }
    }

    /// A `nameUnseal`: the sender opens its own commitment. A valid one is
    /// answered alike whether it contends for the name or has lost, so the
    /// answer tells nothing of commitments still sealed.
    pub(super) fn name_unseal(&mut self, accepted: &Accepted, name: Name, salt: &Salt) -> Reply {
        match self.unseal_here(accepted.sender, name, salt) {
            Ok(()) => ok(accepted, &[]),
            Err(refusal) => refusal,
        }
    }

    /// Takes `sender`'s unsealing at our registrar, or the answer that
    /// refuses it.
    fn unseal_here(&mut self, sender: NodeId, name: Name, salt: &Salt) -> Result<(), Reply> {
        let (own, now) = (self.own, self.now_ms);
        let Some(registrar) = self.registrar.as_mut() else {
            return Err(Reply::refuse(409, NOT_REGISTRAR));
        };
        match registrar.unseal(sender, name, salt, now) {
            Ok(Unsealed::Again) => {}
            Ok(Unsealed::New { contends }) => {
                // A copied commitment may have been logged by another node:
                // the holder has a part of its own now.
                if sender != own {
                    self.auth.pin(sender);
                }
                self.event(Event::Name(NameEvent::Unsealed {
                    name,
                    holder: sender,
                    contends,
                }));
            }
            Err(error) => return Err(Reply::refuse(409, unseal_refusal(error))),
        }
        Ok(())
    }

    /// A `nameRenew`, from the name's holder: answered with its expiry.
    pub(super) fn name_renew(&mut self, accepted: &Accepted, name: Name) -> Reply {
        match self.renew_here(accepted.sender, name) {
            Ok(expires_ms) => ok(accepted, &encode_expiry(expires_ms)),
            Err(refusal) => refusal,
        }
    }

    /// Renews `name` for `sender` at our registrar: its expiry, or the
    /// answer that refuses it.
    fn renew_here(&mut self, sender: NodeId, name: Name) -> Result<u64, Reply> {
        let now = self.now_ms;
        let Some(registrar) = self.registrar.as_mut() else {
            return Err(Reply::refuse(409, NOT_REGISTRAR));
        };
        let renewal = match registrar.renew(&sender, &name, now) {
            Ok(renewal) => renewal,
            Err(RenewError::NotHeld) => {
                return Err(Reply::refuse(409, "the name is not held here"));
            }
            Err(RenewError::NotHolder) => {
                return Err(Reply::refuse(409, "another node holds the name"));
            }
        };
        if renewal.extended {
            self.stats.names_renewed += 1;
            self.event(Event::Name(NameEvent::Renewed {
                name,
                holder: sender,
                expires_ms: renewal.expires_ms,
            }));
        }
        Ok(renewal.expires_ms)
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
        if !self.config.names_enabled {
            return;
        }
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
        let queued = self.events_queued();
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
        assert!(
            self.events_since(queued) <= MAX_HOLDERS_EVENTS,
            "a taking of the view queues a bounded number of events"
        );
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

    /// Our registrations of names, and the renewals of the names we hold.
    #[must_use]
    pub const fn registrant(&self) -> &Registrant {
        &self.registrant
    }

    /// Starts registering `name` for this node at every registrar
    /// (spec/areas/network.md#the-registrant): with a salt of its own,
    /// persisted with the next batch, before its first commitment leaves.
    ///
    /// # Errors
    ///
    /// The name is reserved, held or taken, there are no registrars, a
    /// registration of it or too many are under way, the node holds as many
    /// names as a registrar grants it, or the registrations are full.
    pub fn register_name(&mut self, now_ms: u64, name: Name) -> Result<(), RegisterError> {
        self.advance(now_ms);
        let mut salt = Salt([0; crate::names::SALT_BYTES]);
        self.rng.fill_bytes(&mut salt.0);
        self.registrant
            .register(name, salt, &self.holders, self.now_ms)?;
        self.stats.registrations_started += 1;
        self.event(Event::Name(NameEvent::Registering { name }));
        self.send_names();
        Ok(())
    }

    /// Renews `name`, which this node holds, at every registrar now.
    ///
    /// # Errors
    ///
    /// This node does not hold the name in its view.
    pub fn renew_name(&mut self, now_ms: u64, name: &Name) -> Result<(), RenewNowError> {
        self.advance(now_ms);
        self.registrant.renew_now(name, &self.holders)?;
        self.send_names();
        Ok(())
    }

    /// How each name of ours stands at `now_ms`: those we register, and
    /// those we hold.
    #[must_use]
    pub fn name_statuses(&self, now_ms: u64) -> Vec<NameStatus> {
        self.registrant
            .statuses(&self.holders, self.now_ms.max(now_ms))
    }

    /// Sends what our registrations and renewals have due. Our own
    /// registrar, if we are one, answers at once, which may make more due.
    ///
    /// Each round queues at most an event for each registration, as time
    /// moves it on, and two for each request our own registrar answers, a
    /// registration's or a renewal of a name we hold, its own and our
    /// progress's: a bounded number, however many names the registrars
    /// list.
    pub(super) fn send_names(&mut self) {
        let (own, queued) = (self.own, self.events_queued());
        let per_round = 3 * MAX_REGISTRATIONS + 2 * self.registrant.config().max_names;
        let most = u64::try_from(SEND_ROUNDS * per_round).unwrap_or(u64::MAX);
        for _ in 0..SEND_ROUNDS {
            let (mut progress, mut requests) = (Vec::new(), Vec::new());
            self.registrant
                .requests(&self.holders, self.now_ms, &mut progress, &mut requests);
            self.progress(progress);
            let local = requests.iter().any(|request| request.to == own);
            for request in requests {
                self.send_name(request);
            }
            if !local {
                break;
            }
        }
        assert!(
            self.events_since(queued) <= most,
            "sending what our registrations have due queues a bounded number of events"
        );
    }

    /// Sends `request` to its registrar, or asks our own.
    fn send_name(&mut self, request: NameRequest) {
        if request.to == self.own {
            self.ask_own_registrar(request.message);
            return;
        }
        let (content, purpose) = match request.message {
            Message::Commit(commitment) => (
                Content::NameCommit(commitment),
                Purpose::NameCommit(commitment),
            ),
            Message::Unseal {
                commitment,
                name,
                salt,
            } => (
                Content::NameUnseal { name, salt },
                Purpose::NameUnseal(commitment),
            ),
            Message::Renew(name) => {
                self.stats.renewals_sent += 1;
                (Content::NameRenew(name), Purpose::NameRenew(name))
            }
        };
        let timeout = self.config.request_timeout_ms;
        if self.send(request.to, &content, purpose, timeout).is_none() {
            self.registrant.not_sent(&request, self.now_ms);
        }
    }

    /// Takes `message` at our own registrar, as a node that registers with
    /// itself: a commitment comes from no network.
    fn ask_own_registrar(&mut self, message: Message) {
        let (own, now) = (self.own, self.now_ms);
        let refused = |reply: Reply| Some(reply.status);
        let mut progress = Vec::new();
        match message {
            Message::Commit(commitment) => {
                let result = self.commit_here(own, commitment, None).map_err(refused);
                self.registrant
                    .on_commit(&own, &commitment, result, now, &mut progress);
            }
            Message::Unseal {
                commitment,
                name,
                salt,
            } => {
                let result = self.unseal_here(own, name, &salt).map_err(refused);
                self.registrant
                    .on_unseal(&own, &commitment, result, now, &mut progress);
            }
            Message::Renew(name) => {
                self.stats.renewals_sent += 1;
                let result = self.renew_here(own, name).map_err(refused);
                self.registrant
                    .on_renew(&own, &name, result, now, &mut progress);
            }
        }
        self.progress(progress);
    }

    /// A registrar's answer to our commitment: its receipt.
    pub(super) fn on_name_commit(
        &mut self,
        from: NodeId,
        commitment: &Commitment,
        result: Result<Vec<u8>, Problem>,
    ) {
        let result = result.map_err(refusal).and_then(|body| {
            NameReceipt::decode_exact(&body).map_err(|_| refusal(Problem::Invalid))
        });
        let mut progress = Vec::new();
        self.registrant
            .on_commit(&from, commitment, result, self.now_ms, &mut progress);
        self.progress(progress);
        self.send_names();
    }

    /// A registrar's answer to our unsealing: empty.
    pub(super) fn on_name_unseal(
        &mut self,
        from: NodeId,
        commitment: &Commitment,
        result: Result<Vec<u8>, Problem>,
    ) {
        let result = result.map(|_| ()).map_err(refusal);
        let mut progress = Vec::new();
        self.registrant
            .on_unseal(&from, commitment, result, self.now_ms, &mut progress);
        self.progress(progress);
    }

    /// A registrar's answer to our renewal: the name's expiry there.
    pub(super) fn on_name_renew(
        &mut self,
        from: NodeId,
        name: &Name,
        result: Result<Vec<u8>, Problem>,
    ) {
        let result = result
            .map_err(refusal)
            .and_then(|body| decode_expiry(&body).map_err(|_| refusal(Problem::Invalid)));
        let mut progress = Vec::new();
        self.registrant
            .on_renew(&from, name, result, self.now_ms, &mut progress);
        self.progress(progress);
    }

    /// Reports what our registrations and renewals did.
    fn progress(&mut self, progress: Vec<Progress>) {
        for step in progress {
            let event = match step {
                Progress::Receipt {
                    name,
                    registrar,
                    position,
                } => NameEvent::Receipt {
                    name,
                    registrar,
                    position,
                },
                Progress::Unsealing { name } => NameEvent::Unsealing { name },
                Progress::UnsealedAt { name, registrar } => {
                    NameEvent::UnsealedAt { name, registrar }
                }
                Progress::Refused {
                    name,
                    registrar,
                    status,
                } => NameEvent::Refused {
                    name,
                    registrar,
                    status,
                },
                Progress::Abandoned { name } => {
                    self.stats.registrations_abandoned += 1;
                    NameEvent::Abandoned { name }
                }
                Progress::RenewedAt {
                    name,
                    registrar,
                    expires_ms,
                } => NameEvent::RenewedAt {
                    name,
                    registrar,
                    expires_ms,
                },
            };
            self.event(Event::Name(event));
        }
    }

    /// The view's invariants, the registrar's, and that every node it
    /// involves keeps its replay state.
    pub(super) fn check_names(&self) {
        self.holders.check_invariants();
        self.registrant.check_invariants();
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

/// The registrars: the bootstrap nodes, or none while names are off.
fn registrars<'a>(config: &Config, bootstrap: &'a [Bootstrap]) -> &'a [Bootstrap] {
    if config.names_enabled { bootstrap } else { &[] }
}

/// A view of the lists of the registrars, the bootstrap nodes, starting from
/// `held`, as persisted (spec/areas/network.md#holders). Lists are fresh as
/// long as node lists are, and vote only while they are. While names are
/// off it has no registrars, holds nothing, and removes what was persisted.
pub(super) fn holders(config: &Config, bootstrap: &[Bootstrap], held: Vec<HeldName>) -> Holders {
    let lists = HoldersConfig {
        list_fresh_ms: config.list_fresh_ms,
    };
    let registrars = registrars(config, bootstrap);
    Holders::new(lists, registrars.iter().map(|node| node.id), held)
}

/// Our registrant, at the bootstrap nodes, from the registrations persisted.
/// It foresees the registrars by our own configuration of one, which every
/// node shares (spec/areas/network.md#the-registrant). While names are off
/// it has no registrars and so registers nothing, and what was persisted
/// stays on disk, unread.
pub(super) fn registrant(
    config: &Config,
    own: NodeId,
    bootstrap: &[Bootstrap],
    registrations: Vec<NameRegistration>,
) -> Registrant {
    let (bootstrap, registrations) = if config.names_enabled {
        (bootstrap, registrations)
    } else {
        (&[][..], Vec::new())
    };
    let names = &config.names;
    let registrant = RegistrantConfig {
        window_ms: names.window_ms,
        request_timeout_ms: config.request_timeout_ms,
        min_backoff_ms: config.min_backoff_ms,
        max_backoff_ms: config.max_backoff_ms,
        renew_before_ms: names.renew_before_ms,
        grace_ms: names.grace_ms,
        release_after_ms: names.release_after_ms,
        max_under_way: names.max_commitments_per_node,
        max_names: names.max_names_per_node,
    };
    Registrant::new(
        registrant,
        own,
        bootstrap.iter().map(|node| node.id),
        registrations,
    )
}

/// The status a registrar refused one of our requests with, if it answered
/// with one.
const fn refusal(problem: Problem) -> Option<u16> {
    match problem {
        Problem::Status(status) => Some(status),
        Problem::Failed(_) | Problem::Invalid => None,
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

    // A node with names off polls no registrar's name list, nor takes its
    // own: its view has no registrars (spec/areas/network.md#names).
    #[test]
    fn a_node_with_names_off_reads_no_name_lists() {
        for names_enabled in [true, false] {
            let config = Config {
                names_enabled,
                ..Config::DEV
            };
            let mut network = Network::new(34, 3, 2, config);
            let mut polled = false;
            for _ in 0..300 {
                network.run_until(network.now + 100);
                polled |= (0..3).any(|index| !network.node(index).name_polling.is_empty());
            }
            assert_eq!(polled, names_enabled);
            for index in 0..3 {
                let registrars = network.node(index).holders().registrars();
                assert_eq!(registrars, if names_enabled { 2 } else { 0 });
            }
        }
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

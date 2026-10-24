//! A registrant's side of names (spec/designs/names.md#3-registering,
//! spec/areas/network.md#the-registrant): registering a name for this node
//! at every registrar, and renewing the names it holds.
//!
//! A registration is:
//!
//! 1. **Started** with a salt from the node's RNG, persisted before its
//!    first commitment leaves, so that a restart commits to the same
//!    commitment again and loses nothing a registrar logged.
//! 2. **Committed** at every registrar at once, each retried with a backoff
//!    until it answers with its receipt. The window is counted from when
//!    the registration started, which is no later than any registrar logged
//!    it, less a margin for clocks that run at different rates: a
//!    registration that does not hold every receipt while an unsealing still
//!    has a request's time left is abandoned, and its name told to no one.
//! 3. **Unsealed** at every registrar once it holds every receipt, each
//!    retried until it is answered or the window closes. A registrar that
//!    refuses an unsealing does not count for the name.
//! 4. **Decided** by the node's view of who holds the name
//!    ([`Holders`]): held, lost to another node, split between holders,
//!    or not held once a split would have been released. A registration
//!    whose name the view gives this node is done, and the name is renewed
//!    from then on.
//!
//! A name this node holds is renewed at every registrar that does not list
//! another node for it, once the expiry that registrar last gave it is
//! within the time to renew: the one its list gives, or its answer to a
//! renewal. A name is this node's while its view gives it the name, or
//! more than f fresh lists name it, so at least one honest registrar; and
//! for the registrars' grace period after that, so that a registrar whose
//! renewal came late is renewed in its grace period even after the view
//! lost the quorum. The owner may also renew at once. Nothing here is sent: the requests are
//! the caller's to send, and their answers come back here.
//!
//! The registrant predicts its registrars by its own node's configuration
//! of a registrar ([`RegistrarConfig`](super::RegistrarConfig)): every node
//! runs the same.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::holders::faults_tolerated;
use super::{Commitment, Holders, Name, Salt};
use crate::persist::PeerBatch;
use crate::table::backoff;
use crate::wire::{NameHolding, NameReceipt};

/// Most registrations kept, under way or finished: a finished one is shown
/// to the owner until room is needed for another.
pub const MAX_REGISTRATIONS: usize = 16;

/// The share of a window, in percent, that a registrant keeps back for its
/// clock and a registrar's running at different rates.
pub const CLOCK_RATE_PERCENT: u64 = 1;

/// A registrant's timers and bounds, from its node's configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrantConfig {
    /// How long after it is logged a commitment may be unsealed, at every
    /// registrar.
    pub window_ms: u64,
    /// How long a request may take.
    pub request_timeout_ms: u64,
    /// The shortest backoff after a request failed.
    pub min_backoff_ms: u64,
    /// The longest backoff.
    pub max_backoff_ms: u64,
    /// How long before its expiry a name is renewed.
    pub renew_before_ms: u64,
    /// How long after its expiry a registrar keeps a name for its holder
    /// alone, and so renews it.
    pub grace_ms: u64,
    /// How long after its window a registration that the lists show split
    /// could still be: a split is released by then.
    pub release_after_ms: u64,
    /// Most registrations under way at once: as many as a registrar logs
    /// from one node.
    pub max_under_way: usize,
    /// Most names renewed: as many as a registrar grants one node. An
    /// honest quorum of registrars never gives a node more.
    pub max_names: usize,
}

impl RegistrantConfig {
    /// The margin a registrant keeps back from the window for clock rates.
    #[must_use]
    pub const fn clock_margin_ms(&self) -> u64 {
        self.window_ms / 100 * CLOCK_RATE_PERCENT
    }

    /// Until when a registration started at `started_ms` may begin to
    /// unseal: an unsealing sent then still has a request's time before the
    /// earliest any registrar's window closes.
    #[must_use]
    pub const fn unseal_by_ms(&self, started_ms: u64) -> u64 {
        self.unseal_until_ms(started_ms)
            .saturating_sub(self.request_timeout_ms)
    }

    /// Until when an unsealing of a registration started at `started_ms`
    /// is sent: later, every registrar's window may have closed.
    #[must_use]
    pub const fn unseal_until_ms(&self, started_ms: u64) -> u64 {
        started_ms
            .saturating_add(self.window_ms)
            .saturating_sub(self.clock_margin_ms())
    }

    /// When a registration started at `started_ms` that the view does not
    /// show held has failed: every registrar decided it a window after it
    /// started at the latest, and a split would have been released.
    #[must_use]
    pub const fn settled_ms(&self, started_ms: u64) -> u64 {
        started_ms
            .saturating_add(self.window_ms)
            .saturating_add(self.release_after_ms)
    }
}

/// How far a registration has come, as persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Committing at every registrar, until it holds every receipt.
    Committing,
    /// It held every receipt in time: unsealing at every registrar until the
    /// window closes, then waiting for the lists.
    Unsealing,
    /// It did not hold every receipt in time to unseal: no registrar was
    /// told the name.
    Abandoned,
}

/// A registration of a name for this node, as persisted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRegistration {
    /// The name.
    pub name: Name,
    /// The salt its commitment is sealed with.
    pub salt: Salt,
    /// When it started, by this node's clock: no later than its first
    /// commitment left, so no later than any registrar logged it.
    pub started_ms: u64,
    /// How far it has come.
    pub stage: Stage,
}

/// How a registration stands with one registrar, in this run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Attempt {
    receipt: Option<NameReceipt>,
    unsealed: bool,
    /// The status of its last refusal.
    refused: Option<u16>,
    in_flight: bool,
    failures: u32,
    next_ms: u64,
}

impl Attempt {
    /// Whether nothing more is sent to this registrar for the unsealing: it
    /// took it, or refused it for good.
    const fn unseal_done(&self) -> bool {
        self.unsealed || matches!(self.refused, Some(409))
    }

    fn failed(&mut self, status: Option<u16>, now_ms: u64, config: &RegistrantConfig) {
        self.in_flight = false;
        if status.is_some() {
            self.refused = status;
        }
        self.failures = self.failures.saturating_add(1);
        self.next_ms = now_ms.saturating_add(backoff(
            config.min_backoff_ms,
            config.max_backoff_ms,
            self.failures,
        ));
    }
}

/// A registration, with how it stands with each registrar.
#[derive(Clone, Debug)]
struct Entry {
    registration: NameRegistration,
    commitment: Commitment,
    at: BTreeMap<NodeId, Attempt>,
}

/// How a renewal of a name stands with one registrar, in this run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Renewing {
    /// The latest expiry the registrar gave the name for us: in its list,
    /// or in its answer to a renewal.
    expires_ms: Option<u64>,
    /// The status of its last refusal.
    refused: Option<u16>,
    in_flight: bool,
    /// The owner asked to renew now.
    forced: bool,
    failures: u32,
    next_ms: u64,
}

/// A message for a registrar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// A `nameCommit`.
    Commit(Commitment),
    /// A `nameUnseal` of the commitment.
    Unseal {
        /// The commitment it opens.
        commitment: Commitment,
        /// The name.
        name: Name,
        /// The salt.
        salt: Salt,
    },
    /// A `nameRenew`.
    Renew(Name),
}

/// A message to send to a registrar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameRequest {
    /// The registrar.
    pub to: NodeId,
    /// The message.
    pub message: Message,
}

/// What a registration or a renewal did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// A registrar logged our commitment.
    Receipt {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
        /// The commitment's position in its log.
        position: u64,
    },
    /// Every registrar logged our commitment in time: we unseal it at each.
    Unsealing {
        /// The name.
        name: Name,
    },
    /// A registrar took our unsealing.
    UnsealedAt {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
    },
    /// A registrar refused a commitment, an unsealing or a renewal of ours.
    Refused {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
        /// Its status.
        status: u16,
    },
    /// Not every registrar logged our commitment in time to unseal it: the
    /// registration ends, and no registrar was told the name.
    Abandoned {
        /// The name.
        name: Name,
    },
    /// A registrar answered our renewal.
    RenewedAt {
        /// The name.
        name: Name,
        /// The registrar.
        registrar: NodeId,
        /// The expiry it gives the name now, by its clock.
        expires_ms: u64,
    },
}

/// Why a registration is not started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterError {
    /// The name is reserved: no registrar grants it.
    Reserved,
    /// No registrars are configured.
    NoRegistrars,
    /// This node holds the name already.
    Held,
    /// A fresh list names it, or the view gives it a holder.
    Taken,
    /// A registration of it is under way.
    UnderWay,
    /// As many registrations are under way as a registrar logs from one
    /// node.
    TooMany,
    /// The registrations are full, and none has finished.
    Full,
}

/// Why a renewal is not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenewError {
    /// This node does not hold the name in its view.
    NotHeld,
}

/// How a name of ours stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Committing at every registrar.
    Committing,
    /// Unsealing at every registrar, within the window.
    Unsealing,
    /// Unsealed: waiting for the registrars to grant it, and for their
    /// lists.
    Waiting,
    /// This node holds it, in its view.
    Held,
    /// This node held it, and no quorum of the lists names it now: it is
    /// renewed while the registrars keep it in their grace periods.
    Lapsing,
    /// Another node holds it, in our view.
    Lost,
    /// The fresh lists name this node and another for it: neither holds it
    /// until the registrars release the split.
    Split,
    /// No holder, long after its window: it was not granted.
    NotHeld,
    /// Not every registrar logged its commitment in time.
    Abandoned,
}

/// How a name of ours stands with one registrar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtRegistrar {
    /// The registrar.
    pub registrar: NodeId,
    /// Whether it logged our commitment, in this run or before.
    pub receipt: bool,
    /// Whether it took our unsealing, in this run.
    pub unsealed: bool,
    /// The status of its last refusal of ours, if it refused.
    pub refused: Option<u16>,
    /// What its list, as last fetched, says of the name.
    pub listed: Option<NameHolding>,
    /// Whether that list is fresh.
    pub fresh: bool,
}

/// How a name of ours stands: one we register, or one we hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameStatus {
    /// The name.
    pub name: Name,
    /// How it stands.
    pub state: State,
    /// Its holder in our view, if it has one.
    pub holder: Option<NodeId>,
    /// When it expires, in our view, by the registrars' clocks.
    pub expires_ms: Option<u64>,
    /// Until when its registration may begin to unseal, while it commits.
    pub unseal_by_ms: Option<u64>,
    /// How it stands with each registrar.
    pub registrars: Vec<AtRegistrar>,
}

/// A registrant's state: its registrations and its renewals.
pub struct Registrant {
    config: RegistrantConfig,
    own: NodeId,
    registrars: Vec<NodeId>,
    entries: BTreeMap<Name, Entry>,
    /// The names this node holds, or held lately, with the latest expiry
    /// that the view, or more than f fresh lists, gave each.
    owned: BTreeMap<Name, u64>,
    renewals: BTreeMap<(Name, NodeId), Renewing>,
    dirty: BTreeSet<Name>,
    removed: Vec<Name>,
}

impl Registrant {
    /// A registrant for `own`, at `registrars`, the configured ones, from
    /// the registrations persisted. What is beyond the bound is left out:
    /// the first by name are kept.
    #[must_use]
    pub fn new(
        config: RegistrantConfig,
        own: NodeId,
        registrars: impl IntoIterator<Item = NodeId>,
        stored: Vec<NameRegistration>,
    ) -> Self {
        let mut registrars: Vec<NodeId> = registrars.into_iter().collect();
        registrars.sort_unstable();
        registrars.dedup();
        let mut registrant = Self {
            config,
            own,
            registrars,
            entries: BTreeMap::new(),
            owned: BTreeMap::new(),
            renewals: BTreeMap::new(),
            dirty: BTreeSet::new(),
            removed: Vec::new(),
        };
        for registration in stored.into_iter().take(MAX_REGISTRATIONS) {
            let entry = registrant.entry(registration);
            registrant.entries.insert(registration.name, entry);
        }
        registrant
    }

    fn entry(&self, registration: NameRegistration) -> Entry {
        Entry {
            commitment: Commitment::seal(&registration.name, &self.own, &registration.salt),
            at: self
                .registrars
                .iter()
                .map(|id| (*id, Attempt::default()))
                .collect(),
            registration,
        }
    }

    /// The configuration.
    #[must_use]
    pub const fn config(&self) -> &RegistrantConfig {
        &self.config
    }

    /// The registrations, by name.
    pub fn registrations(&self) -> impl Iterator<Item = &NameRegistration> {
        self.entries.values().map(|entry| &entry.registration)
    }

    /// Whether a registration is still under way: committing, or unsealing
    /// within its window.
    fn is_under_way(&self, registration: &NameRegistration, now_ms: u64) -> bool {
        match registration.stage {
            Stage::Committing => true,
            Stage::Unsealing => now_ms < self.config.unseal_until_ms(registration.started_ms),
            Stage::Abandoned => false,
        }
    }

    /// Starts registering `name` with `salt`, which the caller draws from
    /// its RNG, at `now_ms`. A finished registration of the name gives way,
    /// and so does the oldest finished one when the registrations are full.
    ///
    /// # Errors
    ///
    /// The name is reserved, there are no registrars, the name is held or
    /// taken, a registration of it or too many are under way, or the
    /// registrations are full.
    pub fn register(
        &mut self,
        name: Name,
        salt: Salt,
        holders: &Holders,
        now_ms: u64,
    ) -> Result<(), RegisterError> {
        if name.is_reserved() {
            return Err(RegisterError::Reserved);
        }
        if self.registrars.is_empty() {
            return Err(RegisterError::NoRegistrars);
        }
        if self.owned.contains_key(&name) {
            return Err(RegisterError::Held);
        }
        if let Some(held) = holders.holder(&name) {
            return Err(if held.holder == self.own {
                RegisterError::Held
            } else {
                RegisterError::Taken
            });
        }
        // A commitment logged while the name is listed is void there
        // (spec/areas/network.md#the-registrar), whoever is listed.
        let listed = self
            .registrars
            .iter()
            .any(|id| holders.is_fresh_list(id, now_ms) && holders.listing(id, &name).is_some());
        if listed {
            return Err(RegisterError::Taken);
        }
        if let Some(entry) = self.entries.get(&name) {
            if self.is_under_way(&entry.registration, now_ms) {
                return Err(RegisterError::UnderWay);
            }
        } else {
            self.make_room(now_ms)?;
        }
        let under_way = self
            .entries
            .values()
            .filter(|entry| self.is_under_way(&entry.registration, now_ms))
            .count();
        if under_way >= self.config.max_under_way {
            return Err(RegisterError::TooMany);
        }
        let registration = NameRegistration {
            name,
            salt,
            started_ms: now_ms,
            stage: Stage::Committing,
        };
        let entry = self.entry(registration);
        self.entries.insert(name, entry);
        self.removed.retain(|removed| *removed != name);
        self.dirty.insert(name);
        Ok(())
    }

    /// Makes room for one more registration, dropping the oldest finished
    /// one if the registrations are full.
    fn make_room(&mut self, now_ms: u64) -> Result<(), RegisterError> {
        if self.entries.len() < MAX_REGISTRATIONS {
            return Ok(());
        }
        let oldest = self
            .entries
            .values()
            .filter(|entry| !self.is_under_way(&entry.registration, now_ms))
            .min_by_key(|entry| (entry.registration.started_ms, entry.registration.name))
            .map(|entry| entry.registration.name)
            .ok_or(RegisterError::Full)?;
        self.forget(oldest);
        Ok(())
    }

    fn forget(&mut self, name: Name) {
        if self.entries.remove(&name).is_some() {
            self.dirty.remove(&name);
            self.removed.push(name);
        }
    }

    /// Takes in which names this node holds: those the view gives it, and
    /// those more than f fresh lists name it for, so at least one honest
    /// registrar. A name without either stays only from its latest expiry
    /// until the registrars' grace periods after it are over, so that a
    /// registrar whose renewal came late is renewed in its grace period
    /// after the view lost the quorum; one that lost both before it
    /// expired was released, or lost, and is ours no more. At most
    /// [`RegistrantConfig::max_names`], those the view gives first. What
    /// each registrar lists for us is noted for its renewal.
    fn refresh_owned(&mut self, holders: &Holders, now_ms: u64) {
        let own = self.own;
        let needed = faults_tolerated(holders.registrars()).saturating_add(1);
        let candidates: Vec<Name> = holders
            .held()
            .filter(|held| held.holder == own)
            .map(|held| held.name)
            .chain(self.owned.keys().copied())
            .chain(self.entries.keys().copied())
            .collect();
        let mut shown = BTreeSet::new();
        for name in candidates {
            let viewed = holders
                .holder(&name)
                .filter(|held| held.holder == own)
                .map(|held| held.expires_ms);
            let mut listed: Vec<u64> = self
                .registrars
                .iter()
                .filter(|registrar| holders.is_fresh_list(registrar, now_ms))
                .filter_map(|registrar| holders.listing(registrar, &name))
                .filter(|listing| listing.holder == own)
                .map(|listing| listing.expires_at_ms)
                .collect();
            listed.sort_unstable_by(|a, b| b.cmp(a));
            let listed = listed.get(needed.saturating_sub(1)).copied();
            let Some(expires_ms) = viewed.max(listed) else {
                continue;
            };
            shown.insert(name);
            if let Some(owned) = self.owned.get_mut(&name) {
                *owned = (*owned).max(expires_ms);
            } else if self.owned.len() < self.config.max_names {
                self.owned.insert(name, expires_ms);
            }
        }
        let grace = self.config.grace_ms;
        self.owned.retain(|name, expires_ms| {
            let in_grace = *expires_ms <= now_ms && now_ms < expires_ms.saturating_add(grace);
            shown.contains(name) || in_grace
        });
        let owned: Vec<Name> = self.owned.keys().copied().collect();
        for name in owned {
            for registrar in &self.registrars {
                let Some(listing) = holders.listing(registrar, &name) else {
                    continue;
                };
                if listing.holder == own {
                    let renewing = self.renewals.entry((name, *registrar)).or_default();
                    let known = renewing.expires_ms.unwrap_or(0);
                    renewing.expires_ms = Some(known.max(listing.expires_at_ms));
                }
            }
        }
        let owned = &self.owned;
        self.renewals
            .retain(|(name, _), _| owned.contains_key(name));
    }

    /// The expiry `registrar` gave `name` for this node, which it renews by:
    /// the latest it listed or answered; or, if it never did, the one the
    /// view or the lists gave. None if its list names another node, or the
    /// name is not ours.
    fn renewal_expiry(&self, holders: &Holders, name: &Name, registrar: &NodeId) -> Option<u64> {
        let owned = *self.owned.get(name)?;
        if holders
            .listing(registrar, name)
            .is_some_and(|listing| listing.holder != self.own)
        {
            return None;
        }
        let known = self
            .renewals
            .get(&(*name, *registrar))
            .and_then(|renewing| renewing.expires_ms);
        Some(known.unwrap_or(owned))
    }

    /// Renews `name` at every registrar now, whatever its expiry. A
    /// registrar renews only once the expiry is near, and answers an early
    /// renewal with the expiry it has.
    ///
    /// # Errors
    ///
    /// This node does not hold the name.
    pub fn renew_now(&mut self, name: &Name, holders: &Holders) -> Result<(), RenewError> {
        if !self.owned.contains_key(name) {
            return Err(RenewError::NotHeld);
        }
        for registrar in self.registrars.clone() {
            if self.renewal_expiry(holders, name, &registrar).is_some() {
                let renewing = self.renewals.entry((*name, registrar)).or_default();
                renewing.forced = true;
            }
        }
        Ok(())
    }

    /// The messages due at `now_ms`, each marked in flight: commitments not
    /// yet logged, unsealings once every receipt is held, and renewals.
    /// Takes in what time did first.
    pub fn requests(
        &mut self,
        holders: &Holders,
        now_ms: u64,
        progress: &mut Vec<Progress>,
        out: &mut Vec<NameRequest>,
    ) {
        self.on_time(holders, now_ms, progress);
        let config = self.config;
        for entry in self.entries.values_mut() {
            let registration = entry.registration;
            let unseal_until = config.unseal_until_ms(registration.started_ms);
            for (registrar, attempt) in &mut entry.at {
                if attempt.in_flight || attempt.next_ms > now_ms {
                    continue;
                }
                let message = match registration.stage {
                    Stage::Committing if attempt.receipt.is_none() => {
                        Message::Commit(entry.commitment)
                    }
                    Stage::Unsealing if !attempt.unseal_done() && now_ms < unseal_until => {
                        Message::Unseal {
                            commitment: entry.commitment,
                            name: registration.name,
                            salt: registration.salt,
                        }
                    }
                    _ => continue,
                };
                attempt.in_flight = true;
                out.push(NameRequest {
                    to: *registrar,
                    message,
                });
            }
        }
        self.renewal_requests(holders, now_ms, out);
    }

    fn renewal_requests(&mut self, holders: &Holders, now_ms: u64, out: &mut Vec<NameRequest>) {
        let owned: Vec<Name> = self.owned.keys().copied().collect();
        for name in owned {
            for registrar in self.registrars.clone() {
                let Some(expires_ms) = self.renewal_expiry(holders, &name, &registrar) else {
                    continue;
                };
                let due_ms = expires_ms.saturating_sub(self.config.renew_before_ms);
                let renewing = self.renewals.entry((name, registrar)).or_default();
                let due = renewing.forced || (due_ms <= now_ms && renewing.next_ms <= now_ms);
                if renewing.in_flight || !due {
                    continue;
                }
                renewing.in_flight = true;
                renewing.forced = false;
                out.push(NameRequest {
                    to: registrar,
                    message: Message::Renew(name),
                });
            }
        }
    }

    /// `request` could not be sent: it is tried again after a backoff.
    pub fn not_sent(&mut self, request: &NameRequest, now_ms: u64) {
        let config = self.config;
        match request.message {
            Message::Commit(commitment) | Message::Unseal { commitment, .. } => {
                if let Some(attempt) = self.attempt(&commitment, &request.to) {
                    attempt.failed(None, now_ms, &config);
                }
            }
            Message::Renew(name) => {
                if let Some(renewing) = self.renewals.get_mut(&(name, request.to)) {
                    renewing.in_flight = false;
                    renewing.failures = renewing.failures.saturating_add(1);
                    renewing.next_ms = now_ms.saturating_add(backoff(
                        config.min_backoff_ms,
                        config.max_backoff_ms,
                        renewing.failures,
                    ));
                }
            }
        }
    }

    fn attempt(&mut self, commitment: &Commitment, registrar: &NodeId) -> Option<&mut Attempt> {
        self.entries
            .values_mut()
            .find(|entry| entry.commitment == *commitment)
            .and_then(|entry| entry.at.get_mut(registrar))
    }

    fn name_of(&self, commitment: &Commitment) -> Option<Name> {
        self.entries
            .values()
            .find(|entry| entry.commitment == *commitment)
            .map(|entry| entry.registration.name)
    }

    /// The answer to our commitment from `registrar`: its receipt, or the
    /// status it refused with, if any.
    pub fn on_commit(
        &mut self,
        registrar: &NodeId,
        commitment: &Commitment,
        result: Result<NameReceipt, Option<u16>>,
        now_ms: u64,
        progress: &mut Vec<Progress>,
    ) {
        let config = self.config;
        let Some(name) = self.name_of(commitment) else {
            return;
        };
        let Some(attempt) = self.attempt(commitment, registrar) else {
            return;
        };
        match result {
            Ok(receipt) => {
                *attempt = Attempt {
                    receipt: Some(receipt),
                    ..Attempt::default()
                };
                progress.push(Progress::Receipt {
                    name,
                    registrar: *registrar,
                    position: receipt.position,
                });
            }
            Err(status) => {
                attempt.failed(status, now_ms, &config);
                if let Some(status) = status {
                    progress.push(Progress::Refused {
                        name,
                        registrar: *registrar,
                        status,
                    });
                }
            }
        }
    }

    /// The answer to our unsealing from `registrar`: taken, or the status
    /// it refused with, if any. A 409 is final: the registrar does not
    /// count for the name.
    pub fn on_unseal(
        &mut self,
        registrar: &NodeId,
        commitment: &Commitment,
        result: Result<(), Option<u16>>,
        now_ms: u64,
        progress: &mut Vec<Progress>,
    ) {
        let config = self.config;
        let Some(name) = self.name_of(commitment) else {
            return;
        };
        let Some(attempt) = self.attempt(commitment, registrar) else {
            return;
        };
        match result {
            Ok(()) => {
                attempt.in_flight = false;
                attempt.unsealed = true;
                progress.push(Progress::UnsealedAt {
                    name,
                    registrar: *registrar,
                });
            }
            Err(status) => {
                attempt.failed(status, now_ms, &config);
                if let Some(status) = status {
                    progress.push(Progress::Refused {
                        name,
                        registrar: *registrar,
                        status,
                    });
                }
            }
        }
    }

    /// The answer to our renewal of `name` from `registrar`: the expiry it
    /// gives the name now, or the status it refused with, if any. An answer
    /// that leaves the name due, which a registrar whose clock is behind
    /// ours gives, is tried again after a backoff.
    pub fn on_renew(
        &mut self,
        registrar: &NodeId,
        name: &Name,
        result: Result<u64, Option<u16>>,
        now_ms: u64,
        progress: &mut Vec<Progress>,
    ) {
        let config = self.config;
        let Some(renewing) = self.renewals.get_mut(&(*name, *registrar)) else {
            return;
        };
        renewing.in_flight = false;
        let due = match result {
            Ok(expires_ms) => {
                let known = renewing.expires_ms.unwrap_or(0);
                renewing.expires_ms = Some(known.max(expires_ms));
                renewing.refused = None;
                progress.push(Progress::RenewedAt {
                    name: *name,
                    registrar: *registrar,
                    expires_ms,
                });
                expires_ms.saturating_sub(config.renew_before_ms) <= now_ms
            }
            Err(status) => {
                if let Some(status) = status {
                    renewing.refused = Some(status);
                    progress.push(Progress::Refused {
                        name: *name,
                        registrar: *registrar,
                        status,
                    });
                }
                true
            }
        };
        if due {
            renewing.failures = renewing.failures.saturating_add(1);
            renewing.next_ms = now_ms.saturating_add(backoff(
                config.min_backoff_ms,
                config.max_backoff_ms,
                renewing.failures,
            ));
        } else {
            renewing.failures = 0;
            renewing.next_ms = 0;
        }
    }

    /// Takes in the passing of time and the view: a registration that holds
    /// every receipt in time unseals, one that cannot is abandoned, one
    /// whose name the view gives this node is done, and the names this node
    /// holds, and so renews, are taken in.
    pub fn on_time(&mut self, holders: &Holders, now_ms: u64, progress: &mut Vec<Progress>) {
        let config = self.config;
        let own = self.own;
        let mut done = Vec::new();
        for (name, entry) in &mut self.entries {
            let registration = &mut entry.registration;
            match registration.stage {
                Stage::Committing => {
                    let all = entry.at.values().all(|attempt| attempt.receipt.is_some());
                    if now_ms >= config.unseal_by_ms(registration.started_ms) {
                        registration.stage = Stage::Abandoned;
                        progress.push(Progress::Abandoned { name: *name });
                    } else if all {
                        registration.stage = Stage::Unsealing;
                        for attempt in entry.at.values_mut() {
                            attempt.failures = 0;
                            attempt.next_ms = 0;
                            attempt.refused = None;
                        }
                        progress.push(Progress::Unsealing { name: *name });
                    } else {
                        continue;
                    }
                    self.dirty.insert(*name);
                }
                Stage::Unsealing => {
                    if holders.holder(name).is_some_and(|held| held.holder == own) {
                        done.push(*name);
                    }
                }
                Stage::Abandoned => {}
            }
        }
        for name in done {
            self.forget(name);
        }
        self.refresh_owned(holders, now_ms);
    }

    /// When [`requests`](Self::requests) next has something to do.
    #[must_use]
    pub fn next_due(&self, holders: &Holders, now_ms: u64) -> Option<u64> {
        let config = &self.config;
        let registrations = self.entries.values().flat_map(|entry| {
            let registration = entry.registration;
            let (until, stage_due) = match registration.stage {
                Stage::Committing => {
                    let by = config.unseal_by_ms(registration.started_ms);
                    (by, Some(by))
                }
                Stage::Unsealing => (config.unseal_until_ms(registration.started_ms), None),
                Stage::Abandoned => (0, None),
            };
            let attempts = entry.at.values().filter_map(move |attempt| {
                let wanted = match registration.stage {
                    Stage::Committing => attempt.receipt.is_none(),
                    Stage::Unsealing => !attempt.unseal_done(),
                    Stage::Abandoned => false,
                };
                // Nothing is sent once its time is up.
                (wanted && !attempt.in_flight && now_ms < until && attempt.next_ms < until)
                    .then_some(attempt.next_ms)
            });
            // A registration whose last receipt came is unsealed at once.
            let all = registration.stage == Stage::Committing
                && entry.at.values().all(|attempt| attempt.receipt.is_some());
            attempts.chain(stage_due).chain(all.then_some(now_ms))
        });
        let renewals = self.owned.keys().flat_map(move |name| {
            self.registrars.iter().filter_map(move |registrar| {
                let expires_ms = self.renewal_expiry(holders, name, registrar)?;
                let renewing = self.renewals.get(&(*name, *registrar));
                if renewing.is_some_and(|renewing| renewing.in_flight) {
                    return None;
                }
                if renewing.is_some_and(|renewing| renewing.forced) {
                    return Some(now_ms);
                }
                let next_ms = renewing.map_or(0, |renewing| renewing.next_ms);
                Some(
                    expires_ms
                        .saturating_sub(config.renew_before_ms)
                        .max(next_ms),
                )
            })
        });
        // A name the view gives us is noticed at the next poll of the lists.
        registrations.chain(renewals).min()
    }

    /// How each name of ours stands at `now_ms`: those we register, then
    /// those we hold and register no more.
    #[must_use]
    pub fn statuses(&self, holders: &Holders, now_ms: u64) -> Vec<NameStatus> {
        let mut statuses: Vec<NameStatus> = self
            .entries
            .values()
            .map(|entry| self.registration_status(entry, holders, now_ms))
            .collect();
        for (name, expires_ms) in &self.owned {
            if self.entries.contains_key(name) {
                continue;
            }
            let registrars = self
                .registrars
                .iter()
                .map(|registrar| {
                    let renewing = self.renewals.get(&(*name, *registrar));
                    AtRegistrar {
                        registrar: *registrar,
                        receipt: false,
                        unsealed: false,
                        refused: renewing.and_then(|renewing| renewing.refused),
                        listed: holders.listing(registrar, name).copied(),
                        fresh: holders.is_fresh_list(registrar, now_ms),
                    }
                })
                .collect();
            let holder = holders.holder(name).map(|held| held.holder);
            statuses.push(NameStatus {
                name: *name,
                state: if holder == Some(self.own) {
                    State::Held
                } else {
                    State::Lapsing
                },
                holder,
                expires_ms: Some(*expires_ms),
                unseal_by_ms: None,
                registrars,
            });
        }
        statuses
    }

    fn registration_status(&self, entry: &Entry, holders: &Holders, now_ms: u64) -> NameStatus {
        let registration = &entry.registration;
        let name = registration.name;
        let held = holders.holder(&name);
        let registrars = entry
            .at
            .iter()
            .map(|(registrar, attempt)| AtRegistrar {
                registrar: *registrar,
                // Unsealing began only with every receipt.
                receipt: attempt.receipt.is_some() || registration.stage == Stage::Unsealing,
                unsealed: attempt.unsealed,
                refused: attempt.refused,
                listed: holders.listing(registrar, &name).copied(),
                fresh: holders.is_fresh_list(registrar, now_ms),
            })
            .collect();
        NameStatus {
            name,
            state: self.state(registration, holders, now_ms),
            holder: held.map(|held| held.holder),
            expires_ms: held.map(|held| held.expires_ms),
            unseal_by_ms: (registration.stage == Stage::Committing)
                .then(|| self.config.unseal_by_ms(registration.started_ms)),
            registrars,
        }
    }

    fn state(&self, registration: &NameRegistration, holders: &Holders, now_ms: u64) -> State {
        match registration.stage {
            Stage::Committing => return State::Committing,
            Stage::Abandoned => return State::Abandoned,
            Stage::Unsealing => {}
        }
        if now_ms < self.config.unseal_until_ms(registration.started_ms) {
            return State::Unsealing;
        }
        let name = &registration.name;
        match holders.holder(name) {
            Some(held) if held.holder == self.own => return State::Held,
            Some(_) => return State::Lost,
            None => {}
        }
        let (mut ours, mut others) = (false, false);
        for registrar in &self.registrars {
            if !holders.is_fresh_list(registrar, now_ms) {
                continue;
            }
            match holders.listing(registrar, name) {
                Some(listing) if listing.holder == self.own => ours = true,
                Some(_) => others = true,
                None => {}
            }
        }
        if ours && others {
            State::Split
        } else if now_ms >= self.config.settled_ms(registration.started_ms) {
            State::NotHeld
        } else {
            State::Waiting
        }
    }

    /// What changed since the last call, for the same transaction as the
    /// ledger's: registrations started or moved on, as they are now, and
    /// those dropped.
    pub fn take_changes(&mut self, batch: &mut PeerBatch) {
        batch.registrations_removed.append(&mut self.removed);
        for name in core::mem::take(&mut self.dirty) {
            if let Some(entry) = self.entries.get(&name) {
                batch.registrations.push(entry.registration);
            }
        }
    }

    /// Checks the registrant's invariants (spec/decisions/tigerstyle.md).
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        assert!(
            self.entries.len() <= MAX_REGISTRATIONS,
            "registrations within bounds"
        );
        assert!(
            self.owned.len() <= self.config.max_names
                && self.renewals.len() <= self.config.max_names * self.registrars.len(),
            "names renewed within bounds"
        );
        assert!(
            self.renewals.keys().all(|(name, registrar)| {
                self.owned.contains_key(name) && self.registrars.binary_search(registrar).is_ok()
            }),
            "only our names are renewed, at registrars"
        );
        for (name, entry) in &self.entries {
            let registration = &entry.registration;
            assert!(
                *name == registration.name,
                "registrations are indexed by name"
            );
            assert!(
                entry.commitment == Commitment::seal(name, &self.own, &registration.salt),
                "a registration's commitment is its own"
            );
            assert!(
                entry.at.keys().eq(self.registrars.iter()),
                "a registration stands with every registrar"
            );
            assert!(
                registration.stage != Stage::Committing
                    || entry.at.values().all(|attempt| !attempt.unsealed),
                "nothing is unsealed before every receipt is held"
            );
        }
        assert!(
            self.dirty
                .iter()
                .all(|name| self.entries.contains_key(name)),
            "only registrations kept are written"
        );
        assert!(
            self.removed
                .iter()
                .all(|name| !self.entries.contains_key(name)),
            "a registration dropped is not kept"
        );
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use dassie_identity::NodeKey;

    use super::*;
    use crate::names::HoldersConfig;
    use crate::wire::NameListResponse;

    const START: u64 = 1_800_000_000_000;

    const CONFIG: RegistrantConfig = RegistrantConfig {
        window_ms: 100_000,
        request_timeout_ms: 10_000,
        min_backoff_ms: 1_000,
        max_backoff_ms: 8_000,
        renew_before_ms: 50_000,
        grace_ms: 50_000,
        release_after_ms: 200_000,
        max_under_way: 2,
        max_names: 4,
    };

    const LISTS: HoldersConfig = HoldersConfig {
        list_fresh_ms: 10_000,
    };

    fn node(seed: u8) -> NodeId {
        NodeKey::from_bytes(&[seed; 32]).node_id()
    }

    fn name(text: &str) -> Name {
        Name::parse(text).unwrap()
    }

    /// Node 9, registering at registrars 1 to `count`.
    fn registrant(count: u8) -> Registrant {
        Registrant::new(CONFIG, node(9), (1..=count).map(node), vec![])
    }

    /// The view of registrars 1 to `count`, with no lists yet.
    fn view(count: u8) -> Holders {
        Holders::new(LISTS, (1..=count).map(node), vec![])
    }

    /// Registrars 1 to `lists.len()` list `name` for the node in each, if
    /// any, until `expires`; the view is taken from them at `now`.
    fn show(view: &mut Holders, name: &Name, lists: &[Option<u8>], expires: u64, now: u64) {
        for (registrar, holder) in (1..).zip(lists) {
            let mut list = NameListResponse::default();
            if let Some(holder) = holder {
                let holding = NameHolding {
                    holder: node(*holder),
                    expires_at_ms: expires,
                };
                list.0.insert(*name, holding);
            }
            assert!(view.take_list(&node(registrar), list, [registrar; 32], now));
        }
        let mut changes = Vec::new();
        view.recompute(now, &mut changes);
    }

    fn requests(registrant: &mut Registrant, view: &Holders, now: u64) -> Vec<NameRequest> {
        let (mut progress, mut out) = (Vec::new(), Vec::new());
        registrant.requests(view, now, &mut progress, &mut out);
        registrant.check_invariants();
        out
    }

    fn receipt(position: u64) -> NameReceipt {
        NameReceipt {
            position,
            window_end_ms: START + CONFIG.window_ms,
        }
    }

    fn commitment(registrant: &Registrant, name: &Name) -> Commitment {
        registrant.entries[name].commitment
    }

    fn state(registrant: &Registrant, view: &Holders, name: &Name, now: u64) -> State {
        let statuses = registrant.statuses(view, now);
        statuses
            .iter()
            .find(|status| status.name == *name)
            .unwrap()
            .state
    }

    /// Registers `alice` at registrars 1 to 3 and answers every commitment
    /// with a receipt.
    fn committed(registrant: &mut Registrant, view: &Holders) {
        let alice = name("alice");
        registrant
            .register(alice, Salt([7; 32]), view, START)
            .unwrap();
        let sent = requests(registrant, view, START);
        assert_eq!(sent.len(), 3);
        let sealed = commitment(registrant, &alice);
        for (position, request) in (0..).zip(&sent) {
            assert_eq!(request.message, Message::Commit(sealed));
            let mut progress = Vec::new();
            registrant.on_commit(
                &request.to,
                &sealed,
                Ok(receipt(position)),
                START,
                &mut progress,
            );
        }
    }

    // spec/designs/names.md#3-registering: the commitment goes to every
    // registrar at once, sealed for this node, and the name is unsealed
    // only once every registrar's receipt is held.
    #[test]
    fn a_registration_unseals_only_with_every_receipt() {
        let (mut registrant, view) = (registrant(3), view(3));
        let alice = name("alice");
        registrant
            .register(alice, Salt([7; 32]), &view, START)
            .unwrap();
        let sealed = commitment(&registrant, &alice);
        assert!(sealed.is_opened_by(&alice, &node(9), &Salt([7; 32])));
        let sent = requests(&mut registrant, &view, START);
        let targets: Vec<NodeId> = sent.iter().map(|request| request.to).collect();
        let mut expected = vec![node(1), node(2), node(3)];
        expected.sort_unstable();
        assert_eq!(targets, expected);
        let mut progress = Vec::new();
        for request in &sent[..2] {
            registrant.on_commit(&request.to, &sealed, Ok(receipt(1)), START, &mut progress);
        }
        assert!(requests(&mut registrant, &view, START + 1).is_empty());
        assert_eq!(
            state(&registrant, &view, &alice, START + 1),
            State::Committing
        );
        registrant.on_commit(&sent[2].to, &sealed, Ok(receipt(1)), START, &mut progress);
        let sent = requests(&mut registrant, &view, START + 2);
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|request| matches!(
            request.message,
            Message::Unseal { name, .. } if name == alice
        )));
        assert_eq!(
            state(&registrant, &view, &alice, START + 2),
            State::Unsealing
        );
    }

    // A registration that does not hold every receipt while an unsealing
    // still has a request's time before any window closes is abandoned: no
    // registrar is told the name.
    #[test]
    fn a_registration_short_of_a_receipt_is_abandoned() {
        let (mut registrant, view) = (registrant(3), view(3));
        let alice = name("alice");
        registrant
            .register(alice, Salt([7; 32]), &view, START)
            .unwrap();
        let sealed = commitment(&registrant, &alice);
        let mut progress = Vec::new();
        let mut now = START;
        let by = CONFIG.unseal_by_ms(START);
        assert_eq!(by, START + 100_000 - 1_000 - 10_000);
        // Registrar 1 refuses, again and again; the others answer.
        while now < by {
            for request in requests(&mut registrant, &view, now) {
                assert!(matches!(request.message, Message::Commit(_)));
                let result = if request.to == node(1) {
                    Err(Some(503))
                } else {
                    Ok(receipt(1))
                };
                registrant.on_commit(&request.to, &sealed, result, now, &mut progress);
            }
            now = registrant.next_due(&view, now).unwrap().max(now + 1);
        }
        assert_eq!(now, by);
        assert!(requests(&mut registrant, &view, now).is_empty());
        assert_eq!(state(&registrant, &view, &alice, now), State::Abandoned);
        assert_eq!(registrant.next_due(&view, now), None);
        let mut progress = Vec::new();
        registrant.on_time(&view, now, &mut progress);
        let status = registrant.statuses(&view, now);
        let first = status[0]
            .registrars
            .iter()
            .find(|at| at.registrar == node(1));
        assert_eq!(first.unwrap().refused, Some(503));
    }

    // A refused commitment is sent again after a backoff that doubles.
    #[test]
    fn a_refused_commitment_is_sent_again_after_a_backoff() {
        let (mut registrant, view) = (registrant(1), view(1));
        let alice = name("alice");
        registrant
            .register(alice, Salt([7; 32]), &view, START)
            .unwrap();
        let sealed = commitment(&registrant, &alice);
        let mut progress = Vec::new();
        let mut at = START;
        for wait in [2_000, 4_000, 8_000, 8_000] {
            assert_eq!(requests(&mut registrant, &view, at).len(), 1);
            registrant.on_commit(&node(1), &sealed, Err(Some(429)), at, &mut progress);
            assert!(requests(&mut registrant, &view, at + wait - 1).is_empty());
            assert_eq!(registrant.next_due(&view, at), Some(at + wait));
            at += wait;
        }
    }

    // A name is not registered when it is reserved, when this node holds
    // it, when a fresh list shows it taken, or too many are under way.
    #[test]
    fn taken_and_held_names_are_not_registered() {
        let (mut registrant, mut view) = (registrant(2), view(2));
        let salt = Salt([7; 32]);
        assert_eq!(
            registrant.register(name("www"), salt, &view, START),
            Err(RegisterError::Reserved)
        );
        let mut alone = Registrant::new(CONFIG, node(9), [], vec![]);
        assert_eq!(
            alone.register(name("alice"), salt, &view, START),
            Err(RegisterError::NoRegistrars)
        );
        show(
            &mut view,
            &name("taken"),
            &[Some(1), None],
            START + 1_000_000,
            START,
        );
        assert_eq!(
            registrant.register(name("taken"), salt, &view, START),
            Err(RegisterError::Taken)
        );
        // Once the list is stale, the registrars decide.
        let later = START + LISTS.list_fresh_ms + 1;
        registrant
            .register(name("taken"), salt, &view, later)
            .unwrap();
        registrant
            .register(name("alice"), salt, &view, later)
            .unwrap();
        assert_eq!(
            registrant.register(name("alice"), salt, &view, later),
            Err(RegisterError::UnderWay)
        );
        assert_eq!(
            registrant.register(name("bob"), salt, &view, later),
            Err(RegisterError::TooMany)
        );
        let mut view = self::view(2);
        show(
            &mut view,
            &name("mine"),
            &[Some(9), Some(9)],
            START + 1_000_000,
            START,
        );
        let mut registrant = self::registrant(2);
        assert_eq!(
            registrant.register(name("mine"), salt, &view, START),
            Err(RegisterError::Held)
        );
    }

    // Finished registrations stay for the owner to see, and give way to
    // new ones, oldest first, once the registrations are full.
    #[test]
    fn a_finished_registration_gives_way() {
        let (mut registrant, view) = (registrant(1), view(1));
        let salt = Salt([7; 32]);
        let mut now = START;
        for index in 0..MAX_REGISTRATIONS {
            let text = alloc::format!("name{index}");
            registrant.register(name(&text), salt, &view, now).unwrap();
            now = CONFIG.unseal_by_ms(now);
            requests(&mut registrant, &view, now);
        }
        assert_eq!(registrant.registrations().count(), MAX_REGISTRATIONS);
        assert!(
            registrant
                .registrations()
                .all(|registration| registration.stage == Stage::Abandoned)
        );
        registrant.register(name("late"), salt, &view, now).unwrap();
        assert_eq!(registrant.registrations().count(), MAX_REGISTRATIONS);
        assert!(registrant.registrations().all(|r| r.name != name("name0")));
        let mut batch = PeerBatch::default();
        registrant.take_changes(&mut batch);
        assert_eq!(batch.registrations_removed, [name("name0")]);
        // A registration finished is started again with a new salt.
        let again = Salt([8; 32]);
        registrant
            .register(name("name1"), again, &view, now)
            .unwrap();
        let restarted = registrant
            .registrations()
            .find(|registration| registration.name == name("name1"))
            .unwrap();
        assert_eq!(
            (restarted.salt, restarted.stage),
            (again, Stage::Committing)
        );
    }

    // A restart commits to the same commitment again, which a registrar
    // that logged it answers with the same receipt; one that was unsealing
    // unseals again.
    #[test]
    fn a_restart_commits_to_the_same_commitment() {
        let (mut registrant, view) = (registrant(3), view(3));
        committed(&mut registrant, &view);
        let mut batch = PeerBatch::default();
        registrant.take_changes(&mut batch);
        let stored = batch.registrations.clone();
        assert_eq!(stored[0].stage, Stage::Committing);
        let mut restarted = Registrant::new(CONFIG, node(9), (1..=3).map(node), stored);
        let sent = requests(&mut restarted, &view, START + 1);
        let sealed = commitment(&registrant, &name("alice"));
        assert_eq!(sent.len(), 3);
        assert!(sent.iter().all(|r| r.message == Message::Commit(sealed)));
        // Unsealing, persisted, unseals again after a restart.
        requests(&mut registrant, &view, START + 1);
        let mut batch = PeerBatch::default();
        registrant.take_changes(&mut batch);
        assert_eq!(batch.registrations[0].stage, Stage::Unsealing);
        let mut restarted =
            Registrant::new(CONFIG, node(9), (1..=3).map(node), batch.registrations);
        let sent = requests(&mut restarted, &view, START + 2);
        assert_eq!(sent.len(), 3);
        assert!(
            sent.iter()
                .all(|r| matches!(r.message, Message::Unseal { .. }))
        );
    }

    // spec/designs/names.md#5-holding-a-name: once unsealed, the
    // registration is decided by the view: held, lost, split, or not held
    // once a split would have been released.
    #[test]
    fn the_view_decides_a_registration() {
        let alice = name("alice");
        let after = CONFIG.unseal_until_ms(START);
        let cases: [(&[Option<u8>], u64, State); 5] = [
            (&[None, None, None], after, State::Waiting),
            (&[Some(9), Some(9), Some(9)], after, State::Held),
            (&[Some(5), Some(5), Some(5)], after, State::Lost),
            (&[Some(9), Some(5), None], after, State::Split),
            (
                &[Some(9), None, None],
                CONFIG.settled_ms(START),
                State::NotHeld,
            ),
        ];
        for (lists, now, expected) in cases {
            let (mut registrant, mut view) = (registrant(3), view(3));
            committed(&mut registrant, &view);
            requests(&mut registrant, &view, START + 1);
            assert_eq!(
                state(&registrant, &view, &alice, after - 1),
                State::Unsealing
            );
            show(&mut view, &alice, lists, now + 1_000_000, now);
            assert_eq!(
                state(&registrant, &view, &alice, now),
                expected,
                "{lists:?}"
            );
        }
        // Held, the registration is done: the name is ours, and renewed.
        let (mut registrant, mut view) = (registrant(3), view(3));
        committed(&mut registrant, &view);
        requests(&mut registrant, &view, START + 1);
        show(
            &mut view,
            &alice,
            &[Some(9), Some(9), Some(9)],
            after * 2,
            after,
        );
        requests(&mut registrant, &view, after);
        assert_eq!(registrant.registrations().count(), 0);
        assert_eq!(state(&registrant, &view, &alice, after), State::Held);
    }

    // A name this node holds is renewed at every registrar once its expiry
    // there is near, at none before; and at one whose renewal came late,
    // in its grace period, after the view lost the quorum.
    #[test]
    fn names_are_renewed_once_their_expiry_is_near() {
        let alice = name("alice");
        let (mut registrant, mut view) = (registrant(2), view(2));
        let expires = START + 1_000_000;
        show(&mut view, &alice, &[Some(9), Some(9)], expires, START);
        assert!(requests(&mut registrant, &view, START).is_empty());
        let due = expires - CONFIG.renew_before_ms;
        assert_eq!(registrant.next_due(&view, START), Some(due));
        show(&mut view, &alice, &[Some(9), Some(9)], expires, due);
        let sent = requests(&mut registrant, &view, due);
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|r| r.message == Message::Renew(alice)));
        // Registrar 1 renews; registrar 2's answer is lost.
        let mut progress = Vec::new();
        let renewed = expires + 1_000_000;
        registrant.on_renew(&node(1), &alice, Ok(renewed), due, &mut progress);
        registrant.on_renew(&node(2), &alice, Err(None), due, &mut progress);
        // Registrar 2 lets it expire: with two registrars, the view loses
        // it, and the name is renewed there in its grace period.
        let late = expires + 1;
        let mut list = NameListResponse::default();
        let holding = NameHolding {
            holder: node(9),
            expires_at_ms: renewed,
        };
        list.0.insert(alice, holding);
        assert!(view.take_list(&node(1), list, [1; 32], late));
        assert!(view.take_list(&node(2), NameListResponse::default(), [2; 32], late));
        let mut changes = Vec::new();
        view.recompute(late, &mut changes);
        assert!(view.holder(&alice).is_none());
        let sent = requests(&mut registrant, &view, late);
        assert_eq!(
            sent,
            [NameRequest {
                to: node(2),
                message: Message::Renew(alice)
            }]
        );
        assert_eq!(state(&registrant, &view, &alice, late), State::Lapsing);
        // Once the grace period after the latest expiry is over, it is
        // ours no more.
        let gone = renewed + CONFIG.grace_ms;
        let mut progress = Vec::new();
        registrant.on_time(&view, gone, &mut progress);
        assert!(registrant.statuses(&view, gone).is_empty());
        assert_eq!(
            registrant.renew_now(&alice, &view),
            Err(RenewError::NotHeld)
        );
    }

    // A name the lists stop showing before it expires was released, or
    // lost: it is ours no more, and may be registered again.
    #[test]
    fn a_name_dropped_before_it_expires_is_ours_no_more() {
        let alice = name("alice");
        let (mut registrant, mut view) = (registrant(2), view(2));
        let expires = START + 1_000_000;
        show(&mut view, &alice, &[Some(9), Some(9)], expires, START);
        requests(&mut registrant, &view, START);
        assert_eq!(state(&registrant, &view, &alice, START), State::Held);
        let later = START + 1_000;
        show(&mut view, &alice, &[None, None], expires, later);
        assert!(requests(&mut registrant, &view, later).is_empty());
        assert!(registrant.statuses(&view, later).is_empty());
        let salt = Salt([7; 32]);
        registrant.register(alice, salt, &view, later).unwrap();
    }

    // The owner renews at once, at every registrar that does not list
    // another node; a registrar whose clock is behind ours and leaves the
    // name due is asked again after a backoff.
    #[test]
    fn the_owner_renews_at_once() {
        let alice = name("alice");
        let (mut registrant, mut view) = (registrant(3), view(3));
        let expires = START + 1_000_000;
        show(
            &mut view,
            &alice,
            &[Some(9), Some(9), Some(9)],
            expires,
            START,
        );
        assert!(requests(&mut registrant, &view, START).is_empty());
        registrant.renew_now(&alice, &view).unwrap();
        let sent = requests(&mut registrant, &view, START);
        assert_eq!(sent.len(), 3);
        let mut progress = Vec::new();
        let due = expires - CONFIG.renew_before_ms;
        for request in &sent {
            registrant.on_renew(&request.to, &alice, Ok(expires), START, &mut progress);
        }
        assert!(requests(&mut registrant, &view, due - 1).is_empty());
        let sent = requests(&mut registrant, &view, due);
        assert_eq!(sent.len(), 3);
        for request in &sent {
            registrant.on_renew(&request.to, &alice, Ok(expires), due, &mut progress);
        }
        assert!(requests(&mut registrant, &view, due + 1_999).is_empty());
        assert_eq!(requests(&mut registrant, &view, due + 2_000).len(), 3);
    }
}

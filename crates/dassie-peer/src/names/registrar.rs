//! A registrar's side of names (spec/designs/names.md#4-the-rule-at-each-registrar,
//! spec/areas/network.md#names): its log of sealed commitments, the rule
//! that grants a name to the commitment logged first among its valid
//! unsealings, expiry, renewal and grace, and the list it serves. All of it
//! is the registrar's own, by its own clock: registrars never agree on time
//! or order with each other.
//!
//! A commitment is:
//!
//! 1. **Logged**, at the next position and at the registrar's time, both
//!    only rising, across restarts too, and answered with its receipt once
//!    it is on disk. Its window closes a set time after it was logged.
//! 2. **Unsealed** within its window by the node named in it, with the name
//!    and salt. The unsealing is valid only if the name was free here when
//!    the commitment was logged: neither listed nor in its grace period. A
//!    valid one contends for the name, unless the name is held by then,
//!    which only a commitment logged earlier can have won.
//! 3. **Decided** once its window has closed: one that contends is granted
//!    the name if it is the earliest contender left, and only once every
//!    earlier window has closed too, since an earlier commitment can still
//!    take the name until then; any other is lost, or lapses if it was never
//!    unsealed.
//!
//! A granted name is listed until it expires, kept for its holder alone for
//! a grace period after, and then free; what it was is kept a while longer,
//! so that a commitment logged while it was held stays void for it. A name
//! that the other registrars' lists show split is released: free at once
//! (spec/areas/network.md#releasing-a-split).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::Encode;
use sha2::{Digest, Sha256};

use super::{Commitment, Name, Salt};
use crate::config::Rate;
use crate::persist::PeerBatch;
use crate::table::{Bucket, Network};
use crate::wire::{MAX_NAME_LIST, NameHolding, NameListResponse, NameReceipt};

/// A registrar's timers and bounds (spec/designs/names.md#5-holding-a-name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrarConfig {
    /// How long after it is logged a commitment may be unsealed: long
    /// enough for a registrant to commit at every registrar and unseal at
    /// each, retrying ([`least_window_ms`]). A name waits this long before
    /// it is granted.
    pub window_ms: u64,
    /// How long a name is held once granted, or from its expiry once
    /// renewed.
    pub ttl_ms: u64,
    /// How long after it expires a name is kept for its holder alone, so
    /// that a renewal that reached some registrars late hands it to no one
    /// else.
    pub grace_ms: u64,
    /// How long before its expiry a name is renewed: an earlier renewal is
    /// answered with the expiry it has, so that a holder cannot move its
    /// list's hash at will.
    pub renew_before_ms: u64,
    /// New commitments the registrar logs: free for now, so throttled
    /// (spec/roadmap.md#registration-fees).
    pub commit_rate: Rate,
    /// Most commitments in the log.
    pub max_commitments: usize,
    /// Most names kept, listed, in grace or freed less than a window ago,
    /// with the commitments in the log, each of which may become one. At
    /// most [`MAX_NAME_LIST`], so the list served always decodes.
    pub max_names: usize,
    /// Most commitments in the log from one /24 or /48.
    pub max_commitments_per_network: usize,
    /// Most commitments in the log from one node.
    pub max_commitments_per_node: usize,
    /// Most names one node holds, with what may become one: checked for
    /// the node that sends a commitment, with the commitments it has in the
    /// log, and for the node named in an unsealing, with its unsealings that
    /// contend, so that commitments other nodes log for it raise it no
    /// higher.
    pub max_names_per_node: usize,
    /// How long more than f of the other registrars' fresh lists must lack
    /// a name's holder before the name is released: longer than a
    /// registration, or a renewal that arrived late, takes to reach every
    /// registrar's list ([`least_release_ms`]).
    pub release_after_ms: u64,
}

/// The least window that gives a commitment and its unsealing each a
/// request's time and a retry after the shortest backoff, twice over.
#[must_use]
pub const fn least_window_ms(request_timeout_ms: u64, min_backoff_ms: u64) -> u64 {
    4 * (request_timeout_ms + min_backoff_ms)
}

/// How late a renewal may reach a registrar: it is retried after the
/// longest backoff, and gets a request's time, twice over. A registrar's
/// grace period, and the time to renew, are at least this.
#[must_use]
pub const fn late_renewal_ms(request_timeout_ms: u64, max_backoff_ms: u64) -> u64 {
    2 * (request_timeout_ms + max_backoff_ms)
}

/// The least wait before a split is released: a registration is granted at
/// every registrar within a window of the first grant, since its
/// commitments are all logged within the first one's window, and a renewal
/// that reached a registrar late, `late_renewal_ms` at most after the name
/// expired there, lists it there again; a list that lacks it is fetched
/// within a poll and a hash poll's and a download's time after, or it is
/// one fetched before, which stays fresh for `list_fresh_ms` at most.
#[must_use]
pub const fn least_release_ms(
    window_ms: u64,
    late_renewal_ms: u64,
    list_fresh_ms: u64,
    list_poll_interval_ms: u64,
    fetch_ms: u64,
) -> u64 {
    let late_ms = if window_ms > late_renewal_ms {
        window_ms
    } else {
        late_renewal_ms
    };
    late_ms + list_fresh_ms + list_poll_interval_ms + fetch_ms
}

impl RegistrarConfig {
    /// The production values.
    pub const DEFAULT: Self = Self {
        window_ms: 3_600_000,
        ttl_ms: 31_536_000_000,
        grace_ms: 2_592_000_000,
        renew_before_ms: 2_592_000_000,
        commit_rate: Rate {
            burst: 20,
            refill_ms: 60_000,
        },
        max_commitments: 1024,
        max_names: MAX_NAME_LIST,
        max_commitments_per_network: 8,
        max_commitments_per_node: 4,
        max_names_per_node: 8,
        release_after_ms: 8_100_000,
    };
}

/// Where the log stands: the position the next commitment takes, and the
/// time the last one was logged at. Persisted, so that both only rise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogHead {
    /// The next position.
    pub next_position: u64,
    /// When the last commitment was logged, by the registrar's clock.
    pub last_logged_ms: u64,
}

/// The unsealing of a logged commitment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsealing {
    /// The name.
    pub name: Name,
    /// The node named in the commitment, which sent the unsealing.
    pub holder: NodeId,
    /// Whether it contends for the name: the name was not held here when
    /// it was unsealed. One that does not has lost, and is kept only so
    /// that a retry is answered as the first was.
    pub contends: bool,
}

/// A commitment in the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Logged {
    /// The commitment.
    pub commitment: Commitment,
    /// Its position: the order the registrar took it in.
    pub position: u64,
    /// When it was logged, by the registrar's clock.
    pub logged_ms: u64,
    /// When its window closes.
    pub window_end_ms: u64,
    /// The node that sent it.
    pub committer: NodeId,
    /// The network it came from, for the cap per network.
    pub network: Option<Network>,
    /// Its unsealing, once unsealed.
    pub unsealing: Option<Unsealing>,
}

/// A name granted here, held or freed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Granted {
    /// The name.
    pub name: Name,
    /// Its holder.
    pub holder: NodeId,
    /// When it was granted.
    pub granted_ms: u64,
    /// When it expires unless renewed.
    pub expires_ms: u64,
    /// When it stopped being held: kept until every commitment logged
    /// before then has left the log.
    pub freed_ms: Option<u64>,
}

impl Granted {
    /// When it stops being held, or stopped: at the end of its grace,
    /// unless it was freed otherwise.
    #[must_use]
    pub const fn held_until_ms(&self, grace_ms: u64) -> u64 {
        match self.freed_ms {
            Some(freed) => freed,
            None => self.expires_ms.saturating_add(grace_ms),
        }
    }

    /// Whether it is held at `now_ms`: listed, or in its grace period.
    #[must_use]
    pub const fn is_held(&self, now_ms: u64, grace_ms: u64) -> bool {
        self.freed_ms.is_none() && now_ms < self.held_until_ms(grace_ms)
    }

    /// Whether it is listed at `now_ms`.
    #[must_use]
    pub const fn is_listed(&self, now_ms: u64) -> bool {
        self.freed_ms.is_none() && now_ms < self.expires_ms
    }
}

/// What came of a commitment that was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Committed {
    /// Logged now, with this receipt.
    New(NameReceipt),
    /// Logged before: its receipt again, at no cost.
    Again(NameReceipt),
}

/// Why a commitment is not logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitError {
    /// The log is full.
    LogFull,
    /// The names, with the commitments that may become names, are full.
    NamesFull,
    /// Its network has as many commitments in the log as it may.
    Network,
    /// Its node has as many commitments in the log as it may.
    Commitments,
    /// Its node holds as many names as it may, with its commitments.
    Names,
    /// Too many new commitments lately.
    Rate,
}

/// What came of an unsealing that was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsealed {
    /// Taken now; it contends for the name, or has lost already.
    New {
        /// Whether it contends.
        contends: bool,
    },
    /// Taken before.
    Again,
}

/// Why an unsealing is not valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsealError {
    /// The name is reserved.
    Reserved,
    /// No commitment in the log opens with it: none was logged, its window
    /// closed long ago, or it is another node's.
    Unknown,
    /// The commitment's window has closed.
    Closed,
    /// The name was held here when the commitment was logged, so the
    /// commitment is void for it.
    Taken,
    /// The node named in it holds as many names as it may, with its
    /// unsealings that contend.
    Names,
}

/// What a renewal did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Renewal {
    /// When the name expires now.
    pub expires_ms: u64,
    /// Whether the renewal moved it: one that comes before the name's
    /// expiry is near leaves it as it is.
    pub extended: bool,
}

/// Why a renewal is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenewError {
    /// The name is not held here: neither listed nor in its grace period.
    NotHeld,
    /// Another node holds it.
    NotHolder,
}

/// What the passing of time did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Happened {
    /// A name was granted.
    Granted {
        /// The name.
        name: Name,
        /// Its holder.
        holder: NodeId,
    },
    /// An unsealed commitment lost the name to one logged earlier.
    Lost {
        /// The name.
        name: Name,
        /// The node that unsealed it.
        holder: NodeId,
    },
    /// A commitment was not unsealed within its window.
    Lapsed {
        /// The node that sent it.
        committer: NodeId,
        /// Its position.
        position: u64,
    },
    /// A name's grace period ended unrenewed: it is free.
    Freed {
        /// The name.
        name: Name,
        /// Its holder until now.
        holder: NodeId,
    },
    /// A name was released: more than f of the other registrars' fresh
    /// lists lacked its holder for long enough. It is free.
    Released {
        /// The name.
        name: Name,
        /// Its holder until now.
        holder: NodeId,
    },
}

/// A registrar's state: the log, the names granted, and the list served.
pub struct Registrar {
    config: RegistrarConfig,
    head: LogHead,
    /// The log, by position.
    log: BTreeMap<u64, Logged>,
    /// The log's positions, by commitment.
    positions: BTreeMap<Commitment, u64>,
    names: BTreeMap<Name, Granted>,
    commits: Bucket,
    /// Up to when the passing of time is taken in.
    processed_ms: u64,
    next_due_ms: Option<u64>,
    served: Vec<u8>,
    served_hash: [u8; 32],
    served_changed: bool,
    head_dirty: bool,
    logged_dirty: BTreeSet<u64>,
    logged_removed: Vec<Commitment>,
    names_dirty: BTreeSet<Name>,
    names_removed: Vec<Name>,
    /// Since when each name listed has been lacking from more than f of
    /// the other registrars' fresh lists, as last checked. Not persisted:
    /// after a restart, the wait starts again.
    splits: BTreeMap<Name, u64>,
    /// The most commitments the log may hold: the bound, or more while what
    /// was restored beyond it is still there ([`Self::restore`]). Only
    /// falls, to the bound.
    log_ceiling: usize,
    /// The most names with commitments in the log, likewise.
    names_ceiling: usize,
}

impl Registrar {
    /// A registrar from what it persisted, at `now_ms`. Every commitment
    /// and every name is kept, even beyond bounds lowered since they were
    /// logged, since each commitment was receipted: the bounds then refuse
    /// new commitments until there is room again. What is kept is bounded
    /// still, by the bounds it was logged under.
    #[must_use]
    pub fn restore(
        config: RegistrarConfig,
        head: LogHead,
        log: Vec<Logged>,
        names: Vec<Granted>,
        now_ms: u64,
    ) -> Self {
        let mut registrar = Self {
            config,
            head,
            log: BTreeMap::new(),
            positions: BTreeMap::new(),
            names: BTreeMap::new(),
            commits: Bucket::full(config.commit_rate, now_ms),
            processed_ms: 0,
            next_due_ms: None,
            served: Vec::new(),
            served_hash: [0; 32],
            served_changed: true,
            head_dirty: false,
            logged_dirty: BTreeSet::new(),
            logged_removed: Vec::new(),
            names_dirty: BTreeSet::new(),
            names_removed: Vec::new(),
            splits: BTreeMap::new(),
            log_ceiling: config.max_commitments,
            names_ceiling: config.max_names,
        };
        for granted in names {
            registrar.names.insert(granted.name, granted);
        }
        for logged in log {
            registrar
                .positions
                .insert(logged.commitment, logged.position);
            registrar.log.insert(logged.position, logged);
        }
        registrar.lower_ceilings();
        // The head stays ahead of what was logged, so neither positions
        // nor times ever go back.
        if let Some(last) = registrar.log.values().next_back() {
            registrar.head.next_position = registrar
                .head
                .next_position
                .max(last.position.saturating_add(1));
            registrar.head.last_logged_ms = registrar.head.last_logged_ms.max(last.logged_ms);
        }
        registrar.recompute_due();
        registrar
    }

    /// The configuration.
    #[must_use]
    pub const fn config(&self) -> &RegistrarConfig {
        &self.config
    }

    /// Where the log stands.
    #[must_use]
    pub const fn head(&self) -> LogHead {
        self.head
    }

    /// The commitments in the log, by position.
    pub fn logged(&self) -> impl Iterator<Item = &Logged> {
        self.log.values()
    }

    /// The names granted here, held or freed a short while ago, by name.
    pub fn granted(&self) -> impl Iterator<Item = &Granted> {
        self.names.values()
    }

    /// The receipt of `commitment`, if it is in the log.
    #[must_use]
    pub fn receipt(&self, commitment: &Commitment) -> Option<NameReceipt> {
        let logged = self.log.get(self.positions.get(commitment)?)?;
        Some(NameReceipt {
            position: logged.position,
            window_end_ms: logged.window_end_ms,
        })
    }

    /// Logs `commitment` from `committer`, or gives its receipt again if
    /// it is in the log. The bounds and caps come first; a token of the
    /// rate is spent only on a commitment logged.
    ///
    /// # Errors
    ///
    /// A bound, a cap or the rate refuses it.
    pub fn commit(
        &mut self,
        commitment: Commitment,
        committer: NodeId,
        network: Option<Network>,
        now_ms: u64,
    ) -> Result<Committed, CommitError> {
        if let Some(receipt) = self.receipt(&commitment) {
            return Ok(Committed::Again(receipt));
        }
        self.commit_refusal(&committer, network)?;
        let next_position = self
            .head
            .next_position
            .checked_add(1)
            .ok_or(CommitError::LogFull)?;
        if !self.commits.take(self.config.commit_rate, now_ms) {
            return Err(CommitError::Rate);
        }
        let logged_ms = now_ms.max(self.head.last_logged_ms.saturating_add(1));
        let logged = Logged {
            commitment,
            position: self.head.next_position,
            logged_ms,
            window_end_ms: logged_ms.saturating_add(self.config.window_ms),
            committer,
            network,
            unsealing: None,
        };
        self.head = LogHead {
            next_position,
            last_logged_ms: logged_ms,
        };
        self.head_dirty = true;
        self.positions.insert(commitment, logged.position);
        self.log.insert(logged.position, logged);
        self.logged_dirty.insert(logged.position);
        self.recompute_due();
        Ok(Committed::New(NameReceipt {
            position: logged.position,
            window_end_ms: logged.window_end_ms,
        }))
    }

    /// Why a new commitment from `committer` is refused, if it is.
    fn commit_refusal(
        &self,
        committer: &NodeId,
        network: Option<Network>,
    ) -> Result<(), CommitError> {
        let config = &self.config;
        if self.log.len() >= config.max_commitments {
            return Err(CommitError::LogFull);
        }
        if self.log.len() + self.names.len() >= config.max_names {
            return Err(CommitError::NamesFull);
        }
        if let Some(network) = network {
            let from_network = self
                .log
                .values()
                .filter(|logged| logged.network == Some(network))
                .count();
            if from_network >= config.max_commitments_per_network {
                return Err(CommitError::Network);
            }
        }
        let commitments = self
            .log
            .values()
            .filter(|logged| logged.committer == *committer)
            .count();
        if commitments >= config.max_commitments_per_node {
            return Err(CommitError::Commitments);
        }
        if commitments + self.names_held_by(committer) >= config.max_names_per_node {
            return Err(CommitError::Names);
        }
        Ok(())
    }

    /// The names `node` holds: listed, or in their grace period.
    fn names_held_by(&self, node: &NodeId) -> usize {
        self.names
            .values()
            .filter(|granted| granted.holder == *node && granted.freed_ms.is_none())
            .count()
    }

    /// The unsealings in the log that contend for a name for `node`.
    fn contending_for(&self, node: &NodeId) -> usize {
        self.log
            .values()
            .filter_map(|logged| logged.unsealing)
            .filter(|unsealing| unsealing.holder == *node && unsealing.contends)
            .count()
    }

    /// Takes `holder`'s unsealing of its commitment to `name` under
    /// `salt`. A retry is answered as the first was, for as long as the
    /// commitment is in the log, its window closed or not.
    ///
    /// # Errors
    ///
    /// The unsealing is not valid here.
    pub fn unseal(
        &mut self,
        holder: NodeId,
        name: Name,
        salt: &Salt,
        now_ms: u64,
    ) -> Result<Unsealed, UnsealError> {
        if name.is_reserved() {
            return Err(UnsealError::Reserved);
        }
        let commitment = Commitment::seal(&name, &holder, salt);
        let position = *self
            .positions
            .get(&commitment)
            .ok_or(UnsealError::Unknown)?;
        let grace = self.config.grace_ms;
        let granted = self.names.get(&name).copied();
        let logged = self.log.get(&position).ok_or(UnsealError::Unknown)?;
        // Taken already, even if the answer was lost and the window has
        // closed since.
        if logged.unsealing.is_some() {
            return Ok(Unsealed::Again);
        }
        if logged.window_end_ms <= now_ms {
            return Err(UnsealError::Closed);
        }
        // Void if the name was held here when the commitment was logged,
        // whatever other registrars list.
        let at = logged.logged_ms;
        if granted.is_some_and(|g| g.granted_ms <= at && at < g.held_until_ms(grace)) {
            return Err(UnsealError::Taken);
        }
        // The cap on names is the holder's, whoever logged the commitment.
        // It is checked whether this one contends or not, so the answer
        // says nothing of whether the name is held.
        let held = self.names_held_by(&holder) + self.contending_for(&holder);
        if held >= self.config.max_names_per_node {
            return Err(UnsealError::Names);
        }
        // Held now, by a commitment logged earlier: this one has lost.
        let contends = !granted.is_some_and(|g| g.is_held(now_ms, grace));
        let logged = self.log.get_mut(&position).ok_or(UnsealError::Unknown)?;
        logged.unsealing = Some(Unsealing {
            name,
            holder,
            contends,
        });
        self.logged_dirty.insert(position);
        Ok(Unsealed::New { contends })
    }

    /// Renews `name` for `holder`: by its lifetime from its expiry, or from
    /// now if it has expired, once its expiry is near.
    ///
    /// # Errors
    ///
    /// The name is not held here, or not by `holder`.
    pub fn renew(
        &mut self,
        holder: &NodeId,
        name: &Name,
        now_ms: u64,
    ) -> Result<Renewal, RenewError> {
        let config = self.config;
        let granted = self
            .names
            .get_mut(name)
            .filter(|granted| granted.is_held(now_ms, config.grace_ms))
            .ok_or(RenewError::NotHeld)?;
        if granted.holder != *holder {
            return Err(RenewError::NotHolder);
        }
        if granted.expires_ms > now_ms.saturating_add(config.renew_before_ms) {
            return Ok(Renewal {
                expires_ms: granted.expires_ms,
                extended: false,
            });
        }
        granted.expires_ms = granted.expires_ms.max(now_ms).saturating_add(config.ttl_ms);
        let expires_ms = granted.expires_ms;
        self.names_dirty.insert(*name);
        self.served_changed = true;
        self.recompute_due();
        Ok(Renewal {
            expires_ms,
            extended: true,
        })
    }

    /// Releases the names listed here that `is_split` says more than f of
    /// the other registrars' fresh lists lack the holder of, once they have
    /// for longer than [`RegistrarConfig::release_after_ms`] by the checks
    /// made: a check that finds a name not split starts its wait again. A
    /// name released is free at once, and a commitment logged while it was
    /// held stays void for it (spec/areas/network.md#releasing-a-split).
    pub fn release_splits(
        &mut self,
        now_ms: u64,
        is_split: impl Fn(&Name, &NodeId) -> bool,
        happened: &mut Vec<Happened>,
    ) {
        let after = self.config.release_after_ms;
        let split: Vec<(Name, NodeId)> = self
            .names
            .values()
            .filter(|granted| granted.is_listed(now_ms))
            .filter(|granted| is_split(&granted.name, &granted.holder))
            .map(|granted| (granted.name, granted.holder))
            .collect();
        let before = core::mem::take(&mut self.splits);
        for (name, holder) in split {
            let since = before.get(&name).copied().unwrap_or(now_ms);
            if now_ms.saturating_sub(since) > after {
                self.release(name, now_ms);
                happened.push(Happened::Released { name, holder });
            } else {
                self.splits.insert(name, since);
            }
        }
    }

    /// Frees `name` now, as at the end of its grace.
    fn release(&mut self, name: Name, now_ms: u64) {
        if let Some(granted) = self.names.get_mut(&name) {
            granted.freed_ms = Some(now_ms);
            self.names_dirty.insert(name);
            self.served_changed = true;
            self.recompute_due();
        }
    }

    /// Takes in the passing of time up to `now_ms`: frees names whose grace
    /// ended, decides the commitments whose windows closed, and forgets
    /// freed names no commitment in the log can be void for any more.
    pub fn on_time(&mut self, now_ms: u64, happened: &mut Vec<Happened>) {
        if self.next_due_ms.is_none_or(|due| due > now_ms) {
            return;
        }
        self.free_names(now_ms, happened);
        self.close_windows(now_ms, happened);
        self.forget_freed();
        let names = &self.names;
        self.splits
            .retain(|name, _| names.get(name).is_some_and(|g| g.freed_ms.is_none()));
        self.processed_ms = self.processed_ms.max(now_ms);
        self.lower_ceilings();
        self.recompute_due();
    }

    /// Lowers the ceilings to what is kept, but never below the bounds: a
    /// restored log beyond them shrinks back to them as its windows close,
    /// and the bounds refuse every new commitment meanwhile.
    fn lower_ceilings(&mut self) {
        let config = &self.config;
        self.log_ceiling = self.log.len().max(config.max_commitments);
        self.names_ceiling = (self.log.len() + self.names.len()).max(config.max_names);
    }

    /// When [`on_time`](Self::on_time) next has something to do.
    #[must_use]
    pub const fn next_due(&self) -> Option<u64> {
        self.next_due_ms
    }

    fn recompute_due(&mut self) {
        let (processed, grace) = (self.processed_ms, self.config.grace_ms);
        let windows = self.log.values().map(|logged| logged.window_end_ms);
        let names = self
            .names
            .values()
            .filter(|granted| granted.freed_ms.is_none())
            .flat_map(|granted| [granted.expires_ms, granted.held_until_ms(grace)]);
        self.next_due_ms = windows.chain(names).filter(|at| *at > processed).min();
    }

    fn free_names(&mut self, now_ms: u64, happened: &mut Vec<Happened>) {
        let (processed, grace) = (self.processed_ms, self.config.grace_ms);
        for (name, granted) in &mut self.names {
            if granted.freed_ms.is_some() {
                continue;
            }
            // Out of the list once it expires.
            if processed < granted.expires_ms && granted.expires_ms <= now_ms {
                self.served_changed = true;
            }
            let until = granted.held_until_ms(grace);
            if until <= now_ms {
                granted.freed_ms = Some(until);
                self.names_dirty.insert(*name);
                happened.push(Happened::Freed {
                    name: *name,
                    holder: granted.holder,
                });
            }
        }
    }

    /// Decides the commitments whose windows closed, in the order of the
    /// log. One that contends waits while an earlier window is open.
    fn close_windows(&mut self, now_ms: u64, happened: &mut Vec<Happened>) {
        let first_open = self
            .log
            .values()
            .find(|logged| logged.window_end_ms > now_ms)
            .map_or(u64::MAX, |logged| logged.position);
        let closed: Vec<Logged> = self
            .log
            .values()
            .filter(|logged| logged.window_end_ms <= now_ms)
            .filter(|logged| {
                logged.position < first_open || !logged.unsealing.is_some_and(|u| u.contends)
            })
            .copied()
            .collect();
        for logged in closed {
            self.remove_logged(&logged);
            let Some(unsealing) = logged.unsealing else {
                happened.push(Happened::Lapsed {
                    committer: logged.committer,
                    position: logged.position,
                });
                continue;
            };
            let (name, holder) = (unsealing.name, unsealing.holder);
            let taken = self
                .names
                .get(&name)
                .is_some_and(|granted| granted.is_held(now_ms, self.config.grace_ms));
            if unsealing.contends && !taken {
                self.grant(name, holder, now_ms);
                happened.push(Happened::Granted { name, holder });
            } else {
                happened.push(Happened::Lost { name, holder });
            }
        }
    }

    fn remove_logged(&mut self, logged: &Logged) {
        self.log.remove(&logged.position);
        self.positions.remove(&logged.commitment);
        self.logged_dirty.remove(&logged.position);
        self.logged_removed.push(logged.commitment);
    }

    /// Grants `name` to `holder`: listed from now. It takes the place of
    /// its commitment, or of what was kept of it when it was freed, so the
    /// bound on names holds.
    fn grant(&mut self, name: Name, holder: NodeId, now_ms: u64) {
        let granted = Granted {
            name,
            holder,
            granted_ms: now_ms,
            expires_ms: now_ms.saturating_add(self.config.ttl_ms),
            freed_ms: None,
        };
        self.names.insert(name, granted);
        assert!(
            self.names.len() + self.log.len() <= self.names_ceiling,
            "a name granted takes its commitment's place"
        );
        self.names_removed.retain(|removed| *removed != name);
        self.names_dirty.insert(name);
        self.served_changed = true;
    }

    /// Forgets the freed names no commitment in the log was logged before
    /// they were freed, and so none can be void for.
    fn forget_freed(&mut self) {
        // Logged times rise with positions.
        let oldest = self.log.values().next().map(|logged| logged.logged_ms);
        let forgotten: Vec<Name> = self
            .names
            .values()
            .filter(|granted| {
                granted
                    .freed_ms
                    .is_some_and(|freed| oldest.is_none_or(|oldest| oldest >= freed))
            })
            .map(|granted| granted.name)
            .collect();
        for name in forgotten {
            self.names.remove(&name);
            self.names_dirty.remove(&name);
            self.names_removed.push(name);
        }
    }

    /// The list served, encoded, and its hash: the names listed now. Built
    /// again only when it may have changed.
    pub fn served(&mut self, now_ms: u64) -> (&[u8], [u8; 32]) {
        if self.served_changed {
            self.served_changed = false;
            let listed: BTreeMap<Name, NameHolding> = self
                .names
                .values()
                .filter(|granted| granted.is_listed(now_ms))
                .take(MAX_NAME_LIST)
                .map(|granted| {
                    let holding = NameHolding {
                        holder: granted.holder,
                        expires_at_ms: granted.expires_ms,
                    };
                    (granted.name, holding)
                })
                .collect();
            self.served = NameListResponse(listed).to_vec();
            self.served_hash = Sha256::digest(&self.served).into();
        }
        (&self.served, self.served_hash)
    }

    /// What changed since the last call, for the same transaction as the
    /// ledger's.
    pub fn take_changes(&mut self, batch: &mut PeerBatch) {
        if core::mem::take(&mut self.head_dirty) {
            batch.name_log = Some(self.head);
        }
        batch.commitments_removed.append(&mut self.logged_removed);
        for position in core::mem::take(&mut self.logged_dirty) {
            if let Some(logged) = self.log.get(&position) {
                batch.commitments.push(*logged);
            }
        }
        batch.names_removed.append(&mut self.names_removed);
        for name in core::mem::take(&mut self.names_dirty) {
            if let Some(granted) = self.names.get(&name) {
                batch.names.push(*granted);
            }
        }
    }

    /// Checks the registrar's invariants (spec/decisions/tigerstyle.md).
    /// The log and the names stay within their bounds, or, after a restore
    /// of more than lowered bounds allow, within what was restored: nothing
    /// new is taken in until they are back within the bounds.
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        let config = &self.config;
        assert!(
            config.max_commitments <= self.log_ceiling && config.max_names <= self.names_ceiling,
            "the ceilings never fall below the bounds"
        );
        assert!(
            self.log.len() <= self.log_ceiling
                && self.log.len() + self.names.len() <= self.names_ceiling,
            "the log and the names within bounds, or within what was restored"
        );
        assert_eq!(
            self.log.len(),
            self.positions.len(),
            "a position per commitment"
        );
        let mut previous: Option<&Logged> = None;
        for (position, logged) in &self.log {
            assert!(
                *position == logged.position
                    && self.positions.get(&logged.commitment) == Some(position),
                "the log is indexed by position and by commitment"
            );
            assert!(
                logged.position < self.head.next_position
                    && logged.logged_ms <= self.head.last_logged_ms,
                "the head is ahead of the log"
            );
            assert!(
                previous.is_none_or(|p| p.logged_ms < logged.logged_ms),
                "logged times rise with positions"
            );
            assert!(
                logged.window_end_ms >= logged.logged_ms,
                "a window closes after it opens"
            );
            previous = Some(logged);
        }
        for (name, granted) in &self.names {
            assert!(*name == granted.name, "names are indexed by name");
            assert!(
                granted.expires_ms >= granted.granted_ms,
                "a name expires after it is granted"
            );
        }
        assert!(
            self.splits
                .keys()
                .all(|name| self.names.get(name).is_some_and(|g| g.freed_ms.is_none())),
            "only names held wait to be released"
        );
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use dassie_identity::NodeKey;

    use super::*;

    const START: u64 = 1_800_000_000_000;

    const CONFIG: RegistrarConfig = RegistrarConfig {
        window_ms: 1_000,
        ttl_ms: 100_000,
        grace_ms: 10_000,
        renew_before_ms: 10_000,
        release_after_ms: 5_000,
        ..RegistrarConfig::DEFAULT
    };

    fn node(seed: u8) -> NodeId {
        NodeKey::from_bytes(&[seed; 32]).node_id()
    }

    fn alice() -> Name {
        Name::parse("alice").unwrap()
    }

    fn registrar() -> Registrar {
        Registrar::restore(CONFIG, LogHead::default(), vec![], vec![], START)
    }

    /// Seals `name` for `holder` and logs it, from `holder`.
    fn commit(registrar: &mut Registrar, holder: u8, name: &Name, now: u64) -> NameReceipt {
        let commitment = Commitment::seal(name, &node(holder), &Salt([holder; 32]));
        match registrar.commit(commitment, node(holder), None, now) {
            Ok(Committed::New(receipt)) => receipt,
            other => panic!("{other:?}"),
        }
    }

    fn unseal(
        registrar: &mut Registrar,
        holder: u8,
        name: &Name,
        now: u64,
    ) -> Result<Unsealed, UnsealError> {
        registrar.unseal(node(holder), *name, &Salt([holder; 32]), now)
    }

    fn run(registrar: &mut Registrar, now: u64) -> Vec<Happened> {
        let mut happened = Vec::new();
        registrar.on_time(now, &mut happened);
        registrar.check_invariants();
        happened
    }

    fn holder(registrar: &mut Registrar, name: &Name, now: u64) -> Option<NodeId> {
        let (served, _) = registrar.served(now);
        let list = <NameListResponse as dassie_oer::Decode>::decode_exact(served).unwrap();
        list.0.get(name).map(|holding| holding.holder)
    }

    // spec/designs/names.md#4-the-rule-at-each-registrar: the earliest
    // commitment wins, even unsealed last, and only once its window closes.
    #[test]
    fn the_earliest_commitment_wins_however_late_it_is_unsealed() {
        let mut registrar = registrar();
        let first = commit(&mut registrar, 1, &alice(), START);
        let second = commit(&mut registrar, 2, &alice(), START + 10);
        assert!(first.position < second.position);
        assert_eq!(first.window_end_ms, START + CONFIG.window_ms);
        assert_eq!(
            unseal(&mut registrar, 2, &alice(), START + 20),
            Ok(Unsealed::New { contends: true })
        );
        assert_eq!(
            unseal(&mut registrar, 1, &alice(), START + 500),
            Ok(Unsealed::New { contends: true })
        );
        // Nothing is shown before the window closes.
        assert!(run(&mut registrar, first.window_end_ms - 1).is_empty());
        assert_eq!(holder(&mut registrar, &alice(), START + 999), None);
        let happened = run(&mut registrar, first.window_end_ms);
        assert_eq!(
            happened,
            [Happened::Granted {
                name: alice(),
                holder: node(1)
            }]
        );
        assert_eq!(registrar.next_due(), Some(second.window_end_ms));
        let happened = run(&mut registrar, second.window_end_ms);
        assert_eq!(
            happened,
            [Happened::Lost {
                name: alice(),
                holder: node(2)
            }]
        );
        assert_eq!(
            holder(&mut registrar, &alice(), START + 2_000),
            Some(node(1))
        );
    }

    // A commitment not unsealed in its window lapses, and an earlier one
    // that lapses takes nothing from a later one.
    #[test]
    fn a_commitment_not_unsealed_lapses() {
        let mut registrar = registrar();
        let first = commit(&mut registrar, 1, &alice(), START);
        let second = commit(&mut registrar, 2, &alice(), START + 10);
        unseal(&mut registrar, 2, &alice(), START + 20).unwrap();
        assert_eq!(
            unseal(&mut registrar, 1, &alice(), first.window_end_ms),
            Err(UnsealError::Closed)
        );
        assert_eq!(
            run(&mut registrar, first.window_end_ms),
            [Happened::Lapsed {
                committer: node(1),
                position: first.position
            }]
        );
        assert_eq!(
            run(&mut registrar, second.window_end_ms),
            [Happened::Granted {
                name: alice(),
                holder: node(2)
            }]
        );
        assert_eq!(
            unseal(&mut registrar, 1, &alice(), START + 5_000),
            Err(UnsealError::Unknown)
        );
    }

    // A commitment opens only for the node named in it, with its name and
    // salt, and an unsealing is taken once.
    #[test]
    fn only_the_holder_opens_its_commitment() {
        let mut registrar = registrar();
        commit(&mut registrar, 1, &alice(), START);
        let bob = Name::parse("bob").unwrap();
        assert_eq!(
            registrar.unseal(node(2), alice(), &Salt([1; 32]), START + 1),
            Err(UnsealError::Unknown)
        );
        assert_eq!(
            unseal(&mut registrar, 1, &bob, START + 1),
            Err(UnsealError::Unknown)
        );
        let www = Name::parse("www").unwrap();
        assert_eq!(
            unseal(&mut registrar, 1, &www, START + 1),
            Err(UnsealError::Reserved)
        );
        assert!(unseal(&mut registrar, 1, &alice(), START + 1).is_ok());
        assert_eq!(
            unseal(&mut registrar, 1, &alice(), START + 2),
            Ok(Unsealed::Again)
        );
    }

    // An unsealing taken just before its window closed, whose answer was
    // lost, is answered as taken when retried after, for as long as its
    // commitment is in the log.
    #[test]
    fn a_retried_unsealing_is_answered_as_the_first_was() {
        let mut registrar = registrar();
        let receipt = commit(&mut registrar, 1, &alice(), START);
        let end = receipt.window_end_ms;
        assert_eq!(
            unseal(&mut registrar, 1, &alice(), end - 1),
            Ok(Unsealed::New { contends: true })
        );
        assert_eq!(
            unseal(&mut registrar, 1, &alice(), end),
            Ok(Unsealed::Again)
        );
        assert_eq!(
            run(&mut registrar, end),
            [Happened::Granted {
                name: alice(),
                holder: node(1)
            }]
        );
        assert_eq!(
            unseal(&mut registrar, 1, &alice(), end + 1),
            Err(UnsealError::Unknown)
        );
    }

    // The cap on names is the holder's: commitments other nodes log for it
    // do not take it past the cap, and a name it holds counts as one.
    #[test]
    fn the_cap_on_names_is_the_holders_whoever_commits() {
        let config = RegistrarConfig {
            max_names_per_node: 2,
            ..CONFIG
        };
        let mut registrar = Registrar::restore(config, LogHead::default(), vec![], vec![], START);
        let names = ["aaa", "bbb", "ccc"].map(|text| Name::parse(text).unwrap());
        let salt = Salt([9; 32]);
        let mut first = None;
        for (from, name) in (1..).zip(&names) {
            let commitment = Commitment::seal(name, &node(9), &salt);
            let committed = registrar.commit(commitment, node(from), None, START);
            let Ok(Committed::New(receipt)) = committed else {
                panic!("{committed:?}");
            };
            first.get_or_insert(receipt);
        }
        let unseal = |registrar: &mut Registrar, name: &Name, now| {
            registrar.unseal(node(9), *name, &salt, now)
        };
        assert!(unseal(&mut registrar, &names[0], START + 1).is_ok());
        assert!(unseal(&mut registrar, &names[1], START + 1).is_ok());
        assert_eq!(
            unseal(&mut registrar, &names[2], START + 1),
            Err(UnsealError::Names)
        );
        let end = first.unwrap().window_end_ms;
        run(&mut registrar, end + 2);
        let held: Vec<Name> = registrar
            .granted()
            .filter(|granted| granted.holder == node(9))
            .map(|granted| granted.name)
            .collect();
        assert_eq!(held, names[..2]);
        // Holding the names, it still commits none past the cap.
        let more = Commitment::seal(&names[2], &node(9), &salt);
        assert_eq!(
            registrar.commit(more, node(9), None, end + 2),
            Err(CommitError::Names)
        );
    }

    // A commitment logged while the name is listed, or in its grace period,
    // is void for it; one logged after the name is free is not.
    #[test]
    fn a_commitment_logged_while_the_name_is_held_is_void() {
        let mut registrar = registrar();
        let won = commit(&mut registrar, 1, &alice(), START);
        unseal(&mut registrar, 1, &alice(), START + 1).unwrap();
        run(&mut registrar, won.window_end_ms);
        let granted = *registrar.granted().next().unwrap();
        commit(&mut registrar, 2, &alice(), START + 2_000);
        assert_eq!(
            unseal(&mut registrar, 2, &alice(), START + 2_001),
            Err(UnsealError::Taken)
        );
        // In grace, out of the list: still void.
        let grace_at = granted.expires_ms + 1;
        run(&mut registrar, grace_at);
        assert_eq!(holder(&mut registrar, &alice(), grace_at), None);
        let free_at = granted.held_until_ms(CONFIG.grace_ms);
        commit(&mut registrar, 3, &alice(), free_at - 10);
        assert_eq!(
            unseal(&mut registrar, 3, &alice(), free_at - 9),
            Err(UnsealError::Taken)
        );
        // Free: still void, since it was logged while the name was held,
        // but one logged now contends, and wins.
        let happened = run(&mut registrar, free_at);
        assert!(happened.contains(&Happened::Freed {
            name: alice(),
            holder: node(1)
        }));
        assert_eq!(
            unseal(&mut registrar, 3, &alice(), free_at + 1),
            Err(UnsealError::Taken)
        );
        let late = commit(&mut registrar, 4, &alice(), free_at + 2);
        unseal(&mut registrar, 4, &alice(), free_at + 3).unwrap();
        run(&mut registrar, late.window_end_ms);
        assert_eq!(
            holder(&mut registrar, &alice(), late.window_end_ms),
            Some(node(4))
        );
    }

    // A commitment logged before the name was granted to another, and
    // unsealed after, has lost: it is answered as taken, and lost when its
    // window closes.
    #[test]
    fn a_later_commitment_unsealed_after_the_grant_loses() {
        let mut registrar = registrar();
        let won = commit(&mut registrar, 1, &alice(), START);
        let later = commit(&mut registrar, 2, &alice(), START + 500);
        unseal(&mut registrar, 1, &alice(), START + 1).unwrap();
        run(&mut registrar, won.window_end_ms);
        assert_eq!(
            unseal(&mut registrar, 2, &alice(), won.window_end_ms + 1),
            Ok(Unsealed::New { contends: false })
        );
        assert_eq!(
            run(&mut registrar, later.window_end_ms),
            [Happened::Lost {
                name: alice(),
                holder: node(2)
            }]
        );
    }

    // A name expires unless its holder renews it near its expiry, or in its
    // grace period; only the holder renews.
    #[test]
    fn names_are_renewed_by_their_holders_alone() {
        let mut registrar = registrar();
        let won = commit(&mut registrar, 1, &alice(), START);
        unseal(&mut registrar, 1, &alice(), START + 1).unwrap();
        run(&mut registrar, won.window_end_ms);
        let expires = registrar.granted().next().unwrap().expires_ms;
        assert_eq!(expires, won.window_end_ms + CONFIG.ttl_ms);
        let (_, hash) = registrar.served(won.window_end_ms);
        // Early: the expiry it has, and the list is the same.
        let renewed = |expires_ms, extended| {
            Ok(Renewal {
                expires_ms,
                extended,
            })
        };
        assert_eq!(
            registrar.renew(&node(1), &alice(), START + 2),
            renewed(expires, false)
        );
        assert_eq!(registrar.served(START + 2).1, hash);
        assert_eq!(
            registrar.renew(&node(2), &alice(), expires - 1),
            Err(RenewError::NotHolder)
        );
        let near = expires - CONFIG.renew_before_ms;
        assert_eq!(
            registrar.renew(&node(1), &alice(), near),
            renewed(expires + CONFIG.ttl_ms, true)
        );
        assert_ne!(registrar.served(near).1, hash);
        // In grace, from now.
        let expires = expires + CONFIG.ttl_ms;
        run(&mut registrar, expires);
        assert_eq!(holder(&mut registrar, &alice(), expires), None);
        let late = expires + CONFIG.grace_ms - 1;
        assert_eq!(
            registrar.renew(&node(1), &alice(), late),
            renewed(late + CONFIG.ttl_ms, true)
        );
        assert_eq!(holder(&mut registrar, &alice(), late), Some(node(1)));
        // Freed after its grace, it is renewed by no one.
        let gone = late + CONFIG.ttl_ms + CONFIG.grace_ms;
        run(&mut registrar, gone);
        assert_eq!(
            registrar.renew(&node(1), &alice(), gone),
            Err(RenewError::NotHeld)
        );
        // Nothing in the log could be void for it: it is forgotten.
        assert_eq!(registrar.granted().count(), 0);
    }

    // The log's positions and times only rise, whatever the clock says.
    #[test]
    fn positions_and_times_only_rise() {
        let mut registrar = registrar();
        let bob = Name::parse("bob").unwrap();
        let first = commit(&mut registrar, 1, &alice(), START + 100);
        let second = commit(&mut registrar, 2, &bob, START);
        assert_eq!(second.position, first.position + 1);
        assert_eq!(second.window_end_ms, first.window_end_ms + 1);
        let head = registrar.head();
        assert_eq!(
            head,
            LogHead {
                next_position: 2,
                last_logged_ms: START + 101
            }
        );
        // Restored from a head ahead of its log, it stays ahead.
        let restored = Registrar::restore(
            CONFIG,
            LogHead {
                next_position: 9,
                last_logged_ms: START + 500,
            },
            registrar.logged().copied().collect(),
            vec![],
            START,
        );
        assert_eq!(restored.head().next_position, 9);
        restored.check_invariants();
    }

    // The caps: per node, per network, and the rate, spent only on what is
    // logged; a duplicate costs nothing.
    #[test]
    fn commitments_are_capped() {
        let config = RegistrarConfig {
            commit_rate: Rate {
                burst: 6,
                refill_ms: 60_000,
            },
            max_commitments_per_network: 3,
            max_commitments_per_node: 2,
            max_names_per_node: 3,
            ..CONFIG
        };
        let mut registrar = Registrar::restore(config, LogHead::default(), vec![], vec![], START);
        let network = Some(Network([4, 198, 51, 100, 0, 0, 0]));
        let c = |seed: u8| Commitment([seed; 32]);
        let mut commit =
            |seed, from, network| registrar.commit(c(seed), node(from), network, START);
        assert!(commit(1, 1, None).is_ok());
        assert!(matches!(commit(1, 2, None), Ok(Committed::Again(_))));
        assert!(commit(2, 1, network).is_ok());
        assert_eq!(commit(3, 1, None), Err(CommitError::Commitments));
        assert!(commit(4, 2, network).is_ok());
        assert!(commit(5, 3, network).is_ok());
        assert_eq!(commit(6, 4, network), Err(CommitError::Network));
        assert!(commit(7, 4, None).is_ok());
        assert!(commit(8, 5, None).is_ok());
        assert_eq!(commit(9, 6, None), Err(CommitError::Rate));
    }

    // spec/areas/network.md#releasing-a-split: a name the others' lists
    // show split for longer than the wait, check after check, is free at
    // once; a check that finds it whole starts the wait again, and a
    // commitment logged while it was held stays void for it.
    #[test]
    fn a_split_is_released_after_the_wait() {
        let mut registrar = registrar();
        let won = commit(&mut registrar, 1, &alice(), START);
        unseal(&mut registrar, 1, &alice(), START + 1).unwrap();
        let granted_at = won.window_end_ms;
        run(&mut registrar, granted_at);
        let void = commit(&mut registrar, 2, &alice(), granted_at + 1);
        let after = CONFIG.release_after_ms;
        let mut happened = Vec::new();
        let mut check = |registrar: &mut Registrar, split: bool, now: u64| {
            registrar.release_splits(now, |_, _| split, &mut happened);
            registrar.check_invariants();
            core::mem::take(&mut happened)
        };
        assert!(check(&mut registrar, true, granted_at).is_empty());
        assert!(check(&mut registrar, true, granted_at + after).is_empty());
        // Whole for a moment: the wait starts again.
        assert!(check(&mut registrar, false, granted_at + after).is_empty());
        let again = granted_at + after + 1;
        assert!(check(&mut registrar, true, again).is_empty());
        assert!(check(&mut registrar, true, again + after).is_empty());
        assert_eq!(
            check(&mut registrar, true, again + after + 1),
            [Happened::Released {
                name: alice(),
                holder: node(1)
            }]
        );
        let released = again + after + 1;
        assert_eq!(holder(&mut registrar, &alice(), released), None);
        assert_eq!(
            registrar.renew(&node(1), &alice(), released),
            Err(RenewError::NotHeld)
        );
        assert_eq!(
            unseal(&mut registrar, 2, &alice(), void.window_end_ms - 1),
            Err(UnsealError::Taken)
        );
        // Free: a commitment logged now may have it.
        let late = commit(&mut registrar, 3, &alice(), released + 1);
        unseal(&mut registrar, 3, &alice(), released + 2).unwrap();
        run(&mut registrar, late.window_end_ms);
        assert_eq!(
            holder(&mut registrar, &alice(), late.window_end_ms),
            Some(node(3))
        );
    }

    // The log and the names it may become stay within their bounds.
    #[test]
    fn the_log_is_bounded() {
        let config = RegistrarConfig {
            max_commitments: 2,
            max_names: 3,
            ..CONFIG
        };
        let mut registrar = Registrar::restore(config, LogHead::default(), vec![], vec![], START);
        let names = ["aaa", "bbb", "ccc"].map(|text| Name::parse(text).unwrap());
        let first = commit(&mut registrar, 1, &names[0], START);
        commit(&mut registrar, 2, &names[1], START);
        let full = Commitment::seal(&names[2], &node(3), &Salt([3; 32]));
        assert_eq!(
            registrar.commit(full, node(3), None, START),
            Err(CommitError::LogFull)
        );
        unseal(&mut registrar, 1, &names[0], START + 1).unwrap();
        unseal(&mut registrar, 2, &names[1], START + 1).unwrap();
        run(&mut registrar, first.window_end_ms + 1);
        assert_eq!(registrar.granted().count(), 2);
        commit(&mut registrar, 3, &names[2], START + 2_000);
        let more = Commitment::seal(&names[2], &node(4), &Salt([4; 32]));
        assert_eq!(
            registrar.commit(more, node(4), None, START + 2_000),
            Err(CommitError::NamesFull)
        );
    }

    // spec/areas/network.md#the-registrar: a restart under bounds lowered
    // since loses no receipted commitment and no name; the bounds refuse
    // new commitments instead, until there is room again.
    #[test]
    fn a_restore_beyond_the_bounds_loses_nothing() {
        let wide = RegistrarConfig {
            max_commitments: 4,
            max_names: 6,
            ..CONFIG
        };
        let mut registrar = Registrar::restore(wide, LogHead::default(), vec![], vec![], START);
        let held = commit(&mut registrar, 1, &alice(), START);
        unseal(&mut registrar, 1, &alice(), START + 1).unwrap();
        run(&mut registrar, held.window_end_ms);
        let names = ["bbb", "ccc", "ddd", "eee"].map(|text| Name::parse(text).unwrap());
        let later = START + 2_000;
        let receipts: Vec<NameReceipt> = (2..)
            .zip(&names)
            .map(|(seed, name)| commit(&mut registrar, seed, name, later))
            .collect();
        let (head, log) = (registrar.head(), registrar.logged().copied().collect());
        let granted = registrar.granted().copied().collect();

        let narrow = RegistrarConfig {
            max_commitments: 2,
            max_names: 3,
            ..CONFIG
        };
        let mut registrar = Registrar::restore(narrow, head, log, granted, later + 1);
        registrar.check_invariants();
        assert_eq!(registrar.logged().count(), 4);
        assert_eq!(registrar.granted().count(), 1);
        for ((seed, name), receipt) in (2..).zip(&names).zip(&receipts) {
            let commitment = Commitment::seal(name, &node(seed), &Salt([seed; 32]));
            assert_eq!(registrar.receipt(&commitment), Some(*receipt));
            assert_eq!(
                registrar.commit(commitment, node(seed), None, later + 1),
                Ok(Committed::Again(*receipt))
            );
        }
        let new = Commitment::seal(&alice(), &node(9), &Salt([9; 32]));
        assert_eq!(
            registrar.commit(new, node(9), None, later + 1),
            Err(CommitError::LogFull)
        );
        for (seed, name) in (2..).zip(&names) {
            assert_eq!(
                unseal(&mut registrar, seed, name, later + 2),
                Ok(Unsealed::New { contends: true })
            );
        }
        let closed = receipts[3].window_end_ms;
        let happened = run(&mut registrar, closed);
        assert_eq!(happened.len(), 4);
        for (seed, name) in (2..).zip(&names) {
            assert_eq!(holder(&mut registrar, name, closed), Some(node(seed)));
        }
        assert_eq!(holder(&mut registrar, &alice(), closed), Some(node(1)));
        // Five names held, more than the three now allowed: still no room.
        assert_eq!(
            registrar.commit(new, node(9), None, closed),
            Err(CommitError::NamesFull)
        );
    }
}

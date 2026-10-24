//! Who holds a name, in a node's view (spec/designs/names.md#5-holding-a-name,
//! spec/areas/network.md#holders): the lists the configured registrars
//! serve, and for each name the holder a quorum of them list.
//!
//! - **The quorum** is n - f of the n configured registrars, not of those
//!   that answer, where f, the faulty registrars tolerated, is the largest
//!   with n ≥ 3f + 1. Any two quorums share n - 2f ≥ f + 1 registrars, at
//!   least one of them honest, which lists one holder for a name, so no
//!   faulty registrar that shows nodes different lists can lead two of
//!   them to different holders. Up to three registrars tolerate none, and
//!   need all of them.
//! - **A list votes** only while it is fresh. An honest registrar lists
//!   another holder for a name only a window or more after it stopped
//!   listing the last, when it released the name or its grace ended, and
//!   a list stays fresh for less than a window (`Config::validate`), so
//!   the lists two nodes vote with show such a registrar listing one holder
//!   at most. A list read before a release would otherwise lend a faulty
//!   registrar the honest one's vote for a holder it no longer lists. The
//!   window is by the registrar's clock: this assumes it never steps
//!   forward by more than the margin between a window and freshness.
//! - **The view freezes** while fewer than a quorum of the lists are fresh:
//!   it keeps the holders it had, as membership does, so registrars going
//!   down, or a node cut off from them, do not take every name away.
//!
//! Nothing here depends on the peer node: a front door can keep a view of
//! its own (spec/designs/names.md#8-open-questions).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use dassie_identity::NodeId;

use super::Name;
use crate::wire::{MAX_NAME_LIST, NameHolding, NameListResponse};

/// Most registrars whose lists a node reads: the bootstrap nodes it is
/// configured with, which `dassie-node` bounds by this.
pub const MAX_REGISTRARS: usize = 64;

/// Most names a node takes a holder for. Each of n lists names at most
/// [`MAX_NAME_LIST`], and a holder needs a quorum of n - f > 2n/3 of them,
/// so no view can hold more than one and a half lists' worth.
pub const MAX_HOLDERS: usize = MAX_NAME_LIST * 3 / 2;

/// The faulty registrars `registrars` tolerate: the largest f with
/// `registrars` ≥ 3f + 1. None for up to three.
#[must_use]
pub const fn faults_tolerated(registrars: usize) -> usize {
    registrars.saturating_sub(1) / 3
}

/// The lists that make a holder, of `registrars` configured: n - f. All of
/// them for up to three. More than two thirds of them, so a quorum names at
/// most one holder for a name.
#[must_use]
pub const fn quorum(registrars: usize) -> usize {
    registrars - faults_tolerated(registrars)
}

/// How long a registrar's list counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldersConfig {
    /// A list fetched this recently is fresh: only fresh lists vote, and
    /// holders are taken only while a quorum of the lists are fresh. With
    /// the time a fetch takes, less than the registrars' window.
    pub list_fresh_ms: u64,
}

/// A name and its holder, in a node's view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldName {
    /// The name.
    pub name: Name,
    /// The node that a quorum of the registrars list for it.
    pub holder: NodeId,
    /// The latest expiry a quorum of those lists give it, by the
    /// registrars' clocks: a frozen view may keep the name past it.
    pub expires_ms: u64,
}

/// What a new view changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HolderChange {
    /// A name has this holder now: it had none, or another.
    Held {
        /// The name.
        name: Name,
        /// Its holder.
        holder: NodeId,
    },
    /// A name has no holder any more.
    Unheld {
        /// The name.
        name: Name,
        /// Its holder until now.
        holder: NodeId,
    },
}

/// A registrar's list, as last fetched.
#[derive(Clone, Debug, Default)]
struct Fetched {
    names: BTreeMap<Name, NameHolding>,
    hash: Option<[u8; 32]>,
    /// When its content was last known to be current.
    fetched_ms: Option<u64>,
}

/// A node's view of who holds each name, and the registrars' lists it is
/// taken from.
pub struct Holders {
    config: HoldersConfig,
    lists: BTreeMap<NodeId, Fetched>,
    held: BTreeMap<Name, HeldName>,
    frozen: bool,
    dirty: BTreeSet<Name>,
    removed: Vec<Name>,
}

impl Holders {
    /// A view of the lists of `registrars`, the configured ones, starting
    /// from `held`, the view persisted. Until a quorum of the lists are
    /// fetched it is frozen, so a restart takes no name away. With no
    /// registrars, no list can ever confirm what was persisted: the view is
    /// empty, and what was persisted is removed.
    ///
    /// # Panics
    ///
    /// With more than [`MAX_REGISTRARS`] registrars (configuration, not
    /// input).
    #[must_use]
    pub fn new(
        config: HoldersConfig,
        registrars: impl IntoIterator<Item = NodeId>,
        held: Vec<HeldName>,
    ) -> Self {
        let lists: BTreeMap<NodeId, Fetched> = registrars
            .into_iter()
            .map(|id| (id, Fetched::default()))
            .collect();
        assert!(lists.len() <= MAX_REGISTRARS, "too many registrars");
        let held = held.into_iter().take(MAX_HOLDERS);
        let (held, removed) = if lists.is_empty() {
            (BTreeMap::new(), held.map(|held| held.name).collect())
        } else {
            (held.map(|held| (held.name, held)).collect(), Vec::new())
        };
        Self {
            config,
            frozen: !lists.is_empty(),
            lists,
            held,
            dirty: BTreeSet::new(),
            removed,
        }
    }

    /// How many registrars are configured.
    #[must_use]
    pub fn registrars(&self) -> usize {
        self.lists.len()
    }

    /// The hash of `registrar`'s list as last fetched.
    #[must_use]
    pub fn hash(&self, registrar: &NodeId) -> Option<[u8; 32]> {
        self.lists.get(registrar).and_then(|list| list.hash)
    }

    /// When `registrar`'s list was last known to be current.
    #[must_use]
    pub fn fetched_ms(&self, registrar: &NodeId) -> Option<u64> {
        self.lists.get(registrar).and_then(|list| list.fetched_ms)
    }

    /// `registrar`'s list is as last fetched, at `now_ms`: its hash has not
    /// changed.
    pub fn confirm(&mut self, registrar: &NodeId, now_ms: u64) {
        if let Some(list) = self.lists.get_mut(registrar) {
            list.fetched_ms = Some(now_ms);
        }
    }

    /// Takes `list`, whose bytes hash to `hash`, as `registrar`'s, fetched at
    /// `now_ms`. Its authenticity is the caller's to check. A list from a
    /// node that is no configured registrar, or longer than any registrar
    /// serves, is not taken. Returns whether it was.
    pub fn take_list(
        &mut self,
        registrar: &NodeId,
        list: NameListResponse,
        hash: [u8; 32],
        now_ms: u64,
    ) -> bool {
        if list.0.len() > MAX_NAME_LIST {
            return false;
        }
        let Some(fetched) = self.lists.get_mut(registrar) else {
            return false;
        };
        *fetched = Fetched {
            names: list.0,
            hash: Some(hash),
            fetched_ms: Some(now_ms),
        };
        true
    }

    fn is_fresh(&self, list: &Fetched, now_ms: u64) -> bool {
        list.fetched_ms
            .is_some_and(|at| at.saturating_add(self.config.list_fresh_ms) >= now_ms)
    }

    /// Takes the holders again at `now_ms`, from the fresh lists, unless
    /// fewer than a quorum of them are: then the view stays as it is. What
    /// changed goes to `changes`.
    pub fn recompute(&mut self, now_ms: u64, changes: &mut Vec<HolderChange>) {
        let total = self.lists.len();
        if total == 0 {
            return;
        }
        let quorum = quorum(total);
        let fresh = self
            .lists
            .values()
            .filter(|list| self.is_fresh(list, now_ms))
            .count();
        self.frozen = fresh < quorum;
        if self.frozen {
            return;
        }
        // Each list names a name once, so each vote is a registrar's.
        let mut votes: BTreeMap<(Name, NodeId), Vec<u64>> = BTreeMap::new();
        for list in self
            .lists
            .values()
            .filter(|list| self.is_fresh(list, now_ms))
        {
            for (name, holding) in &list.names {
                votes
                    .entry((*name, holding.holder))
                    .or_default()
                    .push(holding.expires_at_ms);
            }
        }
        let mut held = BTreeMap::new();
        for ((name, holder), mut expiries) in votes {
            expiries.sort_unstable_by(|a, b| b.cmp(a));
            let Some(&expires_ms) = expiries.get(quorum - 1) else {
                continue;
            };
            let previous = held.insert(
                name,
                HeldName {
                    name,
                    holder,
                    expires_ms,
                },
            );
            assert!(previous.is_none(), "a quorum lists one holder for a name");
        }
        assert!(held.len() <= MAX_HOLDERS, "holders within bounds");
        self.replace(held, changes);
    }

    /// Takes `held` as the view, noting what changed.
    fn replace(&mut self, held: BTreeMap<Name, HeldName>, changes: &mut Vec<HolderChange>) {
        for (name, before) in &self.held {
            if !held.contains_key(name) {
                changes.push(HolderChange::Unheld {
                    name: *name,
                    holder: before.holder,
                });
                self.dirty.remove(name);
                self.removed.push(*name);
            }
        }
        for (name, now) in &held {
            let before = self.held.get(name);
            if before.is_none_or(|before| before.holder != now.holder) {
                changes.push(HolderChange::Held {
                    name: *name,
                    holder: now.holder,
                });
            }
            if before != Some(now) {
                self.removed.retain(|removed| removed != name);
                self.dirty.insert(*name);
            }
        }
        self.held = held;
    }

    /// Whether more than f of the registrars' fresh lists other than
    /// `registrar`'s lack `holder` for `name`, at `now_ms`: they list nobody
    /// for it, or another node. A list not fresh counts for nothing. That is
    /// what releases a split at a registrar
    /// (spec/areas/network.md#releasing-a-split): a holder with a quorum is
    /// missing from at most f lists, however late they are.
    #[must_use]
    pub fn is_split(&self, registrar: &NodeId, name: &Name, holder: &NodeId, now_ms: u64) -> bool {
        let lacking = self
            .lists
            .iter()
            .filter(|(id, list)| *id != registrar && self.is_fresh(list, now_ms))
            .filter(|(_, list)| {
                list.names
                    .get(name)
                    .is_none_or(|holding| holding.holder != *holder)
            })
            .count();
        lacking > faults_tolerated(self.lists.len())
    }

    /// How many of the lists fresh at `now_ms` name `holder` for `name`.
    #[must_use]
    pub fn votes(&self, name: &Name, holder: &NodeId, now_ms: u64) -> usize {
        self.lists
            .values()
            .filter(|list| self.is_fresh(list, now_ms))
            .filter(|list| list.names.get(name).is_some_and(|h| h.holder == *holder))
            .count()
    }

    /// What `registrar`'s list, as last fetched, says of `name`: whom it
    /// lists for it, and until when.
    #[must_use]
    pub fn listing(&self, registrar: &NodeId, name: &Name) -> Option<&NameHolding> {
        self.lists.get(registrar)?.names.get(name)
    }

    /// Whether `registrar`'s list is fresh at `now_ms`, and so votes.
    #[must_use]
    pub fn is_fresh_list(&self, registrar: &NodeId, now_ms: u64) -> bool {
        self.lists
            .get(registrar)
            .is_some_and(|list| self.is_fresh(list, now_ms))
    }

    /// The holder of `name`, if the view has one.
    #[must_use]
    pub fn holder(&self, name: &Name) -> Option<&HeldName> {
        self.held.get(name)
    }

    /// Every name with a holder, by name.
    pub fn held(&self) -> impl Iterator<Item = &HeldName> {
        self.held.values()
    }

    /// Whether the view is frozen: fewer than a quorum of the lists were
    /// fresh when it was last taken.
    #[must_use]
    pub const fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// What changed since the last call, for the same transaction as the
    /// ledger's: names whose holder or expiry changed, as they are now, and
    /// names that have none any more.
    pub fn take_changes(&mut self, changed: &mut Vec<HeldName>, removed: &mut Vec<Name>) {
        removed.append(&mut self.removed);
        for name in core::mem::take(&mut self.dirty) {
            if let Some(held) = self.held.get(&name) {
                changed.push(*held);
            }
        }
    }

    /// Checks the view's invariants (spec/decisions/tigerstyle.md).
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        assert!(
            self.lists.len() <= MAX_REGISTRARS,
            "registrars within bounds"
        );
        assert!(
            self.lists
                .values()
                .all(|list| list.names.len() <= MAX_NAME_LIST),
            "lists within bounds"
        );
        assert!(self.held.len() <= MAX_HOLDERS, "holders within bounds");
        for (name, held) in &self.held {
            assert!(*name == held.name, "holders are indexed by name");
        }
        assert!(
            self.dirty.iter().all(|name| self.held.contains_key(name)),
            "only names held are written"
        );
        assert!(
            self.removed
                .iter()
                .all(|name| !self.held.contains_key(name)),
            "a name removed is not held"
        );
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use dassie_identity::NodeKey;

    use super::*;

    const START: u64 = 1_800_000_000_000;

    const CONFIG: HoldersConfig = HoldersConfig {
        list_fresh_ms: 10_000,
    };

    fn node(seed: u8) -> NodeId {
        NodeKey::from_bytes(&[seed; 32]).node_id()
    }

    fn alice() -> Name {
        Name::parse("alice").unwrap()
    }

    /// Registrars 1 to `count`.
    fn holders(count: u8) -> Holders {
        Holders::new(CONFIG, (1..=count).map(node), vec![])
    }

    /// A list that names `holder` for `alice`, or nobody.
    fn list(holder: Option<u8>) -> NameListResponse {
        let mut list = NameListResponse::default();
        if let Some(holder) = holder {
            let holding = NameHolding {
                holder: node(holder),
                expires_at_ms: START + u64::from(holder),
            };
            list.0.insert(alice(), holding);
        }
        list
    }

    /// Takes the lists `holders` name, registrar by registrar from 1, at
    /// `now`, and the view from them.
    fn take(view: &mut Holders, lists: &[Option<u8>], now: u64) -> Vec<HolderChange> {
        for (registrar, holder) in (1..).zip(lists) {
            assert!(view.take_list(&node(registrar), list(*holder), [registrar; 32], now));
        }
        let mut changes = Vec::new();
        view.recompute(now, &mut changes);
        view.check_invariants();
        changes
    }

    fn holder(view: &Holders) -> Option<NodeId> {
        view.holder(&alice()).map(|held| held.holder)
    }

    // n ≥ 3f + 1, and a quorum of n - f: all of up to three.
    #[test]
    fn the_quorum_is_n_minus_f() {
        let expected = [(1, 0, 1), (2, 0, 2), (3, 0, 3), (4, 1, 3), (5, 1, 4)];
        for (n, f, q) in expected {
            assert_eq!((faults_tolerated(n), quorum(n)), (f, q), "{n}");
        }
        for n in 1..=MAX_REGISTRARS {
            let (f, q) = (faults_tolerated(n), quorum(n));
            assert!(n > 3 * f && n < 3 * (f + 1) + 1, "f is the largest for {n}");
            assert!(
                2 * q > n && 2 * q - n > f,
                "two quorums share an honest one"
            );
            assert!(
                n * MAX_NAME_LIST / q <= MAX_HOLDERS,
                "the bound holds for {n}"
            );
        }
    }

    // spec/designs/names.md#5-holding-a-name: the holder is whom a quorum of
    // the configured lists name, not a majority of them.
    #[test]
    fn a_holder_needs_a_quorum_of_the_configured_lists() {
        let mut view = holders(4);
        assert!(take(&mut view, &[Some(7), Some(7), None, None], START).is_empty());
        assert_eq!(holder(&view), None);
        let changes = take(&mut view, &[Some(7), Some(7), Some(7), Some(8)], START);
        assert_eq!(
            changes,
            [HolderChange::Held {
                name: alice(),
                holder: node(7)
            }]
        );
        // The latest expiry three lists give it.
        assert_eq!(view.holder(&alice()).unwrap().expires_ms, START + 7);
        let changes = take(&mut view, &[Some(7), Some(7), Some(8), Some(8)], START + 1);
        assert_eq!(
            changes,
            [HolderChange::Unheld {
                name: alice(),
                holder: node(7)
            }]
        );
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        view.take_changes(&mut changed, &mut removed);
        assert_eq!(
            (changed.len(), removed.as_slice()),
            (0, [alice()].as_slice())
        );
    }

    // Fewer than a quorum of the lists fresh: the view keeps what it had, a
    // view restored from disk too, until a quorum is fresh again.
    #[test]
    fn the_view_freezes_without_a_quorum_of_fresh_lists() {
        let restored = HeldName {
            name: alice(),
            holder: node(7),
            expires_ms: START,
        };
        let mut view = Holders::new(CONFIG, (1..=4).map(node), vec![restored]);
        assert!(view.is_frozen());
        let mut changes = Vec::new();
        assert!(view.take_list(&node(1), list(None), [1; 32], START));
        assert!(view.take_list(&node(2), list(None), [2; 32], START));
        view.recompute(START, &mut changes);
        assert!(view.is_frozen() && changes.is_empty());
        assert_eq!(holder(&view), Some(node(7)));
        assert!(view.take_list(&node(3), list(None), [3; 32], START));
        view.recompute(START, &mut changes);
        assert!(!view.is_frozen());
        assert_eq!(holder(&view), None);
        // Two lists go stale: frozen again, with what it had.
        take(&mut view, &[Some(8), Some(8), Some(8), None], START + 1);
        view.confirm(&node(1), START + 20_000);
        view.recompute(START + 20_000, &mut changes);
        assert!(view.is_frozen());
        assert_eq!(holder(&view), Some(node(8)));
    }

    // With no registrars configured, nothing can confirm a view restored
    // from disk: it is not taken, and is removed from disk.
    #[test]
    fn no_registrars_keep_no_restored_view() {
        let restored = HeldName {
            name: alice(),
            holder: node(7),
            expires_ms: START,
        };
        let mut view = Holders::new(CONFIG, [], vec![restored]);
        view.check_invariants();
        assert!(!view.is_frozen());
        assert_eq!(holder(&view), None);
        let mut changes = Vec::new();
        view.recompute(START, &mut changes);
        assert!(changes.is_empty() && view.held().next().is_none());
        let (mut changed, mut removed) = (Vec::new(), Vec::new());
        view.take_changes(&mut changed, &mut removed);
        assert_eq!(
            (changed.len(), removed.as_slice()),
            (0, [alice()].as_slice())
        );
    }

    // A list votes only while it is fresh: once one of the three that name
    // the holder is not, the other two are too few, though a quorum of the
    // lists are fresh.
    #[test]
    fn a_list_votes_only_while_fresh() {
        let mut view = holders(4);
        take(&mut view, &[Some(7), Some(7), Some(7), None], START);
        assert_eq!(holder(&view), Some(node(7)));
        let later = START + CONFIG.list_fresh_ms;
        for registrar in [2, 3, 4] {
            view.confirm(&node(registrar), later);
        }
        let mut changes = Vec::new();
        view.recompute(later, &mut changes);
        assert_eq!(holder(&view), Some(node(7)));
        for registrar in [2, 3, 4] {
            view.confirm(&node(registrar), later + 1);
        }
        view.recompute(later + 1, &mut changes);
        assert_eq!(holder(&view), None);
    }

    // Up to three registrars tolerate none: every list must name the holder.
    #[test]
    fn three_registrars_need_all_three() {
        let mut view = holders(3);
        take(&mut view, &[Some(7), Some(7), None], START);
        assert_eq!(holder(&view), None);
        take(&mut view, &[Some(7), Some(7), Some(7)], START);
        assert_eq!(holder(&view), Some(node(7)));
        let mut alone = holders(1);
        take(&mut alone, &[Some(7)], START);
        assert_eq!(holder(&alone), Some(node(7)));
    }

    // spec/areas/network.md#releasing-a-split: more than f of the others'
    // fresh lists lacking the holder is a split; stale lists count for
    // nothing, nor does the registrar's own.
    #[test]
    fn a_split_counts_only_the_others_fresh_lists() {
        let mut view = holders(4);
        take(&mut view, &[Some(7), Some(7), Some(8), None], START);
        let split = |view: &Holders, registrar: u8, holder: u8, now| {
            view.is_split(&node(registrar), &alice(), &node(holder), now)
        };
        assert!(split(&view, 1, 7, START));
        assert!(split(&view, 3, 8, START));
        // Only one of the others lacks 7 once registrar 3's list is stale.
        let later = START + CONFIG.list_fresh_ms + 1;
        for registrar in [1, 2, 4] {
            view.confirm(&node(registrar), later);
        }
        assert!(!split(&view, 1, 7, later));
        assert!(split(&view, 3, 8, later));
    }

    // A list longer than a registrar serves, or from a node that is none, is
    // not taken.
    #[test]
    fn only_registrars_lists_within_bounds_are_taken() {
        let mut view = holders(1);
        assert!(!view.take_list(&node(9), list(Some(7)), [9; 32], START));
        let mut long = NameListResponse::default();
        for index in 0..=MAX_NAME_LIST {
            let name = Name::parse(&alloc::format!("n{index:04}")).unwrap();
            let holding = NameHolding {
                holder: node(7),
                expires_at_ms: START,
            };
            long.0.insert(name, holding);
        }
        assert!(!view.take_list(&node(1), long, [1; 32], START));
        assert_eq!(view.hash(&node(1)), None);
    }
}

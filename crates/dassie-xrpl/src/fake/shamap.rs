//! A ledger's hash tree built as rippled builds it, for the fake ledger and
//! tests: its root, and the branch of any key as a proof server gives it,
//! each node as rippled hashes it, deepest first, which proves the key's
//! leaf, or for a key the tree does not hold, its absence
//! ([`crate::shamap`]).
//!
//! It keeps only the leaves, by key, each with its hash, and builds the
//! inner nodes from them when asked, with a stack of at most [`MAX_DEPTH`]
//! frames, never by recursion: a leaf hangs where its key is unique, so the
//! leaves alone decide the tree. A leaf's item is shared, so a copy of a
//! tree, as the fake ledger keeps of each recent ledger's state, costs a
//! count per leaf; and an object set again as it was keeps its leaf's hash.

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use crate::codec::hash;
use crate::shamap::{
    BRANCH_FACTOR, MAX_DEPTH, MAX_ITEM_BYTES, MIN_ITEM_BYTES, Tree, inner_hash, inner_node,
    leaf_hash, leaf_node, nibble, transaction_item,
};

/// The most leaves a tree holds: far more than a test or a simulation
/// makes.
pub const MAX_LEAVES: usize = 1 << 20;

const EMPTY: [u8; 32] = [0; 32];

/// A leaf as the builder reads it: its key, its item and its hash.
type Leaf<'a> = (&'a [u8; 32], &'a [u8], &'a [u8; 32]);

/// A leaf as the tree keeps it: its item, shared, and its hash.
type Kept = (Arc<[u8]>, [u8; 32]);

/// A tree of one kind, by its leaves.
#[derive(Clone, Debug)]
pub struct ShaMap {
    tree: Tree,
    /// Each leaf's item and hash, by its key.
    leaves: BTreeMap<[u8; 32], Kept>,
}

impl ShaMap {
    /// An empty tree.
    #[must_use]
    pub const fn new(tree: Tree) -> Self {
        Self {
            tree,
            leaves: BTreeMap::new(),
        }
    }

    /// Which tree it is.
    #[must_use]
    pub const fn tree(&self) -> Tree {
        self.tree
    }

    /// Its leaves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// Adds a transaction with its metadata, under the transaction's hash,
    /// which it gives. Only into a transaction tree (a bug otherwise).
    pub fn insert_transaction(&mut self, transaction: &[u8], metadata: &[u8]) -> [u8; 32] {
        assert_eq!(self.tree, Tree::Transactions, "a transaction tree");
        let key = hash(transaction);
        self.insert(key, transaction_item(transaction, metadata));
        key
    }

    /// Adds or replaces an object, under its key, which is not zero. Only
    /// into a state tree, and an object of at least [`MIN_ITEM_BYTES`] (a
    /// bug otherwise). An object set again as it was keeps its leaf's hash.
    pub fn insert_object(&mut self, key: [u8; 32], object: &[u8]) {
        assert_eq!(self.tree, Tree::State, "a state tree");
        let unchanged = self
            .leaves
            .get(&key)
            .is_some_and(|(item, _)| item.as_ref() == object);
        if !unchanged {
            self.insert(key, object.to_vec());
        }
    }

    /// The object under `key`, if the tree holds one.
    #[must_use]
    pub fn object(&self, key: &[u8; 32]) -> Option<&[u8]> {
        self.leaves.get(key).map(|(item, _)| item.as_ref())
    }

    /// Its keys, in order.
    pub fn keys(&self) -> impl Iterator<Item = &[u8; 32]> {
        self.leaves.keys()
    }

    /// Removes the leaf of a key, if there is one.
    pub fn remove(&mut self, key: &[u8; 32]) {
        self.leaves.remove(key);
        self.check_invariants();
    }

    /// Keeps only the leaves whose keys `keep` takes.
    pub fn retain(&mut self, mut keep: impl FnMut(&[u8; 32]) -> bool) {
        self.leaves.retain(|key, _| keep(key));
        self.check_invariants();
    }

    fn insert(&mut self, key: [u8; 32], item: Vec<u8>) {
        assert_ne!(key, EMPTY, "no leaf is keyed zero");
        assert!(
            item.len() >= MIN_ITEM_BYTES && item.len() <= MAX_ITEM_BYTES,
            "a leaf's item is as long as rippled's are"
        );
        let leaf = leaf_hash(self.tree, &key, &item);
        self.leaves.insert(key, (Arc::from(item), leaf));
        self.check_invariants();
    }

    /// Its root: zero while it is empty.
    #[must_use]
    pub fn root(&self) -> [u8; 32] {
        let leaves = self.sorted();
        if leaves.is_empty() {
            return EMPTY;
        }
        inner_hash(&children(&leaves, 0, leaves.len(), 0))
    }

    /// The branch of `key`, deepest node first: down to its leaf if the
    /// tree holds it, else to the empty slot where it would hang or the
    /// leaf of another key that hangs there. Empty while the tree is.
    #[must_use]
    pub fn branch(&self, key: &[u8; 32]) -> Vec<Vec<u8>> {
        let leaves = self.sorted();
        let mut branch = Vec::new();
        let (mut low, mut high) = (0, leaves.len());
        // Each turn goes a level down, and a key has MAX_DEPTH nibbles.
        for depth in 0..MAX_DEPTH {
            if low == high {
                break;
            }
            branch.push(inner_node(&children(&leaves, low, high, depth)));
            let slot = nibble(key, depth);
            let (start, end) = group_of(&leaves, low, high, depth, slot);
            match end - start {
                0 => break,
                1 => {
                    if let Some((leaf_key, item, _)) = leaves.get(start) {
                        branch.push(leaf_node(self.tree, leaf_key, item));
                    }
                    break;
                }
                _ => (low, high) = (start, end),
            }
        }
        assert!(branch.len() <= MAX_DEPTH + 1, "a branch is a path");
        branch.reverse();
        branch
    }

    /// The branch of `key`, which the tree holds (a bug otherwise), as
    /// `tx_proof` gives it: from the inner node above its leaf, deepest
    /// first, since the node builds the leaf itself.
    #[must_use]
    pub fn branch_above(&self, key: &[u8; 32]) -> Vec<Vec<u8>> {
        assert!(self.leaves.contains_key(key), "a branch above a leaf held");
        let mut branch = self.branch(key);
        branch.remove(0);
        branch
    }

    fn sorted(&self) -> Vec<Leaf<'_>> {
        self.leaves
            .iter()
            .map(|(key, (item, leaf))| (key, item.as_ref(), leaf))
            .collect()
    }

    fn check_invariants(&self) {
        assert!(
            self.leaves.len() <= MAX_LEAVES,
            "a tree holds a bounded number of leaves"
        );
    }
}

/// The leaves of `low..high` whose nibble at `depth` is `slot`, given that
/// they share every nibble above and are in order.
fn group_of(
    leaves: &[Leaf<'_>],
    low: usize,
    high: usize,
    depth: usize,
    slot: usize,
) -> (usize, usize) {
    let range = leaves.get(low..high).unwrap_or_default();
    let start = low + range.partition_point(|(key, _, _)| nibble(key, depth) < slot);
    let end = low + range.partition_point(|(key, _, _)| nibble(key, depth) <= slot);
    (start, end)
}

/// The children's hashes of the inner node at `depth` over the leaves of
/// `low..high`, which share every nibble above it.
fn children(
    leaves: &[Leaf<'_>],
    low: usize,
    high: usize,
    depth: usize,
) -> [[u8; 32]; BRANCH_FACTOR] {
    let mut out = [EMPTY; BRANCH_FACTOR];
    for (slot, child) in out.iter_mut().enumerate() {
        let (start, end) = group_of(leaves, low, high, depth, slot);
        *child = match end - start {
            0 => EMPTY,
            1 => leaves.get(start).map_or(EMPTY, |(_, _, leaf)| **leaf),
            _ => subtree_hash(leaves, start, end, depth + 1),
        };
    }
    out
}

/// The hash of the inner node at `depth` over the leaves of `low..high`,
/// at least two, which share every nibble above it. A stack of the inner
/// nodes being built, each with the next of its leaves to place and the
/// slot it takes in the one above.
fn subtree_hash(leaves: &[Leaf<'_>], low: usize, high: usize, depth: usize) -> [u8; 32] {
    struct Frame {
        next: usize,
        high: usize,
        depth: usize,
        slot: usize,
        children: [[u8; 32]; BRANCH_FACTOR],
    }
    let frame = |low, high, depth, slot| Frame {
        next: low,
        high,
        depth,
        slot,
        children: [EMPTY; BRANCH_FACTOR],
    };
    let mut stack = vec![frame(low, high, depth, 0)];
    // Each turn places a leaf, opens an inner node a level down, or closes
    // one: each leaf is placed once, under at most MAX_DEPTH inner nodes.
    while let Some(top) = stack.last_mut() {
        assert!(
            top.depth < MAX_DEPTH,
            "distinct keys part by the last nibble"
        );
        if top.next < top.high {
            let slot = nibble(
                leaves.get(top.next).map_or(&EMPTY, |(key, _, _)| key),
                top.depth,
            );
            let (start, end) = group_of(leaves, top.next, top.high, top.depth, slot);
            top.next = end;
            if end - start == 1 {
                if let (Some(child), Some((_, _, leaf))) =
                    (top.children.get_mut(slot), leaves.get(start))
                {
                    *child = **leaf;
                }
            } else {
                let below = frame(start, end, top.depth + 1, slot);
                stack.push(below);
            }
            continue;
        }
        let done = stack.pop();
        let hash = done
            .as_ref()
            .map_or(EMPTY, |done| inner_hash(&done.children));
        match (stack.last_mut(), done) {
            (Some(parent), Some(done)) => {
                if let Some(child) = parent.children.get_mut(done.slot) {
                    *child = hash;
                }
            }
            _ => return hash,
        }
    }
    EMPTY
}

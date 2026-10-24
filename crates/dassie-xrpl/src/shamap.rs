//! The ledger's two hash trees (SHAMaps), of its transactions and of its
//! state, and the check of a branch in either
//! (spec/designs/xrpl-validation.md#8-the-verifier).
//!
//! A tree is a trie of sixteen ways over 256-bit keys, read four bits at a
//! time from the top: the root is at depth 0 and takes the key's first
//! nibble. An inner node's hash is the SHA-512Half of `MIN\0` and its
//! sixteen children's hashes, zero for an empty slot; an inner node with no
//! children hashes to zero, which is an empty tree's root. A leaf hangs at
//! the shallowest depth at which its key is unique among the tree's keys,
//! so the shape follows from the keys alone. A transaction's leaf is keyed
//! by the transaction's hash and hashes `SND\0`, the transaction and its
//! metadata each with its length prefix, and the key; a ledger object's
//! hashes `MLN\0`, the object and its key.
//!
//! A branch is what rippled's `SHAMap::getProofPath` gives: the nodes on the
//! key's path, leaf first and root last, each as rippled sends a node to
//! its peers (`serializeForWire`). [`verify`] reads it from the root down, as
//! rippled's `verifyProofPath` does, and also takes a branch that proves the
//! key absent: one that ends at an inner node whose slot for the key is
//! empty, or at the leaf of another key. Either proves it, since had the
//! key been in the tree, an inner node would stand in that place. Every
//! node is read in the one form rippled writes, and a shape rippled never
//! builds, such as a leaf below an inner node with no other child, is
//! refused, so a branch holds only if it is a path of a canonical tree.
//! Untrusted bytes get an error, never a panic, and nothing recurses.

use alloc::vec::Vec;

use sha2::{Digest, Sha512};

use crate::binary::{length_prefix, write_length};

/// The most inner nodes a branch passes: one per nibble of a key, since
/// two keys differ by the last.
pub const MAX_DEPTH: usize = 64;

/// The most nodes in a branch: its inner nodes and a leaf.
pub const MAX_BRANCH_NODES: usize = MAX_DEPTH + 1;

/// The longest leaf as it is sent: a transaction and its metadata, each the
/// most a length prefix writes, its key and its type. No ledger object is
/// as long.
pub const MAX_LEAF_BYTES: usize = 2 * (3 + crate::binary::MAX_VL_BYTES) + KEY_BYTES + 1;

/// The children of an inner node.
pub const BRANCH_FACTOR: usize = 16;

/// The bytes of a key and of a hash.
const KEY_BYTES: usize = 32;

/// The shortest item a leaf holds, as rippled reads them.
pub const MIN_ITEM_BYTES: usize = 12;

/// rippled sends an inner node with fewer children than this compressed:
/// each child's hash with its slot. With more, all sixteen hashes.
const COMPRESS_BELOW: usize = 12;

/// The longest inner node as it is sent: all sixteen hashes and its type.
const FULL_INNER_BYTES: usize = BRANCH_FACTOR * KEY_BYTES + 1;

/// A compressed inner node's child: its hash and its slot.
const COMPRESSED_CHILD_BYTES: usize = KEY_BYTES + 1;

// The last byte of a node as it is sent: its type (`SHAMapTreeNode.h`). A
// transaction without its metadata is only in sets not yet in a ledger.
const WIRE_STATE: u8 = 1;
const WIRE_INNER: u8 = 2;
const WIRE_COMPRESSED_INNER: u8 = 3;
const WIRE_TRANSACTION_WITH_META: u8 = 4;

// What each node's hash covers first.
const INNER_PREFIX: [u8; 4] = *b"MIN\0";
const TRANSACTION_LEAF_PREFIX: [u8; 4] = *b"SND\0";
const STATE_LEAF_PREFIX: [u8; 4] = *b"MLN\0";

const EMPTY: [u8; 32] = [0; 32];

/// Which of a ledger's two trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tree {
    /// Its transactions with their metadata, by the transactions' hashes.
    Transactions,
    /// Its ledger objects, by their keys.
    State,
}

/// What a branch is to prove of its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaf<'a> {
    /// The transaction tree holds this transaction with this metadata, under
    /// the transaction's hash.
    Transaction {
        /// The transaction, signed, in binary.
        transaction: &'a [u8],
        /// Its metadata, in binary.
        metadata: &'a [u8],
    },
    /// The state tree holds this object, in binary, under the key.
    State(&'a [u8]),
    /// The tree holds nothing under the key.
    Absent(Tree),
}

impl Leaf<'_> {
    /// The tree it is a leaf of.
    #[must_use]
    pub const fn tree(&self) -> Tree {
        match self {
            Self::Transaction { .. } => Tree::Transactions,
            Self::State(_) => Tree::State,
            Self::Absent(tree) => *tree,
        }
    }
}

/// Why a branch proves nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchError {
    /// No nodes, more than [`MAX_BRANCH_NODES`], one missing below the last,
    /// or nodes left over below a leaf or an empty slot.
    Length,
    /// A node is not one rippled writes, or not of this tree.
    Node,
    /// A node does not hash to what the one above names at the key's
    /// nibble, or the top one to the root.
    Hash,
    /// A shape rippled never builds: a leaf as the root, a leaf below an
    /// inner node other than the root that has no other child, an inner
    /// node below the last nibble, or a leaf off the key's path.
    Shape,
    /// It proves something else: another leaf than the one claimed, a
    /// transaction under another key than its hash, or the key's absence
    /// for a leaf, or its leaf for an absence.
    Leaf,
}

/// Checks that `branch`, leaf first, leads from what `leaf` says of `key`
/// to `root`: that the tree holds the leaf under the key, or, for
/// [`Leaf::Absent`], nothing. An empty tree, whose root is zero, has no
/// branch, so nothing is proven of it; a ledger's state is never empty.
///
/// # Errors
///
/// Why it does not.
pub fn verify<B: AsRef<[u8]>>(
    root: &[u8; 32],
    key: &[u8; 32],
    leaf: Leaf<'_>,
    branch: &[B],
) -> Result<(), BranchError> {
    if branch.is_empty() || branch.len() > MAX_BRANCH_NODES {
        return Err(BranchError::Length);
    }
    if let Leaf::Transaction { transaction, .. } = leaf
        && crate::codec::hash(transaction) != *key
    {
        return Err(BranchError::Leaf);
    }
    let tree = leaf.tree();
    let mut expected = *root;
    // The children of the inner node above, once there is one.
    let mut above = 0;
    for (depth, bytes) in branch.iter().rev().enumerate() {
        let last = depth + 1 == branch.len();
        match Node::from_wire(bytes.as_ref(), tree)? {
            Node::Inner(children) => {
                if inner_hash(&children) != expected {
                    return Err(BranchError::Hash);
                }
                if depth >= MAX_DEPTH {
                    return Err(BranchError::Shape);
                }
                let below = children.get(nibble(key, depth)).copied().unwrap_or(EMPTY);
                if below == EMPTY {
                    if !last {
                        return Err(BranchError::Length);
                    }
                    return match leaf {
                        Leaf::Absent(_) => Ok(()),
                        _ => Err(BranchError::Leaf),
                    };
                }
                if last {
                    return Err(BranchError::Length);
                }
                expected = below;
                above = child_count(&children);
            }
            Node::Leaf {
                key: leaf_key,
                item,
            } => {
                if leaf_hash(tree, &leaf_key, item) != expected {
                    return Err(BranchError::Hash);
                }
                if depth == 0 || (depth > 1 && above < 2) || !same_path(&leaf_key, key, depth) {
                    return Err(BranchError::Shape);
                }
                if !last {
                    return Err(BranchError::Length);
                }
                return if holds(leaf, key, &leaf_key, item) {
                    Ok(())
                } else {
                    Err(BranchError::Leaf)
                };
            }
        }
    }
    // Each node above returns or descends, and the last always returns.
    Err(BranchError::Length)
}

/// Whether the leaf of `leaf_key` holding `item` is what `leaf` says of
/// `key`.
fn holds(leaf: Leaf<'_>, key: &[u8; 32], leaf_key: &[u8; 32], item: &[u8]) -> bool {
    match leaf {
        Leaf::Absent(_) => leaf_key != key,
        Leaf::State(object) => leaf_key == key && item == object,
        Leaf::Transaction {
            transaction,
            metadata,
        } => leaf_key == key && split_transaction_item(item) == Some((transaction, metadata)),
    }
}

/// A transaction leaf's item: the transaction and its metadata, each with
/// its length prefix.
#[must_use]
pub fn transaction_item(transaction: &[u8], metadata: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(transaction.len() + metadata.len() + 6);
    write_length(&mut out, transaction.len());
    out.extend_from_slice(transaction);
    write_length(&mut out, metadata.len());
    out.extend_from_slice(metadata);
    out
}

/// The transaction and the metadata in a transaction leaf's item, if it is
/// one.
fn split_transaction_item(item: &[u8]) -> Option<(&[u8], &[u8])> {
    let (prefix, length) = length_prefix(item).ok()?;
    let (transaction, rest) = item.get(prefix..)?.split_at_checked(length)?;
    let (prefix, length) = length_prefix(rest).ok()?;
    let metadata = rest.get(prefix..)?;
    (metadata.len() == length).then_some((transaction, metadata))
}

/// A node of a tree, as it is sent. One at a time, on the stack.
#[allow(clippy::large_enum_variant)]
enum Node<'a> {
    /// An inner node's children's hashes, zero where empty.
    Inner([[u8; 32]; BRANCH_FACTOR]),
    /// A leaf: its key and its item.
    Leaf { key: [u8; 32], item: &'a [u8] },
}

impl<'a> Node<'a> {
    /// The node in `bytes`, in the form rippled sends it: an inner node
    /// with children, compressed exactly when it has fewer than
    /// [`COMPRESS_BELOW`], its children in order; or a leaf of `tree`, at
    /// most [`MAX_LEAF_BYTES`], with an item of at least
    /// [`MIN_ITEM_BYTES`] and a key that is not zero.
    fn from_wire(bytes: &'a [u8], tree: Tree) -> Result<Self, BranchError> {
        let (&kind, body) = bytes.split_last().ok_or(BranchError::Node)?;
        match (kind, tree) {
            (WIRE_INNER, _) => {
                let children = full_inner(body).ok_or(BranchError::Node)?;
                if child_count(&children) < COMPRESS_BELOW {
                    return Err(BranchError::Node);
                }
                Ok(Self::Inner(children))
            }
            (WIRE_COMPRESSED_INNER, _) => {
                let children = compressed_inner(body).ok_or(BranchError::Node)?;
                Ok(Self::Inner(children))
            }
            (WIRE_TRANSACTION_WITH_META, Tree::Transactions) | (WIRE_STATE, Tree::State) => {
                if bytes.len() > MAX_LEAF_BYTES {
                    return Err(BranchError::Node);
                }
                let (item, key) = body
                    .split_last_chunk::<KEY_BYTES>()
                    .ok_or(BranchError::Node)?;
                if item.len() < MIN_ITEM_BYTES || *key == EMPTY {
                    return Err(BranchError::Node);
                }
                Ok(Self::Leaf { key: *key, item })
            }
            _ => Err(BranchError::Node),
        }
    }
}

/// All sixteen children's hashes, at least one of them not zero.
fn full_inner(body: &[u8]) -> Option<[[u8; 32]; BRANCH_FACTOR]> {
    if body.len() != FULL_INNER_BYTES - 1 {
        return None;
    }
    let mut children = [EMPTY; BRANCH_FACTOR];
    for (child, hash) in children.iter_mut().zip(body.as_chunks::<KEY_BYTES>().0) {
        *child = *hash;
    }
    (child_count(&children) > 0).then_some(children)
}

/// From one to [`COMPRESS_BELOW`] less one children, each its hash, not
/// zero, and its slot, the slots ascending.
fn compressed_inner(body: &[u8]) -> Option<[[u8; 32]; BRANCH_FACTOR]> {
    let (chunks, rest) = body.as_chunks::<COMPRESSED_CHILD_BYTES>();
    if !rest.is_empty() || chunks.is_empty() || chunks.len() >= COMPRESS_BELOW {
        return None;
    }
    let mut children = [EMPTY; BRANCH_FACTOR];
    let mut next = 0;
    for chunk in chunks {
        let (hash, slot) = chunk.split_first_chunk::<KEY_BYTES>()?;
        let slot = usize::from(*slot.first()?);
        if slot < next || *hash == EMPTY {
            return None;
        }
        *children.get_mut(slot)? = *hash;
        next = slot + 1;
    }
    Some(children)
}

/// An inner node as rippled sends it: compressed with fewer than twelve
/// children, else all sixteen. An inner node without children is never
/// sent (a bug).
#[must_use]
pub fn inner_wire(children: &[[u8; 32]; BRANCH_FACTOR]) -> Vec<u8> {
    let count = child_count(children);
    assert!(count > 0, "an inner node sent has children");
    let mut out = Vec::with_capacity(FULL_INNER_BYTES);
    if count < COMPRESS_BELOW {
        for (slot, hash) in (0u8..).zip(children) {
            if *hash != EMPTY {
                out.extend_from_slice(hash);
                out.push(slot);
            }
        }
        out.push(WIRE_COMPRESSED_INNER);
    } else {
        for hash in children {
            out.extend_from_slice(hash);
        }
        out.push(WIRE_INNER);
    }
    out
}

/// A leaf of `tree` as rippled sends it: its item, its key and its type.
#[must_use]
pub fn leaf_wire(tree: Tree, key: &[u8; 32], item: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(item.len() + KEY_BYTES + 1);
    out.extend_from_slice(item);
    out.extend_from_slice(key);
    out.push(match tree {
        Tree::Transactions => WIRE_TRANSACTION_WITH_META,
        Tree::State => WIRE_STATE,
    });
    out
}

/// An inner node's hash: zero without children.
#[must_use]
pub fn inner_hash(children: &[[u8; 32]; BRANCH_FACTOR]) -> [u8; 32] {
    if child_count(children) == 0 {
        return EMPTY;
    }
    let mut hasher = Sha512::new();
    hasher.update(INNER_PREFIX);
    for hash in children {
        hasher.update(hash);
    }
    half(&hasher.finalize())
}

/// A leaf's hash.
#[must_use]
pub fn leaf_hash(tree: Tree, key: &[u8; 32], item: &[u8]) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update(match tree {
        Tree::Transactions => TRANSACTION_LEAF_PREFIX,
        Tree::State => STATE_LEAF_PREFIX,
    });
    hasher.update(item);
    hasher.update(key);
    half(&hasher.finalize())
}

fn half(digest: &[u8]) -> [u8; 32] {
    let mut out = EMPTY;
    out.copy_from_slice(digest.get(..32).unwrap_or(&EMPTY));
    out
}

/// The key's nibble at `depth`, which is below [`MAX_DEPTH`] (a bug
/// otherwise): the slot it takes in the inner node there.
#[must_use]
pub fn nibble(key: &[u8; 32], depth: usize) -> usize {
    assert!(depth < MAX_DEPTH, "a key has {MAX_DEPTH} nibbles");
    let byte = key.get(depth / 2).copied().unwrap_or(0);
    usize::from(if depth.is_multiple_of(2) {
        byte >> 4
    } else {
        byte & 0x0f
    })
}

/// Whether two keys share their first `depth` nibbles.
fn same_path(one: &[u8; 32], other: &[u8; 32], depth: usize) -> bool {
    (0..depth.min(MAX_DEPTH)).all(|at| nibble(one, at) == nibble(other, at))
}

fn child_count(children: &[[u8; 32]; BRANCH_FACTOR]) -> usize {
    children.iter().filter(|hash| **hash != EMPTY).count()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn nibbles_are_read_from_the_top() {
        let mut key = EMPTY;
        key[0] = 0xAB;
        key[31] = 0xCD;
        assert_eq!(nibble(&key, 0), 0xA);
        assert_eq!(nibble(&key, 1), 0xB);
        assert_eq!(nibble(&key, 2), 0);
        assert_eq!(nibble(&key, 62), 0xC);
        assert_eq!(nibble(&key, 63), 0xD);
    }

    #[test]
    fn inner_nodes_are_sent_compressed_below_twelve_children() {
        let mut children = [EMPTY; BRANCH_FACTOR];
        for (slot, child) in children.iter_mut().enumerate().take(11) {
            *child = [u8::try_from(slot).unwrap() + 1; 32];
        }
        let wire = inner_wire(&children);
        assert_eq!(wire.len(), 11 * COMPRESSED_CHILD_BYTES + 1);
        assert!(matches!(
            Node::from_wire(&wire, Tree::State),
            Ok(Node::Inner(read)) if read == children
        ));
        children[11] = [12; 32];
        let wire = inner_wire(&children);
        assert_eq!(wire.len(), FULL_INNER_BYTES);
        assert!(matches!(
            Node::from_wire(&wire, Tree::State),
            Ok(Node::Inner(read)) if read == children
        ));
    }

    #[test]
    fn only_the_form_rippled_sends_reads() {
        let mut children = [EMPTY; BRANCH_FACTOR];
        children[3] = [3; 32];
        children[9] = [9; 32];
        let wire = inner_wire(&children);
        let refused =
            |bytes: &[u8]| matches!(Node::from_wire(bytes, Tree::State), Err(BranchError::Node));
        // Slots out of order, the same twice, beyond sixteen, a zero hash.
        let mut swapped = [&wire[33..66], &wire[..33], &wire[66..]].concat();
        assert!(refused(&swapped));
        swapped[32] = 3;
        assert!(refused(&swapped));
        let mut beyond = wire.clone();
        beyond[65] = 16;
        assert!(refused(&beyond));
        let mut zero = wire.clone();
        zero[..32].fill(0);
        assert!(refused(&zero));
        // A few children in full, none, a byte short, no type, another type.
        let mut full = vec![0; FULL_INNER_BYTES - 1];
        full[..32].copy_from_slice(&[3; 32]);
        full.push(WIRE_INNER);
        assert!(refused(&full));
        assert!(refused(&[WIRE_COMPRESSED_INNER]));
        assert!(refused(&wire[1..]));
        assert!(refused(&[]));
        assert!(refused(&[&wire[..wire.len() - 1], &[7]].concat()));
        // A leaf of the other tree, without its metadata, or keyed zero.
        let item = [1; MIN_ITEM_BYTES];
        assert!(refused(&leaf_wire(Tree::Transactions, &[1; 32], &item)));
        assert!(refused(&[&item[..], &[1; 32], &[0]].concat()));
        assert!(refused(&leaf_wire(Tree::State, &EMPTY, &item)));
        assert!(refused(&leaf_wire(Tree::State, &[1; 32], &item[1..])));
        assert!(Node::from_wire(&leaf_wire(Tree::State, &[1; 32], &item), Tree::State).is_ok());
    }

    #[test]
    fn an_empty_inner_node_hashes_to_zero() {
        assert_eq!(inner_hash(&[EMPTY; BRANCH_FACTOR]), EMPTY);
    }

    #[test]
    fn a_transaction_item_splits_into_what_it_was_made_of() {
        for length in [0, 192, 193, 12_480, 12_481] {
            let transaction = vec![1; length];
            let item = transaction_item(&transaction, &[2; 300]);
            assert_eq!(
                split_transaction_item(&item),
                Some((transaction.as_slice(), [2; 300].as_slice()))
            );
            assert_eq!(split_transaction_item(&item[..item.len() - 1]), None);
            assert_eq!(
                split_transaction_item(&[item.as_slice(), &[0]].concat()),
                None
            );
        }
    }
}

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
//! A branch is what a proof server gives
//! (spec/designs/xrpl-validation.md#3-the-proof-api): the nodes on the
//! key's path from the deepest one reached up to the root, each as rippled
//! hashes it, prefix included. [`verify`] hashes each and reads it from the
//! root down, and takes a branch that proves the key absent: one that ends
//! at an inner node whose slot for the key is empty, or at the leaf of
//! another key. Either proves it, since had the key been in the tree, an
//! inner node would stand in that place. An empty tree's branch is empty.
//! A node is read by its prefix, so it is of one kind only, and a shape
//! rippled never builds, such as a leaf below an inner node with no other
//! child, is refused, so a branch holds only if it is a path of a canonical
//! tree. Untrusted bytes get an error, never a panic, and nothing recurses.

use alloc::vec::Vec;

use sha2::{Digest, Sha512};

use crate::binary::{length_prefix, write_length};

/// The most inner nodes a branch passes: one per nibble of a key, since
/// two keys differ by the last.
pub const MAX_DEPTH: usize = 64;

/// The most nodes in a branch: its inner nodes and a leaf.
pub const MAX_BRANCH_NODES: usize = MAX_DEPTH + 1;

/// The longest item a leaf holds: a transaction and its metadata, each
/// the most a length prefix writes. No ledger object is as long.
pub const MAX_ITEM_BYTES: usize = 2 * (3 + crate::binary::MAX_VL_BYTES);

/// The longest node in a branch: a leaf with the longest item, its prefix
/// and its key.
pub const MAX_NODE_BYTES: usize = PREFIX_BYTES + MAX_ITEM_BYTES + KEY_BYTES;

/// The children of an inner node.
pub const BRANCH_FACTOR: usize = 16;

/// The bytes of a key and of a hash.
const KEY_BYTES: usize = 32;

/// The bytes of a hash prefix.
const PREFIX_BYTES: usize = 4;

/// The shortest item a leaf holds, as rippled reads them.
pub const MIN_ITEM_BYTES: usize = 12;

/// An inner node as it is hashed: its prefix and sixteen hashes.
const INNER_NODE_BYTES: usize = PREFIX_BYTES + BRANCH_FACTOR * KEY_BYTES;

// What each node's hash covers first (`HashPrefix.h`).
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

impl Tree {
    /// The prefix its leaves are hashed with.
    const fn leaf_prefix(self) -> [u8; 4] {
        match self {
            Self::Transactions => TRANSACTION_LEAF_PREFIX,
            Self::State => STATE_LEAF_PREFIX,
        }
    }
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
    /// No nodes for a tree that is not empty, more than
    /// [`MAX_BRANCH_NODES`], one missing below the last, or nodes left over
    /// below a leaf or an empty slot.
    Length,
    /// A node is not one rippled hashes, or not of this tree.
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

/// Checks that `branch`, deepest node first, leads from what `leaf` says
/// of `key` to `root`: that the tree holds the leaf under the key, or, for
/// [`Leaf::Absent`], nothing. An empty tree, whose root is zero, has an
/// empty branch, which proves every key absent and nothing held.
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
    if branch.len() > MAX_BRANCH_NODES {
        return Err(BranchError::Length);
    }
    if branch.is_empty() {
        return match leaf {
            Leaf::Absent(_) if *root == EMPTY => Ok(()),
            _ => Err(BranchError::Length),
        };
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
        let bytes = bytes.as_ref();
        let last = depth + 1 == branch.len();
        // Bounded before it is hashed, so a node costs at most the longest
        // leaf.
        if bytes.len() > MAX_NODE_BYTES {
            return Err(BranchError::Node);
        }
        if node_hash(bytes) != expected {
            return Err(BranchError::Hash);
        }
        match Node::parse(bytes, tree)? {
            Node::Inner(children) => {
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
/// its length prefix, so each at most [`crate::binary::MAX_VL_BYTES`] (a
/// bug otherwise).
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

/// A node of a tree, as it is hashed. One at a time, on the stack.
#[allow(clippy::large_enum_variant)]
enum Node<'a> {
    /// An inner node's children's hashes, zero where empty.
    Inner([[u8; 32]; BRANCH_FACTOR]),
    /// A leaf: its key and its item.
    Leaf { key: [u8; 32], item: &'a [u8] },
}

impl<'a> Node<'a> {
    /// The node in `bytes`, as rippled hashes it: an inner node, its
    /// prefix and sixteen hashes, at least one of them not zero; or a leaf
    /// of `tree`, its prefix, an item of at least [`MIN_ITEM_BYTES`] and
    /// at most [`MAX_ITEM_BYTES`], and a key that is not zero.
    fn parse(bytes: &'a [u8], tree: Tree) -> Result<Self, BranchError> {
        let (prefix, body) = bytes
            .split_first_chunk::<PREFIX_BYTES>()
            .ok_or(BranchError::Node)?;
        if *prefix == INNER_PREFIX {
            if bytes.len() != INNER_NODE_BYTES {
                return Err(BranchError::Node);
            }
            let mut children = [EMPTY; BRANCH_FACTOR];
            for (child, hash) in children.iter_mut().zip(body.as_chunks::<KEY_BYTES>().0) {
                *child = *hash;
            }
            if child_count(&children) == 0 {
                return Err(BranchError::Node);
            }
            return Ok(Self::Inner(children));
        }
        if *prefix != tree.leaf_prefix() {
            return Err(BranchError::Node);
        }
        let (item, key) = body
            .split_last_chunk::<KEY_BYTES>()
            .ok_or(BranchError::Node)?;
        if item.len() < MIN_ITEM_BYTES || item.len() > MAX_ITEM_BYTES || *key == EMPTY {
            return Err(BranchError::Node);
        }
        Ok(Self::Leaf { key: *key, item })
    }
}

/// An inner node as rippled hashes it: its prefix and its sixteen
/// children's hashes. An inner node without children is never hashed so
/// (a bug).
#[must_use]
pub fn inner_node(children: &[[u8; 32]; BRANCH_FACTOR]) -> Vec<u8> {
    assert!(child_count(children) > 0, "an inner node sent has children");
    let mut out = Vec::with_capacity(INNER_NODE_BYTES);
    out.extend_from_slice(&INNER_PREFIX);
    for hash in children {
        out.extend_from_slice(hash);
    }
    out
}

/// A leaf of `tree` as rippled hashes it: its prefix, its item and its
/// key.
#[must_use]
pub fn leaf_node(tree: Tree, key: &[u8; 32], item: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(PREFIX_BYTES + item.len() + KEY_BYTES);
    out.extend_from_slice(&tree.leaf_prefix());
    out.extend_from_slice(item);
    out.extend_from_slice(key);
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
    hasher.update(tree.leaf_prefix());
    hasher.update(item);
    hasher.update(key);
    half(&hasher.finalize())
}

/// The hash of a node as it is hashed, prefix included.
fn node_hash(bytes: &[u8]) -> [u8; 32] {
    half(&Sha512::digest(bytes))
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
    fn a_node_hashes_as_its_bytes() {
        let mut children = [EMPTY; BRANCH_FACTOR];
        children[3] = [3; 32];
        let inner = inner_node(&children);
        assert_eq!(inner.len(), INNER_NODE_BYTES);
        assert_eq!(node_hash(&inner), inner_hash(&children));
        let item = [1; MIN_ITEM_BYTES];
        for tree in [Tree::State, Tree::Transactions] {
            let leaf = leaf_node(tree, &[1; 32], &item);
            assert_eq!(node_hash(&leaf), leaf_hash(tree, &[1; 32], &item));
        }
    }

    #[test]
    fn only_the_form_rippled_hashes_reads() {
        let mut children = [EMPTY; BRANCH_FACTOR];
        children[3] = [3; 32];
        children[9] = [9; 32];
        let inner = inner_node(&children);
        assert!(matches!(
            Node::parse(&inner, Tree::State),
            Ok(Node::Inner(read)) if read == children
        ));
        let refused =
            |bytes: &[u8], tree| matches!(Node::parse(bytes, tree), Err(BranchError::Node));
        // A byte short or long, no children, no prefix, another prefix.
        assert!(refused(&inner[..inner.len() - 1], Tree::State));
        assert!(refused(&[&inner[..], &[0]].concat(), Tree::State));
        assert!(refused(
            &[&INNER_PREFIX[..], &[0; BRANCH_FACTOR * KEY_BYTES]].concat(),
            Tree::State
        ));
        assert!(refused(&[], Tree::State));
        assert!(refused(b"MIN", Tree::State));
        assert!(refused(&[b"NIM\0", &inner[4..]].concat(), Tree::State));
        // A leaf of the other tree, keyed zero, or with too short an item.
        let item = [1; MIN_ITEM_BYTES];
        assert!(refused(
            &leaf_node(Tree::Transactions, &[1; 32], &item),
            Tree::State
        ));
        assert!(refused(
            &leaf_node(Tree::State, &[1; 32], &item),
            Tree::Transactions
        ));
        assert!(refused(&leaf_node(Tree::State, &EMPTY, &item), Tree::State));
        assert!(refused(
            &leaf_node(Tree::State, &[1; 32], &item[1..]),
            Tree::State
        ));
        assert!(Node::parse(&leaf_node(Tree::State, &[1; 32], &item), Tree::State).is_ok());
        // The longest leaf rippled hashes, and one a byte longer.
        let longest = vec![1; MAX_ITEM_BYTES];
        let leaf = leaf_node(Tree::State, &[1; 32], &longest);
        assert_eq!(leaf.len(), MAX_NODE_BYTES);
        assert!(Node::parse(&leaf, Tree::State).is_ok());
        let longer = [&longest[..], &[1]].concat();
        assert!(refused(
            &leaf_node(Tree::State, &[1; 32], &longer),
            Tree::State
        ));
    }

    #[test]
    fn an_empty_tree_proves_every_key_absent_and_nothing_held() {
        assert_eq!(inner_hash(&[EMPTY; BRANCH_FACTOR]), EMPTY);
        let empty: [&[u8]; 0] = [];
        assert_eq!(
            verify(&EMPTY, &[1; 32], Leaf::Absent(Tree::State), &empty),
            Ok(())
        );
        assert_eq!(
            verify(&EMPTY, &[1; 32], Leaf::State(&[1; 20]), &empty),
            Err(BranchError::Length)
        );
        assert_eq!(
            verify(&[1; 32], &[1; 32], Leaf::Absent(Tree::State), &empty),
            Err(BranchError::Length)
        );
    }

    #[test]
    fn a_node_longer_than_any_is_refused_before_it_is_hashed() {
        let long = vec![0; MAX_NODE_BYTES + 1];
        let hash = node_hash(&long);
        assert_eq!(
            verify(&hash, &[1; 32], Leaf::Absent(Tree::State), &[long]),
            Err(BranchError::Node)
        );
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

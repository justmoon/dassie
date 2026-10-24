//! Branches in the ledger's hash trees (spec/designs/xrpl-validation.md#8-the-verifier):
//! a real mainnet ledger's transaction tree rebuilt to its root, real
//! branches in the form rippled sends them, state trees with proofs of
//! absence, and branches that lie.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::sync::OnceLock;

use dassie_xrpl::codec;
use dassie_xrpl::fake::shamap::ShaMap;
use dassie_xrpl::header::LedgerHeader;
use dassie_xrpl::shamap::{
    BRANCH_FACTOR, BranchError, Leaf, MAX_BRANCH_NODES, MAX_DEPTH, Tree, inner_hash, inner_wire,
    leaf_hash, leaf_wire, nibble, transaction_item, verify,
};
use dassie_xrpl::{from_hex, hex_upper};
use proptest::prelude::*;
use serde_json::Value;

fn read(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

fn bytes(value: &Value, key: &str) -> Vec<u8> {
    from_hex(text(value, key)).unwrap()
}

fn hash(value: &Value, key: &str) -> [u8; 32] {
    bytes(value, key).try_into().unwrap()
}

fn transaction_set() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| read("tests/data/transaction-set.json"))
}

/// A branch given as each inner node's sixteen children, root first, and
/// its leaf, in the form rippled sends it: the leaf first, then each inner
/// node up to the root.
fn branch_from_levels(levels: &Value, leaf: Vec<u8>) -> Vec<Vec<u8>> {
    let mut branch = vec![leaf];
    for level in levels.as_array().unwrap().iter().rev() {
        let mut children = [[0; 32]; BRANCH_FACTOR];
        for (child, hash) in children.iter_mut().zip(level.as_array().unwrap()) {
            *child = from_hex(hash.as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
        }
        branch.push(inner_wire(&children));
    }
    branch
}

// The tree of a real ledger's 146 transactions, built as rippled builds it,
// has the root its header names, and the header hashes to the ledger; each
// transaction's branch proves it, its leaf at the depth rippled hangs it.
#[test]
fn a_mainnet_transaction_tree_rebuilds_its_root() {
    let data = transaction_set();
    let header = LedgerHeader::from_bytes(&bytes(data, "ledger_data")).unwrap();
    assert_eq!(header.hash(), hash(data, "ledger_hash"));
    assert_eq!(header.transaction_hash, hash(data, "transaction_hash"));
    let mut tree = ShaMap::new(Tree::Transactions);
    let transactions: Vec<(Vec<u8>, Vec<u8>)> = data["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (bytes(entry, "tx_blob"), bytes(entry, "meta")))
        .collect();
    assert_eq!(transactions.len(), 146);
    for (transaction, metadata) in &transactions {
        tree.insert_transaction(transaction, metadata);
    }
    let root = tree.root();
    assert_eq!(root, header.transaction_hash);
    let mut depths = BTreeMap::new();
    for (transaction, metadata) in &transactions {
        let key = codec::hash(transaction);
        let branch = tree.branch(&key).unwrap();
        *depths.entry(branch.len() - 1).or_insert(0) += 1;
        let leaf = Leaf::Transaction {
            transaction,
            metadata,
        };
        assert_eq!(verify(&root, &key, leaf, &branch), Ok(()));
        assert_eq!(
            verify(&root, &key, Leaf::Absent(Tree::Transactions), &branch),
            Err(BranchError::Leaf)
        );
    }
    // As counted when the ledger was captured.
    assert_eq!(depths, BTreeMap::from([(2, 90), (3, 54), (5, 2)]));
}

// Branches taken from mainnet ledgers verify in the form rippled sends
// them, against the transaction roots of headers that hash to their
// ledgers, and the tree builder gives the same bytes.
#[test]
fn mainnet_branches_verify() {
    let data = transaction_set();
    let branch = &data["branch"];
    let key = hash(branch, "key");
    let entry = data["transactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| codec::hash(&bytes(entry, "tx_blob")) == key)
        .unwrap();
    let (transaction, metadata) = (bytes(entry, "tx_blob"), bytes(entry, "meta"));
    let item = transaction_item(&transaction, &metadata);
    assert_eq!(
        hex_upper(&leaf_hash(Tree::Transactions, &key, &item)),
        text(branch, "leaf_hash")
    );
    let wire = branch_from_levels(
        &branch["levels"],
        leaf_wire(Tree::Transactions, &key, &item),
    );
    let root = hash(data, "transaction_hash");
    let leaf = Leaf::Transaction {
        transaction: &transaction,
        metadata: &metadata,
    };
    assert_eq!(verify(&root, &key, leaf, &wire), Ok(()));
    let mut tree = ShaMap::new(Tree::Transactions);
    for entry in data["transactions"].as_array().unwrap() {
        tree.insert_transaction(&bytes(entry, "tx_blob"), &bytes(entry, "meta"));
    }
    assert_eq!(tree.branch(&key).unwrap(), wire);

    let others = data["other_branches"].as_array().unwrap();
    assert_eq!(others.len(), 2);
    for other in others {
        let header = LedgerHeader::from_bytes(&bytes(other, "ledger_data")).unwrap();
        assert_eq!(header.hash(), hash(other, "ledger_hash"));
        assert_eq!(header.transaction_hash, hash(other, "transaction_hash"));
        let (transaction, metadata) = (bytes(other, "tx_blob"), bytes(other, "meta"));
        let key = hash(other, "key");
        assert_eq!(codec::hash(&transaction), key);
        let item = transaction_item(&transaction, &metadata);
        let wire = branch_from_levels(&other["levels"], leaf_wire(Tree::Transactions, &key, &item));
        let leaf = Leaf::Transaction {
            transaction: &transaction,
            metadata: &metadata,
        };
        assert_eq!(
            verify(&hash(other, "transaction_hash"), &key, leaf, &wire),
            Ok(())
        );
        assert_eq!(verify(&root, &key, leaf, &wire), Err(BranchError::Hash));
    }
}

/// A ledger object and its key.
type Object = ([u8; 32], Vec<u8>);

/// Real ledger objects under their keys, and filler objects under `keys`.
fn state_tree(keys: &[[u8; 32]]) -> (ShaMap, Vec<Object>) {
    let data = transaction_set();
    let mut objects: Vec<Object> = data["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|object| (hash(object, "index"), bytes(object, "node_binary")))
        .collect();
    for key in keys {
        objects.push((*key, [&key[..], b"an object"].concat()));
    }
    let mut tree = ShaMap::new(Tree::State);
    for (key, object) in &objects {
        tree.insert_object(*key, object);
    }
    (tree, objects)
}

/// A key that shares its first `nibbles` nibbles with `key` and differs in
/// the next.
fn neighbour(key: &[u8; 32], nibbles: usize) -> [u8; 32] {
    let mut other = *key;
    let byte = nibbles / 2;
    other[byte] ^= if nibbles.is_multiple_of(2) {
        0x80
    } else {
        0x08
    };
    other
}

// In a state tree, an object's branch proves it and not another object
// or its absence; the branch of a key the tree does not hold proves its
// absence, ending at an empty slot or at another key's leaf, and not an
// object under it.
#[test]
fn a_state_tree_proves_objects_and_absences() {
    let keys: Vec<[u8; 32]> = (0u8..40).map(|n| [n.wrapping_mul(37) ^ 0x5A; 32]).collect();
    let (tree, objects) = state_tree(&keys);
    let root = tree.root();
    for (key, object) in &objects {
        let branch = tree.branch(key).unwrap();
        assert_eq!(verify(&root, key, Leaf::State(object), &branch), Ok(()));
        assert_eq!(
            verify(&root, key, Leaf::State(b"another object"), &branch),
            Err(BranchError::Leaf)
        );
        assert_eq!(
            verify(&root, key, Leaf::Absent(Tree::State), &branch),
            Err(BranchError::Leaf)
        );
    }
    let (mut at_empty_slot, mut at_another_leaf) = (0, 0);
    for (key, _) in &objects {
        for nibbles in [0, 1, 2, 3, 63] {
            let missing = neighbour(key, nibbles);
            if objects.iter().any(|(key, _)| *key == missing) {
                continue;
            }
            let branch = tree.branch(&missing).unwrap();
            if branch[0].last() == Some(&1) {
                at_another_leaf += 1;
            } else {
                at_empty_slot += 1;
            }
            assert_eq!(
                verify(&root, &missing, Leaf::Absent(Tree::State), &branch),
                Ok(())
            );
            assert_eq!(
                verify(&root, &missing, Leaf::State(b"an object"), &branch),
                Err(BranchError::Leaf)
            );
            // Another key's leaf is of its tree's kind; an empty slot is
            // the same in both, and the root tells the trees apart.
            if branch[0].last() == Some(&1) {
                assert_eq!(
                    verify(&root, &missing, Leaf::Absent(Tree::Transactions), &branch),
                    Err(BranchError::Node)
                );
            }
        }
    }
    assert!(at_empty_slot > 0 && at_another_leaf > 0);
}

#[test]
fn an_empty_tree_and_a_tree_of_one() {
    let mut tree = ShaMap::new(Tree::State);
    assert_eq!(tree.root(), [0; 32]);
    assert_eq!(tree.branch(&[1; 32]), None);
    tree.insert_object([0x12; 32], b"the only object");
    let leaf = leaf_hash(Tree::State, &[0x12; 32], b"the only object");
    let mut children = [[0; 32]; BRANCH_FACTOR];
    children[1] = leaf;
    let root = tree.root();
    assert_eq!(root, inner_hash(&children));
    let branch = tree.branch(&[0x12; 32]).unwrap();
    assert_eq!(branch.len(), 2);
    let object = Leaf::State(b"the only object");
    assert_eq!(verify(&root, &[0x12; 32], object, &branch), Ok(()));
    // Absent at the root's empty slot, and beside the only leaf.
    for missing in [[0x22; 32], [0x13; 32]] {
        let branch = tree.branch(&missing).unwrap();
        assert_eq!(
            verify(&root, &missing, Leaf::Absent(Tree::State), &branch),
            Ok(())
        );
    }
    tree.remove(&[0x12; 32]);
    assert_eq!(tree.root(), [0; 32]);
}

// Two keys that differ only in their last nibble hang at the deepest
// level, below an inner node for every nibble they share; a third key
// that parts from them there is absent at that inner node's empty slot.
#[test]
fn keys_that_part_in_the_last_nibble_hang_at_the_deepest_level() {
    let one = [0x5C; 32];
    let other = neighbour(&one, MAX_DEPTH - 1);
    let (tree, _) = state_tree(&[one, other]);
    let root = tree.root();
    for key in [one, other] {
        let object = [&key[..], b"an object"].concat();
        let branch = tree.branch(&key).unwrap();
        assert_eq!(branch.len(), MAX_BRANCH_NODES);
        assert_eq!(verify(&root, &key, Leaf::State(&object), &branch), Ok(()));
    }
    let mut beside = one;
    beside[31] ^= 0x01;
    let branch = tree.branch(&beside).unwrap();
    assert_eq!(branch.len(), MAX_DEPTH);
    assert_eq!(
        verify(&root, &beside, Leaf::Absent(Tree::State), &branch),
        Ok(())
    );
    assert_eq!(
        verify(&root, &beside, Leaf::State(b"an object"), &branch),
        Err(BranchError::Leaf)
    );
}

// A branch is cut short, at the leaf or at the root, a node is cut, or
// nodes are added below the leaf or above the root: none proves anything.
#[test]
fn a_truncated_or_padded_branch_proves_nothing() {
    let (tree, objects) = state_tree(&[[0x33; 32], [0x34; 32], [0x3F; 32]]);
    let root = tree.root();
    let (key, object) = &objects[0];
    let leaf = Leaf::State(object);
    let branch = tree.branch(key).unwrap();
    assert!(branch.len() >= 2);
    let empty: [Vec<u8>; 0] = [];
    assert_eq!(verify(&root, key, leaf, &empty), Err(BranchError::Length));
    assert_eq!(
        verify(&root, key, leaf, &branch[1..]),
        Err(BranchError::Length)
    );
    assert_eq!(
        verify(&root, key, leaf, &branch[..branch.len() - 1]),
        Err(BranchError::Hash)
    );
    let mut cut = branch.clone();
    cut[0].remove(0);
    assert_eq!(verify(&root, key, leaf, &cut), Err(BranchError::Hash));
    let mut cut = branch.clone();
    let last = cut.len() - 1;
    cut[last].remove(0);
    assert_eq!(verify(&root, key, leaf, &cut), Err(BranchError::Node));
    let below = [vec![branch[0].clone()], branch.clone()].concat();
    assert_eq!(verify(&root, key, leaf, &below), Err(BranchError::Length));
    let above = [branch.clone(), vec![branch[branch.len() - 1].clone()]].concat();
    assert_eq!(verify(&root, key, leaf, &above), Err(BranchError::Hash));
    let long = vec![branch[0].clone(); MAX_BRANCH_NODES + 1];
    assert_eq!(verify(&root, key, leaf, &long), Err(BranchError::Length));
    // An absence proof cut above its empty slot names a node not given.
    let missing = neighbour(key, branch.len() - 1);
    let absence = tree.branch(&missing).unwrap();
    assert_eq!(
        verify(&root, &missing, Leaf::Absent(Tree::State), &absence[1..]),
        Err(BranchError::Length)
    );
}

// A leaf's bytes altered, or claimed under another key: the branch proves
// neither.
#[test]
fn a_leaf_of_another_key_proves_nothing() {
    let (tree, objects) = state_tree(&[[0x41; 32], [0x42; 32]]);
    let root = tree.root();
    let (key, object) = &objects[0];
    let branch = tree.branch(key).unwrap();
    // Another key that hangs at the same slot: the leaf is not its.
    let other = neighbour(key, branch.len() - 1);
    assert_eq!(
        verify(&root, &other, Leaf::State(object), &branch),
        Err(BranchError::Leaf)
    );
    // The leaf's key altered in its bytes.
    let mut altered = branch.clone();
    let at = altered[0].len() - 2;
    altered[0][at] ^= 1;
    assert_eq!(
        verify(&root, key, Leaf::State(object), &altered),
        Err(BranchError::Hash)
    );
    // A key on another path entirely.
    let far = neighbour(key, 0);
    assert!(verify(&root, &far, Leaf::State(object), &branch).is_err());

    // A transaction under a key that is not its hash, or with other
    // metadata than its leaf holds.
    let data = transaction_set();
    let entry = &data["transactions"][0];
    let (transaction, metadata) = (bytes(entry, "tx_blob"), bytes(entry, "meta"));
    let mut transactions = ShaMap::new(Tree::Transactions);
    let key = transactions.insert_transaction(&transaction, &metadata);
    transactions.insert_transaction(&bytes(&data["transactions"][1], "tx_blob"), &metadata);
    let root = transactions.root();
    let branch = transactions.branch(&key).unwrap();
    let leaf = Leaf::Transaction {
        transaction: &transaction,
        metadata: &metadata,
    };
    assert_eq!(verify(&root, &key, leaf, &branch), Ok(()));
    assert_eq!(
        verify(&root, &neighbour(&key, 63), leaf, &branch),
        Err(BranchError::Leaf)
    );
    let other = Leaf::Transaction {
        transaction: &transaction,
        metadata: &metadata[1..],
    };
    assert_eq!(verify(&root, &key, other, &branch), Err(BranchError::Leaf));
    // A transaction tree's leaf is not read as a state tree's.
    assert_eq!(
        verify(&root, &key, Leaf::Absent(Tree::State), &branch),
        Err(BranchError::Node)
    );
}

/// The root over a branch built from the bottom up: `nodes` from the
/// bottom, each node's wire form, the root's last.
fn root_of(nodes: &[Vec<u8>], tree: Tree) -> [u8; 32] {
    let top = nodes.last().unwrap();
    let (kind, body) = top.split_last().unwrap();
    match kind {
        2 | 3 => {
            let mut children = [[0; 32]; BRANCH_FACTOR];
            if *kind == 2 {
                for (child, hash) in children.iter_mut().zip(body.chunks(32)) {
                    child.copy_from_slice(hash);
                }
            } else {
                for chunk in body.chunks(33) {
                    children[usize::from(chunk[32])].copy_from_slice(&chunk[..32]);
                }
            }
            inner_hash(&children)
        }
        _ => {
            let (item, key) = body.split_at(body.len() - 32);
            leaf_hash(tree, key.try_into().unwrap(), item)
        }
    }
}

/// An inner node with these children.
fn inner(children: &[(usize, [u8; 32])]) -> Vec<u8> {
    let mut all = [[0; 32]; BRANCH_FACTOR];
    for (slot, hash) in children {
        all[*slot] = *hash;
    }
    inner_wire(&all)
}

/// The hash of a node in wire form.
fn node_hash(node: &[u8], tree: Tree) -> [u8; 32] {
    root_of(&[node.to_vec()], tree)
}

// Trees rippled never builds, with their roots: whatever their hashes, a
// branch through them proves nothing.
#[test]
fn shapes_rippled_never_builds_prove_nothing() {
    let key = [0xAB; 32];
    let object = b"an object at its key";
    let leaf = leaf_wire(Tree::State, &key, object);
    let claim = Leaf::State(object);

    // The leaf one level deeper than it hangs, below an inner node with no
    // other child.
    let deeper = inner(&[(nibble(&key, 1), node_hash(&leaf, Tree::State))]);
    let root = inner(&[
        (nibble(&key, 0), node_hash(&deeper, Tree::State)),
        (0, [7; 32]),
    ]);
    let branch = [leaf.clone(), deeper, root];
    assert_eq!(
        verify(&root_of(&branch, Tree::State), &key, claim, &branch),
        Err(BranchError::Shape)
    );
    // With a sibling, the same depth is rippled's.
    let beside = inner(&[
        (nibble(&key, 1), node_hash(&leaf, Tree::State)),
        (0, [8; 32]),
    ]);
    let root = inner(&[(nibble(&key, 0), node_hash(&beside, Tree::State))]);
    let branch = [leaf.clone(), beside, root];
    assert_eq!(
        verify(&root_of(&branch, Tree::State), &key, claim, &branch),
        Ok(())
    );

    // A leaf as the root.
    let branch = [leaf.clone()];
    assert_eq!(
        verify(&root_of(&branch, Tree::State), &key, claim, &branch),
        Err(BranchError::Shape)
    );

    // A leaf off the key's path, offered as the key's absence: its key
    // differs in the first nibble, so it cannot hang under the key's.
    let stray_key = neighbour(&key, 0);
    let stray = leaf_wire(Tree::State, &stray_key, object);
    let root = inner(&[
        (nibble(&key, 0), node_hash(&stray, Tree::State)),
        (0, [9; 32]),
    ]);
    let branch = [stray, root];
    assert_eq!(
        verify(
            &root_of(&branch, Tree::State),
            &key,
            Leaf::Absent(Tree::State),
            &branch
        ),
        Err(BranchError::Shape)
    );

    // Inner nodes below the last nibble.
    let mut branch = vec![inner(&[(0, [1; 32])])];
    for depth in (0..MAX_DEPTH).rev() {
        let below = node_hash(branch.last().unwrap(), Tree::State);
        branch.push(inner(&[
            (nibble(&key, depth), below),
            ((nibble(&key, depth) + 1) % 16, [2; 32]),
        ]));
    }
    assert_eq!(branch.len(), MAX_BRANCH_NODES);
    assert_eq!(
        verify(
            &root_of(&branch, Tree::State),
            &key,
            Leaf::Absent(Tree::State),
            &branch
        ),
        Err(BranchError::Shape)
    );
}

prop_compose! {
    fn keys()(keys in prop::collection::btree_set(any::<[u8; 32]>(), 1..64)) -> Vec<[u8; 32]> {
        // Keys that share long prefixes, as well as keys at random.
        let mut keys: Vec<[u8; 32]> = keys.into_iter().filter(|key| *key != [0; 32]).collect();
        let near: Vec<[u8; 32]> = keys.iter().take(8).map(|key| neighbour(key, usize::from(key[0] % 64))).collect();
        keys.extend(near);
        keys.sort_unstable();
        keys.dedup();
        keys
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Any tree's branches prove what it holds and what it does not, and a
    // branch with any byte changed proves nothing it did not.
    #[test]
    fn branches_prove_what_a_tree_holds(
        keys in keys(),
        probe in any::<[u8; 32]>(),
        flip in any::<prop::sample::Index>(),
    ) {
        let (tree, objects) = state_tree(&keys);
        let root = tree.root();
        for (key, object) in &objects {
            let branch = tree.branch(key).unwrap();
            prop_assert!(branch.len() <= MAX_BRANCH_NODES);
            prop_assert_eq!(verify(&root, key, Leaf::State(object), &branch), Ok(()));
        }
        let held = objects.iter().find(|(key, _)| *key == probe);
        let branch = tree.branch(&probe).unwrap();
        let claim = held.map_or(Leaf::Absent(Tree::State), |(_, object)| Leaf::State(object));
        prop_assert_eq!(verify(&root, &probe, claim, &branch), Ok(()));
        // One bit changed anywhere in the branch.
        let total: usize = branch.iter().map(Vec::len).sum();
        let mut at = flip.index(total);
        let mut changed = branch.clone();
        for node in &mut changed {
            if at < node.len() {
                node[at] ^= 1;
                break;
            }
            at -= node.len();
        }
        prop_assert!(verify(&root, &probe, claim, &changed).is_err());
    }
}

//! The core, as the signed data defines it
//! (spec/decisions/promotion-to-core.md): grown from the bootstrap nodes in
//! the 2-core of the edges both ends advertise between registered nodes,
//! in its component with the most bootstrap nodes, by every registered node
//! with two edges into it.
//!
//! Every node holds the same signed data, the bootstrap nodes' lists and
//! the link states, so every node computes the same core from it, but for
//! the time a link state takes to spread. Nothing a node says about itself
//! makes it core: only edges its neighbors confirm do. Growing it one node
//! at a time, each by edges into the core, keeps a group of nodes that
//! advertise each other out of it, however many registered nodes it has:
//! each needs two core nodes that promoted it.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

use dassie_identity::NodeId;

use crate::routing::is_edge;

/// Most nodes on the dev core list (`DASSIE_DEV_CORE_NODES`), as many as
/// a node takes bootstrap nodes.
pub const MAX_DEV_CORE: usize = 64;

/// How many edges a node needs in the 2-core the bootstrap nodes are taken
/// from: two, or, in a network with fewer than three bootstrap nodes, as
/// many as it has less one, so one bootstrap node is a core alone and two
/// are one together.
#[must_use]
pub const fn edges_needed(bootstraps: usize) -> usize {
    let fewer = bootstraps.saturating_sub(1);
    if fewer < 2 { fewer } else { 2 }
}

/// How many edges into the core a node needs to join it: as many as
/// [`edges_needed`], but at least one.
#[must_use]
pub const fn edges_to_join(bootstraps: usize) -> usize {
    let needed = edges_needed(bootstraps);
    if needed < 1 { 1 } else { needed }
}

/// Computes the core.
///
/// - `registered`: the nodes on the majority of the bootstrap nodes' lists.
/// - `bootstrap`: the bootstrap nodes' IDs.
/// - `dev_core`: the nodes a dev network makes core outright, whatever
///   their edges (empty outside one).
/// - `neighbors`: a node's advertised neighbors, sorted, if its link state
///   is fresh.
///
/// No bootstrap nodes at all asks no edges, and gives no core but the dev
/// core: the caller passes a node that runs alone as its own bootstrap
/// node.
pub fn compute<'a>(
    registered: &BTreeSet<NodeId>,
    bootstrap: &BTreeSet<NodeId>,
    dev_core: &BTreeSet<NodeId>,
    neighbors: impl Fn(&NodeId) -> Option<&'a [NodeId]>,
) -> BTreeSet<NodeId> {
    // The graph: registered nodes and the dev core, joined where both ends
    // list each other.
    let mut graph: BTreeMap<NodeId, Vec<NodeId>> = registered
        .iter()
        .chain(dev_core)
        .map(|node| (*node, Vec::new()))
        .collect();
    let nodes: Vec<NodeId> = graph.keys().copied().collect();
    for node in &nodes {
        let edges: Vec<NodeId> = neighbors(node)
            .unwrap_or_default()
            .iter()
            .filter(|other| {
                *other != node && graph.contains_key(other) && is_edge(&neighbors, node, other)
            })
            .copied()
            .collect();
        if let Some(entry) = graph.get_mut(node) {
            *entry = edges;
        }
    }
    let mut peeled = graph.clone();
    peel(&mut peeled, edges_needed(bootstrap.len()), dev_core);
    // The bootstrap nodes of that component are the seed, with the dev
    // core: core because they peer, or because a dev network says so.
    let seed: BTreeSet<NodeId> = best_component(&peeled, bootstrap)
        .intersection(bootstrap)
        .chain(dev_core)
        .copied()
        .collect();
    grow(&graph, seed, edges_to_join(bootstrap.len()))
}

/// Grows `core` by every node of `graph` with `needed` edges into it, again
/// until none joins. The result does not depend on the order nodes join
/// in: one with enough edges into the core keeps them as the core grows.
fn grow(
    graph: &BTreeMap<NodeId, Vec<NodeId>>,
    mut core: BTreeSet<NodeId>,
    needed: usize,
) -> BTreeSet<NodeId> {
    let mut into: BTreeMap<NodeId, usize> = BTreeMap::new();
    let mut queue: VecDeque<NodeId> = core.iter().copied().collect();
    while let Some(node) = queue.pop_front() {
        for other in graph.get(&node).map(Vec::as_slice).unwrap_or_default() {
            if core.contains(other) {
                continue;
            }
            let count = into.entry(*other).or_default();
            *count += 1;
            if *count >= needed {
                core.insert(*other);
                queue.push_back(*other);
            }
        }
    }
    core
}

/// Drops the nodes with fewer than `needed` edges, again until none is left
/// to drop, but those in `kept`.
fn peel(graph: &mut BTreeMap<NodeId, Vec<NodeId>>, needed: usize, kept: &BTreeSet<NodeId>) {
    let mut queue: VecDeque<NodeId> = graph
        .iter()
        .filter(|(node, edges)| edges.len() < needed && !kept.contains(*node))
        .map(|(node, _)| *node)
        .collect();
    while let Some(node) = queue.pop_front() {
        let Some(edges) = graph.remove(&node) else {
            continue;
        };
        for other in edges {
            let Some(left) = graph.get_mut(&other) else {
                continue;
            };
            let before = left.len();
            left.retain(|edge| *edge != node);
            // Queued once: as it falls below what is needed.
            if before == needed && left.len() < needed && !kept.contains(&other) {
                queue.push_back(other);
            }
        }
    }
}

/// The component of `graph` with the most bootstrap nodes; among those
/// with equally many, the one whose lowest bootstrap node ID is lowest.
/// None without a bootstrap node.
fn best_component(
    graph: &BTreeMap<NodeId, Vec<NodeId>>,
    bootstrap: &BTreeSet<NodeId>,
) -> BTreeSet<NodeId> {
    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    let mut best: Option<(usize, NodeId, BTreeSet<NodeId>)> = None;
    // From each bootstrap node in ID order: a component found first holds
    // the lowest bootstrap node ID among those it ties with.
    for start in bootstrap.iter().filter(|node| graph.contains_key(*node)) {
        if seen.contains(start) {
            continue;
        }
        let mut component = BTreeSet::new();
        let mut queue = VecDeque::from([*start]);
        seen.insert(*start);
        while let Some(node) = queue.pop_front() {
            component.insert(node);
            for other in graph.get(&node).map(Vec::as_slice).unwrap_or_default() {
                if seen.insert(*other) {
                    queue.push_back(*other);
                }
            }
        }
        let count = component.iter().filter(|n| bootstrap.contains(*n)).count();
        if best.as_ref().is_none_or(|(most, _, _)| count > *most) {
            best = Some((count, *start, component));
        }
    }
    best.map(|(_, _, component)| component).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    /// The node ID that starts with `name`, the rest `a`s: IDs in the
    /// order of their names.
    fn id(name: &str) -> NodeId {
        NodeId::parse(&alloc::format!("{name:a<39}")).unwrap()
    }

    fn ids(names: &[&str]) -> BTreeSet<NodeId> {
        names.iter().map(|name| id(name)).collect()
    }

    fn graph(edges: &[(&str, &str)]) -> BTreeMap<NodeId, Vec<NodeId>> {
        let mut map: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        for (a, b) in edges {
            map.entry(id(a)).or_default().push(id(b));
            map.entry(id(b)).or_default().push(id(a));
        }
        for list in map.values_mut() {
            list.sort_unstable();
            list.dedup();
        }
        map
    }

    fn core(
        map: &BTreeMap<NodeId, Vec<NodeId>>,
        registered: &[&str],
        bootstrap: &[&str],
        dev_core: &[&str],
    ) -> BTreeSet<NodeId> {
        compute(&ids(registered), &ids(bootstrap), &ids(dev_core), |node| {
            map.get(node).map(Vec::as_slice)
        })
    }

    // A ring of three bootstrap nodes is a core; a node with two edges
    // into it joins, one with a single edge does not, and neither does
    // one hanging off it.
    #[test]
    fn the_2_core_of_registered_nodes() {
        let map = graph(&[
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("na", "ba"),
            ("na", "bb"),
            ("nb", "bc"),
            ("nc", "nb"),
        ]);
        let all = ["ba", "bb", "bc", "na", "nb", "nc"];
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc"], &[]),
            ids(&["ba", "bb", "bc", "na"])
        );
    }

    // A group of registered nodes that advertise each other does not join
    // with a single honest edge: each node needs two edges into the core,
    // so two core nodes that promoted it. In a 2-core alone, a triangle of
    // the attacker's own nodes with one edge to an honest node would.
    #[test]
    fn a_group_vouching_for_itself_stays_out() {
        let map = graph(&[
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("aa", "ba"),
            ("aa", "ab"),
            ("ab", "ac"),
            ("ac", "aa"),
        ]);
        let all = ["ba", "bb", "bc", "aa", "ab", "ac"];
        let bootstrap = ["ba", "bb", "bc"];
        assert_eq!(core(&map, &all, &bootstrap, &[]), ids(&bootstrap));
        // Nor a chain that closes through the core at both ends.
        let map = graph(&[
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("na", "ba"),
            ("na", "nb"),
            ("nb", "nc"),
            ("nc", "bb"),
        ]);
        let all = ["ba", "bb", "bc", "na", "nb", "nc"];
        assert_eq!(core(&map, &all, &bootstrap, &[]), ids(&bootstrap));
    }

    // Nodes join one by one, each by two edges into the core as it has
    // grown: a node whose second edge is to a node that joined first joins
    // after it.
    #[test]
    fn the_core_grows_one_node_at_a_time() {
        let map = graph(&[
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("na", "ba"),
            ("na", "bb"),
            ("nb", "na"),
            ("nb", "bc"),
            ("nc", "nb"),
        ]);
        let all = ["ba", "bb", "bc", "na", "nb", "nc"];
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc"], &[]),
            ids(&["ba", "bb", "bc", "na", "nb"])
        );
    }

    // Bootstrap nodes joined only through other nodes are seeds all the
    // same, if those nodes are in the 2-core with them: the nodes between
    // them then join by their edges into the seed.
    #[test]
    fn seeds_are_the_bootstrap_nodes_in_the_2_core() {
        let map = graph(&[
            ("ba", "na"),
            ("na", "bb"),
            ("bb", "nb"),
            ("nb", "bc"),
            ("bc", "nc"),
            ("nc", "ba"),
        ]);
        let all = ["ba", "bb", "bc", "na", "nb", "nc"];
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc"], &[]),
            ids(&all),
            "a ring through all three"
        );
    }

    // Edges count only if both ends list each other, and only between
    // registered nodes.
    #[test]
    fn edges_need_both_ends_and_registration() {
        let mut map = graph(&[("ba", "bb"), ("bb", "bc"), ("bc", "ba"), ("na", "ba")]);
        // na claims bb too, which does not list it.
        map.get_mut(&id("na")).unwrap().push(id("bb"));
        map.get_mut(&id("na")).unwrap().sort_unstable();
        let all = ["ba", "bb", "bc", "na"];
        let bootstrap = ["ba", "bb", "bc"];
        assert_eq!(core(&map, &all, &bootstrap, &[]), ids(&["ba", "bb", "bc"]));
        // Unregistered nodes are no part of it, nor are their edges.
        let map = graph(&[
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("na", "ba"),
            ("na", "bb"),
        ]);
        assert_eq!(
            core(&map, &["ba", "bb", "bc"], &bootstrap, &[]),
            ids(&["ba", "bb", "bc"])
        );
        // A node without a fresh link state has no edges.
        let without = |node: &NodeId| (*node != id("na")).then(|| map.get(node)).flatten();
        let core = compute(&ids(&all), &ids(&bootstrap), &BTreeSet::new(), |node| {
            without(node).map(Vec::as_slice)
        });
        assert_eq!(core, ids(&["ba", "bb", "bc"]));
    }

    // Of two components, the one with more bootstrap nodes; with equally
    // many, the one whose lowest bootstrap node ID is lowest.
    #[test]
    fn the_component_with_the_most_bootstrap_nodes() {
        let rings = [
            ("ba", "bb"),
            ("bb", "na"),
            ("na", "ba"),
            ("bc", "bd"),
            ("bd", "be"),
            ("be", "bc"),
        ];
        let map = graph(&rings);
        let all = ["ba", "bb", "bc", "bd", "be", "na"];
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc", "bd", "be"], &[]),
            ids(&["bc", "bd", "be"])
        );
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc", "bd"], &[]),
            ids(&["ba", "bb", "na"]),
            "a tie goes to the lowest bootstrap node ID"
        );
        assert_eq!(
            core(&map, &all, &["bb", "bc", "bd", "be"], &[]),
            ids(&["bc", "bd", "be"]),
        );
    }

    // spec/decisions/promotion-to-core.md: a network with fewer than three
    // bootstrap nodes asks for as many edges as it has less one.
    #[test]
    fn small_networks_ask_fewer_edges() {
        assert_eq!(
            [0, 1, 2, 3, 64].map(edges_needed),
            [0, 0, 1, 2, 2],
            "edges needed by bootstrap nodes"
        );
        assert_eq!(
            [0, 1, 2, 3, 64].map(edges_to_join),
            [1, 1, 1, 2, 2],
            "edges into the core needed to join it"
        );
        // A node with no edge does not join a lone bootstrap node.
        let map = graph(&[("na", "nb")]);
        assert_eq!(core(&map, &["ba", "na", "nb"], &["ba"], &[]), ids(&["ba"]));
        // One bootstrap node is a core alone.
        let map = graph(&[]);
        assert_eq!(core(&map, &["ba"], &["ba"], &[]), ids(&["ba"]));
        // Two are one together, and a node with an edge joins them.
        let map = graph(&[("ba", "bb"), ("na", "bb")]);
        assert_eq!(
            core(&map, &["ba", "bb", "na"], &["ba", "bb"], &[]),
            ids(&["ba", "bb", "na"])
        );
        // Two that are not joined yet are no core: each needs an edge.
        let map = graph(&[]);
        assert!(core(&map, &["ba", "bb"], &["ba", "bb"], &[]).is_empty());
    }

    // Without a bootstrap node in the graph, there is no core: a network
    // the configured bootstrap nodes are not in counts as none.
    #[test]
    fn no_core_without_a_bootstrap_node() {
        let map = graph(&[("na", "nb"), ("nb", "nc"), ("nc", "na")]);
        assert!(core(&map, &["na", "nb", "nc"], &["ba", "bb", "bc"], &[]).is_empty());
        assert!(core(&map, &["na", "nb", "nc"], &[], &[]).is_empty());
        // Bootstrap nodes that are not registered are not in it either.
        let map = graph(&[("ba", "bb"), ("bb", "bc"), ("bc", "ba")]);
        assert!(core(&map, &[], &["ba", "bb", "bc"], &[]).is_empty());
    }

    // Two networks with bootstrap nodes of their own never both count as
    // the core, where a node sees both.
    #[test]
    fn one_core_of_two_networks() {
        let map = graph(&[
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("xa", "xb"),
            ("xb", "xc"),
            ("xc", "xa"),
        ]);
        let all = ["ba", "bb", "bc", "xa", "xb", "xc"];
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc"], &[]),
            ids(&["ba", "bb", "bc"])
        );
    }

    // The dev core is core outright, and keeps its own edges: a node with
    // two edges into it joins, though it holds no bootstrap node.
    #[test]
    fn the_dev_core_is_core_outright() {
        let map = graph(&[("ba", "bb"), ("bb", "bc"), ("bc", "ba"), ("da", "na")]);
        let all = ["ba", "bb", "bc", "na"];
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc"], &["da", "db"]),
            ids(&["ba", "bb", "bc", "da", "db"]),
            "da is core with a single edge; na, with one, is not"
        );
        let map = graph(&[
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("da", "na"),
            ("na", "ba"),
        ]);
        assert_eq!(
            core(&map, &all, &["ba", "bb", "bc"], &["da"]),
            ids(&["ba", "bb", "bc", "da", "na"]),
        );
    }

    // The same data gives the same core, whatever order it comes in.
    #[test]
    fn deterministic() {
        let edges = vec![
            ("ba", "bb"),
            ("bb", "bc"),
            ("bc", "ba"),
            ("na", "ba"),
            ("na", "bb"),
            ("nb", "na"),
        ];
        let mut reversed = edges.clone();
        reversed.reverse();
        let all = ["ba", "bb", "bc", "na", "nb"];
        let bootstrap = ["ba", "bb", "bc"];
        assert_eq!(
            core(&graph(&edges), &all, &bootstrap, &[]),
            core(&graph(&reversed), &all, &bootstrap, &[])
        );
    }
}

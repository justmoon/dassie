//! Routes from link states (spec/areas/network.md#routing): a breadth-first
//! search from ourselves over the edges both ends advertise, giving each
//! reachable node its first hops on shortest paths.
//!
//! An edge counts only if both ends list each other, so no node can claim
//! a link the other end does not confirm; the caller passes only fresh
//! link states, so a node that went silent drops out.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use dassie_identity::NodeId;

/// Most first hops kept per destination.
pub const MAX_FIRST_HOPS: usize = 4;

/// The route to a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// How many hops away it is: 1 for a neighbor.
    pub distance_hops: u32,
    /// Its first hops on shortest paths, sorted.
    pub first_hops: Vec<NodeId>,
}

/// The routes: one for each reachable node. There is none for ourselves.
pub type Routes = BTreeMap<NodeId, Route>;

/// Computes the routes from `own`. `neighbors` gives a node's advertised
/// neighbors, sorted, if its link state is fresh.
pub fn compute<'a>(own: &NodeId, neighbors: impl Fn(&NodeId) -> Option<&'a [NodeId]>) -> Routes {
    let Some(own_neighbors) = neighbors(own) else {
        return Routes::new();
    };
    let first: Vec<NodeId> = own_neighbors
        .iter()
        .filter(|neighbor| *neighbor != own && is_edge(&neighbors, own, neighbor))
        .copied()
        .collect();
    through(own, &first, neighbors)
}

/// Computes the routes from `own` through `first_hops`, whatever they
/// list: a periphery node's, whose uplinks never list it, but carry its
/// packets (spec/areas/network.md#routing). Beyond them, edges count only
/// if both ends list each other, as always.
pub fn through<'a>(
    own: &NodeId,
    first_hops: &[NodeId],
    neighbors: impl Fn(&NodeId) -> Option<&'a [NodeId]>,
) -> Routes {
    let mut routes = Routes::new();
    // Level by level: each node's level, and the queue of the next.
    let mut level: BTreeMap<NodeId, u32> = BTreeMap::new();
    level.insert(*own, 0);
    let mut queue = VecDeque::new();
    for neighbor in first_hops {
        if neighbor != own && neighbors(neighbor).is_some() && !level.contains_key(neighbor) {
            level.insert(*neighbor, 1);
            routes.insert(
                *neighbor,
                Route {
                    distance_hops: 1,
                    first_hops: alloc::vec![*neighbor],
                },
            );
            queue.push_back(*neighbor);
        }
    }
    while let Some(current) = queue.pop_front() {
        let Some(&depth) = level.get(&current) else {
            continue;
        };
        let hops = routes
            .get(&current)
            .map(|route| route.first_hops.clone())
            .unwrap_or_default();
        for next in neighbors(&current).unwrap_or_default() {
            if !is_edge(&neighbors, &current, next) {
                continue;
            }
            match level.get(next) {
                None => {
                    level.insert(*next, depth + 1);
                    routes.insert(
                        *next,
                        Route {
                            distance_hops: depth + 1,
                            first_hops: hops.clone(),
                        },
                    );
                    queue.push_back(*next);
                }
                Some(&other) if other == depth + 1 => {
                    if let Some(existing) = routes.get_mut(next) {
                        merge(&mut existing.first_hops, &hops);
                    }
                }
                Some(_) => {}
            }
        }
    }
    routes
}

/// Whether `a` and `b` list each other.
fn is_edge<'a>(
    neighbors: &impl Fn(&NodeId) -> Option<&'a [NodeId]>,
    a: &NodeId,
    b: &NodeId,
) -> bool {
    neighbors(b).is_some_and(|listed| listed.binary_search(a).is_ok())
        && neighbors(a).is_some_and(|listed| listed.binary_search(b).is_ok())
}

/// Adds `hops` to `existing`, keeping it sorted and bounded.
fn merge(existing: &mut Vec<NodeId>, hops: &[NodeId]) {
    for hop in hops {
        if let Err(at) = existing.binary_search(hop) {
            existing.insert(at, *hop);
        }
    }
    existing.truncate(MAX_FIRST_HOPS);
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

    fn routes(map: &BTreeMap<NodeId, Vec<NodeId>>, own: &str) -> Routes {
        compute(&id(own), |node| map.get(node).map(Vec::as_slice))
    }

    #[test]
    fn shortest_paths_give_every_first_hop() {
        // da - db - dd, da - dc - dd, dd - de
        let map = graph(&[
            ("da", "db"),
            ("da", "dc"),
            ("db", "dd"),
            ("dc", "dd"),
            ("dd", "de"),
        ]);
        let routes = routes(&map, "da");
        assert_eq!(routes[&id("db")].first_hops, vec![id("db")]);
        assert_eq!(routes[&id("dd")].first_hops, vec![id("db"), id("dc")]);
        assert_eq!(routes[&id("de")].first_hops, vec![id("db"), id("dc")]);
        let distances: Vec<u32> = ["db", "dc", "dd", "de"]
            .iter()
            .map(|name| routes[&id(name)].distance_hops)
            .collect();
        assert_eq!(distances, vec![1, 1, 2, 3]);
        assert!(!routes.contains_key(&id("da")), "no route to ourselves");
    }

    // A node claiming a link the other end does not list gets no route
    // through it.
    #[test]
    fn one_sided_links_do_not_count() {
        let mut map = graph(&[("da", "db")]);
        map.get_mut(&id("db")).unwrap().push(id("dx"));
        map.insert(id("dx"), vec![]);
        let routes = routes(&map, "da");
        assert!(!routes.contains_key(&id("dx")));
        // And a node that does not list us is not our first hop.
        let mut map = graph(&[("db", "dc")]);
        map.insert(id("da"), vec![id("db")]);
        assert!(routes_from(&map, "da").is_empty());
    }

    fn routes_from(map: &BTreeMap<NodeId, Vec<NodeId>>, own: &str) -> Routes {
        routes(map, own)
    }

    #[test]
    fn first_hops_are_bounded() {
        let mut edges = vec![];
        let names = ["db", "dc", "dd", "de", "df", "dg"];
        for name in names {
            edges.push(("da", name));
            edges.push((name, "dz"));
        }
        let map = graph(&edges);
        assert_eq!(
            routes(&map, "da")[&id("dz")].first_hops.len(),
            MAX_FIRST_HOPS
        );
    }
}

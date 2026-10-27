//! Routes from link states (spec/areas/network.md#routing): a breadth-first
//! search from ourselves over the edges both ends advertise, giving each
//! reachable node its first hops on shortest paths.
//!
//! An edge counts only if both ends list each other, so no node can claim
//! a link the other end does not confirm; the caller passes only fresh
//! link states, so a node that went silent drops out.
//!
//! Where more first hops reach a node than a route keeps, the cut keeps
//! those that rank lowest by a keyed hash of their node IDs, under a key
//! only this node knows (spec/decisions/weighted-next-hops.md): a node
//! cannot grind its ID to be kept by everyone, as it could to come first
//! by ID.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use dassie_identity::{NodeId, NodeKey, NodeSecret};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// Most first hops kept per destination.
pub const MAX_FIRST_HOPS: usize = 4;

/// The route to a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// How many hops away it is: 1 for a neighbor.
    pub distance_hops: u32,
    /// Its first hops on shortest paths, at most [`MAX_FIRST_HOPS`], sorted
    /// by node ID.
    pub first_hops: Vec<NodeId>,
}

/// The routes: one for each reachable node. There is none for ourselves.
pub type Routes = BTreeMap<NodeId, Route>;

/// The secret that ranks first hops for the cut, derived from the node key
/// as [`NodeSecret::Routing`]: the same node ranks the same way after a
/// restart, and no other node can tell how.
pub struct RouteKey([u8; 32]);

impl RouteKey {
    /// The route key of the node with `key`.
    #[must_use]
    pub fn of(key: &NodeKey) -> Self {
        Self(*key.secret(NodeSecret::Routing).as_bytes())
    }

    /// A route key from raw bytes, for tests and simulations.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Where `hop` ranks for the cut: the lowest are kept.
    // HMAC takes keys of any length, so this cannot panic.
    #[allow(clippy::panic)]
    fn rank(&self, hop: &NodeId) -> [u8; 32] {
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&self.0) else {
            panic!("HMAC accepts keys of any length");
        };
        mac.update(hop.as_str().as_bytes());
        mac.finalize().into_bytes().into()
    }
}

impl Drop for RouteKey {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.0);
    }
}

impl core::fmt::Debug for RouteKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("RouteKey(<redacted>)")
    }
}

/// Computes the routes from `own`, cut by `key`. `neighbors` gives a
/// node's advertised neighbors, sorted, if its link state is fresh.
pub fn compute<'a>(
    own: &NodeId,
    key: &RouteKey,
    neighbors: impl Fn(&NodeId) -> Option<&'a [NodeId]>,
) -> Routes {
    let Some(own_neighbors) = neighbors(own) else {
        return Routes::new();
    };
    let first: Vec<NodeId> = own_neighbors
        .iter()
        .filter(|neighbor| *neighbor != own && is_edge(&neighbors, own, neighbor))
        .copied()
        .collect();
    through(own, key, &first, neighbors)
}

/// Computes the routes from `own` through `first_hops`, whatever they
/// list: a periphery node's, whose uplinks never list it, but carry its
/// packets (spec/areas/network.md#routing). Beyond them, edges count only
/// if both ends list each other, as always. Where more first hops reach a
/// node than [`MAX_FIRST_HOPS`], those that rank lowest under `key` are
/// kept.
pub fn through<'a>(
    own: &NodeId,
    key: &RouteKey,
    first_hops: &[NodeId],
    neighbors: impl Fn(&NodeId) -> Option<&'a [NodeId]>,
) -> Routes {
    // The first hops in the order the cut keeps them. The search carries
    // each node's first hops as positions in this order, so keeping the
    // lowest positions keeps the lowest ranks.
    let mut ranked: Vec<([u8; 32], NodeId)> = first_hops
        .iter()
        .filter(|hop| *hop != own && neighbors(hop).is_some())
        .map(|hop| (key.rank(hop), *hop))
        .collect();
    ranked.sort_unstable();
    ranked.dedup_by(|a, b| a.1 == b.1);
    // Level by level: each node's level, and the queue of the next.
    let mut level: BTreeMap<NodeId, u32> = BTreeMap::new();
    level.insert(*own, 0);
    let mut via: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
    let mut queue = VecDeque::new();
    for (position, (_, hop)) in ranked.iter().enumerate() {
        if !level.contains_key(hop) {
            level.insert(*hop, 1);
            via.insert(*hop, alloc::vec![position]);
            queue.push_back(*hop);
        }
    }
    while let Some(current) = queue.pop_front() {
        let Some(&depth) = level.get(&current) else {
            continue;
        };
        let hops = via.get(&current).cloned().unwrap_or_default();
        for next in neighbors(&current).unwrap_or_default() {
            if !is_edge(&neighbors, &current, next) {
                continue;
            }
            match level.get(next) {
                None => {
                    level.insert(*next, depth + 1);
                    via.insert(*next, hops.clone());
                    queue.push_back(*next);
                }
                Some(&other) if other == depth + 1 => {
                    if let Some(existing) = via.get_mut(next) {
                        merge(existing, &hops);
                    }
                }
                Some(_) => {}
            }
        }
    }
    via.into_iter()
        .filter_map(|(node, positions)| {
            let distance_hops = *level.get(&node)?;
            let mut first_hops: Vec<NodeId> = positions
                .iter()
                .filter_map(|position| ranked.get(*position).map(|(_, hop)| *hop))
                .collect();
            first_hops.sort_unstable();
            Some((
                node,
                Route {
                    distance_hops,
                    first_hops,
                },
            ))
        })
        .collect()
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

/// Adds `hops`, positions in the cut's order, to `existing`, keeping it
/// sorted and bounded: the lowest-ranked stay.
fn merge(existing: &mut Vec<usize>, hops: &[usize]) {
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

    const KEY: RouteKey = RouteKey::from_bytes([7; 32]);

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
        compute(&id(own), &KEY, |node| map.get(node).map(Vec::as_slice))
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

    // Six first hops reach dz, and its route keeps four.
    fn six_ways() -> BTreeMap<NodeId, Vec<NodeId>> {
        let mut edges = vec![];
        for name in ["db", "dc", "dd", "de", "df", "dg"] {
            edges.push(("da", name));
            edges.push((name, "dz"));
        }
        graph(&edges)
    }

    #[test]
    fn first_hops_are_bounded() {
        assert_eq!(
            routes(&six_ways(), "da")[&id("dz")].first_hops.len(),
            MAX_FIRST_HOPS
        );
    }

    // spec/decisions/weighted-next-hops.md: the cut keeps the first hops
    // that rank lowest under the node's own key, not those with the lowest
    // IDs, so no ID is kept by every node; the same key always keeps the
    // same ones, sorted by ID.
    #[test]
    fn the_cut_ranks_by_a_keyed_hash() {
        let map = six_ways();
        let mut ever_dropped_lowest = false;
        for byte in 0..8u8 {
            let key = RouteKey::from_bytes([byte; 32]);
            let kept = compute(&id("da"), &key, |node| map.get(node).map(Vec::as_slice))[&id("dz")]
                .first_hops
                .clone();
            let mut expected: Vec<([u8; 32], NodeId)> = ["db", "dc", "dd", "de", "df", "dg"]
                .iter()
                .map(|name| (key.rank(&id(name)), id(name)))
                .collect();
            expected.sort_unstable();
            let mut expected: Vec<NodeId> = expected
                .into_iter()
                .take(MAX_FIRST_HOPS)
                .map(|(_, hop)| hop)
                .collect();
            expected.sort_unstable();
            assert_eq!(kept, expected, "the lowest ranks under key {byte}");
            let again = compute(&id("da"), &key, |node| map.get(node).map(Vec::as_slice));
            assert_eq!(again[&id("dz")].first_hops, kept, "deterministic");
            ever_dropped_lowest |= !kept.contains(&id("db"));
        }
        assert!(ever_dropped_lowest, "the lowest ID is not always kept");
    }

    // The route key derives from the node key, the same each time.
    #[test]
    fn the_route_key_derives_from_the_node_key() {
        let key = NodeKey::from_bytes(&[3; 32]);
        let (a, b) = (RouteKey::of(&key), RouteKey::of(&key));
        assert_eq!(a.0, b.0);
        assert_ne!(a.0, RouteKey::of(&NodeKey::from_bytes(&[4; 32])).0);
        assert_ne!(&a.0, key.as_bytes(), "not the node key itself");
    }
}

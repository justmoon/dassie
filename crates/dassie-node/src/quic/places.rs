//! Places for the connections the endpoint takes, by who opened them
//! (spec/areas/peer-protocol.md#quic).
//!
//! A connection arriving takes a place among those being set up, counted
//! per client address, for at most the time to connect. Once its handshake
//! proved the other side's node key, it takes a place in its class instead,
//! by what our own peerings say of that key:
//!
//! - a core peer's, among the core peers' places;
//! - a downlink's, among the downlinks' places, which fit `max_downlinks`;
//! - anyone else's (relay clients, nodes that chose us as their relay,
//!   registrants, nodes asking to peer, tools), among the others' places,
//!   counted per client address too.
//!
//! So downlinks never take the room of core peers or relayed streams, and
//! no stranger takes the core peers'. Core peers and downlinks hold a share
//! each, per node ([`NodeTable`]). When a peering begins or ends, its
//! node's connections move to their new class, or close if it has no room.
//! What a node says about itself decides nothing: only the key it proved,
//! which it cannot borrow, and our own peerings.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dassie_identity::NodeId;
use dassie_node_core::connections::{ConnectionLimits, NodeLimits, NodeTable, Refused};
use dassie_node_core::rate_limit::ClientKey;
use quinn::{Connection, VarInt};
use tokio::sync::watch;

use super::{NO_ROOM, keys};
use crate::log::target;
use crate::node::lock;
use crate::slots::{ClientSlot, ClientSlots};

/// Most connections one core peer or downlink holds at once: the one it
/// uses, and one beside it, such as its connection to a chosen relay that
/// is also its peer (spec/areas/node.md#a-chosen-relay). Another one takes
/// the place of its oldest.
pub const MAX_CONNECTIONS_PER_NODE: usize = 2;

/// The code a connection closes with when it loses its place, to a newer
/// connection of its node's or to a change of class into one without room:
/// as at shutdown, since requests may have been read on it.
const LOST_PLACE: VarInt = VarInt::from_u32(0);

/// Who opened a connection, as our peerings say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A core node we are peered with.
    Core,
    /// Our downlink.
    Downlink,
    /// Anyone else.
    Other,
}

/// The nodes peered with us, ending it or not, by kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Classes {
    /// Our core peers.
    pub core: BTreeSet<NodeId>,
    /// Our downlinks.
    pub downlinks: BTreeSet<NodeId>,
}

impl Classes {
    /// The class of `node`'s connections.
    #[must_use]
    pub fn of(&self, node: &NodeId) -> Class {
        if self.core.contains(node) {
            Class::Core
        } else if self.downlinks.contains(node) {
            Class::Downlink
        } else {
            Class::Other
        }
    }
}

/// What a handler tells the endpoint of the node's peerings.
#[derive(Clone, Debug)]
pub struct Peerings {
    /// Who is peered with the node, as it changes.
    pub classes: watch::Receiver<Arc<Classes>>,
    /// Most core peers at once.
    pub max_core_peers: usize,
    /// Most downlinks at once.
    pub max_downlinks: usize,
}

/// The places held now, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// By connections being set up.
    pub setting_up: usize,
    /// By core peers' connections.
    pub core: usize,
    /// By downlinks' connections.
    pub downlinks: usize,
    /// By anyone else's.
    pub others: usize,
}

/// The endpoint's places.
pub(super) struct Places {
    /// For connections being set up, and for tunnels from their upgrade,
    /// for the connection arriving on them ([`super::tunnel`]).
    setting_up: ClientSlots,
    /// Those being set up past which a client must show it receives at its
    /// address.
    validate_from: usize,
    /// For anyone's connections.
    others: ClientSlots,
    core: NodeSlots,
    downlinks: NodeSlots,
    /// None for a handler without peerings: every connection is anyone's.
    classes: Option<watch::Receiver<Arc<Classes>>>,
    /// The next connection's age, in the order they were made, by which a
    /// node's oldest gives up its place.
    next_age: AtomicU64,
    /// All places, of every kind.
    total: usize,
}

impl Places {
    /// Places for connections being set up and for anyone's, within
    /// `limits` each, and, with `peerings`, for core peers and downlinks.
    pub(super) fn new(limits: ConnectionLimits, peerings: Option<Peerings>) -> Self {
        let per_node = |max_nodes| {
            NodeSlots::new(NodeLimits {
                max_nodes,
                max_per_node: MAX_CONNECTIONS_PER_NODE,
            })
        };
        let (core, downlinks) = peerings.as_ref().map_or((0, 0), |peerings| {
            (peerings.max_core_peers, peerings.max_downlinks)
        });
        let total = core
            .saturating_add(downlinks)
            .saturating_mul(MAX_CONNECTIONS_PER_NODE)
            .saturating_add(limits.max_total.saturating_mul(2));
        Self {
            setting_up: ClientSlots::new(limits),
            validate_from: limits.max_total / 2,
            others: ClientSlots::new(limits),
            core: per_node(core),
            downlinks: per_node(downlinks),
            classes: peerings.map(|peerings| peerings.classes),
            next_age: AtomicU64::new(0),
            total,
        }
    }

    /// All places, of every kind: those for connections being set up, and
    /// those of every class.
    pub(super) const fn total(&self) -> usize {
        self.total
    }

    /// A place for a connection from `client` while it is set up.
    ///
    /// # Errors
    ///
    /// Too many are being set up, in all or from `client`.
    pub(super) fn set_up(&self, client: Option<ClientKey>) -> Result<ClientSlot, Refused> {
        self.setting_up.take(client)
    }

    /// The places held now.
    pub(super) fn counts(&self) -> Counts {
        Counts {
            setting_up: self.setting_up.open_count(),
            core: self.core.open_count(),
            downlinks: self.downlinks.open_count(),
            others: self.others.open_count(),
        }
    }

    /// Whether places for connections being set up run short, so that a
    /// client must show it receives at its address before it takes one:
    /// spoofed handshakes cannot.
    pub(super) fn short(&self) -> bool {
        self.setting_up.open_count() >= self.validate_from
    }

    /// The place in its class for `connection`, set up from `client`; the
    /// caller gives back the one it held while it was set up only then, so
    /// that it holds one throughout. None, and the connection closed with
    /// [`NO_ROOM`], when its class has no room: no stream on it has been
    /// read, so nothing sent on it was delivered.
    pub(super) fn settle(
        &self,
        connection: &Connection,
        client: Option<ClientKey>,
    ) -> Option<Held> {
        let node = keys::peer_node(connection);
        let age = self.next_age.fetch_add(1, Ordering::Relaxed);
        let mut classes = self.classes.clone();
        let class = class_of(node, classes.as_mut());
        let place = self.take(class, (node, age), client, connection);
        let place = refused(place, connection, class, NO_ROOM)?;
        Some(Held {
            place,
            node,
            age,
            client,
            classes,
        })
    }

    /// Keeps `held`'s connection in the place of its class while it is
    /// open: when its node's class changes, it moves, or closes if the new
    /// class has no room. Never returns; the place goes when the caller
    /// drops this, once the connection is done.
    pub(super) async fn follow(&self, connection: &Connection, held: Held) {
        let Held {
            mut place,
            node,
            age,
            client,
            mut classes,
        } = held;
        if let (Some(_), Some(following)) = (node, classes.as_mut()) {
            while following.changed().await.is_ok() {
                let class = class_of(node, Some(following));
                if class == place.class() {
                    continue;
                }
                let moved = self.take(class, (node, age), client, connection);
                match refused(moved, connection, class, LOST_PLACE) {
                    Some(moved) => place = moved,
                    None => break,
                }
            }
        }
        std::future::pending::<()>().await;
        drop(place);
    }

    fn take(
        &self,
        class: Class,
        (node, age): (Option<NodeId>, u64),
        client: Option<ClientKey>,
        connection: &Connection,
    ) -> Result<Place, Refused> {
        let by_node = match class {
            Class::Core => &self.core,
            Class::Downlink => &self.downlinks,
            Class::Other => return self.others.take(client).map(Place::other),
        };
        // A class but anyone's needs a node, which only a proven key gives.
        let node = node.ok_or(Refused::Full)?;
        by_node
            .take(node, age, connection)
            .map(|slot| Place::node(class, slot))
    }
}

/// The class of `node`'s connections, marking the classes seen.
fn class_of(node: Option<NodeId>, classes: Option<&mut watch::Receiver<Arc<Classes>>>) -> Class {
    match (node, classes) {
        (Some(node), Some(classes)) => classes.borrow_and_update().of(&node),
        _ => Class::Other,
    }
}

/// The place taken, or none, and `connection` closed with `code`.
fn refused(
    place: Result<Place, Refused>,
    connection: &Connection,
    class: Class,
    code: VarInt,
) -> Option<Place> {
    match place {
        Ok(place) => Some(place),
        Err(refused) => {
            let client = connection.remote_address();
            tracing::debug!(target: target::PEER, %client, ?class, ?refused, "no place for a QUIC connection; closing");
            connection.close(code, b"no place");
            None
        }
    }
}

/// A connection's place in its class, and what it needs to follow it.
pub(super) struct Held {
    place: Place,
    node: Option<NodeId>,
    age: u64,
    client: Option<ClientKey>,
    classes: Option<watch::Receiver<Arc<Classes>>>,
}

/// A connection's place in its class, given back when it drops: a core
/// peer's or a downlink's among its node's, anyone else's among its client
/// address's.
struct Place {
    class: Class,
    _node: Option<NodeSlot>,
    _client: Option<ClientSlot>,
}

impl Place {
    const fn node(class: Class, slot: NodeSlot) -> Self {
        Self {
            class,
            _node: Some(slot),
            _client: None,
        }
    }

    const fn other(slot: ClientSlot) -> Self {
        Self {
            class: Class::Other,
            _node: None,
            _client: Some(slot),
        }
    }

    const fn class(&self) -> Class {
        self.class
    }
}

/// Places for connections per node, and the connection in each, by its
/// age, so that one replaced by a newer of its node's closes.
#[derive(Clone)]
struct NodeSlots(Arc<Mutex<ByNode>>);

struct ByNode {
    table: NodeTable<NodeId>,
    /// Each place's connection, by age: as many as the table has places.
    connections: BTreeMap<u64, Connection>,
}

impl NodeSlots {
    fn new(limits: NodeLimits) -> Self {
        Self(Arc::new(Mutex::new(ByNode {
            table: NodeTable::new(limits),
            connections: BTreeMap::new(),
        })))
    }

    fn open_count(&self) -> usize {
        lock(&self.0).table.open_count()
    }

    /// Takes a place for `connection` from `node`, made `age`-th; the
    /// node's oldest connection closes if it held its share.
    fn take(&self, node: NodeId, age: u64, connection: &Connection) -> Result<NodeSlot, Refused> {
        let replaced = {
            let mut by_node = lock(&self.0);
            let replaced = by_node.table.take(node, age)?;
            by_node.connections.insert(age, connection.clone());
            let replaced = replaced.and_then(|oldest| by_node.connections.remove(&oldest));
            assert!(by_node.connections.len() == by_node.table.open_count());
            replaced
        };
        if let Some(replaced) = replaced {
            tracing::debug!(target: target::PEER, %node, "a newer QUIC connection takes the place of the oldest of its node's");
            replaced.close(LOST_PLACE, b"replaced");
        }
        Ok(NodeSlot {
            slots: self.clone(),
            node,
            age,
        })
    }
}

/// One connection's place among its node's, given back when it drops.
struct NodeSlot {
    slots: NodeSlots,
    node: NodeId,
    age: u64,
}

impl Drop for NodeSlot {
    fn drop(&mut self) {
        let mut by_node = lock(&self.slots.0);
        // Gone already if a newer connection took it over.
        if by_node.table.release(self.node, self.age) {
            by_node.connections.remove(&self.age);
        }
        assert!(by_node.connections.len() == by_node.table.open_count());
    }
}

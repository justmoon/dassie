//! Which transport carries our requests to each peer, logged at info level
//! when it is first known and when it changes, never per request
//! (spec/areas/node.md#logging). A node's journal so says whether its peers
//! talk to it over QUIC, or fell back to HTTPS.

use std::collections::BTreeMap;

use dassie_identity::NodeId;

use crate::log::target;
use crate::quic::Transport;

/// The transport each peer last answered us over.
#[derive(Debug, Default)]
pub(super) struct PeerTransports {
    /// Only peers: an entry goes when its peering does, so the table holds
    /// at most as many entries as the node has peers.
    last: BTreeMap<NodeId, Transport>,
}

impl PeerTransports {
    /// Notes that `peer` answered a request of ours over `over`, and logs
    /// it if that is new for this peering. `is_peer` says which nodes are
    /// still peers, so that entries of peerings that ended are dropped
    /// before a new one is added.
    pub(super) fn answered(
        &mut self,
        peer: NodeId,
        over: Transport,
        is_peer: impl Fn(&NodeId) -> bool,
    ) {
        if !is_peer(&peer) {
            return;
        }
        match self.last.get(&peer) {
            Some(&was) if was == over => {}
            Some(&was) => {
                self.last.insert(peer, over);
                tracing::info!(target: target::PEER, %peer, %over, %was, "talking to the peer over another transport");
            }
            None => {
                self.last.retain(|node, _| is_peer(node));
                self.last.insert(peer, over);
                tracing::info!(target: target::PEER, %peer, %over, "talking to the peer");
            }
        }
    }

    /// Forgets `peer`, whose peering started or ended: the next answer
    /// logs its transport again.
    pub(super) fn forget(&mut self, peer: &NodeId) {
        self.last.remove(peer);
    }
}

#[cfg(test)]
mod tests {
    use dassie_identity::NodeKey;

    use super::*;
    use crate::log::{LogLevel, capture};

    fn node(byte: u8) -> NodeId {
        NodeKey::from_bytes(&[byte; 32]).node_id()
    }

    /// The lines `steps` log, as (message, fields).
    fn logged(steps: impl FnOnce()) -> Vec<(String, Vec<(String, String)>)> {
        capture(steps)
            .into_iter()
            .map(|line| {
                assert_eq!(
                    (line.level, line.target.as_str()),
                    (LogLevel::Info, target::PEER)
                );
                (line.message, line.fields)
            })
            .collect()
    }

    fn fields(pairs: &[(&str, String)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    // One line when a peer first answers, one when its transport changes,
    // and nothing for answers in between.
    #[test]
    fn a_peers_transport_is_logged_once_and_on_change() {
        let (a, b) = (node(1), node(2));
        let mut transports = PeerTransports::default();
        let lines = logged(|| {
            transports.answered(a, Transport::Quic, |_| true);
            transports.answered(a, Transport::Quic, |_| true);
            transports.answered(b, Transport::Https, |_| true);
            transports.answered(a, Transport::Https, |_| true);
            transports.answered(a, Transport::Https, |_| true);
        });
        assert_eq!(
            lines,
            [
                (
                    "talking to the peer".to_owned(),
                    fields(&[("peer", a.to_string()), ("over", "quic".into())])
                ),
                (
                    "talking to the peer".to_owned(),
                    fields(&[("peer", b.to_string()), ("over", "https".into())])
                ),
                (
                    "talking to the peer over another transport".to_owned(),
                    fields(&[
                        ("peer", a.to_string()),
                        ("over", "https".into()),
                        ("was", "quic".into())
                    ])
                ),
            ]
        );
    }

    // Answers from nodes that are not peers (bootstrap nodes, nodes whose
    // link states we fetch) are not logged at info level, and a peering
    // made again logs its transport again.
    #[test]
    fn only_peers_are_logged_and_a_new_peering_logs_again() {
        let (a, b) = (node(1), node(2));
        let mut transports = PeerTransports::default();
        let lines = logged(|| {
            transports.answered(a, Transport::Quic, |node| *node != a);
            transports.answered(b, Transport::Quic, |_| true);
            transports.forget(&b);
            transports.answered(b, Transport::Quic, |_| true);
        });
        assert_eq!(lines.len(), 2);
        let peer = ("peer".to_owned(), b.to_string());
        assert!(
            lines
                .iter()
                .all(|(_, fields)| fields.first() == Some(&peer))
        );
    }

    // Entries of peerings that ended go before a new one is added, so the
    // table never holds more than the node's peers.
    #[test]
    fn ended_peerings_leave_the_table() {
        let (a, b) = (node(1), node(2));
        let mut transports = PeerTransports::default();
        transports.answered(a, Transport::Quic, |_| true);
        transports.answered(b, Transport::Quic, |node| *node == b);
        assert_eq!(transports.last.keys().copied().collect::<Vec<_>>(), [b]);
    }
}

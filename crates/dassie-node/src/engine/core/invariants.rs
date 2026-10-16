//! What holds between steps, checked before each commit (D16): every
//! state machine the core owns checks its own, and the core checks how
//! they fit together. The parts whose fields a module keeps to itself are
//! checked in that module.

use super::Core;
use super::tokens::{MAX_TOKEN_ROWS, MAX_TOKENS};

impl Core {
    pub(super) fn check_invariants(&self) {
        self.ledger.check_invariants();
        self.connector.check_invariants(&self.ledger);
        self.peer.check_invariants();
        for active in self.schemes.values() {
            active.scheme.check_invariants();
        }
        self.check_receivers();
        self.check_peer_endpoints();
        self.check_tokens();
        self.check_links();
        self.check_incoming();
        self.check_outgoing();
        self.check_probes();
        self.owed.check();
    }

    /// A peer's connector endpoint, both ways round.
    fn check_peer_endpoints(&self) {
        assert_eq!(self.peer_endpoints.len(), self.endpoint_peers.len());
        for (node, endpoint) in &self.peer_endpoints {
            assert_eq!(
                self.endpoint_peers.get(endpoint),
                Some(node),
                "a peer's endpoint names the peer"
            );
            assert!(*endpoint < self.next_peer_endpoint);
        }
    }

    /// The tokens: the hash index holds the live ones, and the connector
    /// holds their caps.
    fn check_tokens(&self) {
        assert!(self.tokens.len() <= MAX_TOKEN_ROWS);
        assert!(self.by_hash.len() <= MAX_TOKENS);
        let live = self
            .tokens
            .values()
            .filter(|t| t.row.revoked_at_ms.is_none());
        assert_eq!(
            live.count(),
            self.by_hash.len(),
            "every live token is found by its hash"
        );
        for (hash, id) in &self.by_hash {
            let token = self.tokens.get(id);
            assert!(
                token.is_some_and(|t| t.row.token_hash == *hash && t.row.revoked_at_ms.is_none()),
                "a hash finds its live token"
            );
        }
        for token in self.tokens.values() {
            let cap = self.connector.cap(token.account, self.now_ms);
            let expected = token.row.cap.filter(|_| token.row.revoked_at_ms.is_none());
            assert_eq!(
                cap.map(|view| view.cap),
                expected,
                "a token's cap is the connector's"
            );
        }
    }

    /// The clients' links: each belongs to its token, and a revoked
    /// token's links are being closed.
    fn check_links(&self) {
        let mut linked = 0;
        for (&id, token) in &self.tokens {
            for link in &token.connections {
                assert!(
                    self.links.get(link).is_some_and(|l| l.token == id),
                    "a token's link names the token"
                );
                assert!(
                    token.row.revoked_at_ms.is_none() || self.closing.contains(link),
                    "a revoked token's links are closing"
                );
            }
            linked += token.connections.len();
        }
        assert_eq!(linked, self.links.len(), "every link is its token's");
        assert!(self.links.keys().all(|link| *link < self.next_link));
    }
}

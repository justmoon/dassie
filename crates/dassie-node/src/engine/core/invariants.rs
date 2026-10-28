//! What holds between steps, checked before each commit
//! (spec/decisions/tigerstyle.md): every state machine the core owns checks
//! its own, and the core checks how they fit together. The parts whose fields
//! a module keeps to itself are checked in that module.

use dassie_ledger::{AccountName, Name, Reference};

use super::Core;
use super::settlement::Settling;
use super::tokens::{MAX_TOKEN_ROWS, MAX_TOKENS};

impl Core {
    pub(super) fn check_invariants(&self) {
        self.ledger.check_invariants();
        self.connector.check_invariants(&self.ledger);
        self.peer.check_invariants();
        let realm = super::ledger_realm(self.realm);
        assert!(
            realm == dassie_ledger::Realm::Test || self.live.allowed(),
            "a node runs in the live realm only as it may"
        );
        for active in self.schemes.values() {
            active.scheme.check_invariants();
            assert_eq!(
                active.scheme.ledger().realm(),
                realm,
                "a scheme runs only in its node's realm"
            );
            assert!(
                active
                    .asset
                    .is_none_or(|asset| self.assets.contains_key(&asset)),
                "a scheme's asset is active"
            );
        }
        for (id, active) in &self.assets {
            active.asset.check_invariants();
            assert_eq!(active.asset.id(), *id, "an asset under its own ID");
            assert!(
                self.schemes
                    .values()
                    .any(|scheme| scheme.asset == Some(*id)),
                "an asset is active only while a scheme settles through it"
            );
        }
        self.check_receivers();
        self.check_peer_endpoints();
        self.check_tokens();
        self.check_links();
        self.check_incoming();
        self.check_outgoing();
        self.check_probes();
        self.check_settling();
        self.check_downlinks();
        self.events.check_invariants();
    }

    /// Our settlements in progress: each is its pending transfer, and what
    /// waits to execute is one of them. A notice that waits is of one on a
    /// scheme whose messages book nothing, at most one for each method of
    /// a channel.
    fn check_settling(&self) {
        for (&(scheme, id), settling) in &self.settle.settling {
            self.check_settling_transfer(scheme, id, settling);
        }
        let known = |key| self.settle.settling.contains_key(key);
        assert!(
            self.settle.to_execute.iter().all(known),
            "executes what is settling"
        );
        let config = self.peer.config();
        let methods = dassie_peer::MAX_METHODS;
        assert!(
            self.settle.notices.len() <= (config.max_channels + config.max_downlinks) * methods,
            "a notice waits for each method of a channel"
        );
        assert!(
            self.settle.notices.iter().all(|notice| {
                Name::new(notice.scheme.as_str()).is_ok_and(|scheme| {
                    self.schemes
                        .get(&scheme)
                        .is_none_or(|active| !active.scheme.books_messages())
                })
            }),
            "only a bare notice waits"
        );
        // Kept per method: requests for a method of a peering at the last
        // check, and backoffs while the channel with the peer is open.
        assert!(
            self.settle.asked.len() <= (config.max_core_peers() + config.max_downlinks) * methods,
            "a request to settle per method of a peering"
        );
        assert!(
            self.settle.backoff.len() <= (config.max_channels + config.max_downlinks) * methods,
            "a backoff per method of an open channel"
        );
    }

    /// A settlement's transfer is pending, as it was booked: posting or
    /// voiding any other would move the wrong money.
    pub(super) fn check_settling_transfer(&self, scheme: Name, id: Name, settling: &Settling) {
        let transfer = self.ledger.pending(settling.transfer);
        assert!(
            transfer.is_some_and(|t| t.reference == Reference::Settlement { scheme, id }
                && t.amount == settling.amount),
            "a settlement's transfer is the one booked for it"
        );
        let debit = transfer.and_then(|t| self.ledger.account(t.debit));
        let peer = Name::new(settling.peer.as_str()).ok();
        assert!(
            debit.is_some_and(
                |account| Some(account.path().name) == peer.map(AccountName::AssetsInterledger)
            ),
            "a settlement debits the peer's account"
        );
    }

    /// A peer's connector endpoint, both ways round, with a slot per
    /// method of its peering whose scheme is active, each on the
    /// peer's account on that method's ledger, over a hub as its scheme is
    /// (spec/decisions/settlement-methods.md): only a downlink nothing
    /// moved with yet has no account
    /// (spec/decisions/lazy-downlink-accounts.md). A slot of a method
    /// dropped stays only while packets are in flight on it. Compared by
    /// ledger, not in order: a slot follows its method's place in the
    /// peering only as far as the two were added at the same time, and a
    /// scheme or an account that comes later adds its slot later.
    fn check_peer_endpoints(&self) {
        assert_eq!(self.peer_endpoints.len(), self.endpoint_peers.len());
        for (node, endpoint) in &self.peer_endpoints {
            assert_eq!(
                self.endpoint_peers.get(endpoint),
                Some(node),
                "a peer's endpoint names the peer"
            );
            assert!(*endpoint < self.next_peer_endpoint);
            let id = dassie_connector::EndpointId::Peer(*endpoint);
            let mut slots: Vec<_> = self
                .connector
                .slots(id)
                .filter(|slot| !slot.removed)
                .map(|slot| (slot.ledger, slot.account, slot.hub))
                .collect();
            let mut wanted: Vec<_> = self
                .wanted_slots(node)
                .into_iter()
                .map(|slot| (slot.ledger, slot.account, slot.hub))
                .collect();
            slots.sort_unstable_by_key(|(ledger, _, _)| *ledger);
            wanted.sort_unstable_by_key(|(ledger, _, _)| *ledger);
            if self.peer.peering_of(node).is_some() {
                assert_eq!(slots, wanted, "a peer's slots are its methods");
            }
            assert!(
                slots
                    .iter()
                    .all(|(_, account, _)| account.is_some() || self.peer.is_downlink(node)),
                "only a downlink's accounts open later"
            );
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

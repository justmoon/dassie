//! The front door's view of who holds each name (spec/areas/http.md#front-doors,
//! spec/areas/network.md#holders), so that it serves `<name>.<domain>` for
//! the name's holder as it serves a node's label.
//!
//! A front door is no node, so it reads the registrars' name lists itself:
//! the configured bootstrap nodes', as every node does, on a node's cadence.
//! It polls each list's hash, downloads the list when the hash changed, and
//! takes a list only in an answer authenticated under the registrar's
//! configured key. It asks as an authenticated client with its own key, and
//! only reads, so no registrar keeps replay state for it. The holder of a
//! name is whom a quorum of the configured registrars' fresh lists name,
//! and the view freezes while fewer than a quorum are fresh, exactly as a
//! node's: both are [`Holders`]'.
//!
//! It keeps nothing on disk: after a start the view is empty, and frozen,
//! until a quorum of the lists is read, so no name resolves before then.
//!
//! It knows no realm, and needs none: names are off in the live realm, so
//! a live network's registrars refuse its list requests, as any node with
//! names off does, and its view stays empty (spec/areas/network.md#names).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dassie_identity::{NodeId, NodeKey};
use dassie_oer::{Decode, Encode};
use dassie_peer::auth::{AuthConfig, Authenticator, Pending};
use dassie_peer::names::{HolderChange, Holders, HoldersConfig, Name};
use dassie_peer::wire::{
    Authentication, Content, NameListResponse, ResponseEnvelope, decode_hash, encode_envelope,
};
use dassie_peer::{Request, RequestId};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::clock::{Clock, SharedRng};
use crate::env::BootstrapNode;
use crate::log::target;
use crate::node::lock;
use crate::outbound::Resolver;
use crate::quic::{Quic, send_peer};

/// Where a front door reads names from, and how often.
#[derive(Clone, Debug)]
pub struct Registrars {
    /// The registrars: the bootstrap nodes the network's nodes are
    /// configured with (`DASSIE_BOOTSTRAP_NODES`), each with its key.
    pub nodes: Vec<BootstrapNode>,
    /// The timers the network's nodes run, production's or dev's
    /// (`DASSIE_DEV_TIMINGS`): how often a list is polled, how long it
    /// stays fresh, and how long a poll and a download may take.
    pub config: dassie_peer::Config,
}

/// Who holds each name, as the front door's view of the lists has it. The
/// HTTPS side reads it for each request; the poller writes it.
pub struct Names {
    view: Mutex<Holders>,
}

impl Names {
    /// An empty view of the lists of `registrars`, frozen until a quorum of
    /// them is read.
    #[must_use]
    pub fn new(registrars: &Registrars) -> Self {
        let config = HoldersConfig {
            list_fresh_ms: registrars.config.list_fresh_ms,
        };
        let ids = registrars.nodes.iter().map(|node| node.id);
        Self {
            view: Mutex::new(Holders::new(config, ids, Vec::new())),
        }
    }

    /// The node that holds `name` in the view, frozen or not.
    #[must_use]
    pub fn holder(&self, name: &Name) -> Option<NodeId> {
        lock(&self.view).holder(name).map(|held| held.holder)
    }

    /// Whether the view is frozen: fewer than a quorum of the lists were
    /// fresh when it was last taken.
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        lock(&self.view).is_frozen()
    }

    /// Takes the holders again at `now_ms`, logging what changed.
    fn recompute(&self, now_ms: u64) {
        let mut changes = Vec::new();
        let mut view = lock(&self.view);
        let was_frozen = view.is_frozen();
        view.recompute(now_ms, &mut changes);
        // A node persists what changed; the front door keeps nothing, and
        // lets it go, so that it does not pile up.
        let (mut changed, mut removed) = (Vec::new(), Vec::new());
        view.take_changes(&mut changed, &mut removed);
        view.check_invariants();
        let (frozen, held) = (view.is_frozen(), view.held().count());
        drop(view);
        if frozen != was_frozen {
            if frozen {
                tracing::warn!(target: target::FRONT_DOOR, "too few of the registrars' name lists are fresh: names keep the holders they had");
            } else {
                tracing::info!(target: target::FRONT_DOOR, held, "a quorum of the registrars' name lists is fresh");
            }
        }
        for change in changes {
            match change {
                HolderChange::Held { name, holder } => {
                    tracing::debug!(target: target::FRONT_DOOR, %name, %holder, "name held");
                }
                HolderChange::Unheld { name, holder } => {
                    tracing::debug!(target: target::FRONT_DOOR, %name, %holder, "name no longer held");
                }
            }
        }
    }
}

/// Reads the registrars' name lists into [`Names`], on the network's
/// cadence.
pub struct Poller {
    names: Arc<Names>,
    registrars: Registrars,
    quic: Arc<Quic>,
    resolver: Resolver,
    clock: Arc<dyn Clock>,
    asking: Mutex<Asking>,
}

/// What authenticating the front door's requests needs: its key's state
/// toward each registrar, and randomness for its sessions.
struct Asking {
    auth: Authenticator,
    rng: Box<dyn SharedRng>,
    next_id: u64,
}

impl Poller {
    /// A poller that reads `registrars`' lists into `names`, asking over
    /// `quic` with `key`, the front door's own, resolving their names with
    /// `resolver`.
    #[must_use]
    pub fn new(
        names: Arc<Names>,
        registrars: Registrars,
        (quic, key): (Arc<Quic>, Arc<NodeKey>),
        resolver: Resolver,
        clock: Arc<dyn Clock>,
        rng: Box<dyn SharedRng>,
    ) -> Self {
        let auth = Authenticator::new(key, AuthConfig::DEFAULT, clock.now_ms());
        Self {
            names,
            registrars,
            quic,
            resolver,
            clock,
            asking: Mutex::new(Asking {
                auth,
                rng,
                next_id: 0,
            }),
        }
    }

    /// Polls every list once per interval until `shutdown`, and takes the
    /// holders again whenever time passes, which alone can freeze the view.
    pub async fn run(self, shutdown: CancellationToken) {
        if self.registrars.nodes.is_empty() {
            tracing::warn!(target: target::FRONT_DOOR, "no bootstrap nodes: no names are served (set DASSIE_BOOTSTRAP_NODES)");
            return;
        }
        let interval_ms = self.registrars.config.list_poll_interval_ms;
        let mut wake = self.clock.subscribe();
        let mut next_poll_ms = self.clock.now_ms();
        loop {
            let now_ms = self.clock.now_ms();
            if now_ms >= next_poll_ms {
                next_poll_ms = now_ms.saturating_add(interval_ms);
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = self.poll() => {}
                }
            }
            self.names.recompute(self.clock.now_ms());
            let delay_ms = next_poll_ms.saturating_sub(self.clock.now_ms()).max(1);
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                () = clock_moved(&mut wake) => {}
            }
        }
    }

    /// Polls every registrar's list at once.
    async fn poll(&self) {
        let polls = self.registrars.nodes.iter().map(|node| self.poll_one(node));
        futures_util::future::join_all(polls).await;
    }

    /// Polls `registrar`'s hash, and downloads its list if the hash is new.
    async fn poll_one(&self, registrar: &BootstrapNode) {
        let config = &self.registrars.config;
        let hash = self
            .ask(
                registrar,
                &Content::NameListHashRequest,
                config.poll_timeout_ms,
            )
            .await;
        let Some(hash) = hash.and_then(|body| decode_hash(&body).ok()) else {
            return;
        };
        if lock(&self.names.view).hash(&registrar.id) == Some(hash) {
            lock(&self.names.view).confirm(&registrar.id, self.clock.now_ms());
            return;
        }
        let Some(body) = self
            .ask(
                registrar,
                &Content::NameListRequest,
                config.request_timeout_ms,
            )
            .await
        else {
            return;
        };
        let Ok(list) = NameListResponse::decode_exact(&body) else {
            tracing::debug!(target: target::FRONT_DOOR, registrar = %registrar.id, "a name list that does not decode");
            return;
        };
        // The hash of what was received, as a node takes it.
        let hash = Sha256::digest(&body).into();
        let now_ms = self.clock.now_ms();
        lock(&self.names.view).take_list(&registrar.id, list, hash, now_ms);
    }

    /// Sends `content` to `registrar`, authenticated with the front door's
    /// key, and returns the answer's body if it is a 200 the registrar's
    /// configured key authenticated.
    async fn ask(
        &self,
        registrar: &BootstrapNode,
        content: &Content<'_>,
        timeout_ms: u64,
    ) -> Option<Vec<u8>> {
        let (request, pending) = self.request(registrar, content, timeout_ms)?;
        let quic = Some(Arc::clone(&self.quic));
        let response = send_peer(quic, &self.resolver, request).await.ok()?;
        if response.status != 200 {
            tracing::debug!(target: target::FRONT_DOOR, registrar = %registrar.id, status = response.status, "a registrar refused a name list request");
            return None;
        }
        let envelope = ResponseEnvelope::decode(&response.body).ok()?;
        pending.check(&envelope).ok()?;
        Some(envelope.body.to_vec())
    }

    /// The request for `content` to `registrar`, and what checks its
    /// answer.
    fn request(
        &self,
        registrar: &BootstrapNode,
        content: &Content<'_>,
        timeout_ms: u64,
    ) -> Option<(Request, Pending)> {
        let bytes = content.to_vec();
        let now_ms = self.clock.now_ms();
        let mut asking = lock(&self.asking);
        let Asking { auth, rng, next_id } = &mut *asking;
        let (hmac, pending) = auth
            .authenticate(
                now_ms,
                rng.as_mut(),
                &registrar.id,
                &registrar.public_key,
                &bytes,
            )
            .ok()?;
        let own = *auth.own();
        let id = RequestId(*next_id);
        *next_id = next_id.wrapping_add(1);
        drop(asking);
        let request = Request {
            id,
            to: registrar.id,
            url: registrar.url.clone(),
            kind: content.kind(),
            body: encode_envelope(&own, &Authentication::Hmac(hmac), &bytes),
            timeout_ms,
        };
        Some((request, pending))
    }
}

/// Waits for a hand-moved clock to move, or forever for one that is not.
async fn clock_moved(wake: &mut Option<watch::Receiver<u64>>) {
    match wake {
        Some(receiver) => {
            if receiver.changed().await.is_err() {
                *wake = None;
            }
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeMap;

    use dassie_peer::wire::NameHolding;

    use super::*;

    fn registrar(seed: u8) -> BootstrapNode {
        let key = NodeKey::from_bytes(&[seed; 32]);
        BootstrapNode {
            id: key.node_id(),
            url: format!("https://r{seed}.example"),
            public_key: key.public_key(),
        }
    }

    /// `registrar`'s list, naming `holder` for `alice`, read at `now_ms`.
    fn read(names: &Names, registrar: &BootstrapNode, holder: NodeId, now_ms: u64) {
        let alice = Name::parse("alice").unwrap();
        let holding = NameHolding {
            holder,
            expires_at_ms: now_ms + 1_000_000,
        };
        let list = NameListResponse(BTreeMap::from([(alice, holding)]));
        let hash = Sha256::digest(list.to_vec()).into();
        assert!(lock(&names.view).take_list(&registrar.id, list, hash, now_ms));
    }

    // The front door takes holders as a node does: from a quorum of the
    // configured registrars' fresh lists, frozen with fewer.
    #[test]
    fn a_name_has_a_holder_once_a_quorum_of_the_configured_lists_name_it() {
        let config = dassie_peer::Config::DEV;
        let registrars = Registrars {
            nodes: (1..=4).map(registrar).collect(),
            config,
        };
        let names = Names::new(&registrars);
        let alice = Name::parse("alice").unwrap();
        let holder = NodeKey::from_bytes(&[9; 32]).node_id();
        // Until a quorum is read, the view is frozen, and empty.
        assert!(names.is_frozen());
        let now_ms = 1_000_000;
        for node in &registrars.nodes[..2] {
            read(&names, node, holder, now_ms);
        }
        names.recompute(now_ms);
        assert!(names.is_frozen());
        assert_eq!(names.holder(&alice), None);
        // Three of four is the quorum.
        read(&names, &registrars.nodes[2], holder, now_ms);
        names.recompute(now_ms);
        assert!(!names.is_frozen());
        assert_eq!(names.holder(&alice), Some(holder));
        // Once the lists are stale, the view freezes and keeps the holder.
        names.recompute(now_ms + config.list_fresh_ms + 1);
        assert!(names.is_frozen());
        assert_eq!(names.holder(&alice), Some(holder));
    }
}

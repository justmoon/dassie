//! Peerings over a settlement hub in the packet path
//! (spec/designs/settlement-hubs.md#4-peering-over-a-hub): the payer signs
//! a commitment with each Prepare and a claim in its reply to each
//! Fulfill; the payee checks the commitment before it forwards, keeps its
//! fulfillments, and takes the payer's claims; and a packet to a peer over
//! a hub that timed out waits for the hub before it is rejected upstream.
//!
//! The scheme does the work ([`Commitments`]); the core only carries what
//! passes between the scheme, the peer protocol and the connector.

use dassie_connector::{EndpointId, HubWait};
use dassie_identity::NodeId;
use dassie_ilp::{Address, ErrorCode, Prepare, Response};
use dassie_ledger::Name;
use dassie_oer::Decode;
use dassie_peer::wire::HubMembership;
use dassie_settlement::{Commitments, Decided, Promised};

use super::Core;

/// What a commitment of ours or a peer's promises: the Prepare's amount,
/// condition and expiry.
fn promised(packet: &[u8]) -> Option<Promised> {
    let prepare = Prepare::decode_exact(packet).ok()?;
    Some(Promised {
        amount: prepare.amount,
        condition: prepare.execution_condition,
        expires_at_ms: prepare.expires_at.unix_ms,
    })
}

impl Core {
    /// The scheme and the peer's name, if the peering with `node` is over a
    /// hub: its scheme signs promises per packet.
    pub(super) fn hub_peering(&self, node: &NodeId) -> Option<(Name, Name)> {
        let scheme = self
            .peer
            .active_peers()
            .find(|(id, _)| *id == node)
            .map(|(_, peering)| peering.scheme)?;
        let scheme = Name::new(scheme.as_str()).ok()?;
        let active = self.schemes.get(&scheme)?;
        let hub =
            active.failed.is_none() && active.scheme.id().as_str() == dassie_hub::member::SCHEME_ID;
        hub.then(|| Some((scheme, Name::new(node.as_str()).ok()?)))
            .flatten()
    }

    /// The settlement hub we are a member of, as our link state names it
    /// (spec/designs/settlement-hubs.md#11-choosing-a-hub): once our hub
    /// account has its number, and while its scheme runs.
    pub(super) fn hub_membership(&self) -> Option<HubMembership> {
        let scheme = Name::new(dassie_hub::member::SCHEME_ID).ok()?;
        let running = self
            .schemes
            .get(&scheme)
            .is_some_and(|active| active.failed.is_none());
        if !running {
            return None;
        }
        let active = self.assets.get(&dassie_hub::member::FIRST)?;
        let any: &dyn core::any::Any = active.asset.as_ref();
        let account = any.downcast_ref::<dassie_hub::member::HubAccount>()?;
        Some(HubMembership {
            hub_id: *account.hub().0.as_bytes(),
            account: account.number()?.0,
            takes_peers: true,
        })
    }

    /// Names in our link state the hub we are a member of now, if that
    /// changed: a new link state follows.
    pub(super) fn advertise_hub(&mut self) {
        let hub = self.hub_membership();
        if self.peer.own_info().hub == hub {
            return;
        }
        let mut info = self.peer.own_info().clone();
        info.hub = hub;
        self.peer.set_own_info(self.now_ms, info);
    }

    /// Runs `f` on the commitments side of `scheme`, if it has one.
    fn with_commitments<R>(
        &mut self,
        scheme: Name,
        f: impl FnOnce(&mut dyn Commitments) -> R,
    ) -> Option<R> {
        let active = self.schemes.get_mut(&scheme)?;
        if active.failed.is_some() {
            return None;
        }
        active.scheme.commitments().map(f)
    }

    /// As the payer: signs the commitment for our Prepare to a peer over a
    /// hub and sends both. `None` if the peer is not over a hub; `Some(false)`
    /// if the scheme refused the packet, which nothing has seen yet.
    pub(super) fn send_over_hub(
        &mut self,
        now: u64,
        node: &NodeId,
        request_id: u32,
        packet: &[u8],
        timeout_ms: u64,
    ) -> Option<bool> {
        let (scheme, peer) = self.hub_peering(node)?;
        let Some(promise) = promised(packet) else {
            return Some(false);
        };
        let mut commitment = Vec::new();
        let committed = self
            .with_commitments(scheme, |c| {
                c.commit(now, peer, request_id, promise, &mut commitment)
            })
            .is_some_and(|result| result.is_ok());
        if !committed || commitment.is_empty() {
            return Some(false);
        }
        let sent =
            self.peer
                .send_hub_packet(now, node, request_id, packet, &commitment, timeout_ms);
        Some(sent.is_ok())
    }

    /// As the payee: tells the scheme what became of a peer's Prepare we
    /// answered: a fulfillment it keeps until a claim covers it, or a
    /// release.
    pub(super) fn answered_over_hub(
        &mut self,
        now: u64,
        node: &NodeId,
        request_id: u32,
        packet: &[u8],
    ) {
        let Some((scheme, peer)) = self.hub_peering(node) else {
            return;
        };
        match Response::decode_exact(packet) {
            Ok(Response::Fulfill(fulfill)) => {
                let fulfillment = fulfill.fulfillment;
                self.with_commitments(scheme, |c| {
                    c.on_fulfilled(now, peer, request_id, fulfillment)
                });
            }
            Ok(Response::Reject(_)) => {
                self.with_commitments(scheme, |c| c.on_released(peer, request_id));
            }
            Err(_) => {}
        }
    }

    /// As the payee: checks a peer's commitment for its Prepare before we
    /// forward it. A packet the commitment does not cover is rejected at
    /// once, as one beyond a peer's credit is (T04).
    pub(super) fn check_hub_prepare(
        &mut self,
        now: u64,
        node: &NodeId,
        request_id: u32,
        packet: &[u8],
        commitment: Option<&[u8]>,
    ) -> bool {
        let Some((scheme, peer)) = self.hub_peering(node) else {
            return true;
        };
        let (Some(promise), Some(commitment)) = (promised(packet), commitment) else {
            return false;
        };
        let accepted = self
            .with_commitments(scheme, |c| {
                c.check_commitment(now, peer, request_id, promise, commitment)
            })
            .is_some_and(|result| result.is_ok());
        if !accepted {
            self.reject_hub_prepare(now, node, request_id);
        }
        accepted
    }

    fn reject_hub_prepare(&mut self, now: u64, node: &NodeId, request_id: u32) {
        let reject = dassie_ilp::Reject {
            code: ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
            triggered_by: Address::parse(self.connector.own_address()).ok(),
            message: "the hold does not cover the packet",
            data: &[],
        };
        let length = dassie_oer::Encode::encode(&reject, &mut self.scratch);
        let packet = self.scratch.get(..length).unwrap_or_default().to_vec();
        self.peer.send_response(now, node, request_id, &packet);
    }

    /// As the payer: the claim that goes after the verdict in our reply to
    /// a peer's Fulfill of our Prepare `request_id`; `taken` says whether
    /// our books took the Fulfill.
    pub(super) fn hub_claim(
        &mut self,
        now: u64,
        node: &NodeId,
        request_id: u32,
        taken: bool,
    ) -> Vec<u8> {
        let mut claim = Vec::new();
        if let Some((scheme, peer)) = self.hub_peering(node) {
            self.with_commitments(scheme, |c| {
                c.claim(now, peer, request_id, taken, &mut claim)
            });
        }
        claim
    }

    /// As the payer: a peer rejected our Prepare `request_id`: what the
    /// scheme kept for it is forgotten.
    pub(super) fn hub_rejected(&mut self, node: &NodeId, request_id: u32) {
        if let Some((scheme, peer)) = self.hub_peering(node) {
            self.with_commitments(scheme, |c| c.on_answered(peer, request_id));
        }
    }

    /// As the payee: the claim a peer sent after the verdict in its reply
    /// to our Fulfill.
    pub(super) fn hub_claim_received(
        &mut self,
        now: u64,
        node: &NodeId,
        request_id: u32,
        claim: &[u8],
    ) {
        if let Some((scheme, peer)) = self.hub_peering(node) {
            self.with_commitments(scheme, |c| c.on_claim(now, peer, request_id, claim));
        }
    }

    /// As the payer: hands the scheme each packet that timed out and waits
    /// for the hub, and settles each the hub has decided: fulfilled
    /// upstream with the fulfillment a claim revealed, or rejected.
    pub(super) fn hub_waits(&mut self, now: u64) {
        while let Some(wait) = self.connector.poll_hub_wait() {
            self.hub_wait(now, wait);
        }
        let hubs: Vec<Name> = self
            .schemes
            .iter()
            .filter(|(_, active)| active.scheme.id().as_str() == dassie_hub::member::SCHEME_ID)
            .map(|(name, _)| *name)
            .collect();
        let mut decided: Vec<Decided> = Vec::new();
        for scheme in hubs {
            self.with_commitments(scheme, |c| c.take_decided(&mut decided));
        }
        for decision in decided {
            self.settle_decided(now, decision);
        }
    }

    fn hub_wait(&mut self, now: u64, wait: HubWait) {
        let EndpointId::Peer(endpoint) = wait.destination else {
            return;
        };
        let Some(node) = self.endpoint_peers.get(&endpoint).copied() else {
            return;
        };
        if let Some((scheme, peer)) = self.hub_peering(&node) {
            self.with_commitments(scheme, |c| {
                c.on_hub_wait(now, peer, wait.request_id, wait.condition, wait.backstop_ms);
            });
        }
    }

    /// Ends the peering with a payee that claimed on the hub a packet it
    /// had rejected, which no honest payee does
    /// (spec/areas/hubs.md#the-members-side): collecting what it owes
    /// first, as any ending of a peering does. The owner decides on a ban.
    pub(super) fn end_for_cheating(&mut self, peer: Name) {
        let Ok(node) = NodeId::parse(peer.as_str()) else {
            return;
        };
        let reason = dassie_peer::wire::TerminationReason::Unspecified;
        if self.peer.terminate(self.now_ms, &node, reason) {
            self.after_peer();
        }
    }

    fn settle_decided(&mut self, now: u64, decision: Decided) {
        let Ok(node) = NodeId::parse(decision.peer.as_str()) else {
            return;
        };
        let Some(&endpoint) = self.peer_endpoints.get(&node) else {
            return;
        };
        let responded = self.connector.settle_hub(
            now,
            EndpointId::Peer(endpoint),
            decision.request_id,
            decision.fulfillment,
            &mut self.ledger,
            &mut self.up,
        );
        self.responded(now, responded, None);
    }
}

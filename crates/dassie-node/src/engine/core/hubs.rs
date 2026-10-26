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
use dassie_ledger::{AccountName, AccountPath, MAX_TOTAL};
use dassie_oer::Decode;
use dassie_peer::wire::HubMembership;
use dassie_settlement::{Commitments, Decided, HoldsWith, Promised, policy};

use super::Core;
use super::events::SettlementEvent;

/// What became of a Prepare to a peer over a hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HubSent {
    /// It went, with its commitment.
    Sent,
    /// The scheme would not commit to it: refused for liquidity.
    Refused,
    /// It could not be sent, and the scheme forgot it.
    Unreachable,
}

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

/// A peering over a hub as a simulation checks it
/// (spec/areas/testing.md#the-simulations): what the scheme holds of the
/// packets between us and the peer, beside what the connector holds pending
/// with it, and the credit we extend to it, beside the holds it follows.
#[cfg(feature = "testing")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubPeer {
    /// The peer.
    pub peer: Name,
    /// What the scheme holds of the packets between us, if it counts them.
    pub held: dassie_settlement::Held,
    /// The connector's packets to the peer, in flight or waiting for the
    /// hub.
    pub pending_to: usize,
    /// The connector's packets from the peer that it has not answered.
    pub pending_from: usize,
    /// Whether we are peered with it, ending the peering or not: only
    /// then does the credit follow the holds.
    pub peered: bool,
    /// The holds between us, as the scheme last read them.
    pub holds: HoldsWith,
    /// The credit we extend to it, while its accounts are open.
    pub credit: Option<u64>,
    /// Our settlements to it still pending: what funds our hold once they
    /// execute.
    pub settling: u64,
}

/// The credit we extend to a peer over a hub: our hold toward it, our money
/// set aside for it, and our settlements to it still on their way, which
/// fund that hold once they execute (spec/areas/hubs.md#in-the-node). Its
/// hold toward us is not counted: it is booked already, as its settlements
/// to us, and counting it here too would let it owe us twice what it holds.
#[must_use]
pub(super) fn hub_credit(holds: HoldsWith, settling: u64) -> u64 {
    holds.toward.saturating_add(settling).min(MAX_TOTAL)
}

impl Core {
    /// The scheme and the peer's name, if the peering with `node` is over a
    /// hub: its scheme signs promises per packet. A peering being ended
    /// counts too, since its packets still resolve
    /// (spec/decisions/ending-a-peering.md), and so does one whose scheme
    /// stopped: its packets are then refused, never let through unchecked.
    pub(super) fn hub_peering(&self, node: &NodeId) -> Option<(Name, Name)> {
        let scheme = self.peer.peering_of(node)?.scheme;
        let scheme = Name::new(scheme.as_str()).ok()?;
        let over_hub = self.schemes.get(&scheme)?.over_hub;
        over_hub
            .then(|| Some((scheme, Name::new(node.as_str()).ok()?)))
            .flatten()
    }

    /// The settlement hub we are a member of, as our link state names it
    /// (spec/designs/settlement-hubs.md#11-choosing-a-hub): that of the
    /// first scheme over a hub that runs and has an account in it.
    pub(super) fn hub_membership(&mut self) -> Option<HubMembership> {
        let hubs: Vec<Name> = self
            .schemes
            .iter()
            .filter(|(_, active)| active.over_hub)
            .map(|(name, _)| *name)
            .collect();
        hubs.into_iter().find_map(|scheme| {
            let membership = self.with_commitments(scheme, |c| c.membership())??;
            Some(HubMembership {
                hub_id: membership.hub,
                account: membership.account,
                takes_peers: true,
            })
        })
    }

    /// Each peering over a hub, and each peer a scheme over a hub holds
    /// packets of, in the order of the schemes and then the peers, as a
    /// simulation checks them. Built only with the `testing` feature.
    #[cfg(feature = "testing")]
    pub fn hub_peers(&mut self) -> Vec<HubPeer> {
        let hubs: Vec<Name> = self
            .schemes
            .iter()
            .filter(|(_, active)| active.over_hub)
            .map(|(name, _)| *name)
            .collect();
        let mut out = Vec::new();
        for scheme in hubs {
            let mut held = Vec::new();
            self.with_commitments(scheme, |c| c.held(&mut held));
            let mut peers: std::collections::BTreeSet<Name> =
                held.iter().map(|(peer, _)| *peer).collect();
            for (node, peering) in self.peer.peers() {
                if peering.scheme.as_str() == scheme.as_str()
                    && let Ok(name) = Name::new(node.as_str())
                {
                    peers.insert(name);
                }
            }
            for peer in peers {
                let Ok(node) = NodeId::parse(peer.as_str()) else {
                    continue;
                };
                let endpoint = self
                    .peer_endpoints
                    .get(&node)
                    .map(|id| EndpointId::Peer(*id));
                let (pending_to, pending_from) = endpoint.map_or((0, 0), |id| {
                    let from = self.connector.pending_from(id);
                    (self.connector.pending_for(id).saturating_sub(from), from)
                });
                let holds = self
                    .with_commitments(scheme, |c| c.holds(peer))
                    .unwrap_or_default();
                let books = self.hub_books(scheme, node);
                out.push(HubPeer {
                    peer,
                    held: held
                        .iter()
                        .find(|(name, _)| *name == peer)
                        .map(|(_, held)| *held)
                        .unwrap_or_default(),
                    pending_to,
                    pending_from,
                    peered: self.peer.peering_of(&node).is_some(),
                    holds,
                    credit: books.map(|(credit, _, _)| credit),
                    settling: books.map_or(0, |(_, _, settling)| settling),
                });
            }
        }
        out
    }

    /// The credit we extend to `node` on `scheme`'s ledger, our position
    /// with it, and our settlements to it still pending, while its accounts
    /// are open.
    pub(super) fn hub_books(&self, scheme: Name, node: NodeId) -> Option<(u64, u128, u64)> {
        let ledger = self.schemes.get(&scheme)?.scheme.ledger();
        let name = Name::new(node.as_str()).ok()?;
        let find = |account| self.ledger.find(&AccountPath::new(ledger, account));
        let interledger = find(AccountName::AssetsInterledger(name))?;
        let trust = find(AccountName::ContraTrust(name))?;
        let settling = self
            .ledger
            .pending_transfers()
            .filter(|transfer| transfer.debit == interledger)
            .filter(|transfer| super::settlement::settles_to(&self.ledger, transfer, &node))
            .fold(0u64, |sum, transfer| sum.saturating_add(transfer.amount));
        Some((
            policy::trust(self.ledger.account(trust)?.balance()),
            policy::position(self.ledger.account(interledger)?.balance()),
            settling,
        ))
    }

    /// Keeps the credit we extend to each peer over a hub at what the holds
    /// between us call for ([`hub_credit`]), with trust transfers, whenever
    /// they changed: a hold funded, taken from, reduced, released or
    /// closed, or a settlement of ours on its way, executed or cancelled.
    /// It runs before every commit, so the books keep the credit in step
    /// with the holds the scheme's state keeps.
    pub(super) fn sync_hub_credit(&mut self) {
        if !self.schemes.values().any(|active| active.over_hub) {
            return;
        }
        let peerings: Vec<(Name, NodeId)> = self
            .peer
            .peers()
            .filter_map(|(node, peering)| {
                let scheme = Name::new(peering.scheme.as_str()).ok()?;
                let active = self.schemes.get(&scheme)?;
                (active.over_hub && active.failed.is_none()).then_some((scheme, *node))
            })
            .collect();
        for (scheme, node) in peerings {
            self.sync_credit_with(scheme, node);
        }
    }

    /// Keeps the credit we extend to `node` over the hub of `scheme` at what
    /// the holds between us call for. It is lowered only as far as what the
    /// peer may still owe us allows, as the ledger's limit on its account
    /// counts it: short of that, the peer owes us more than our hold toward
    /// it and our settlements on their way leave room for, and the books
    /// keep the higher credit, say so when they lower it part of the way,
    /// and lower it further at the next commit that leaves room. The holds
    /// and the books move together for every scheme that reports what it
    /// took when the books took it, as the stub does; the hub's scheme
    /// learns of a take from the hub, later.
    // Panics only on a broken invariant: the amounts were checked against
    // the accounts just now, and the journal was drained before the write.
    #[allow(clippy::panic)]
    pub(super) fn sync_credit_with(&mut self, scheme: Name, node: NodeId) {
        let Ok(name) = Name::new(node.as_str()) else {
            return;
        };
        let Some(holds) = self.with_commitments(scheme, |c| c.holds(name)) else {
            return;
        };
        let Some((credit, position, settling)) = self.hub_books(scheme, node) else {
            return;
        };
        let Some(ledger) = self.schemes.get(&scheme).map(|a| a.scheme.ledger()) else {
            return;
        };
        let target = hub_credit(holds, settling);
        let now = self.now_ms;
        if target == credit {
            return;
        }
        // However many peerings move in one step, each write finds the
        // journal empty: what came before is in the batch.
        self.take_journal();
        let changed = if target > credit {
            super::growth::raise_trust(&mut self.ledger, now, ledger, name, target - credit)
                .map(|()| target)
        } else {
            let room = u64::try_from(position).unwrap_or(u64::MAX);
            let lower = (credit - target).min(room);
            if lower == 0 {
                return;
            }
            super::growth::lower_trust(&mut self.ledger, now, ledger, name, lower)
                .map(|()| credit - lower)
        };
        match changed {
            Ok(credit) => self.report(SettlementEvent::HubCredit {
                peer: name,
                credit,
                holds: target,
            }),
            Err(error) => panic!("the credit over a hub was refused: {error}"),
        }
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
    pub(super) fn with_commitments<R>(
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
    /// hub and sends both. `None` if the peer is not over a hub. A packet the
    /// scheme will not commit to, as when the hold has no room, is refused
    /// for liquidity; one committed to that could not be sent is forgotten
    /// by the scheme at once, as nothing reached the peer.
    pub(super) fn send_over_hub(
        &mut self,
        now: u64,
        node: &NodeId,
        request_id: u32,
        packet: &[u8],
        timeout_ms: u64,
    ) -> Option<HubSent> {
        let (scheme, peer) = self.hub_peering(node)?;
        let Some(promise) = promised(packet) else {
            return Some(HubSent::Refused);
        };
        let mut commitment = Vec::new();
        let committed = self
            .with_commitments(scheme, |c| {
                c.commit(now, peer, request_id, promise, &mut commitment)
            })
            .is_some_and(|result| result.is_ok());
        if !committed || commitment.is_empty() {
            return Some(HubSent::Refused);
        }
        let sent =
            self.peer
                .send_hub_packet(now, node, request_id, packet, &commitment, timeout_ms);
        if sent.is_err() {
            self.with_commitments(scheme, |c| c.on_answered(peer, request_id));
            return Some(HubSent::Unreachable);
        }
        Some(HubSent::Sent)
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
        // A Prepare that does not decode is the connector's to answer.
        let Some(promise) = promised(packet) else {
            return true;
        };
        let Some(commitment) = commitment else {
            self.reject_hub_prepare(now, node, request_id);
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

    /// As the payer: the claim that goes in our reply to a peer's Fulfill
    /// of our Prepare `request_id`; `taken` says whether our books took the
    /// Fulfill.
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
        // A payer that lies, in a simulation, keeps its claim to itself.
        if self.settle.sends_no_claims {
            claim.clear();
        }
        claim
    }

    /// As the payer: a peer rejected our Prepare `request_id`, or it never
    /// reached the peer: what the scheme kept for it is forgotten.
    pub(super) fn hub_rejected(&mut self, node: &NodeId, request_id: u32) {
        if let Some((scheme, peer)) = self.hub_peering(node) {
            self.with_commitments(scheme, |c| c.on_answered(peer, request_id));
        }
    }

    /// As the payee: the claim a peer sent in its reply to our Fulfill.
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
            .filter(|(_, active)| active.over_hub)
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

    /// As the payee: a Prepare whose commitment we took never reached the
    /// connector, so nothing will answer it: the scheme lets it go.
    pub(super) fn hub_prepare_dropped(&mut self, node: &NodeId, request_id: u32) {
        if let Some((scheme, peer)) = self.hub_peering(node) {
            self.with_commitments(scheme, |c| c.on_released(peer, request_id));
        }
    }

    fn settle_decided(&mut self, now: u64, decision: Decided) {
        let Ok(node) = NodeId::parse(decision.peer.as_str()) else {
            return;
        };
        let Some(&endpoint) = self.peer_endpoints.get(&node) else {
            return;
        };
        // A decision posts up to a packet's transfers: each finds the
        // journal empty, however many the hub decided in one step.
        self.take_journal();
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

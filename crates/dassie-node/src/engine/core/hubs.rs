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
use dassie_peer::MAX_OWN_HUBS;
use dassie_peer::wire::{HubListing, HubMembership, SchemeId};
use dassie_settlement::{Commitments, Decided, HoldsWith, Membership, Promised, policy};

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

/// The hubs our link state lists, from the membership of each scheme over
/// a hub, in the schemes' order: at most [`MAX_OWN_HUBS`], each hub and each
/// scheme once, the first scheme of a hub naming it. Each takes peers over
/// its hub.
fn listings(memberships: impl IntoIterator<Item = (Name, Membership)>) -> Vec<HubListing> {
    let mut listings: Vec<HubListing> = Vec::new();
    for (scheme, membership) in memberships {
        let Some(scheme) = SchemeId::parse(scheme.as_str()) else {
            continue;
        };
        let named = listings
            .iter()
            .any(|listing| listing.scheme == scheme || listing.membership.hub_id == membership.hub);
        if named {
            continue;
        }
        if listings.len() == MAX_OWN_HUBS {
            break;
        }
        listings.push(HubListing {
            scheme,
            membership: HubMembership {
                hub_id: membership.hub,
                account: membership.account,
                takes_peers: true,
            },
        });
    }
    listings
}

impl Core {
    /// The scheme and the peer's name, if the peering with `node` has a
    /// method on `scheme` and it is over a hub: its scheme signs promises
    /// per packet (spec/decisions/settlement-methods.md). A peering being
    /// ended counts too, since its packets still resolve
    /// (spec/decisions/ending-a-peering.md), and so does one whose scheme
    /// stopped: its packets are then refused, never let through unchecked.
    pub(super) fn hub_peering(&self, node: &NodeId, scheme: SchemeId) -> Option<(Name, Name)> {
        if !self.peer.peering_of(node)?.has(scheme) {
            return None;
        }
        let scheme = Name::new(scheme.as_str()).ok()?;
        let over_hub = self.schemes.get(&scheme)?.over_hub;
        over_hub
            .then(|| Some((scheme, Name::new(node.as_str()).ok()?)))
            .flatten()
    }

    /// The settlement hubs we are a member of, as our link state lists them
    /// (spec/designs/settlement-hubs.md#11-choosing-a-hub): one per scheme
    /// over a hub that runs and reports a membership
    /// (`Commitments::membership`), which a hub scheme does once its
    /// account has its number, in the order of the schemes.
    pub(super) fn hub_listings(&mut self) -> Vec<HubListing> {
        let hubs: Vec<Name> = self
            .schemes
            .iter()
            .filter(|(_, active)| active.over_hub)
            .map(|(name, _)| *name)
            .collect();
        let memberships: Vec<(Name, Membership)> = hubs
            .into_iter()
            .filter_map(|scheme| {
                let membership = self.with_commitments(scheme, |c| c.membership())??;
                Some((scheme, membership))
            })
            .collect();
        listings(memberships)
    }

    /// The first of the hubs we are a member of, which the hub extension of
    /// our link state names for nodes that read no list.
    pub(super) fn hub_membership(&mut self) -> Option<HubMembership> {
        self.hub_listings()
            .first()
            .map(|listing| listing.membership)
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
                if peering.schemes().any(|of| of.as_str() == scheme.as_str())
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
        // Each method over a hub of each peering: the credit and the holds
        // are the method's, on its own ledger.
        let peerings: Vec<(Name, NodeId)> = self
            .peer
            .peers()
            .flat_map(|(node, peering)| peering.schemes().map(move |scheme| (*node, scheme)))
            .filter_map(|(node, scheme)| {
                let scheme = Name::new(scheme.as_str()).ok()?;
                let active = self.schemes.get(&scheme)?;
                (active.over_hub && active.failed.is_none()).then_some((scheme, node))
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

    /// Lists in our link state the hubs we are a member of now, if that
    /// changed: a new link state follows. The node's hubs change with its
    /// schemes, so their funding follows them here too.
    pub(super) fn advertise_hub(&mut self) {
        let hubs = self.hub_listings();
        self.peer.set_own_hubs(self.now_ms, hubs);
        self.follow_hubs();
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

    /// As the payer: signs the commitment for our Prepare to a peer on its
    /// method on `method`, over a hub, and sends both. `None` if the method
    /// is not over a hub. A packet the scheme will not commit to, as when
    /// the hold has no room, is refused for liquidity, and never moved to
    /// another method (spec/decisions/settlement-methods.md); one committed
    /// to that could not be sent is forgotten by the scheme at once, as
    /// nothing reached the peer.
    pub(super) fn send_over_hub(
        &mut self,
        now: u64,
        (node, method): (&NodeId, SchemeId),
        request_id: u32,
        packet: &[u8],
        timeout_ms: u64,
    ) -> Option<HubSent> {
        let (scheme, peer) = self.hub_peering(node, method)?;
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
        let sent = self.peer.send_method_packet(
            now,
            node,
            method,
            request_id,
            packet,
            &commitment,
            timeout_ms,
        );
        if sent.is_err() {
            self.with_commitments(scheme, |c| c.on_answered(peer, request_id));
            return Some(HubSent::Unreachable);
        }
        Some(HubSent::Sent)
    }

    /// As the payee: tells the scheme of `method` what became of a peer's
    /// Prepare on it we answered: a fulfillment it keeps until a claim
    /// covers it, or a release.
    pub(super) fn answered_over_hub(
        &mut self,
        now: u64,
        (node, method): (&NodeId, SchemeId),
        request_id: u32,
        packet: &[u8],
    ) {
        let Some((scheme, peer)) = self.hub_peering(node, method) else {
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

    /// As the payee: checks a peer's commitment for its Prepare on its
    /// method on `method` before we forward it; one on a method that is not
    /// over a hub needs none. A packet the commitment does not cover is
    /// rejected at
    /// once, as one beyond a peer's credit is (T04). So is one that came
    /// without a commitment, which the peer protocol hands on for the node
    /// to judge: the node does not yet take Prepares on what a peer with no
    /// account in the hub is owed (spec/roadmap.md#joining-by-being-paid).
    pub(super) fn check_hub_prepare(
        &mut self,
        now: u64,
        node: &NodeId,
        method: SchemeId,
        request_id: u32,
        packet: &[u8],
        commitment: Option<&[u8]>,
    ) -> bool {
        let Some((scheme, peer)) = self.hub_peering(node, method) else {
            return true;
        };
        // A Prepare that does not decode is the connector's to answer.
        let Some(promise) = promised(packet) else {
            return true;
        };
        let refused = "the hold does not cover the packet";
        let Some(commitment) = commitment else {
            self.refuse_prepare(now, node, request_id, refused);
            return false;
        };
        let accepted = self
            .with_commitments(scheme, |c| {
                c.check_commitment(now, peer, request_id, promise, commitment)
            })
            .is_some_and(|result| result.is_ok());
        if !accepted {
            self.refuse_prepare(now, node, request_id, refused);
        }
        accepted
    }

    /// Rejects a peer's Prepare before the connector sees it, for want of
    /// room on its method (T04), saying `message`.
    pub(super) fn refuse_prepare(
        &mut self,
        now: u64,
        node: &NodeId,
        request_id: u32,
        message: &str,
    ) {
        let reject = dassie_ilp::Reject {
            code: ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
            triggered_by: Address::parse(self.connector.own_address()).ok(),
            message,
            data: &[],
        };
        let length = dassie_oer::Encode::encode(&reject, &mut self.scratch);
        let packet = self.scratch.get(..length).unwrap_or_default().to_vec();
        self.peer.send_response(now, node, request_id, &packet);
    }

    /// As the payer: the claim that goes in our reply to a peer's Fulfill
    /// of our Prepare `request_id`, which went on its method on `method`;
    /// `taken` says whether our books took the Fulfill.
    pub(super) fn hub_claim(
        &mut self,
        now: u64,
        node: &NodeId,
        method: SchemeId,
        request_id: u32,
        taken: bool,
    ) -> Vec<u8> {
        let mut claim = Vec::new();
        if let Some((scheme, peer)) = self.hub_peering(node, method) {
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

    /// As the payer: a peer rejected our Prepare `request_id` on its method
    /// on `method`, or it never reached the peer: what the scheme kept for
    /// it is forgotten.
    pub(super) fn hub_rejected(&mut self, node: &NodeId, method: SchemeId, request_id: u32) {
        if let Some((scheme, peer)) = self.hub_peering(node, method) {
            self.with_commitments(scheme, |c| c.on_answered(peer, request_id));
        }
    }

    /// As the payee: the claim a peer sent in its reply to our Fulfill of
    /// its Prepare on its method on `method`.
    pub(super) fn hub_claim_received(
        &mut self,
        now: u64,
        node: &NodeId,
        method: SchemeId,
        request_id: u32,
        claim: &[u8],
    ) {
        if let Some((scheme, peer)) = self.hub_peering(node, method) {
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
        // Our packets to it take its endpoint's method.
        let Some(method) = self.packet_method(&node) else {
            return;
        };
        if let Some((scheme, peer)) = self.hub_peering(&node, method) {
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

    /// As the payee: a Prepare on the method on `method` whose commitment
    /// we took never reached the connector, so nothing will answer it: the
    /// scheme lets it go.
    pub(super) fn hub_prepare_dropped(&mut self, node: &NodeId, method: SchemeId, request_id: u32) {
        if let Some((scheme, peer)) = self.hub_peering(node, method) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn member(scheme: &str, hub: u8, account: u32) -> (Name, Membership) {
        (
            Name::new(scheme).unwrap(),
            Membership {
                hub: [hub; 32],
                account,
            },
        )
    }

    // spec/areas/peer-protocol.md#link-states: the link state lists every
    // hub the node is a member of, each with its scheme, in the schemes'
    // order; the first is the one the hub extension names.
    #[test]
    fn every_hub_is_listed_with_its_scheme() {
        let listed = listings([
            member("hub-xrpl-testnet-dsusd", 2, 9),
            member("hub-xrpl-testnet-xrp", 1, 4),
        ]);
        let named: Vec<(&str, [u8; 32], u32)> = listed
            .iter()
            .map(|listing| {
                (
                    listing.scheme.as_str(),
                    listing.membership.hub_id,
                    listing.membership.account,
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                ("hub-xrpl-testnet-dsusd", [2; 32], 9),
                ("hub-xrpl-testnet-xrp", [1; 32], 4)
            ]
        );
        assert!(listed.iter().all(|listing| listing.membership.takes_peers));
        assert!(dassie_peer::wire::HubList(listed).is_valid());
        assert!(listings([]).is_empty());
    }

    // A hub is named once, by its first scheme, and the list is bounded.
    #[test]
    fn the_list_names_each_hub_once_and_is_bounded() {
        let listed = listings([
            member("x", 5, 0),
            member("aa", 1, 1),
            member("bb", 1, 2),
            member("cc", 3, 3),
        ]);
        let schemes: Vec<&str> = listed.iter().map(|l| l.scheme.as_str()).collect();
        assert_eq!(schemes, ["aa", "cc"], "no scheme the wire cannot name");
        let many = (0..10u8).map(|n| member(&format!("hub-{n}"), n, 0));
        let listed = listings(many);
        assert_eq!(listed.len(), MAX_OWN_HUBS);
        assert!(dassie_peer::wire::HubList(listed).is_valid());
    }
}

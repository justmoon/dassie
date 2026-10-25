//! A downlink's accounts, which open only once value first moves with it:
//! with the first packet that moves money to it, a settlement from it that
//! the books take, or, over a hub, ours to it
//! (spec/decisions/lazy-downlink-accounts.md). Until
//! then it has none, and is a peer with nothing on them. The downlinks'
//! share of the peers' accounts (spec/decisions/account-budgets.md) keeps
//! room for every downlink whose settlement channel is open and that has
//! none yet, so that its accounts open once value moves.

use std::collections::{BTreeMap, BTreeSet};

use dassie_connector::{EndpointId, OpensAccounts};
use dassie_identity::NodeId;
use dassie_ledger::{AccountId, AccountName, AccountPath, JournalEntry, Ledger, LedgerId, Name};
use dassie_settlement::Effect;

use super::Core;
use super::ledgers::ActiveScheme;
use super::peers::peer_budget;

/// The downlinks' accounts, and those the node keeps room for.
#[derive(Debug, Default)]
pub(super) struct Downlinks {
    /// The peers' accounts opened for downlinks, by ledger and node: they
    /// take the downlinks' share of the peers' accounts, for good.
    pub opened: BTreeSet<(LedgerId, Name)>,
    /// The downlinks we hold an open settlement channel with that have no
    /// accounts on any ledger yet: the share keeps room for theirs.
    pub waiting: BTreeSet<NodeId>,
}

impl Downlinks {
    /// Whether accounts for the downlink `node` fit the downlinks' share,
    /// beside those opened for downlinks and the room kept for every other
    /// downlink waiting for its own.
    pub fn fit(&self, max_accounts: u32, node: &NodeId) -> bool {
        let others = self.waiting.len() - usize::from(self.waiting.contains(node));
        2 * (self.opened.len() + others + 1) <= peer_budget(max_accounts, true)
    }

    /// Whether `node` waits for its accounts: we hold an open settlement
    /// channel with it as a downlink, and it has none on any ledger.
    pub fn waits(peer: &dassie_peer::PeerNode, ledger: &Ledger, node: &NodeId) -> bool {
        peer.channel(node)
            .is_some_and(|channel| channel.url.is_empty())
            && Name::new(node.as_str()).is_ok_and(|name| {
                LedgerId::ALL.into_iter().all(|on| {
                    ledger
                        .find(&AccountPath::new(on, AccountName::AssetsInterledger(name)))
                        .is_none()
                })
            })
    }
}

/// A downlink's accounts, as an opening found them.
#[derive(Clone, Copy, Debug)]
pub(super) struct Opened {
    /// The account its packets debit and credit.
    pub account: AccountId,
    /// Whether the opening opened them, rather than found them open.
    pub new: bool,
    /// Whether the node kept room for them.
    pub waited: bool,
}

/// Opening a downlink's accounts, and the batch that writes them.
pub(super) struct Opening<'a> {
    pub downlinks: &'a mut Downlinks,
    /// The batch's record of the accounts opened for downlinks.
    pub written: &'a mut Vec<(LedgerId, Name)>,
    /// The batch's journal: the ledger's is taken into it first, so that
    /// it has room for the opening.
    pub journal: &'a mut Vec<JournalEntry>,
}

impl Opening<'_> {
    /// Opens the downlink `node`'s accounts on `on`, extending it `start`,
    /// if it has none there and they fit.
    pub fn open(
        &mut self,
        now_ms: u64,
        ledger: &mut Ledger,
        node: NodeId,
        on: LedgerId,
        start: u64,
    ) -> Option<Opened> {
        let name = Name::new(node.as_str()).ok()?;
        let path = AccountPath::new(on, AccountName::AssetsInterledger(name));
        if let Some(account) = ledger.find(&path) {
            return Some(Opened {
                account,
                new: false,
                waited: false,
            });
        }
        if !self.downlinks.fit(ledger.config().max_accounts, &node) {
            return None;
        }
        while let Some(entry) = ledger.poll_journal() {
            self.journal.push(entry);
        }
        let (account, new) = ledger.open_peer(now_ms, on, name, start).ok()?;
        assert!(new, "a downlink's accounts open once");
        let recorded = self.downlinks.opened.insert((on, name));
        assert!(recorded, "a downlink's new accounts are recorded once");
        self.written.push((on, name));
        let waited = self.downlinks.waiting.remove(&node);
        Some(Opened {
            account,
            new: true,
            waited,
        })
    }

    /// Takes back what [`open`](Self::open) just did for `node`: the ledger
    /// refused the packet they were opened for, so they never were.
    pub fn take_back(&mut self, ledger: &mut Ledger, node: NodeId, on: LedgerId, waited: bool) {
        let Ok(name) = Name::new(node.as_str()) else {
            return;
        };
        ledger.unopen_peer(on, name);
        let forgotten = self.downlinks.opened.remove(&(on, name));
        assert!(
            forgotten && self.written.pop() == Some((on, name)),
            "what is taken back was just opened"
        );
        if waited {
            self.downlinks.waiting.insert(node);
        }
    }
}

/// The credit a downlink's new accounts start with on `scheme`: what a
/// senior extends at first, and nothing over a hub, whose hold is all the
/// credit there is (spec/decisions/earned-trust.md).
pub(super) fn start_credit(
    schemes: &BTreeMap<Name, ActiveScheme>,
    scheme: Name,
    senior_credit: u64,
) -> u64 {
    if schemes.get(&scheme).is_some_and(|active| active.over_hub) {
        0
    } else {
        senior_credit
    }
}

/// Opens a downlink's accounts for the first packet that moves money to it.
pub(super) struct PacketOpening<'a> {
    pub opening: Opening<'a>,
    pub peer: &'a dassie_peer::PeerNode,
    pub endpoint_peers: &'a BTreeMap<u64, NodeId>,
    pub schemes: &'a BTreeMap<Name, ActiveScheme>,
    pub senior_credit: u64,
    /// What the last opening opened, for whom, until the packet is booked.
    pub last: Option<(NodeId, Opened)>,
}

impl OpensAccounts for PacketOpening<'_> {
    fn open(
        &mut self,
        now_ms: u64,
        ledger: &mut Ledger,
        endpoint: EndpointId,
        on: LedgerId,
    ) -> Option<AccountId> {
        let EndpointId::Peer(id) = endpoint else {
            return None;
        };
        let node = *self.endpoint_peers.get(&id)?;
        let peering = self
            .peer
            .peering_of(&node)
            .filter(|_| self.peer.is_downlink(&node))?;
        let scheme = Name::new(peering.scheme.as_str()).ok()?;
        let start = start_credit(self.schemes, scheme, self.senior_credit);
        let opened = self.opening.open(now_ms, ledger, node, on, start)?;
        self.last = opened.new.then_some((node, opened));
        Some(opened.account)
    }

    fn take_back(&mut self, ledger: &mut Ledger, _: EndpointId, on: LedgerId) {
        if let Some((node, opened)) = self.last.take() {
            self.opening.take_back(ledger, node, on, opened.waited);
        }
    }
}

/// Where a Prepare for the connector is.
pub(super) enum PrepareIn<'p> {
    /// Here.
    Given(&'p [u8]),
    /// The first this many bytes of the core's scratch buffer.
    Scratch(usize),
}

impl Core {
    /// Hands the connector a Prepare from `source` with `request_id`; a
    /// packet that moves money to a downlink without accounts opens them
    /// (spec/decisions/lazy-downlink-accounts.md).
    pub(super) fn connect_prepare(
        &mut self,
        now_ms: u64,
        (source, request_id): (EndpointId, u32),
        packet: PrepareIn<'_>,
    ) -> dassie_connector::Outcome {
        let Self {
            connector,
            ledger,
            prices,
            out,
            scratch,
            downlinks,
            batch,
            peer,
            endpoint_peers,
            schemes,
            senior_credit,
            ..
        } = self;
        let packet = match packet {
            PrepareIn::Given(packet) => packet,
            PrepareIn::Scratch(length) => scratch.get(..length).unwrap_or_default(),
        };
        let mut opening = PacketOpening {
            opening: Opening {
                downlinks,
                written: &mut batch.downlink_accounts,
                journal: &mut batch.journal,
            },
            peer,
            endpoint_peers,
            schemes,
            senior_credit: *senior_credit,
            last: None,
        };
        connector.handle_prepare_with(
            now_ms,
            (source, request_id),
            packet,
            ledger,
            &prices.table,
            &mut opening,
            out,
        )
    }

    /// Opens the accounts of `node` on `scheme`'s ledger for value that
    /// moves with it outside a packet: its settlement that the books are
    /// about to take, or, over a hub, ours to it, which funds our hold
    /// toward it. Only for a downlink we hold an open channel with that has
    /// none there, and only if they fit. Returns whether it then has
    /// accounts there. A downlink's settlement without room for them is
    /// booked as a stranger's, the owner's deposit.
    pub(super) fn open_downlink_accounts(&mut self, node: NodeId, scheme: Name) -> bool {
        let Some(on) = self
            .schemes
            .get(&scheme)
            .map(|active| active.scheme.ledger())
        else {
            return false;
        };
        let Ok(name) = Name::new(node.as_str()) else {
            return false;
        };
        let path = AccountPath::new(on, AccountName::AssetsInterledger(name));
        if self.ledger.find(&path).is_some() {
            return true;
        }
        if !self
            .peer
            .channel(&node)
            .is_some_and(|channel| channel.url.is_empty())
        {
            return false;
        }
        let start = start_credit(&self.schemes, scheme, self.senior_credit);
        let mut opening = Opening {
            downlinks: &mut self.downlinks,
            written: &mut self.batch.downlink_accounts,
            journal: &mut self.batch.journal,
        };
        let Some(opened) = opening.open(self.now_ms, &mut self.ledger, node, on, start) else {
            return false;
        };
        // Its packets book on them from now on, if it peers on this ledger.
        let peered_here = self.peer.peering_of(&node).is_some_and(|peering| {
            Name::new(peering.scheme.as_str()).is_ok_and(|peered| {
                self.schemes
                    .get(&peered)
                    .is_some_and(|active| active.scheme.ledger() == on)
            })
        });
        if peered_here && let Some(&endpoint) = self.peer_endpoints.get(&node) {
            self.connector
                .open_account(EndpointId::Peer(endpoint), opened.account);
        }
        true
    }

    /// Opens the accounts of each downlink whose settlement `effects` book,
    /// if it has none on `scheme`'s ledger and there is room for them;
    /// without, its settlement is booked as a stranger's.
    pub(super) fn open_for_settlements(&mut self, scheme: Name, effects: &[Effect]) {
        for effect in effects {
            if let Effect::IncomingSettlement { peer, .. } = effect
                && let Ok(node) = NodeId::parse(peer.as_str())
            {
                self.open_downlink_accounts(node, scheme);
            }
        }
    }

    /// Whether the accounts of `node` may open now for its settlement: it
    /// is a downlink we hold an open channel with, and they fit.
    pub(super) fn downlink_may_open(&self, node: &NodeId) -> bool {
        self.peer
            .channel(node)
            .is_some_and(|channel| channel.url.is_empty())
            && self.downlinks.fit(self.ledger.config().max_accounts, node)
    }

    /// Whether `node` waits for its accounts now, as its channel and the
    /// ledger say: after its peering began, or its channel closed.
    pub(super) fn refresh_waiting(&mut self, node: NodeId) {
        if Downlinks::waits(&self.peer, &self.ledger, &node) {
            self.downlinks.waiting.insert(node);
        } else {
            self.downlinks.waiting.remove(&node);
        }
    }

    /// The downlinks waiting for their accounts, as the channels restored
    /// and the ledger say.
    pub(super) fn restore_waiting(&mut self) {
        let waiting: BTreeSet<NodeId> = self
            .peer
            .channels()
            .map(|(node, _)| *node)
            .filter(|node| Downlinks::waits(&self.peer, &self.ledger, node))
            .collect();
        self.downlinks.waiting = waiting;
    }

    /// The downlinks waiting for their accounts are those the channels and
    /// the ledger say (spec/decisions/lazy-downlink-accounts.md), and the
    /// accounts recorded as opened for downlinks are open.
    pub(super) fn check_downlinks(&self) {
        for (on, node) in &self.downlinks.opened {
            assert!(
                super::peers::has_accounts(&self.ledger, *on, *node),
                "a downlink's recorded accounts are open"
            );
        }
        let waiting = self
            .peer
            .channels()
            .filter(|(node, _)| Downlinks::waits(&self.peer, &self.ledger, node))
            .count();
        assert!(
            waiting == self.downlinks.waiting.len()
                && self.downlinks.waiting.iter().all(|node| Downlinks::waits(
                    &self.peer,
                    &self.ledger,
                    node
                )),
            "the downlinks waiting for accounts are those the channels say"
        );
    }
}

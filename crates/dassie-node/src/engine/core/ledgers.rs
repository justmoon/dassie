//! Settlement schemes and the ledgers they serve.

use dassie_connector::EndpointId;
use dassie_ledger::{
    AccountId, AccountName, AccountPath, LedgerId, Name, Realm, Reference, TransferSpec,
};
use dassie_settlement::{
    Activation, AssetId, Detach, Effect, LedgerStatus, LossCause, PegPause, PegView, SchemeError,
    SettlementScheme, activation,
};

use super::Core;
use super::EngineError;
use super::events::SettlementEvent;
use super::packets::{Receiver, receiver_address};
use crate::store::{Checkpoint, Store};

/// An active scheme.
pub(super) struct ActiveScheme {
    pub scheme: Box<dyn SettlementScheme>,
    /// The external asset it settles through, if any.
    pub asset: Option<AssetId>,
    /// The state version last written with a checkpoint.
    pub version: u64,
    /// Why it stopped, if the books could not take what it said: it is fed
    /// nothing more until the node starts again.
    pub failed: Option<String>,
    /// Whether its peerings are over a settlement hub: it signs and takes
    /// promises per packet ([`SettlementScheme::commitments`]).
    pub over_hub: bool,
    /// Whether its peers trust each other without limit
    /// ([`SettlementScheme::unlimited_trust`]).
    pub unlimited: bool,
    /// Why its peg guard paused it, as the owner was last told.
    pub paused: Option<PegPause>,
    /// Whether it converted when the prices were last built
    /// ([`ActiveScheme::converts_now`]).
    pub converts: bool,
    /// Since when its owner removes it, while its settlement methods are
    /// wound down: it is offered to no peer, and runs until each is
    /// dropped, the packets on their slots are resolved and what the peers
    /// may still settle on it has had its time, then goes
    /// ([`Core::finish_removals`]). Kept across a restart, which carries
    /// the removal on.
    pub removing: Option<u64>,
    /// While it is being removed, since when no method of a peering has
    /// been on it: it runs for [`Core::removal_wait_ms`] after. Only in
    /// memory: a restart counts the wait from the start, which only makes
    /// it longer.
    pub methods_gone_ms: Option<u64>,
}

impl ActiveScheme {
    /// Whether the scheme lets the connector convert to and from its ledger
    /// now ([`SettlementScheme::converts`]). A dollar token's scheme the
    /// books stopped does not: its peg guard is fed nothing more, so it can
    /// no longer tell a token off its peg (spec/decisions/issued-assets.md).
    /// A hub's ledger also needs its owner book's guard
    /// ([`Core::converts_now`]).
    pub fn converts_now(&self) -> bool {
        self.scheme.converts() && (self.failed.is_none() || self.scheme.peg().is_none())
    }

    /// Whether a method on it would settle directly: it is neither over a
    /// hub nor of unlimited trust, as every XRPL scheme, an owner book. A
    /// peering stored from when the XRPL schemes peered may still have a
    /// method on one: it carries no packet, its peer is asked for nothing
    /// on it, and the peer protocol drops it at its next review
    /// (spec/areas/settlement.md#owner-books).
    pub fn settles_directly(&self) -> bool {
        !self.over_hub && !self.unlimited
    }

    /// The credit the node extends at first to a peer on it, whichever
    /// node asked to peer and for a downlink alike: nothing over a hub,
    /// whose hold is all the credit there is;
    /// [`UNLIMITED_CREDIT`](dassie_settlement::policy::UNLIMITED_CREDIT)
    /// with unlimited trust (spec/areas/settlement.md#credit).
    ///
    /// # Panics
    ///
    /// On a scheme that would settle directly: no peering or method is
    /// made on one (spec/decisions/hubs-only.md), so nothing opens a peer's
    /// accounts on it.
    pub fn start_credit(&self) -> u64 {
        assert!(
            !self.settles_directly(),
            "no peer's accounts open on a scheme that would settle directly"
        );
        if self.over_hub {
            0
        } else {
            dassie_settlement::policy::UNLIMITED_CREDIT
        }
    }
}

/// Whether the owner book on `book` has a peg guard: a dollar token that a
/// pool prices (spec/areas/xrpl.md#the-peg-guard).
fn guarded(book: LedgerId) -> bool {
    [
        dassie_xrpl::dsusd::TOKEN,
        dassie_xrpl::rlusd::TOKEN,
        dassie_xrpl::rlusd::TESTNET_TOKEN,
    ]
    .iter()
    .any(|token| token.ledger == book && token.peg.is_some())
}

/// A ledger as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerView {
    /// The ledger.
    pub id: LedgerId,
    /// The scheme serving it.
    pub scheme: String,
    /// The owner's posted total: the pool and its sub-accounts.
    pub owner_balance: i128,
    /// What packets in flight hold of it.
    pub owner_pending: u128,
    /// The scheme's balance on the underlying ledger, as the books have
    /// it: less what the ledger holds back.
    pub on_ledger_balance: u64,
    /// What the underlying ledger holds back, never spendable.
    pub reserve: u64,
    /// Where the owner sends money to fund it, if anywhere: its asset's
    /// address, or for a hub's ledger, the hub's escrow, once the node's
    /// account there has its number.
    pub deposit_address: Option<String>,
    /// The destination tag a deposit to `deposit_address` must carry: for
    /// a hub's ledger, the number of the node's account, which names it to
    /// the hub (spec/areas/hubs.md#the-escrow). `None` where any payment
    /// to the address arrives.
    pub deposit_tag: Option<u32>,
    /// Who issues what it settles in, for a token: an address on its
    /// underlying ledger.
    pub issuer: Option<String>,
    /// Whether the node can ask a faucet for test money on its asset.
    pub faucet: bool,
    /// How the scheme stands with its ledger.
    pub status: LedgerStatus,
    /// What its asset waits for, if something keeps it from going on, such
    /// as a validator list.
    pub waiting: Option<&'static str>,
    /// What its peg guard says, for a dollar token's scheme that has one,
    /// or for a hub's ledger, its owner book's guard
    /// ([`Core::hub_peg`]).
    pub peg: Option<PegView>,
    /// What keeps the scheme from being removed now.
    pub removal_blockers: Vec<RemovalBlocker>,
    /// While its owner removes it: how many settlement methods of peerings
    /// on it are left to wind down before it goes.
    pub removing: Option<u32>,
}

/// What keeps a scheme from being removed, with how many. Peerings that
/// settle on it do not: removal winds their methods on it down
/// (spec/decisions/settlement-methods.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalBlocker {
    /// BTP tokens use its ledger.
    BtpTokens(u32),
    /// Outgoing payments from its ledger have not finished.
    OutgoingPayments(u32),
    /// A move between its hub and the owner book of the hub's asset is on
    /// its way: the move is its account's in that hub.
    HubMove,
}

impl RemovalBlocker {
    /// The reason, for people.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::BtpTokens(_) => "BTP tokens use its ledger",
            Self::OutgoingPayments(_) => "outgoing payments from its ledger have not finished",
            Self::HubMove => "a move between its hub and the owner book is on its way",
        }
    }
}

/// The accounts every active ledger has
/// (spec/areas/ledger.md#chart-of-accounts).
const COMMON_ACCOUNTS: [AccountName; 8] = [
    AccountName::AssetsSettlement,
    AccountName::EquityOwner,
    AccountName::EquitySuspense,
    AccountName::RevenueFx,
    AccountName::RevenueFees,
    AccountName::ExpensesFx,
    AccountName::ExpensesFees,
    AccountName::ExpensesLosses,
];

/// Most accounts the node opens for itself, never a peer's: one for every
/// BTP token ever made, and on every ledger its common accounts, its
/// payer's and its swap endpoint's. Peers' accounts, which stay when their
/// channels close, leave room for these (spec/decisions/closing-channels.md).
pub(super) const OWN_ACCOUNTS: usize =
    super::tokens::MAX_TOKEN_ROWS + LedgerId::ALL.len() * (COMMON_ACCOUNTS.len() + 2);

impl Core {
    /// Whether the connector converts to and from the ledger of `active`
    /// now: the scheme lets it ([`ActiveScheme::converts_now`]), and on a
    /// hub's ledger, the peg guard of its owner book does not pause it
    /// ([`Self::hub_peg`]). Peers settle only over hubs, so a hub's ledger
    /// is where a dollar token converts (spec/decisions/issued-assets.md).
    pub(super) fn converts_now(&self, active: &ActiveScheme) -> bool {
        active.converts_now() && !self.hub_halted(active.scheme.ledger())
    }

    /// Whether `ledger` is a hub's ledger whose conversions the peg guard of
    /// its owner book halts ([`Self::hub_peg`]).
    pub(super) fn hub_halted(&self, ledger: LedgerId) -> bool {
        self.hub_peg(ledger).is_some_and(|peg| peg.paused.is_some())
    }

    /// The peg guard that the hub's ledger `ledger` follows: the guard of the
    /// owner book of the hub's asset
    /// ([`owner_book_ledger`](super::hub_moves::owner_book_ledger)), whose
    /// pause halts the conversions on the hub's ledger too, though settling
    /// over the hub goes on: the hub moves the token as it is
    /// (spec/decisions/token-hubs.md). It fails closed: a book with a guard
    /// that is not active, or that the books stopped, reads nothing, so the
    /// hub's ledger is paused as by a guard without a reading. `None` for a
    /// ledger that is no hub's, or whose book has no guard, as the XRP
    /// hub's.
    pub(super) fn hub_peg(&self, ledger: LedgerId) -> Option<PegView> {
        let book = super::hub_moves::owner_book_ledger(ledger)?;
        let active = self
            .schemes
            .values()
            .find(|active| active.scheme.ledger() == book);
        let peg = active.and_then(|active| active.scheme.peg());
        match (active, peg) {
            (Some(active), Some(peg)) if active.failed.is_none() => Some(peg),
            _ => guarded(book).then(|| PegView {
                paused: Some(PegPause::Stale),
                last: peg.and_then(|peg| peg.last),
            }),
        }
    }

    /// Activates the stored schemes, booking what changed while the node was
    /// down. One whose config or state no longer makes a scheme, such as a
    /// hub's whose rules no longer decode, or whose account's state is of
    /// another hub, is left out and reported, so that
    /// the owner can add it again or remove it. One of another realm than the node's, which the node's realm
    /// was changed past, is left as it is, and does not run: the owner is
    /// told, and the node starts with the rest
    /// (spec/decisions/mainnet-schemes.md). So is a disabled one, such as
    /// test RLUSD: its books and its line stay as they are, and nothing is
    /// written off (spec/decisions/issued-assets.md), and the stub in a
    /// release build (spec/decisions/stub-in-debug-builds.md). The peerings on a
    /// scheme left out are ended, collecting nothing, by the peer
    /// protocol (spec/areas/settlement.md#schemes-and-ledgers).
    pub(super) fn load_schemes(&mut self, store: &Store) -> Result<(), String> {
        for (id, config) in store.settlement_schemes().map_err(|e| e.to_string())? {
            if super::schemes::not_run(id.as_str()).is_some() {
                self.report(SettlementEvent::Disabled { scheme: id });
                continue;
            }
            // A scheme this version does not know stops the start: the
            // database is not one this version can run, and a release that
            // cannot start is rolled back (spec/areas/release.md).
            let known = super::SCHEMES
                .iter()
                .find(|info| info.id == id.as_str())
                .map(|info| info.ledger);
            // The tests' second and third stubs, and their scheme on the XRP
            // hub's ledger, which tests restart with.
            #[cfg(test)]
            let known = known
                .or_else(|| super::tests::StubTwo::of(id.as_str()).map(|(l, _)| l))
                .or_else(|| {
                    (id.as_str() == super::tests::XRP_HUB).then_some(LedgerId::HubXrplTestnetXrp)
                });
            let Some(ledger) = known else {
                return Err(format!("settlement scheme {id}: unknown to this version"));
            };
            if ledger.realm() != super::ledger_realm(self.realm) {
                self.report(SettlementEvent::OtherRealm { scheme: id });
                continue;
            }
            let checkpoint = self.checkpoints.get(&id).cloned();
            // Made before anything of it is opened: a known scheme whose
            // config or state no longer makes one leaves nothing behind.
            let made = match self.make_scheme(id, &config, checkpoint.as_ref()) {
                Ok(made) => made,
                Err(EngineError::Invalid(reason)) => {
                    self.unusable.insert(id);
                    self.report(SettlementEvent::Unusable { scheme: id, reason });
                    continue;
                }
                Err(e) => return Err(format!("settlement scheme {id}: {e}")),
            };
            self.start_scheme(id, config, made, checkpoint, false)
                .map_err(|e| format!("settlement scheme {id}: {e}"))?;
        }
        // A removal under way carries on: the scheme runs until its
        // methods are wound down (spec/decisions/settlement-methods.md).
        for (id, since) in store.removing_schemes().map_err(|e| e.to_string())? {
            if let Some(active) = self.schemes.get_mut(&id) {
                active.removing = Some(since);
            }
        }
        Ok(())
    }

    /// Activates a settlement scheme. Its ID must be one the node knows,
    /// where the original's RPC took any text. A hub's scheme added without
    /// rules is stored with those of the default hub of its asset, so it
    /// stays in that hub whatever default a later release carries
    /// (spec/decisions/hub-named-when-added.md).
    ///
    /// # Errors
    ///
    /// The scheme is unknown, disabled, refuses its config, or is active;
    /// a hub's scheme added without rules while no default hub of its asset
    /// is built in.
    pub fn add_scheme(&mut self, id: &str, config: String) -> Result<LedgerView, EngineError> {
        if let Some(why) = super::schemes::not_run(id) {
            return Err(EngineError::Precondition(why.into()));
        }
        let id =
            Name::new(id).map_err(|_| EngineError::Invalid("unknown settlement scheme".into()))?;
        if self.schemes.contains_key(&id) {
            return Err(EngineError::Precondition("the scheme is active".into()));
        }
        let config =
            super::schemes::config_to_store(id.as_str(), config, &self.settle.default_hubs)?;
        let checkpoint = self.checkpoints.get(&id).cloned();
        let ledger = self.activate(id, config, checkpoint, true)?;
        // One left out at start is still stored: its old config goes, as
        // removals are written before additions.
        if self.unusable.remove(&id) {
            self.batch.schemes_removed.push(id);
        }
        // Settlements left pending when it was last removed.
        self.reconcile_settlements(Some(id));
        self.reconcile_owner_payments();
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Starts a scheme: makes it and the external asset it settles through,
    /// then [starts](Self::start_scheme) it.
    fn activate(
        &mut self,
        id: Name,
        config: String,
        checkpoint: Option<Checkpoint>,
        new: bool,
    ) -> Result<LedgerId, EngineError> {
        let made = self.make_scheme(id, &config, checkpoint.as_ref())?;
        self.start_scheme(id, config, made, checkpoint, new)
    }

    /// Starts a scheme made with its asset: opens its ledger's accounts,
    /// books the difference to its checkpoint, and makes sure the receiver
    /// has a ledger.
    fn start_scheme(
        &mut self,
        id: Name,
        config: String,
        (mut scheme, asset): super::schemes::MadeScheme,
        checkpoint: Option<Checkpoint>,
        new: bool,
    ) -> Result<LedgerId, EngineError> {
        let over_hub = scheme.commitments().is_some();
        let unlimited = scheme.unlimited_trust();
        assert!(
            !(over_hub && unlimited),
            "a scheme over a hub trusts no further than its holds"
        );
        // One neither over a hub nor of unlimited trust would settle
        // directly: it is an owner book, which peers with nobody and whose
        // settlement messages, as a channel from when the XRPL schemes
        // peered may still carry, book nothing (spec/decisions/hubs-only.md).
        // So no peer's accounts open on it, for a peering, a method or a
        // downlink's settlement ([`ActiveScheme::start_credit`]).
        assert!(
            over_hub || unlimited || (!scheme.peers() && !scheme.books_messages()),
            "a scheme that would settle directly is an owner book"
        );
        let converts = scheme.converts();
        let ledger = scheme.ledger();
        if let Err(error) = self.open_ledger(id, scheme.as_ref(), checkpoint.as_ref()) {
            if let Some(asset) = asset {
                self.release_asset(asset);
            }
            return Err(error);
        }
        if new {
            self.batch.schemes_added.push((id, config));
        }
        if unlimited {
            self.raise_to_unlimited(ledger);
        }
        self.schemes.insert(
            id,
            ActiveScheme {
                scheme,
                asset,
                version: 0,
                failed: None,
                over_hub,
                unlimited,
                // A guard paused before a restart is told again.
                paused: None,
                converts,
                removing: None,
                methods_gone_ms: None,
            },
        );
        if let Some(asset) = asset {
            // The scheme tells its asset where it is, before the asset reads
            // its ledger for it.
            self.tell_scheme(id, asset);
            self.carry_out(asset);
            self.connect_for_scheme(asset);
        }
        self.checkpoint(id);
        self.update_peer_schemes();
        self.add_receiver(ledger)?;
        self.add_payer(ledger)?;
        self.add_swap_endpoint(ledger)?;
        Ok(ledger)
    }

    /// Raises every peer's credit on `ledger`, a scheme's with unlimited
    /// trust, to the unlimited credit: a peering stored from when the stub
    /// without a config settled, and kept a finite credit, would otherwise
    /// run short, as a new one never does
    /// (spec/areas/settlement.md#the-stub-scheme).
    // Panics only on a broken invariant: a peer's trust account has its
    // `assets/interledger` beside it, which takes any credit up to the
    // unlimited one.
    #[allow(clippy::panic)]
    fn raise_to_unlimited(&mut self, ledger: LedgerId) {
        let unlimited = dassie_settlement::policy::UNLIMITED_CREDIT;
        let short: Vec<(Name, u64)> = self
            .ledger
            .accounts()
            .filter_map(|(_, account)| {
                let path = account.path();
                let AccountName::ContraTrust(peer) = path.name else {
                    return None;
                };
                let credit = dassie_settlement::policy::trust(account.balance());
                (path.ledger == ledger && credit < unlimited).then_some((peer, unlimited - credit))
            })
            .collect();
        let now = self.now_ms();
        for (peer, amount) in short {
            // Each write finds the journal empty: what came before is in
            // the batch.
            self.take_journal();
            if let Err(error) =
                super::trust::raise_trust(&mut self.ledger, now, ledger, peer, amount)
            {
                panic!("a peer's credit could not be raised to the unlimited credit: {error}");
            }
        }
        self.take_journal();
    }

    /// Opens the accounts of `scheme`'s ledger and books the difference
    /// between its balance and its checkpoint: only a ledger of the
    /// node's realm, and one no other scheme serves. The realm gate: a node
    /// runs only its realm's ledgers, so test and live nodes never share a
    /// scheme, and a ledger of real money runs only where the live realm
    /// may, which it may not but by the dev switch
    /// (spec/decisions/mainnet-schemes.md).
    fn open_ledger(
        &mut self,
        id: Name,
        scheme: &dyn SettlementScheme,
        checkpoint: Option<&Checkpoint>,
    ) -> Result<(), EngineError> {
        let ledger = scheme.ledger();
        if ledger.realm() != super::ledger_realm(self.realm) {
            return Err(EngineError::Precondition(
                "the scheme is of another realm than the node's".into(),
            ));
        }
        if ledger.realm() == Realm::Live && !self.live.allowed() {
            return Err(EngineError::Precondition(
                "real money is not supported yet (spec/decisions/real-money.md)".into(),
            ));
        }
        if self
            .schemes
            .values()
            .any(|active| active.scheme.ledger() == ledger)
        {
            return Err(EngineError::Precondition(
                "another scheme serves that ledger".into(),
            ));
        }
        for name in COMMON_ACCOUNTS {
            self.ledger
                .open(AccountPath::new(ledger, name))
                .map_err(|e| EngineError::Internal(e.to_string()))?;
        }
        let settlement = self.account(ledger, AccountName::AssetsSettlement)?;
        let owner = self.account(ledger, AccountName::EquityOwner)?;
        let suspense = self.account(ledger, AccountName::EquitySuspense)?;
        let booking = match activation(checkpoint.map(|c| c.balance), scheme.balance()) {
            Activation::InStep => None,
            Activation::FirstDeposit(amount) => {
                Some((settlement, owner, amount, Reference::Deposit))
            }
            Activation::Gain(amount) => {
                Some((settlement, suspense, amount, Reference::Reconciliation))
            }
            Activation::Loss(amount) => {
                Some((suspense, settlement, amount, Reference::Reconciliation))
            }
        };
        if let Some((debit, credit, amount, reference)) = booking {
            self.post(debit, credit, amount, reference)?;
            self.report(SettlementEvent::BalanceBooked {
                scheme: id,
                reference,
                amount,
            });
        }
        Ok(())
    }

    /// Deactivates a settlement scheme, and only it: the original's removal
    /// cut its list after the first scheme. A scheme that peerings settle
    /// on is not refused for them: it is offered to no peer from then on,
    /// each of their methods on it is wound down, collected once and
    /// dropped within a bounded time, and the scheme runs until then, and
    /// until the packets on their slots are resolved and what the peers
    /// may still have settled on it has had its time, and then goes
    /// ([`finish_removals`](Self::finish_removals),
    /// spec/decisions/settlement-methods.md). A peering left with no other
    /// method ends. Removing a scheme already being removed succeeds, and
    /// changes nothing: the removal is under way.
    ///
    /// # Errors
    ///
    /// The scheme is neither active nor stored and left out at start, or,
    /// active, tokens use its ledger, outgoing payments from it have not
    /// finished, or a move of its hub is on its way.
    pub fn remove_scheme(&mut self, id: &str) -> Result<(), EngineError> {
        let id =
            Name::new(id).map_err(|_| EngineError::NotFound("the scheme is not active".into()))?;
        let ledger = match self.schemes.get(&id) {
            Some(active) => active.scheme.ledger(),
            // Stored but left out at start: it opened nothing, so only its
            // stored config goes. A move of its hub that the database shows
            // on its way stays with its row, and goes on if the owner adds
            // the scheme again; the owner, who cannot give it up while the
            // scheme does not run, is never kept from removing it.
            None if self.unusable.contains(&id) => {
                self.unusable.remove(&id);
                self.batch.schemes_removed.push(id);
                return Ok(());
            }
            None => return Err(EngineError::NotFound("the scheme is not active".into())),
        };
        if self
            .schemes
            .get(&id)
            .is_some_and(|active| active.removing.is_some())
        {
            return Ok(());
        }
        if let Some(blocker) = self.removal_blockers(id, ledger).first() {
            return Err(EngineError::Precondition(blocker.message().into()));
        }
        // Nothing of a peering's on it: it goes at once. A method dropped
        // before, whose packets are still in flight on its slot, keeps it as
        // a method does.
        if self.methods_on(id) > 0 || self.packets_on(ledger) {
            let now = self.now_ms();
            if let Some(active) = self.schemes.get_mut(&id)
                && active.removing.is_none()
            {
                active.removing = Some(now);
                self.batch.schemes_removing.push((id, now));
                self.changes.ledger = true;
                self.update_peer_schemes();
            }
            self.wind_down_methods(id);
            return Ok(());
        }
        self.drop_scheme(id, ledger);
        Ok(())
    }

    /// How many settlement methods of our peerings, being ended or not, are
    /// on scheme `id`.
    fn methods_on(&self, id: Name) -> usize {
        self.peer
            .peers()
            .filter(|(_, peering)| peering.schemes().any(|of| of.as_str() == id.as_str()))
            .count()
    }

    /// Winds down each method of an active peering on scheme `id` that is
    /// not being wound down already: at most one per peer.
    fn wind_down_methods(&mut self, id: Name) {
        let Some(scheme) = dassie_peer::wire::SchemeId::parse(id.as_str()) else {
            return;
        };
        let peers: Vec<dassie_identity::NodeId> = self
            .peer
            .active_peers()
            .filter(|(_, peering)| {
                peering.depeering.is_none()
                    && peering
                        .method(scheme)
                        .is_some_and(|method| !method.winding_down())
            })
            .map(|(node, _)| *node)
            .collect();
        if peers.is_empty() {
            return;
        }
        let now = self.now_ms;
        for node in &peers {
            self.peer.wind_down(now, node, scheme);
        }
        self.after_peer();
    }

    /// Whether a packet is in flight on a peer's slot on `ledger`, either
    /// way, its method dropped or not.
    fn packets_on(&self, ledger: LedgerId) -> bool {
        self.peer_endpoints.values().any(|id| {
            self.connector
                .slots(EndpointId::Peer(*id))
                .any(|slot| slot.ledger == ledger && slot.pending > 0)
        })
    }

    /// How long a scheme being removed still runs once no method of a
    /// peering is on it (spec/decisions/settlement-methods.md), from the
    /// rules that bound what can still come on it. A packet in flight on a
    /// dropped method's slot then resolves within the longest a packet is
    /// held, and its claim over a hub, in the reply to its Fulfill or on
    /// the hub before its commitment expires, comes with it. A settlement
    /// the peer had on its way when it heard of the drop reaches the
    /// ledger and is announced within the wait for one, and its message,
    /// if it did not get through, goes again within the longest wait
    /// between tries.
    pub(super) fn removal_wait_ms(&self) -> u64 {
        let peer = self.peer.config();
        self.connector
            .config()
            .max_hold_ms
            .saturating_add(peer.depeer_wait_ms)
            .saturating_add(peer.settlement_retry_max_ms)
    }

    /// When the next scheme being removed may go, once nothing else keeps
    /// it.
    pub(super) fn removals_deadline(&self) -> Option<u64> {
        let wait = self.removal_wait_ms();
        self.schemes
            .values()
            .filter(|active| active.removing.is_some())
            .filter_map(|active| active.methods_gone_ms)
            .map(|since| since.saturating_add(wait))
            .min()
    }

    /// Carries on the owner's removals of schemes: winds down each method
    /// on a scheme being removed that is not yet, as after a restart, and
    /// removes a scheme no peering has a method on any more once the
    /// packets on their slots are resolved, the wait for what peers may
    /// still settle on it is over ([`removal_wait_ms`](Self::removal_wait_ms)),
    /// and nothing else keeps it (spec/decisions/settlement-methods.md).
    /// Until then it runs: it books what peers settle on it, and hears what
    /// becomes of each packet of a dropped method, with its claims, either
    /// way. Looked at after the peer node's timers.
    pub(super) fn finish_removals(&mut self) {
        let removing: Vec<(Name, LedgerId)> = self
            .schemes
            .iter()
            .filter(|(_, active)| active.removing.is_some())
            .map(|(id, active)| (*id, active.scheme.ledger()))
            .collect();
        let (now, wait) = (self.now_ms, self.removal_wait_ms());
        for (id, ledger) in removing {
            if self.methods_on(id) > 0 {
                self.wind_down_methods(id);
                continue;
            }
            let Some(since) = self
                .schemes
                .get_mut(&id)
                .map(|active| *active.methods_gone_ms.get_or_insert(now))
            else {
                continue;
            };
            if now < since.saturating_add(wait)
                || self.packets_on(ledger)
                || !self.removal_blockers(id, ledger).is_empty()
            {
                continue;
            }
            self.drop_scheme(id, ledger);
            self.changes.ledger = true;
            self.after_peer();
        }
    }

    /// Takes scheme `id`, serving `ledger`, out of the node: its books
    /// stay, and come back when it is added again.
    fn drop_scheme(&mut self, id: Name, ledger: LedgerId) {
        if let Some(asset) = self.schemes.remove(&id).and_then(|active| active.asset) {
            if let Some(active) = self.assets.get_mut(&asset) {
                active.asset.detach(id, Detach::Removed);
            }
            self.release_asset(asset);
        }
        self.update_peer_schemes();
        self.batch.schemes_added.retain(|(added, _)| *added != id);
        // Its removal is written after the additions: one left in the batch
        // would mark the scheme removing if it is added again in it.
        self.batch
            .schemes_removing
            .retain(|(removing, _)| *removing != id);
        self.batch.schemes_removed.push(id);
        let now = self.now_ms();
        if let Some(receiver) = self.receivers.remove(&ledger) {
            self.connector.remove_endpoint(now, receiver.endpoint);
        }
        if let Some(payer) = self.spending.payers.remove(&ledger) {
            self.connector.remove_endpoint(now, payer.endpoint);
        }
        if let Some(swaps) = self.connector.swap_endpoint(ledger) {
            self.connector.remove_endpoint(now, swaps);
        }
        self.expire(now);
    }

    /// Adds test money through the stub.
    ///
    /// # Errors
    ///
    /// The stub is not active, or stopped, or the books cannot take it.
    pub fn stub_deposit(&mut self, amount: u64) -> Result<LedgerView, EngineError> {
        let stub = dassie_ledger::name!("stub");
        let inactive = || EngineError::Precondition("the stub scheme is not active".into());
        let ledger = self
            .schemes
            .get(&stub)
            .map(|active| active.scheme.ledger())
            .ok_or_else(inactive)?;
        if self.schemes.get(&stub).is_some_and(|a| a.failed.is_some()) {
            return Err(EngineError::Precondition(
                "the stub scheme has stopped; restart the node".into(),
            ));
        }
        // The books must take it before the stub's balance moves: they
        // could not follow it otherwise.
        let deposit = TransferSpec {
            debit: self.account(ledger, AccountName::AssetsSettlement)?,
            credit: self.account(ledger, AccountName::EquityOwner)?,
            amount,
            pending: false,
            reference: Reference::Deposit,
        };
        self.ledger
            .validate(&deposit)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        let active = self.schemes.get_mut(&stub).ok_or_else(inactive)?;
        let mut effects = Vec::new();
        active
            .scheme
            .deposit(amount, &mut effects)
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        self.apply_effects(stub, ledger, &effects)?;
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Asks the faucet of a scheme's test ledger for money. What it sends is
    /// booked as a deposit once the ledger shows it.
    ///
    /// # Errors
    ///
    /// The scheme is not active, or stopped, has no faucet, or asked it
    /// too recently.
    pub fn ask_faucet(&mut self, id: &str) -> Result<LedgerView, EngineError> {
        let inactive = || EngineError::NotFound("the scheme is not active".into());
        let id = Name::new(id).map_err(|_| inactive())?;
        let active = self.schemes.get(&id).ok_or_else(inactive)?;
        if active.failed.is_some() {
            return Err(EngineError::Precondition(
                "the scheme has stopped; restart the node".into(),
            ));
        }
        let ledger = active.scheme.ledger();
        self.asset_faucet(id).map_err(|error| match error {
            SchemeError::Unsupported => {
                EngineError::Precondition("the ledger has no faucet".into())
            }
            SchemeError::Unavailable => EngineError::Exhausted(
                "the faucet was asked less than a minute ago, or the ledger cannot be reached"
                    .into(),
            ),
            other => EngineError::Internal(other.to_string()),
        })?;
        self.ledger_view(ledger)
            .ok_or_else(|| EngineError::Internal("the ledger vanished".into()))
    }

    /// Books what a scheme reports, and checkpoints its balance in the
    /// same batch. All or none: the books check every transfer the
    /// effects make, in order, before they book any, and if they refuse
    /// one, none of the effects is carried out. A refusal is a bug, which
    /// stops the scheme, so its checkpoint stays with what the books hold
    /// (spec/areas/settlement.md#the-scheme-interface).
    pub(super) fn apply_effects(
        &mut self,
        scheme: Name,
        ledger: LedgerId,
        effects: &[Effect],
    ) -> Result<(), EngineError> {
        self.open_for_settlements(scheme, effects);
        let bookings = match self.check_effects(scheme, ledger, effects) {
            Ok(bookings) => bookings,
            Err(error) => {
                self.stop_scheme(scheme, error.clone());
                return Err(error);
            }
        };
        assert_eq!(bookings.len(), effects.len(), "one booking per effect");
        for (effect, booking) in effects.iter().zip(bookings) {
            match effect {
                Effect::Deposit { .. } | Effect::Withdrawal { .. } | Effect::Fee { .. } => {
                    self.book(booking);
                }
                Effect::Reconcile {
                    amount,
                    gain,
                    cause,
                } => {
                    self.report(SettlementEvent::Reconciled {
                        scheme,
                        amount: *amount,
                        gain: *gain,
                        cause: *cause,
                    });
                    self.book(booking);
                }
                Effect::IncomingSettlement { peer, amount, id } => {
                    assert!(booking.is_some(), "a settlement books a transfer");
                    if let Some(spec) = booking {
                        self.settlement_in(scheme, *peer, *amount, *id, spec);
                    }
                }
                Effect::PeerLoss {
                    peer,
                    amount,
                    cause,
                    ..
                } => {
                    self.report(SettlementEvent::PeerLoss {
                        scheme,
                        peer: *peer,
                        amount: *amount,
                        cause: *cause,
                    });
                    self.book(booking);
                    if *cause == LossCause::Cheated {
                        self.end_for_cheating(*peer);
                    }
                }
                Effect::Finalize { settlement: id } => self.settlement_done(scheme, *id),
                Effect::OwnerPaid { id, applied } => self.owner_paid(scheme, *id, *applied),
                Effect::Cancel { settlement: id } => self.settlement_cancelled(scheme, *id),
            }
        }
        self.take_journal();
        self.checkpoint_if_changed(scheme);
        Ok(())
    }

    /// The transfer each of a scheme's effects books, if any, once the
    /// books have checked them all: each as they would find the accounts
    /// with the ones before it booked. Posting or voiding one of our
    /// settlements (`Finalize`, `Cancel`) or the hold of an owner's payment
    /// (`OwnerPaid`) is not checked: it raises no
    /// account's total and tightens no limit, so it cannot make a later
    /// transfer fail, and it is never refused itself.
    fn check_effects(
        &self,
        scheme: Name,
        ledger: LedgerId,
        effects: &[Effect],
    ) -> Result<Vec<Option<TransferSpec>>, EngineError> {
        let accounts = [
            self.account(ledger, AccountName::AssetsSettlement)?,
            self.account(ledger, AccountName::EquityOwner)?,
            self.account(ledger, AccountName::EquitySuspense)?,
        ];
        let mut bookings = Vec::with_capacity(effects.len());
        for effect in effects {
            let booking = self.effect_transfer(scheme, ledger, accounts, effect)?;
            bookings.push(
                booking.map(|(debit, credit, amount, reference)| TransferSpec {
                    debit,
                    credit,
                    amount,
                    pending: false,
                    reference,
                }),
            );
        }
        // Nothing to book for nothing: the journal holds no empty transfers.
        let specs: Vec<TransferSpec> = bookings
            .iter()
            .flatten()
            .filter(|spec| spec.amount > 0)
            .copied()
            .collect();
        self.ledger
            .validate_posted(&specs)
            .map_err(|(_, error)| EngineError::Invalid(error.to_string()))?;
        Ok(bookings)
    }

    /// What one of a scheme's effects books, if anything: debit, credit,
    /// amount and reference. `accounts` are the ledger's settlement, owner
    /// and suspense accounts.
    fn effect_transfer(
        &self,
        scheme: Name,
        ledger: LedgerId,
        accounts: [AccountId; 3],
        effect: &Effect,
    ) -> Result<Option<(AccountId, AccountId, u64, Reference)>, EngineError> {
        let [settlement, owner, suspense] = accounts;
        let transfer = match effect {
            Effect::Deposit { amount } => (settlement, owner, *amount, Reference::Deposit),
            Effect::Withdrawal { amount } => (owner, settlement, *amount, Reference::Withdrawal),
            Effect::Reconcile {
                amount, gain: true, ..
            } => (settlement, suspense, *amount, Reference::Reconciliation),
            Effect::Reconcile {
                amount,
                gain: false,
                ..
            } => (suspense, settlement, *amount, Reference::Reconciliation),
            Effect::Fee { amount, id } => {
                let fees = self.account(ledger, AccountName::ExpensesFees)?;
                (
                    fees,
                    settlement,
                    *amount,
                    Reference::Fee { scheme, id: *id },
                )
            }
            // A peer with no accounts on the ledger cannot have settled
            // with us: what it sent is a deposit. One without an ID
            // gets the next transfer's as it is booked, which the check
            // does not look at: a settlement is not a report either way.
            Effect::IncomingSettlement { peer, amount, id } => {
                let path = AccountPath::new(ledger, AccountName::AssetsInterledger(*peer));
                match self.ledger.find(&path) {
                    Some(interledger) => (
                        settlement,
                        interledger,
                        *amount,
                        Reference::Settlement {
                            scheme,
                            id: id.unwrap_or(scheme),
                        },
                    ),
                    None => (settlement, owner, *amount, Reference::Deposit),
                }
            }
            Effect::PeerLoss {
                peer,
                amount,
                cause,
                id,
            } => self.loss_transfer(scheme, ledger, settlement, *peer, *cause, *amount, *id)?,
            Effect::Finalize { .. } | Effect::Cancel { .. } | Effect::OwnerPaid { .. } => {
                return Ok(None);
            }
        };
        Ok(Some(transfer))
    }

    /// What a peering over a hub cost beyond its hold: Dr `expenses/losses`,
    /// Cr the account the money left, as its cause says.
    #[allow(clippy::too_many_arguments)]
    fn loss_transfer(
        &self,
        scheme: Name,
        ledger: LedgerId,
        settlement: AccountId,
        peer: Name,
        cause: LossCause,
        amount: u64,
        id: Name,
    ) -> Result<(AccountId, AccountId, u64, Reference), EngineError> {
        let losses = self.account(ledger, AccountName::ExpensesLosses)?;
        let from = match cause {
            LossCause::ClaimFee => settlement,
            LossCause::PaidLate | LossCause::Cheated => {
                self.account(ledger, AccountName::AssetsInterledger(peer))?
            }
        };
        Ok((losses, from, amount, Reference::Loss { scheme, id }))
    }

    /// Books a transfer of a batch the books checked whole.
    // Panics only on a broken invariant: what the check took, the books
    // take, with the journal drained before each.
    #[allow(clippy::panic)]
    fn book(&mut self, spec: Option<TransferSpec>) {
        let Some(spec) = spec.filter(|spec| spec.amount > 0) else {
            return;
        };
        self.take_journal();
        if let Err(error) = self.ledger.create(self.now_ms(), &spec) {
            panic!("the books refused a transfer they had checked: {error}");
        }
        self.take_journal();
    }

    /// A peer's settlement, as the scheme saw it arrive: credits the peer's
    /// account, keyed by the scheme's ID for it where it has one, or, from
    /// a node with no accounts on the ledger, the owner's
    /// ([`check_effects`](Self::check_effects)).
    fn settlement_in(
        &mut self,
        scheme: Name,
        peer: Name,
        amount: u64,
        id: Option<Name>,
        spec: TransferSpec,
    ) {
        if spec.reference == Reference::Deposit {
            self.report(SettlementEvent::DepositFromStranger { peer, amount });
            self.book(Some(spec));
            return;
        }
        let reference = match id {
            Some(id) => Reference::Settlement { scheme, id },
            None => self.incoming_reference(scheme),
        };
        self.book(Some(TransferSpec { reference, ..spec }));
        // It answered, and our books show it: it may be asked again at the
        // next check (spec/decisions/own-fulfills.md). Not on its message
        // alone, which on a ledger like the XRP Ledger books nothing: asked
        // again before our books moved, an honest peer would pay twice. And a
        // peer that settles is not stuck: the grace before it is withdrawn
        // for want of room starts again
        // (spec/decisions/peering-lifecycle.md). A peer whose peering we end
        // collects what it said was on its way
        // (spec/decisions/ending-a-peering.md). All of it on the method the
        // settlement came on, on its ledger
        // (spec/decisions/settlement-methods.md).
        if let Ok(node) = dassie_identity::NodeId::parse(peer.as_str())
            && let Some(method) = dassie_peer::wire::SchemeId::parse(scheme.as_str())
        {
            self.settle.asked.remove(&(node, method));
            let has = self
                .peer
                .peering_of(&node)
                .is_some_and(|peering| peering.has(method));
            // Counted toward what it said was on its way, if we end the
            // peering or wind the method down, only on the method it is
            // in the units of.
            if has {
                self.peer
                    .room(self.now_ms, &node, method, dassie_peer::Room::Enough);
                self.peer
                    .settlement_booked(self.now_ms, &node, method, amount);
            }
            // At least the least settlement, on any of its ledgers: it paid
            // us, for good, and is no free receiver
            // (spec/decisions/free-receivers-half.md).
            if amount >= self.settle.minimum {
                self.with_peer(|peer, hooks, now| peer.paid_by(now, hooks, &node));
            }
            self.report(SettlementEvent::SettlementBooked {
                from: node,
                scheme: method,
                amount,
            });
        }
    }

    fn post(
        &mut self,
        debit: AccountId,
        credit: AccountId,
        amount: u64,
        reference: Reference,
    ) -> Result<(), EngineError> {
        // Nothing to book; the journal holds no empty transfers.
        if amount == 0 {
            return Ok(());
        }
        let now = self.now_ms();
        let spec = TransferSpec {
            debit,
            credit,
            amount,
            pending: false,
            reference,
        };
        self.ledger
            .create(now, &spec)
            .map(|_| ())
            .map_err(|e| EngineError::Invalid(e.to_string()))?;
        self.take_journal();
        Ok(())
    }

    pub(super) fn account(
        &self,
        ledger: LedgerId,
        name: AccountName,
    ) -> Result<AccountId, EngineError> {
        self.ledger
            .find(&AccountPath::new(ledger, name))
            .ok_or_else(|| EngineError::Internal(format!("{ledger}:{name} is missing")))
    }

    /// Gives a ledger its STREAM receiver, a local endpoint paying into the
    /// owner's pool.
    fn add_receiver(&mut self, ledger: LedgerId) -> Result<(), EngineError> {
        if self.receivers.contains_key(&ledger) {
            return Ok(());
        }
        let endpoint = self.free_local();
        let own = self.connector.own_address().to_string();
        let receiver = Receiver::new(&self.key, &own, ledger, endpoint)?;
        let owner = self.account(ledger, AccountName::EquityOwner)?;
        self.connector
            .add_endpoint(endpoint, &receiver_address(&own, ledger), owner, ledger)
            .map_err(|e| EngineError::Internal(e.to_string()))?;
        self.receivers.insert(ledger, receiver);
        Ok(())
    }

    /// The next local endpoint, skipping any in use: a number comes back
    /// only after 65 536 activations, long after anything for it is gone.
    pub(super) fn free_local(&mut self) -> EndpointId {
        let taken = |n: u16| {
            let endpoint = EndpointId::Local(n);
            self.receivers
                .values()
                .any(|receiver| receiver.endpoint == endpoint)
                || self
                    .spending
                    .payers
                    .values()
                    .any(|payer| payer.endpoint == endpoint)
                || self.connector.swap_ledger(endpoint).is_some()
        };
        let mut free = self.next_local;
        for _ in 0..=super::MAX_LOCAL_ENDPOINTS {
            if !taken(free) {
                break;
            }
            free = free.wrapping_add(1);
        }
        assert!(!taken(free), "more local endpoints than there may be");
        self.next_local = free.wrapping_add(1);
        dassie_connector::EndpointId::Local(free)
    }

    /// The active schemes' IDs.
    pub fn active_schemes(&self) -> Vec<String> {
        self.schemes
            .keys()
            .map(|id| id.as_str().to_string())
            .collect()
    }

    /// The ledgers of active schemes.
    pub fn ledger_views(&self) -> Vec<LedgerView> {
        let mut ledgers: Vec<LedgerId> = self
            .schemes
            .values()
            .map(|active| active.scheme.ledger())
            .collect();
        ledgers.sort();
        ledgers
            .into_iter()
            .filter_map(|ledger| self.ledger_view(ledger))
            .collect()
    }

    /// What keeps scheme `id`, serving `ledger`, from being removed: the
    /// one list both the wallet and [`remove_scheme`](Self::remove_scheme)
    /// go by.
    fn removal_blockers(&self, id: Name, ledger: LedgerId) -> Vec<RemovalBlocker> {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let tokens = self
            .tokens
            .values()
            .filter(|token| token.row.revoked_at_ms.is_none() && token.row.ledger == ledger)
            .count();
        // Peerings with a method on the scheme block nothing: removal winds
        // their methods down (spec/decisions/settlement-methods.md).
        [
            (tokens, RemovalBlocker::BtpTokens(count(tokens))),
            (
                self.unfinished_on(ledger),
                RemovalBlocker::OutgoingPayments(count(self.unfinished_on(ledger))),
            ),
            (
                usize::from(self.hub_move_on_its_way(id)),
                RemovalBlocker::HubMove,
            ),
        ]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(_, blocker)| blocker)
        .collect()
    }

    fn ledger_view(&self, ledger: LedgerId) -> Option<LedgerView> {
        let (id, active) = self
            .schemes
            .iter()
            .find(|(_, active)| active.scheme.ledger() == ledger)?;
        let owner = self.account(ledger, AccountName::EquityOwner).ok()?;
        let group = self.ledger.group_balance(owner)?;
        let asset = active.asset.and_then(|asset| self.assets.get(&asset));
        // A deposit to a hub's escrow names the node's account by its tag,
        // once the account has its number and the hub agreed on it; the
        // first deposit, which opens it, is the node's own. An account that
        // failed, as one the hub names another number for, names none: the
        // number it knew may be someone else's, who a deposit would credit.
        let (deposit_address, deposit_tag) = match self.hub_account(*id) {
            Some(account) => match (
                account.number(),
                account.view(),
                dassie_settlement::ExternalAsset::status(account),
            ) {
                (_, _, LedgerStatus::Failed(_)) => (None, None),
                (Some(number), Some(_), _) => (
                    Some(dassie_xrpl::AccountId(account.terms().escrow).to_address()),
                    Some(number.0),
                ),
                _ => (None, None),
            },
            None => (asset.and_then(|asset| asset.asset.address()), None),
        };
        Some(LedgerView {
            id: ledger,
            scheme: id.as_str().to_string(),
            owner_balance: group.net_credit(),
            owner_pending: group.debits_pending,
            on_ledger_balance: active.scheme.balance(),
            reserve: active.scheme.reserve(),
            deposit_address,
            deposit_tag,
            issuer: active.scheme.issuer(),
            faucet: asset.is_some_and(|asset| asset.asset.has_faucet()),
            status: match active.failed {
                Some(_) => LedgerStatus::Failed(
                    "the books could not take what the ledger said; restart the node",
                ),
                None => self.scheme_status(active),
            },
            waiting: asset.and_then(|asset| asset.asset.waiting()),
            peg: active.scheme.peg().or_else(|| self.hub_peg(ledger)),
            removal_blockers: self.removal_blockers(*id, ledger),
            removing: active
                .removing
                .map(|_| u32::try_from(self.methods_on(*id)).unwrap_or(u32::MAX)),
        })
    }
}

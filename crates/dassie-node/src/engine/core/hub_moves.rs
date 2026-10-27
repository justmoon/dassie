//! Making the moves between the owner book of each hub's asset (on the
//! node's XRPL account on the hub's network: `xrpl-testnet-xrp` for the
//! testnet's XRP hub, `xrpl-testnet-dsusd` for its DSUSD hub, `xrpl-xrp`
//! and `xrpl-rlusd` for mainnet's) and the node's account in that hub that
//! `hub_funding.rs` decides on, keeping the one on its way in the
//! database, and the owner's view and moves (spec/areas/hubs.md#in-the-node).
//! A node is in one hub per asset ([hubs-only](spec/decisions/hubs-only.md)):
//! each hub's moves go on their own, under the scheme over it. Amounts
//! are counted in the hub asset's smallest unit, drops of XRP or millionths
//! of a token, which the books count alike (`UNITS_PER_DROP`); the XRP
//! Ledger's fee is XRP the account pays, never taken out of a token's move.
//! Before a token hub's withdrawal, the escrow's gas is checked, and topped
//! up from the XRP owner book of the hub's network if short
//! (spec/decisions/token-hubs.md).

use dassie_hub::member::{HubAccount, UNITS_PER_DROP};
use dassie_ledger::{LedgerId, Name};
use dassie_peer::MAX_OWN_HUBS;
use dassie_settlement::{
    ExternalAsset, LedgerStatus, OwnerMemo, OwnerPayment, OwnerPaymentState, SchemeError, policy,
};
use dassie_xrpl::XrplAccount;

use super::hub_funding::{
    Direction, Funding, Gas, GasNext, GasSeen, Held, Move, Planned, Standing,
};
use super::{Core, EngineError};
use crate::store::{HubFundingRow, HubMoveRow};

/// The most XRP Ledger fee a payment out of an owner book is sized to leave
/// room for, in drops: XRP the account pays, from the XRP book.
const XRPL_FEE_DROPS: u64 = dassie_xrpl::account::MAX_FEE_DROPS;

/// The fee a payment out of the owner book on `ledger` holds back of the
/// owner's funds there, in the book asset's smallest unit: the XRP Ledger's
/// fee from the XRP book, and nothing from a token's, whose fee is XRP the
/// account pays as the XRP book's gas.
fn book_fee(ledger: LedgerId) -> u64 {
    if ledger.asset().code == "XRP" {
        XRPL_FEE_DROPS
    } else {
        0
    }
}

/// `smallest` of the asset of the hub of `scheme`, in its smallest unit at
/// scale 6, for the owner to read as the command line shows it: `2.5 XRP`,
/// `0.000001 DSUSD`. The asset is the last word of the scheme's ID
/// (spec/decisions/hub-scheme-names.md), where the ledger's books count
/// DSUSD as `USD`.
fn in_asset(scheme: Name, smallest: u64) -> String {
    let whole = smallest / 1_000_000;
    let fraction = format!("{:06}", smallest % 1_000_000);
    let fraction = fraction.trim_end_matches('0');
    let code = scheme
        .as_str()
        .rsplit('-')
        .next()
        .unwrap_or("XRP")
        .to_ascii_uppercase();
    if fraction.is_empty() {
        format!("{whole} {code}")
    } else {
        format!("{whole}.{fraction} {code}")
    }
}

/// The ledger of the owner book of what a hub on `hub` holds: the book of
/// that asset on the hub's underlying ledger, which the hub's deposits
/// leave and its withdrawals reach (spec/areas/hubs.md#in-the-node). On
/// the testnet, XRP for the XRP hub and DSUSD for the DSUSD hub; on
/// mainnet, XRP for the XRP hub and RLUSD for the RLUSD hub, each of the
/// hub's realm. `None` for a ledger that is no hub's.
pub(super) const fn owner_book_ledger(hub: LedgerId) -> Option<LedgerId> {
    match hub {
        LedgerId::HubXrplTestnetXrp => Some(LedgerId::XrplTestnetXrp),
        LedgerId::HubXrplTestnetDsusdUsd => Some(LedgerId::XrplTestnetDsusdUsd),
        LedgerId::HubXrplXrp => Some(LedgerId::XrplXrp),
        LedgerId::HubXrplRlusdUsd => Some(LedgerId::XrplRlusdUsd),
        LedgerId::StubUsd
        | LedgerId::XrplTestnetXrp
        | LedgerId::XrplXrp
        | LedgerId::XrplTestnetRlusdUsd
        | LedgerId::XrplRlusdUsd
        | LedgerId::XrplTestnetDsusdUsd => None,
    }
}

/// The ledger of the XRP owner book on the network of a hub on `hub`: the
/// owner book of its network's XRP hub. A token hub's escrow pays the XRP
/// Ledger's fees in XRP, which the node reads and tops up from it
/// (spec/decisions/token-hubs.md). `None` for a ledger that is no hub's.
pub(super) const fn gas_book_ledger(hub: LedgerId) -> Option<LedgerId> {
    match hub {
        LedgerId::HubXrplTestnetXrp | LedgerId::HubXrplTestnetDsusdUsd => {
            owner_book_ledger(LedgerId::HubXrplTestnetXrp)
        }
        LedgerId::HubXrplXrp | LedgerId::HubXrplRlusdUsd => owner_book_ledger(LedgerId::HubXrplXrp),
        LedgerId::StubUsd
        | LedgerId::XrplTestnetXrp
        | LedgerId::XrplXrp
        | LedgerId::XrplTestnetRlusdUsd
        | LedgerId::XrplRlusdUsd
        | LedgerId::XrplTestnetDsusdUsd => None,
    }
}

/// The ledger of the owner book that funds the escrow of a hub the node
/// founds: hubs are founded on the XRP Ledger testnet, in XRP
/// (spec/areas/hubs.md#founding-a-hub).
const FOUNDING_LEDGER: LedgerId = LedgerId::XrplTestnetXrp;

/// A hold of ours or toward us in the hub, as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubHold {
    /// The peer on its other side, if the node peers with it over the hub.
    pub peer: Option<dassie_identity::NodeId>,
    /// The other side's account.
    pub account: u32,
    /// What is left in it, in internal units.
    pub amount: u64,
}

/// The node's account in one hub and its funding, as the owner sees them.
/// Amounts are in internal units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubStatus {
    /// The hub's ID.
    pub hub_id: [u8; 32],
    /// The scheme over the hub, which names its ledger and asset.
    pub scheme: Name,
    /// Our account's number, once the hub has said it.
    pub account: Option<u32>,
    /// How the account stands with the hub.
    pub status: LedgerStatus,
    /// What it can spend, as last agreed.
    pub balance: u64,
    /// Its account reserve.
    pub reserve: u64,
    /// What deposits credited it.
    pub deposited: u64,
    /// What withdrawals took from it.
    pub withdrawn: u64,
    /// What withdrawals that came back credited it.
    pub returned: u64,
    /// Our holds toward peers.
    pub paying: Vec<HubHold>,
    /// Peers' holds toward us.
    pub taking: Vec<HubHold>,
    /// The floor and the ceiling.
    pub floor: u64,
    /// The ceiling.
    pub ceiling: u64,
    /// Whether the node moves funds by itself.
    pub automatic: bool,
    /// Until when the owner's last move pauses the automatic ones.
    pub paused_until_ms: Option<u64>,
    /// The move on its way.
    pub moving: Option<Move>,
    /// Whether it is late: the owner should look, and nothing is paid
    /// again for it.
    pub late: bool,
    /// The move the owner gave up, which way and how much in the hub
    /// asset's smallest unit, while it holds the node's own moves back: the
    /// other side may yet show it.
    pub given_up: Option<(Direction, u64)>,
    /// How many validators the hub has, and how many answer.
    pub validators: (usize, usize),
    /// The stamp of the hub's block from which a token hub's issuer
    /// freezes it, as f + 1 validators last said
    /// (spec/areas/hubs.md#a-frozen-hub).
    pub frozen_since_ms: Option<u64>,
    /// Whether the issuer holds the escrow: frozen, or charging a transfer
    /// fee.
    pub escrow_held: bool,
}

/// Internal units in the hub asset's smallest unit, rounded up: a floor is
/// never short.
const fn drops_up(units: u64) -> u64 {
    units.div_ceil(UNITS_PER_DROP)
}

/// A move as the database keeps it.
fn row(funding: &Funding) -> HubFundingRow {
    HubFundingRow {
        paused_until_ms: funding.paused_until_ms,
        moving: funding.moving.map(|moving| HubMoveRow {
            deposit: moving.direction == Direction::Deposit,
            drops: moving.drops,
            target: moving.target,
            payment: moving.payment.as_str().into(),
            by_owner: moving.owner,
            sent_ms: moving.sent_ms,
            applied_ms: moving.applied_ms,
        }),
        given_up: funding
            .given_up
            .map(|(direction, drops)| (direction == Direction::Deposit, drops)),
    }
}

/// A move from what the database keeps.
fn from_row(row: &HubMoveRow) -> Result<Move, String> {
    Ok(Move {
        direction: if row.deposit {
            Direction::Deposit
        } else {
            Direction::Withdraw
        },
        drops: row.drops,
        owner: row.by_owner,
        target: row.target,
        payment: Name::new(&row.payment).map_err(|_| "a hub move's payment ID".to_string())?,
        sent_ms: row.sent_ms,
        applied_ms: row.applied_ms,
    })
}

/// A hub's moves as the database keeps them, checked as of `now_ms`.
fn restored(
    now_ms: u64,
    interval_ms: u64,
    automatic: bool,
    row: &HubFundingRow,
) -> Result<Funding, String> {
    let mut funding = Funding::new(now_ms, interval_ms);
    funding.automatic = automatic;
    funding.paused_until_ms = row.paused_until_ms;
    funding.moving = row.moving.as_ref().map(from_row).transpose()?;
    funding.given_up = row.given_up.map(|(deposit, drops)| {
        let direction = if deposit {
            Direction::Deposit
        } else {
            Direction::Withdraw
        };
        (direction, drops)
    });
    Ok(funding)
}

/// The hubs the node is in, each by the scheme over it.
impl Core {
    /// The active schemes over a hub, in their order: at most
    /// [`MAX_OWN_HUBS`], one per asset, each the node's account in a hub
    /// once it has one.
    pub(super) fn hub_schemes(&self) -> Vec<Name> {
        self.schemes
            .iter()
            .filter(|(_, active)| active.over_hub)
            .map(|(name, _)| *name)
            .take(MAX_OWN_HUBS)
            .collect()
    }

    /// Restores the moves of each hub the node is in from the database, or
    /// from the batch where it changed them since, and forgets those of a
    /// scheme no longer over a hub, which the database keeps. A hub's moves
    /// are read once, when its scheme starts.
    ///
    /// # Errors
    ///
    /// The database failed, or holds what it may not.
    pub(super) fn load_funding(&mut self) -> Result<(), String> {
        let hubs = self.hub_schemes();
        self.funding.retain(|scheme, _| hubs.contains(scheme));
        for scheme in hubs {
            if self.funding.contains_key(&scheme) {
                continue;
            }
            let (automatic, row) = self.store.hub_funding(scheme).map_err(|e| e.to_string())?;
            let automatic = self
                .batch
                .hub_automatic
                .get(&scheme)
                .copied()
                .unwrap_or(automatic);
            let row = self.batch.hub_funding.get(&scheme).unwrap_or(&row);
            let funding = restored(self.now_ms, self.settle.interval_ms, automatic, row)?;
            self.funding.insert(scheme, funding);
        }
        assert!(self.funding.len() <= MAX_OWN_HUBS, "a funding per hub");
        Ok(())
    }

    /// [`load_funding`](Self::load_funding) in a step.
    // A database that cannot be read leaves the core unable to tell a move
    // on its way from none: it stops, as when a write fails, and restarts
    // from the disk.
    #[allow(clippy::panic)]
    pub(super) fn follow_hubs(&mut self) {
        if let Err(error) = self.load_funding() {
            panic!("reading a hub's funding failed: {error}");
        }
    }

    /// Keeps the moves' state of the hub of `scheme` with this batch.
    fn keep_funding(&mut self, scheme: Name) {
        if let Some(funding) = self.funding.get(&scheme) {
            self.batch.hub_funding.insert(scheme, row(funding));
        }
    }

    /// Whether a move between the hub of `scheme` and its owner book is on
    /// its way: the scheme is not removed meanwhile, since the move is its
    /// account's in that hub.
    pub(super) fn hub_move_on_its_way(&self, scheme: Name) -> bool {
        self.funding
            .get(&scheme)
            .is_some_and(|funding| funding.moving.is_some())
    }

    /// The scheme that books `ledger`, if one is active: a ledger has at
    /// most one (spec/areas/settlement.md#schemes-and-ledgers).
    fn book_on(&self, ledger: LedgerId) -> Option<Name> {
        self.schemes
            .iter()
            .find(|(_, active)| active.scheme.ledger() == ledger)
            .map(|(name, _)| *name)
    }

    /// The owner book of the asset of the hub of `scheme`, and its ledger:
    /// the active scheme on that asset's ledger, whether the node peers on
    /// it or not (spec/areas/settlement.md#owner-books). `None` while the
    /// hub scheme or the book is not active.
    fn owner_book(&self, scheme: Name) -> Option<(Name, LedgerId)> {
        let hub = self.schemes.get(&scheme)?.scheme.ledger();
        let ledger = owner_book_ledger(hub)?;
        Some((self.book_on(ledger)?, ledger))
    }

    /// The owner book that funds the escrow of a hub the node founds, and
    /// its ledger: `None` while none is active.
    fn founding_book(&self) -> Option<(Name, LedgerId)> {
        Some((self.book_on(FOUNDING_LEDGER)?, FOUNDING_LEDGER))
    }

    /// The XRP owner book on the network of the hub of `scheme`, and its
    /// ledger ([`gas_book_ledger`]): where a token hub's escrow's gas is
    /// read and paid from. `None` while the hub scheme or the book is not
    /// active.
    fn gas_book(&self, scheme: Name) -> Option<(Name, LedgerId)> {
        let hub = self.schemes.get(&scheme)?.scheme.ledger();
        let ledger = gas_book_ledger(hub)?;
        Some((self.book_on(ledger)?, ledger))
    }

    /// The node's account in the hub of `scheme`, while the scheme runs.
    fn hub_account(&self, scheme: Name) -> Option<&HubAccount> {
        let active = self.schemes.get(&scheme)?;
        if active.failed.is_some() {
            return None;
        }
        let asset = self.assets.get(&active.asset?)?;
        let any: &dyn core::any::Any = asset.asset.as_ref();
        any.downcast_ref::<HubAccount>()
    }

    /// The node's account in the first of its hubs, while its scheme runs.
    #[cfg(feature = "testing")]
    fn first_hub_account(&self) -> Option<&HubAccount> {
        self.hub_schemes()
            .into_iter()
            .find_map(|scheme| self.hub_account(scheme))
    }

    /// The scheme of the hub an owner's call names: `hub`, its ID, or none
    /// while the node is in one hub only.
    ///
    /// # Errors
    ///
    /// No hub scheme runs, none has that ID, or the node is in several
    /// and the call names none.
    fn chosen_hub(&self, hub: Option<[u8; 32]>) -> Result<Name, EngineError> {
        let running: Vec<(Name, [u8; 32])> = self
            .hub_schemes()
            .into_iter()
            .filter_map(|scheme| Some((scheme, *self.hub_account(scheme)?.hub().0.as_bytes())))
            .collect();
        match (hub, running.as_slice()) {
            (_, []) => Err(EngineError::Precondition(
                "the settlement hub scheme is not running".into(),
            )),
            (None, [(scheme, _)]) => Ok(*scheme),
            (None, _) => Err(EngineError::Invalid(
                "the node is in several settlement hubs: name one".into(),
            )),
            (Some(id), running) => running
                .iter()
                .find(|(_, running)| *running == id)
                .map(|(scheme, _)| *scheme)
                .ok_or_else(|| EngineError::NotFound("the node is in no such hub".into())),
        }
    }

    /// The sequence the account in the node's first hub learnt its next
    /// operation carries, from the operations of its own it read and the
    /// states agreed, for a simulation to tell whether it read those the
    /// hub applied. Not its newest state's, which it asks for only every
    /// few seconds.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn hub_sequence(&self) -> Option<u64> {
        self.first_hub_account()?.sequence()
    }

    /// How far the account in the node's first hub read the hub, for a
    /// simulation to tell one behind from one in step: the height through
    /// which it read every operation that touched it, the height of the
    /// state it last heard of itself, and whether it reads its state anew.
    #[cfg(feature = "testing")]
    #[must_use]
    pub fn hub_reading(&self) -> Option<(u64, Option<u64>, bool)> {
        let account = self.first_hub_account()?;
        Some((
            account.cursor(),
            account.view().map(|(height, _)| *height),
            account.is_resyncing(),
        ))
    }

    /// The peerings over the hub of `scheme`: each peer's node and its
    /// account in the hub, as the scheme knows it, which for a peer paid
    /// into the hub is not the none it gave at peering
    /// (spec/designs/settlement-hubs.md#5-joining); 0 for a peer without
    /// one.
    fn hub_peerings(&self, scheme: Name) -> Vec<(dassie_identity::NodeId, u32)> {
        let Some(active) = self.schemes.get(&scheme) else {
            return Vec::new();
        };
        let Some(method) = dassie_peer::wire::SchemeId::parse(scheme.as_str()) else {
            return Vec::new();
        };
        // Every peering with a method over the hub, whichever of its
        // methods comes first (spec/areas/network.md#settlement-methods).
        self.peer
            .active_peers()
            .filter_map(|(node, peering)| {
                peering.method(method)?;
                let peer = Name::new(node.as_str()).ok()?;
                let number = active.scheme.peer_hub_account(peer).unwrap_or(0);
                Some((*node, number))
            })
            .collect()
    }

    /// The floor and the ceiling, in internal units, for `peerings`.
    fn hub_limits(&self, peerings: usize) -> (u64, u64) {
        let max_packet = self.settle.max_packet;
        let peerings = u64::try_from(peerings).unwrap_or(u64::MAX);
        (
            policy::hub_floor(max_packet, self.peer_credit, peerings),
            policy::hub_ceiling(max_packet),
        )
    }

    /// The hub account as last agreed, once the account read up to it.
    /// What the node keeps there is all a failing hub could cost it: the
    /// balance and the reserves, its holds toward peers, which are the
    /// credit it extends them over the hub, paid in advance, and what the
    /// holds toward it hold, not taken yet.
    fn held(account: &HubAccount) -> Option<Held> {
        let hold_reserve = account.terms().fees.hold_reserve;
        account
            .view()
            .filter(|_| account.is_synced())
            .map(|(_, view)| {
                let number = account.number();
                let toward_us: u64 = account
                    .holds()
                    .filter(|(_, hold)| Some(hold.payee) == number)
                    .map(|(_, hold)| hold.amount)
                    .fold(0, u64::saturating_add);
                let paying = account
                    .holds()
                    .filter(|(_, hold)| Some(hold.payer) == number)
                    .count();
                let ours: u64 = account
                    .holds()
                    .filter(|(_, hold)| Some(hold.payer) == number)
                    .map(|(_, hold)| hold.amount)
                    .fold(0, u64::saturating_add);
                let hold_reserves =
                    hold_reserve.saturating_mul(u64::try_from(paying).unwrap_or(u64::MAX));
                Held {
                    balance: view.balance,
                    exposure: view
                        .balance
                        .saturating_add(view.reserve)
                        .saturating_add(hold_reserves)
                        .saturating_add(ours)
                        .saturating_add(toward_us),
                    deposited: view.deposited,
                    withdrawn: view.withdrawn,
                }
            })
    }

    /// What a move of the hub of `scheme` is decided from, while the
    /// scheme runs.
    fn standing(&self, scheme: Name) -> Option<Standing> {
        let account = self.hub_account(scheme)?;
        let active = self.schemes.get(&scheme)?;
        let terms = account.terms();
        let fees = terms.fees;
        let (floor, ceiling) = self.hub_limits(self.hub_peerings(scheme).len());
        let (floor, ceiling) = (drops_up(floor), ceiling / UNITS_PER_DROP);
        let unfunded = active
            .asset
            .and_then(|asset| self.assets.get(&asset))
            .is_some_and(|asset| asset.asset.status() == LedgerStatus::Unfunded);
        // A hub whose scheme is not in step with it, or a token hub its
        // validators say is frozen, is unavailable: nothing moves into or
        // out of it (spec/areas/hubs.md#a-frozen-hub).
        let unavailable =
            active.scheme.status() != LedgerStatus::Synced || account.frozen().is_some();
        Some(Standing {
            held: Self::held(account),
            unfunded,
            unavailable,
            floor,
            ceiling,
            opening: fees
                .account_reserve
                .saturating_add(floor)
                .saturating_add(fees.starting_balance)
                .max(terms.new_account_minimum()),
            least_payout: fees.least_payout,
            ledger_fee: fees.ledger,
            withdrawal_fees: fees.operation.saturating_add(fees.ledger),
            least_deposit: drops_up(self.settle.max_packet),
            xrpl: self
                .owner_book(scheme)
                .and_then(|book| self.book_spendable(book)),
            hub_owner: self.owner_available(active.scheme.ledger()) / UNITS_PER_DROP,
            paying: self.paying(scheme),
        })
    }

    /// What the owner may move out of the XRPL account through the owner
    /// book `(scheme, ledger)`, in its asset's smallest unit, after the
    /// fee it holds back ([`book_fee`]); `None` while the book does not
    /// run, or its account is not up to date with its ledger and could sign
    /// nothing.
    fn book_spendable(&self, (scheme, ledger): (Name, LedgerId)) -> Option<u64> {
        let active = self.schemes.get(&scheme)?;
        let synced = self
            .assets
            .get(&active.asset?)
            .is_some_and(|asset| asset.asset.status() == LedgerStatus::Synced);
        if active.failed.is_some() || !active.scheme.has_synced() || !synced {
            return None;
        }
        let funds = self.owner_available(ledger) / UNITS_PER_DROP;
        Some(funds.saturating_sub(book_fee(ledger)))
    }

    /// What became of the payment of the move of the hub of `scheme` on
    /// its way, as the scheme of the side it leaves says; unknown while
    /// that scheme does not run.
    fn paying(&self, scheme: Name) -> OwnerPaymentState {
        let Some(moving) = self.funding.get(&scheme).and_then(|funding| funding.moving) else {
            return OwnerPaymentState::Unknown;
        };
        let leaves = match moving.direction {
            Direction::Deposit => match self.owner_book(scheme) {
                Some((book, _)) => book,
                None => return OwnerPaymentState::Unknown,
            },
            Direction::Withdraw => scheme,
        };
        self.schemes
            .get(&leaves)
            .filter(|active| active.failed.is_none())
            .map_or(OwnerPaymentState::Unknown, |active| {
                active.scheme.owner_payment(moving.payment)
            })
    }

    /// Follows the move on its way of each hub, and makes the next
    /// automatic one when it is due, at each of its funding checks.
    pub(super) fn fund_hub(&mut self) {
        self.follow_hubs();
        let hubs: Vec<Name> = self.funding.keys().copied().collect();
        for scheme in hubs {
            self.fund_one_hub(scheme);
        }
    }

    /// [`fund_hub`](Self::fund_hub) for the hub of `scheme`.
    fn fund_one_hub(&mut self, scheme: Name) {
        let now = self.now_ms;
        if !self
            .funding
            .get_mut(&scheme)
            .is_some_and(|funding| funding.check_due(now))
        {
            return;
        }
        let Some(standing) = self.standing(scheme) else {
            return;
        };
        let Some(funding) = self.funding.get_mut(&scheme) else {
            return;
        };
        if funding.follow(now, &standing) {
            self.keep_funding(scheme);
        }
        let due = self
            .funding
            .get(&scheme)
            .and_then(|funding| funding.due(now, &standing));
        let Some(planned) = due else {
            // A check of a token hub's gas the owner started goes on.
            if self.funding.get(&scheme).is_some_and(Funding::gas_checking) {
                // Nothing to tell anyone: the XRP book not running holds
                // it back until it runs.
                let _ = self.check_gas(scheme);
            }
            return;
        };
        if planned.direction == Direction::Withdraw
            && self.token_hub(scheme)
            && !matches!(self.check_gas(scheme), Ok(true))
        {
            return;
        }
        if self.make_move(scheme, planned, &standing).is_err()
            && let Some(funding) = self.funding.get_mut(&scheme)
        {
            funding.failed(now);
        }
    }

    /// Whether the hub of `scheme` holds a token, whose escrow pays the XRP
    /// Ledger's fees from XRP anyone may send it.
    fn token_hub(&self, scheme: Name) -> bool {
        self.hub_account(scheme)
            .is_some_and(|account| account.terms().asset != dassie_hub::rules::Asset::Xrp)
    }

    /// The XRPL account of the XRP owner book on the network of the hub of
    /// `scheme`, while it runs: where the escrow's gas is read and paid
    /// from.
    fn xrp_book_account(&self, scheme: Name) -> Option<&XrplAccount> {
        let (book, _) = self.gas_book(scheme)?;
        let active = self.schemes.get(&book).filter(|a| a.failed.is_none())?;
        let asset = self.assets.get(&active.asset?)?;
        let any: &dyn core::any::Any = asset.asset.as_ref();
        any.downcast_ref::<XrplAccount>()
    }

    /// What the check of the gas of the token hub of `scheme` is decided
    /// from: the escrow's XRP as the XRP owner book's account proved it.
    fn gas_seen(&self, scheme: Name) -> Option<GasSeen> {
        let escrow = dassie_xrpl::AccountId(self.hub_account(scheme)?.terms().escrow);
        let account = self.xrp_book_account(scheme)?;
        let topping_up = match self.funding.get(&scheme)?.gas {
            Gas::ToppingUp { payment, .. } => self.book_payment(self.gas_book(scheme), payment),
            _ => OwnerPaymentState::Unknown,
        };
        let book_held = self
            .gas_book(scheme)
            .is_some_and(|(book, _)| self.owner_payment_held(book));
        Some(GasSeen {
            anchor: account
                .anchor()
                .filter(|_| account.is_synced())
                .map(|anchor| anchor.index),
            proven: account
                .proven_balance(&escrow)
                .map(|proven| (proven.drops, proven.ledger_index)),
            topping_up,
            book_held,
        })
    }

    /// Takes the next step of the check of the gas of the token hub of
    /// `scheme` before a withdrawal (spec/decisions/token-hubs.md): reads
    /// the escrow's XRP through the XRP owner book's account, and if it is
    /// short pays the escrow the shortfall from the book, a payment of XRP
    /// that credits no one in the hub. Returns whether the escrow has
    /// its gas now. The escrow requires a destination tag on what it is
    /// paid, so the top-up names the node's account in the hub, which XRP
    /// credits nothing. A short read while the XRP book has another
    /// payment on its way, which may be a top-up from before a restart the
    /// read does not show yet, waits for the book to be free, and the
    /// escrow is read again then. A top-up the XRP book still cannot take
    /// now is decided again from a read proven at a later ledger, with no
    /// backoff; one refused otherwise backs the check off.
    ///
    /// # Errors
    ///
    /// The XRP owner book does not run in step with its ledger: the check
    /// waits for it.
    fn check_gas(&mut self, scheme: Name) -> Result<bool, EngineError> {
        let now = self.now_ms;
        let Some(escrow) = self
            .hub_account(scheme)
            .map(|account| dassie_xrpl::AccountId(account.terms().escrow))
        else {
            return Ok(false);
        };
        let seen = self
            .gas_seen(scheme)
            .filter(|seen| seen.anchor.is_some())
            .ok_or_else(|| {
                EngineError::Precondition(
                    "a token hub's escrow pays its fees in XRP, which the node reads and tops \
                     up through its owner book of XRP on the hub's network (xrpl-testnet-xrp \
                     or xrpl-xrp): that book is not running synced"
                        .into(),
                )
            })?;
        let Some(funding) = self.funding.get_mut(&scheme) else {
            return Ok(false);
        };
        Ok(match funding.gas_step(now, &seen) {
            GasNext::Ready => true,
            GasNext::Wait => false,
            GasNext::Read => {
                self.read_balance_of(scheme, escrow);
                false
            }
            GasNext::TopUp { drops, at } => {
                // The escrow takes no payment without a tag: the account's
                // own number names who paid it, and XRP credits no one in
                // a token hub.
                let tag = self
                    .hub_account(scheme)
                    .and_then(HubAccount::number)
                    .map(|number| number.0);
                let book = self.gas_book(scheme);
                let paid = self.pay_from_xrp_book(book, &escrow.to_address(), tag, drops);
                let refused = matches!(paid, Err(EngineError::Exhausted(_)));
                if let Some(funding) = self.funding.get_mut(&scheme) {
                    match paid {
                        Ok(payment) => funding.topping_up(payment, at),
                        // Not now: decided again from a later read.
                        Err(EngineError::Exhausted(_)) => funding.top_up_refused(at),
                        Err(_) => funding.gas_failed(now),
                    }
                }
                if refused {
                    self.read_balance_of(scheme, escrow);
                }
                false
            }
        })
    }

    /// Has the account of the XRP owner book on the network of the hub of
    /// `scheme` read the XRP of `account`, proven, and sends what it asks
    /// of its ledger.
    fn read_balance_of(&mut self, scheme: Name, account: dassie_xrpl::AccountId) {
        let Some((book, _)) = self.gas_book(scheme) else {
            return;
        };
        let Some(asset) = self.schemes.get(&book).and_then(|active| active.asset) else {
            return;
        };
        let Some(active) = self.assets.get_mut(&asset) else {
            return;
        };
        let any: &mut dyn core::any::Any = active.asset.as_mut();
        if let Some(xrpl) = any.downcast_mut::<XrplAccount>() {
            xrpl.read_balance(account);
            self.carry_out(asset);
        }
    }

    /// When the next funding check is, the soonest of the hubs whose
    /// scheme runs.
    pub(super) fn funding_deadline(&self) -> Option<u64> {
        self.funding
            .iter()
            .filter(|(scheme, _)| self.hub_account(**scheme).is_some())
            .map(|(_, funding)| funding.at_ms)
            .min()
    }

    /// Makes `planned` for the hub of `scheme`: pays out of the side it
    /// leaves, and keeps it, with its payment's ID, in the batch that keeps
    /// the payment.
    fn make_move(
        &mut self,
        scheme: Name,
        planned: Planned,
        standing: &Standing,
    ) -> Result<(), SchemeError> {
        // The hub's funding is there before anything is paid, so that no
        // error can follow a payment gone out without its move recorded.
        if !self.funding.contains_key(&scheme) {
            return Err(SchemeError::Unavailable);
        }
        let (pays, payment) = self.payment_for(scheme, planned, standing)?;
        // The most its fees may cost: the XRP Ledger's fee out of the XRP
        // book (a token's fee is the XRP book's gas), or the hub's
        // operation fee and ledger fee.
        let fee_drops = match planned.direction {
            Direction::Deposit => {
                let (_, ledger) = self.owner_book(scheme).ok_or(SchemeError::Unavailable)?;
                book_fee(ledger)
            }
            Direction::Withdraw => standing.withdrawal_fees,
        };
        let fee = fee_drops.saturating_mul(UNITS_PER_DROP);
        self.check_owner_payment(pays, payment.amount, fee)?;
        let id = self.pay_owner(pays, &payment)?;
        self.hold_owner_payment(pays, id, payment.amount, fee);
        let now = self.now_ms;
        let funding = self.funding.get_mut(&scheme);
        assert!(funding.is_some(), "the hub's funding outlives its payment");
        if let Some(funding) = funding {
            funding.start(now, planned, id);
        }
        self.keep_funding(scheme);
        Ok(())
    }

    /// The payment that makes `planned` for the hub of `scheme`, and the
    /// scheme that pays it.
    fn payment_for(
        &self,
        scheme: Name,
        planned: Planned,
        standing: &Standing,
    ) -> Result<(Name, OwnerPayment), SchemeError> {
        let account = self.hub_account(scheme).ok_or(SchemeError::Unavailable)?;
        let amount = planned.drops.saturating_mul(UNITS_PER_DROP);
        Ok(match planned.direction {
            Direction::Deposit => {
                let escrow = dassie_xrpl::AccountId(account.terms().escrow);
                let (tag, memo) = match (standing.held, account.number()) {
                    (Some(_), Some(number)) => (number.0, None),
                    _ => (
                        0,
                        Some(OwnerMemo {
                            memo_type: dassie_hub::escrow::OPEN_MEMO_TYPE.to_vec(),
                            data: account.key().as_bytes().to_vec(),
                        }),
                    ),
                };
                let payment = OwnerPayment {
                    destination: escrow.to_address(),
                    tag: Some(tag),
                    memo,
                    amount,
                };
                let (book, _) = self.owner_book(scheme).ok_or(SchemeError::Unavailable)?;
                (book, payment)
            }
            Direction::Withdraw => {
                let network =
                    super::schemes::hub_network(account.terms()).ok_or(SchemeError::Unavailable)?;
                let own = super::schemes::own_xrpl_account(&self.key, network);
                let payment = OwnerPayment {
                    destination: own.to_address(),
                    tag: None,
                    memo: None,
                    amount,
                };
                (scheme, payment)
            }
        })
    }

    /// Has `scheme` pay `payment` out of the owner's funds, and carries out
    /// what its asset asks. Returns the payment's ID.
    fn pay_owner(&mut self, scheme: Name, payment: &OwnerPayment) -> Result<Name, SchemeError> {
        let asset = self
            .schemes
            .get(&scheme)
            .and_then(|active| active.asset)
            .ok_or(SchemeError::Unavailable)?;
        self.tell_owner_funds(asset);
        let (active, on) = self
            .scheme_and_asset(scheme)
            .ok_or(SchemeError::Unavailable)?;
        let paid = active.scheme.pay_owner(on, payment);
        self.carry_out(asset);
        paid
    }
}

/// Funding the escrow of a hub the node founds
/// (spec/areas/hubs.md#founding-a-hub).
impl Core {
    /// Pays `drops` from the owner's funds on the XRPL testnet account to
    /// `destination`, with no tag, through the owner book of XRP on the
    /// testnet, whether the node peers on it or not: the payment that
    /// creates the escrow of a hub the node founds. Held against the
    /// owner's funds while it is on its way, as any owner's payment. Its ID,
    /// to follow it by.
    ///
    /// # Errors
    ///
    /// The XRP scheme does not run synced, the owner's funds there do not
    /// cover it with its fee, or the scheme has an owner's payment on its
    /// way already.
    pub fn pay_from_xrpl(&mut self, destination: &str, drops: u64) -> Result<Name, EngineError> {
        let book = self.founding_book();
        self.pay_from_xrp_book(book, destination, None, drops)
    }

    /// [`pay_from_xrpl`](Self::pay_from_xrpl) through the XRP owner book
    /// `book`, the testnet's or mainnet's, with destination tag `tag`.
    fn pay_from_xrp_book(
        &mut self,
        book: Option<(Name, LedgerId)>,
        destination: &str,
        tag: Option<u32>,
        drops: u64,
    ) -> Result<Name, EngineError> {
        let not_running =
            || EngineError::Precondition("the XRP Ledger scheme is not running synced".into());
        let book = book.ok_or_else(not_running)?;
        assert_eq!(
            book.1.asset().code,
            "XRP",
            "only an XRP book pays an escrow's XRP"
        );
        let spendable = self.book_spendable(book).ok_or_else(not_running)?;
        let (book, _) = book;
        if drops == 0 || drops > spendable {
            return Err(EngineError::Refused(
                "more than the owner's funds on the XRP Ledger, with its fee".into(),
            ));
        }
        let amount = drops.saturating_mul(UNITS_PER_DROP);
        let payment = OwnerPayment {
            destination: destination.into(),
            tag,
            memo: None,
            amount,
        };
        let fee = XRPL_FEE_DROPS.saturating_mul(UNITS_PER_DROP);
        let refused = |error: SchemeError| {
            EngineError::Exhausted(format!("the XRP scheme cannot pay now: {error:?}"))
        };
        self.check_owner_payment(book, amount, fee)
            .map_err(refused)?;
        let id = self.pay_owner(book, &payment).map_err(refused)?;
        self.hold_owner_payment(book, id, amount, fee);
        Ok(id)
    }

    /// The rules of each hub the node is a member of, while its scheme
    /// runs, in the order of the schemes.
    #[must_use]
    pub fn hub_rules(&self) -> Vec<dassie_hub::Rules> {
        self.hub_schemes()
            .into_iter()
            .filter_map(|scheme| Some(self.hub_account(scheme)?.rules().clone()))
            .collect()
    }

    /// What became of the owner's payment `id` out of the XRPL testnet
    /// account, as the owner book of XRP on the testnet says; unknown while
    /// it does not run.
    #[must_use]
    pub fn xrpl_owner_payment(&self, id: Name) -> OwnerPaymentState {
        self.book_payment(self.founding_book(), id)
    }

    /// What became of the owner's payment `id` out of the owner book
    /// `book`, as it says; unknown while it does not run.
    fn book_payment(&self, book: Option<(Name, LedgerId)>, id: Name) -> OwnerPaymentState {
        book.and_then(|(book, _)| self.schemes.get(&book))
            .filter(|active| active.failed.is_none())
            .map_or(OwnerPaymentState::Unknown, |active| {
                active.scheme.owner_payment(id)
            })
    }
}

/// The owner's moves and view. Each call names the hub by its ID, or none
/// while the node is in one hub only.
impl Core {
    /// The node's account in each of its hubs and its funding, in the
    /// order of the schemes over them, for those whose scheme runs.
    #[must_use]
    pub fn hub_statuses(&self) -> Vec<HubStatus> {
        self.hub_schemes()
            .into_iter()
            .filter_map(|scheme| self.hub_status_of(scheme))
            .collect()
    }

    /// The node's account in its first hub and its funding, while the
    /// scheme over it runs: the hub of a node in one.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn hub_status(&self) -> Option<HubStatus> {
        self.hub_statuses().into_iter().next()
    }

    /// The node's account in the hub of `scheme` and its funding, while
    /// the scheme runs.
    fn hub_status_of(&self, scheme: Name) -> Option<HubStatus> {
        let account = self.hub_account(scheme)?;
        let peers = self.hub_peerings(scheme);
        let (floor, ceiling) = self.hub_limits(peers.len());
        let number = account.number();
        let view = account.view().map(|(_, view)| *view);
        let units = dassie_hub::member::units;
        let hold = |other: dassie_hub::operation::AccountNumber, amount: u64| HubHold {
            peer: peers
                .iter()
                .find(|(_, account)| *account == other.0)
                .map(|(node, _)| *node),
            account: other.0,
            amount: units(amount),
        };
        let (mut paying, mut taking) = (Vec::new(), Vec::new());
        for (_, mirror) in account.holds() {
            if Some(mirror.payer) == number {
                paying.push(hold(mirror.payee, mirror.amount));
            } else {
                taking.push(hold(mirror.payer, mirror.amount));
            }
        }
        let now = self.now_ms;
        // A scheme that started in this step has its moves read at the
        // step's end: until then they are the defaults.
        let starting;
        let funding = match self.funding.get(&scheme) {
            Some(funding) => funding,
            None => {
                starting = Funding::new(now, self.settle.interval_ms);
                &starting
            }
        };
        Some(HubStatus {
            hub_id: *account.hub().0.as_bytes(),
            scheme,
            account: number.map(|n| n.0),
            status: account.status(),
            balance: units(view.map_or(0, |v| v.balance)),
            reserve: units(view.map_or(0, |v| v.reserve)),
            deposited: units(view.map_or(0, |v| v.deposited)),
            withdrawn: units(view.map_or(0, |v| v.withdrawn)),
            returned: units(view.map_or(0, |v| v.returned)),
            paying,
            taking,
            floor,
            ceiling,
            automatic: funding.automatic,
            paused_until_ms: (funding.paused_until_ms > now).then_some(funding.paused_until_ms),
            moving: funding.moving,
            late: funding.late(now),
            given_up: funding.given_up,
            validators: account.answering(),
            frozen_since_ms: account.frozen(),
            escrow_held: account.escrow_held(),
        })
    }

    /// The owner deposits `amount` (internal units, whole units of the hub's
    /// asset) from the owner book into the node's first hub:
    /// [`hub_deposit_to`](Self::hub_deposit_to) for a node in one hub.
    ///
    /// # Errors
    ///
    /// As [`hub_deposit_to`](Self::hub_deposit_to).
    #[cfg(any(test, feature = "testing"))]
    pub fn hub_deposit(&mut self, amount: u64) -> Result<HubStatus, EngineError> {
        self.hub_deposit_to(None, amount)
    }

    /// The owner deposits `amount` (internal units, whole units of the hub
    /// asset's smallest, drops of XRP or millionths of a token) from the
    /// owner book of the hub's asset into the hub `hub` (its ID, or none for
    /// the node's only hub), opening the account if the hub has none for
    /// us. Pauses that hub's automatic moves.
    ///
    /// # Errors
    ///
    /// No such hub, the hub unavailable, no owner book synced, a move on
    /// its way, too little to open the account, or more than the owner's
    /// funds there.
    pub fn hub_deposit_to(
        &mut self,
        hub: Option<[u8; 32]>,
        amount: u64,
    ) -> Result<HubStatus, EngineError> {
        let scheme = self.chosen_hub(hub)?;
        let standing = self.owner_standing(scheme)?;
        let drops = amount / UNITS_PER_DROP;
        let spendable = standing.xrpl.ok_or_else(|| {
            EngineError::Precondition(
                "the owner book of the hub's asset is not running synced".into(),
            )
        })?;
        if drops == 0 {
            return Err(EngineError::Invalid(format!(
                "the amount is less than {}, the least the hub counts",
                in_asset(scheme, 1)
            )));
        }
        if drops > spendable {
            return Err(EngineError::Refused(
                "more than the owner's funds in the owner book of the hub's asset, with its fee"
                    .into(),
            ));
        }
        let deposited = match standing.held {
            Some(held) => held.deposited,
            None if standing.unfunded && drops >= standing.opening => 0,
            None if standing.unfunded => {
                return Err(EngineError::Invalid(format!(
                    "opening the account takes at least {}",
                    in_asset(scheme, standing.opening)
                )));
            }
            None => {
                return Err(EngineError::Precondition(
                    "the hub's validators have not said yet whether the account exists".into(),
                ));
            }
        };
        self.owner_move(scheme, Planned::deposit(drops, deposited, true), &standing)
    }

    /// The owner withdraws `amount` (internal units, whole units of the hub
    /// asset's smallest) from the node's first hub:
    /// [`hub_withdraw_from`](Self::hub_withdraw_from) for a node in one hub.
    ///
    /// # Errors
    ///
    /// As [`hub_withdraw_from`](Self::hub_withdraw_from).
    #[cfg(any(test, feature = "testing"))]
    pub fn hub_withdraw(&mut self, amount: u64) -> Result<HubStatus, EngineError> {
        self.hub_withdraw_from(None, amount)
    }

    /// The owner withdraws `amount` (internal units, whole units of the hub
    /// asset's smallest) from the hub `hub` (its ID, or none for the node's
    /// only hub) to the owner book of its asset on the XRPL account. Pauses
    /// that hub's automatic moves. From a token hub, only once the escrow
    /// has its gas: the call that finds it unchecked, or short, starts the
    /// check or the top-up, and is refused until it is done.
    ///
    /// # Errors
    ///
    /// No such hub, the hub unavailable, no account in it, a move on its
    /// way, less than the hub pays out, more than the account can spend
    /// with the fees, or a token hub's gas not yet checked.
    pub fn hub_withdraw_from(
        &mut self,
        hub: Option<[u8; 32]>,
        amount: u64,
    ) -> Result<HubStatus, EngineError> {
        let scheme = self.chosen_hub(hub)?;
        let standing = self.owner_standing(scheme)?;
        let held = standing.held.ok_or_else(|| {
            EngineError::Precondition("the node has no account in the hub yet".into())
        })?;
        let drops = amount / UNITS_PER_DROP;
        if drops < standing.least_payout {
            return Err(EngineError::Invalid(format!(
                "the hub pays out at least {}",
                in_asset(scheme, standing.least_payout)
            )));
        }
        if drops.saturating_add(standing.withdrawal_fees) > held.balance {
            return Err(EngineError::Refused(
                "more than the hub account can spend, with the fees".into(),
            ));
        }
        if drops.saturating_add(standing.withdrawal_fees) > standing.hub_owner {
            return Err(EngineError::Refused(
                "more than the owner's funds in the hub: the rest is owed to peers".into(),
            ));
        }
        if self.token_hub(scheme) && !self.check_gas(scheme)? {
            let now = self.now_ms;
            let backing_off = self
                .funding
                .get(&scheme)
                .and_then(|funding| funding.gas_backing_off(now));
            return Err(EngineError::Exhausted(match backing_off {
                Some(until) => format!(
                    "the hub's escrow pays its fees in XRP, and the node's top-up of it \
                     failed: it checks again in {} s",
                    until.saturating_sub(now).div_ceil(1_000)
                ),
                None => "the hub's escrow pays its fees in XRP: the node checks it holds \
                         enough, and tops it up first if not; try again shortly"
                    .into(),
            }));
        }
        let planned = Planned::withdraw(drops, standing.ledger_fee, held.withdrawn, true);
        self.owner_move(scheme, planned, &standing)
    }

    /// Turns the automatic moves of the hub `hub` (its ID, or none for the
    /// node's only hub) on or off; the database keeps it. Its status.
    ///
    /// # Errors
    ///
    /// No such hub.
    pub fn set_hub_automatic(
        &mut self,
        hub: Option<[u8; 32]>,
        automatic: bool,
    ) -> Result<HubStatus, EngineError> {
        let scheme = self.chosen_hub(hub)?;
        self.follow_hubs();
        let funding = self
            .funding
            .get_mut(&scheme)
            .ok_or_else(|| EngineError::Internal("a hub without its funding".into()))?;
        funding.automatic = automatic;
        let acted = automatic && funding.given_up.is_some();
        if acted {
            funding.owner_acted();
        }
        self.batch.hub_automatic.insert(scheme, automatic);
        if acted {
            self.keep_funding(scheme);
        }
        self.hub_status_of(scheme)
            .ok_or_else(|| EngineError::Internal("the hub scheme vanished".into()))
    }

    /// The owner gives up the move on its way of the hub `hub` (its ID, or
    /// none for the node's only hub), once it is late: the node forgets it
    /// and books nothing. Each side books its half when it sees it, so if
    /// the hub credits a given-up deposit later, or the XRPL account sees a
    /// given-up withdrawal arrive, it is booked then, once. That hub's own
    /// moves wait until the owner turns them on again or moves funds, so
    /// the node never pays again by itself what may yet arrive.
    ///
    /// # Errors
    ///
    /// No such hub, or no move is on its way, or it is not late.
    pub fn hub_give_up(&mut self, hub: Option<[u8; 32]>) -> Result<HubStatus, EngineError> {
        let scheme = self.chosen_hub(hub)?;
        self.owner_standing_any(scheme)?;
        let now = self.now_ms;
        let given_up = self
            .funding
            .get_mut(&scheme)
            .and_then(|funding| funding.give_up(now));
        if given_up.is_none() {
            return Err(EngineError::Precondition(
                "only a move that is late can be given up".into(),
            ));
        }
        self.keep_funding(scheme);
        self.hub_status_of(scheme)
            .ok_or_else(|| EngineError::Internal("the hub scheme vanished".into()))
    }

    /// What an owner's move over the hub of `scheme` is decided from: the
    /// move on its way followed first, so one that is done no longer
    /// stands in the way.
    fn owner_standing_any(&mut self, scheme: Name) -> Result<Standing, EngineError> {
        self.follow_hubs();
        let standing = self.standing(scheme).ok_or_else(|| {
            EngineError::Precondition("the settlement hub scheme is not running".into())
        })?;
        let now = self.now_ms;
        let funding = self
            .funding
            .get_mut(&scheme)
            .ok_or_else(|| EngineError::Internal("a hub without its funding".into()))?;
        if funding.follow(now, &standing) {
            self.keep_funding(scheme);
        }
        Ok(standing)
    }

    /// [`owner_standing_any`](Self::owner_standing_any), with no move on
    /// its way, one at a time, and the hub available: nothing moves into
    /// or out of a hub its scheme is not in step with, or a frozen token
    /// hub.
    fn owner_standing(&mut self, scheme: Name) -> Result<Standing, EngineError> {
        let standing = self.owner_standing_any(scheme)?;
        if self.hub_move_on_its_way(scheme) {
            return Err(EngineError::Exhausted(
                "a move between the hub and the XRP Ledger is on its way".into(),
            ));
        }
        if standing.unavailable {
            return Err(EngineError::Precondition(
                "the hub is unavailable: frozen, or its scheme is not in step with it".into(),
            ));
        }
        Ok(standing)
    }

    fn owner_move(
        &mut self,
        scheme: Name,
        planned: Planned,
        standing: &Standing,
    ) -> Result<HubStatus, EngineError> {
        self.make_move(scheme, planned, standing)
            .map_err(|error| match error {
                SchemeError::Unavailable => EngineError::Exhausted(
                    "the ledger cannot take the payment now; try again shortly".into(),
                ),
                other => EngineError::Invalid(other.to_string()),
            })?;
        self.hub_status_of(scheme)
            .ok_or_else(|| EngineError::Internal("the hub scheme vanished".into()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn standing(balance: u64) -> Standing {
        Standing {
            held: Some(Held {
                balance,
                exposure: balance,
                deposited: 7,
                withdrawn: 3,
            }),
            floor: 400_000,
            ceiling: 10_000_000,
            least_payout: 1_000_000,
            least_deposit: 100_000,
            xrpl: Some(100_000_000),
            hub_owner: 100_000_000,
            ..Standing::default()
        }
    }

    // spec/areas/hubs.md#in-the-node: what the database keeps of a hub's
    // moves restores them as they were.
    #[test]
    fn a_hubs_moves_restore_as_kept() {
        let mut funding = Funding::new(0, 4_000);
        funding.automatic = false;
        let planned = Planned::deposit(5_000, 7, true);
        funding.start(10, planned, Name::new("pay-1").unwrap());
        funding.given_up = Some((Direction::Withdraw, 9));
        let kept = row(&funding);
        let back = restored(20, 4_000, false, &kept).unwrap();
        assert!(!back.automatic);
        assert_eq!(back.paused_until_ms, funding.paused_until_ms);
        assert_eq!(back.moving, funding.moving);
        assert_eq!(back.given_up, funding.given_up);
        assert_eq!(row(&back), kept);
    }

    // spec/areas/hubs.md#in-the-node: a funding keeps its move on its way,
    // its failure's backoff and the owner's pause in itself, so the core's
    // funding of one hub holds back no other's. Only one scheme over a hub
    // exists so far: that the core routes each hub's calls to its own
    // funding is in its tests of the one hub.
    #[test]
    fn a_hubs_funding_holds_back_no_other() {
        let xrp = Name::new("hub-xrpl-testnet-xrp").unwrap();
        let dsusd = Name::new("hub-xrpl-testnet-dsusd").unwrap();
        let mut fundings: BTreeMap<Name, Funding> = [xrp, dsusd]
            .into_iter()
            .map(|scheme| (scheme, Funding::new(0, 4_000)))
            .collect();
        let low = standing(100_000);
        let planned = fundings[&xrp].due(1, &low).unwrap();
        fundings
            .get_mut(&xrp)
            .unwrap()
            .start(1, planned, Name::new("pay-1").unwrap());
        assert_eq!(fundings[&xrp].due(2, &low), None, "one on its way");
        assert!(fundings[&dsusd].due(2, &low).is_some(), "the other's own");
        fundings.get_mut(&dsusd).unwrap().failed(2);
        assert_eq!(fundings[&dsusd].due(3, &low), None, "backing off");
        assert!(fundings[&xrp].moving.is_some());
        let owner = Planned::withdraw(1_000_000, 100, 3, true);
        fundings
            .get_mut(&dsusd)
            .unwrap()
            .start(4, owner, Name::new("out-1").unwrap());
        assert!(fundings[&dsusd].paused_until_ms > 4);
        assert_eq!(fundings[&xrp].paused_until_ms, 0, "a pause is per hub");
    }
}

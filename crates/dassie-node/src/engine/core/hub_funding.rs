//! Moving the owner's funds between the node's XRPL testnet account and its
//! account in a settlement hub (spec/areas/hubs.md#in-the-node,
//! spec/designs/settlement-hubs.md#11-choosing-a-hub).
//!
//! - **Between a floor and a ceiling.** What the hub account can spend
//!   stays above a floor of what its peerings over the hub need, and what
//!   the node keeps in the hub (the balance, its reserves and the holds
//!   toward it) below a ceiling, both counted in largest packets
//!   (`policy::hub_floor`, `policy::hub_ceiling`). Below the floor, the node
//!   deposits from its XRPL account up to the midpoint, never past the
//!   ceiling; above the ceiling, it withdraws down to the midpoint, never
//!   below the floor. Holds toward us grow by our peers' settlements alone,
//!   so only they can take the hub past the ceiling, until they are taken
//!   and withdrawn.
//! - **Joining by deposit.** While the hub has no account for the node's
//!   key, the first deposit opens it: tag 0, the account key as the memo,
//!   sized to the account reserve, the floor and the fees of its first
//!   operations.
//! - **One move at a time.** A deposit is on its way until the hub credited
//!   it, a withdrawal until the hub executed it. A move that failed, or a
//!   deposit the hub did not credit in time, waits a backoff, doubling, before
//!   the next. A move is not kept across a restart, so after a start the
//!   node waits a backoff before it moves anything by itself: a deposit the
//!   XRP Ledger applied may not be credited yet.
//! - **The owner's moves** go the same way, and pause the automatic ones
//!   for a while, so the node does not undo them at once. The owner may
//!   turn the automatic moves off, which the database keeps.
//!
//! Each side books its half when it sees it happen: the XRP scheme the
//! owner's withdrawal once its Payment applied, the hub scheme the owner's
//! deposit once the hub credited it, and the other way round
//! (spec/areas/hubs.md#the-members-side).

use dassie_hub::member::{HubAccount, UNITS_PER_DROP};
use dassie_ledger::{LedgerId, Name};
use dassie_settlement::{
    ExternalAsset, LedgerStatus, OwnerPayment, OwnerPaymentState, SchemeError, policy,
};

use super::{Core, EngineError};

/// How long an owner's own move pauses the automatic ones.
pub const OWNER_PAUSE_MS: u64 = 3_600_000;
/// The first wait after a move failed, and after a start.
pub const FIRST_BACKOFF_MS: u64 = 60_000;
/// The longest wait after moves failed in a row.
const MAX_BACKOFF_MS: u64 = 3_600_000;
/// How long a deposit the XRP Ledger applied may wait for the hub to credit
/// it before the node takes it for one that went wrong: the validators
/// attest it within seconds of a validated ledger.
const CREDIT_WAIT_MS: u64 = 600_000;
/// The most XRP Ledger fee a deposit is sized to leave room for, in drops.
const XRPL_FEE_DROPS: u64 = dassie_xrpl::account::MAX_FEE_DROPS;

const HUB: Name = dassie_ledger::name!(dassie_hub::member::SCHEME_ID);
const XRPL: Name = dassie_ledger::name!(dassie_xrpl::scheme::SCHEME_ID);

/// Which way a move goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From the XRPL account into the hub.
    Deposit,
    /// From the hub to the XRPL account.
    Withdraw,
}

/// A move on its way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    /// Which way.
    pub direction: Direction,
    /// How much, in drops.
    pub drops: u64,
    /// Whether the owner asked for it.
    pub owner: bool,
    /// What the hub had credited the account by deposits when it was
    /// made; `None` while the hub had no account for us.
    deposited: Option<u64>,
    /// When the XRP Ledger applied a deposit: from then it waits for the
    /// hub.
    applied_ms: Option<u64>,
}

/// The node's hub account, as last agreed, in drops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// What it can spend.
    pub balance: u64,
    /// What the node keeps in the hub: the balance, the reserves and the
    /// holds toward it.
    pub exposure: u64,
    /// What deposits credited it.
    pub deposited: u64,
}

/// What the moves are decided from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    /// The hub account, once agreed.
    pub held: Option<Held>,
    /// Whether the hub says it has no account for the node's key.
    pub unfunded: bool,
    /// The floor and the ceiling, in drops.
    pub floor: u64,
    /// The ceiling.
    pub ceiling: u64,
    /// What a deposit that opens the account pays: its reserve, the floor
    /// and the fees of its first operations.
    pub opening: u64,
    /// The least the hub pays out.
    pub least_payout: u64,
    /// What a withdrawal costs beyond its amount: the operation's fee and
    /// the ledger's.
    pub withdrawal_fees: u64,
    /// The least deposit worth its fees: a largest packet.
    pub least_deposit: u64,
    /// The owner's funds on the XRPL account a deposit may take, in drops;
    /// `None` while its scheme does not run synced.
    pub xrpl: Option<u64>,
    /// What became of the owner's last payment out of each side.
    pub xrpl_paying: OwnerPaymentState,
    /// The hub side's.
    pub hub_paying: OwnerPaymentState,
}

/// The node's moves between its XRPL account and its hub account.
#[derive(Debug)]
pub struct Funding {
    /// Whether the node moves funds by itself: the owner's choice.
    pub automatic: bool,
    /// Until when an owner's move pauses the automatic ones.
    pub paused_until_ms: u64,
    /// The move on its way.
    pub moving: Option<Move>,
    /// No automatic move before this.
    not_before_ms: u64,
    /// The wait after the next failure.
    backoff_ms: u64,
    /// The next check.
    pub at_ms: u64,
    interval_ms: u64,
}

impl Funding {
    /// Nothing on its way, and no automatic move for a backoff after the
    /// start; checked as often as settlements are.
    pub const fn new(now_ms: u64, interval_ms: u64) -> Self {
        Self {
            automatic: true,
            paused_until_ms: 0,
            moving: None,
            not_before_ms: now_ms.saturating_add(FIRST_BACKOFF_MS),
            backoff_ms: FIRST_BACKOFF_MS,
            at_ms: now_ms,
            interval_ms,
        }
    }

    /// Follows the move on its way: done once the side it goes to shows it,
    /// failed if the side it leaves says so, or a deposit applied and not
    /// credited in time.
    pub fn follow(&mut self, now_ms: u64, standing: &Standing) {
        let Some(moving) = &mut self.moving else {
            return;
        };
        let done = match moving.direction {
            Direction::Deposit => {
                if moving.applied_ms.is_none() && standing.xrpl_paying == OwnerPaymentState::Applied
                {
                    moving.applied_ms = Some(now_ms);
                }
                let credited = standing
                    .held
                    .is_some_and(|held| moving.deposited.is_none_or(|d| held.deposited > d));
                let late = moving
                    .applied_ms
                    .is_some_and(|at| now_ms >= at.saturating_add(CREDIT_WAIT_MS));
                if credited {
                    Some(true)
                } else if standing.xrpl_paying == OwnerPaymentState::Failed || late {
                    Some(false)
                } else {
                    None
                }
            }
            Direction::Withdraw => match standing.hub_paying {
                OwnerPaymentState::Applied => Some(true),
                OwnerPaymentState::Failed | OwnerPaymentState::Idle => Some(false),
                OwnerPaymentState::InFlight => None,
            },
        };
        match done {
            Some(true) => self.succeeded(),
            Some(false) => self.failed(now_ms),
            None => {}
        }
    }

    fn succeeded(&mut self) {
        self.moving = None;
        self.backoff_ms = FIRST_BACKOFF_MS;
    }

    /// A move that failed, or could not be made: the next waits.
    pub fn failed(&mut self, now_ms: u64) {
        self.moving = None;
        self.not_before_ms = now_ms.saturating_add(self.backoff_ms);
        self.backoff_ms = self.backoff_ms.saturating_mul(2).min(MAX_BACKOFF_MS);
    }

    /// The automatic move due now, if any: none while one is on its way,
    /// the owner turned them off or paused them, or a failure's backoff
    /// lasts.
    pub fn due(&self, now_ms: u64, standing: &Standing) -> Option<Move> {
        let waiting = self.moving.is_some()
            || !self.automatic
            || now_ms < self.paused_until_ms
            || now_ms < self.not_before_ms;
        if waiting {
            return None;
        }
        let Some(held) = standing.held else {
            return opening(standing);
        };
        let middle = standing.floor.midpoint(standing.ceiling);
        if held.balance < standing.floor {
            let drops = (middle - held.balance)
                .min(standing.ceiling.saturating_sub(held.exposure))
                .min(standing.xrpl?);
            return (drops >= standing.least_deposit)
                .then(|| Move::deposit(drops, Some(held.deposited), false));
        }
        if held.exposure > standing.ceiling {
            let spare = held
                .balance
                .saturating_sub(standing.floor)
                .saturating_sub(standing.withdrawal_fees);
            let drops = (held.exposure - middle).min(spare);
            return (drops >= standing.least_payout).then(|| Move::withdraw(drops, false));
        }
        None
    }

    /// Starts `moving`; an owner's pauses the automatic moves.
    pub fn start(&mut self, now_ms: u64, moving: Move) {
        assert!(self.moving.is_none(), "one move at a time");
        if moving.owner {
            self.paused_until_ms = now_ms.saturating_add(OWNER_PAUSE_MS);
        }
        self.moving = Some(moving);
    }

    /// Whether a check is due, and when the next is.
    pub fn check_due(&mut self, now_ms: u64) -> bool {
        if now_ms < self.at_ms {
            return false;
        }
        self.at_ms = now_ms.saturating_add(self.interval_ms);
        true
    }
}

/// The deposit that opens the account, once the hub says it has none for
/// us, if the XRPL account has it.
fn opening(standing: &Standing) -> Option<Move> {
    let enough = standing.unfunded && standing.xrpl? >= standing.opening;
    enough.then(|| Move::deposit(standing.opening, None, false))
}

impl Move {
    /// A deposit of `drops`, made when the hub had credited `deposited`.
    pub const fn deposit(drops: u64, deposited: Option<u64>, owner: bool) -> Self {
        Self {
            direction: Direction::Deposit,
            drops,
            owner,
            deposited,
            applied_ms: None,
        }
    }

    /// A withdrawal of `drops`.
    pub const fn withdraw(drops: u64, owner: bool) -> Self {
        Self {
            direction: Direction::Withdraw,
            drops,
            owner,
            deposited: None,
            applied_ms: None,
        }
    }
}

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

/// The node's hub account and its funding, as the owner sees them.
/// Amounts are in internal units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubStatus {
    /// The hub's ID.
    pub hub_id: [u8; 32],
    /// Our account's number, once the hub has said it.
    pub account: Option<u32>,
    /// How the account stands with the hub.
    pub status: LedgerStatus,
    /// What it can spend, as last agreed.
    pub balance: u64,
    /// Its account reserve.
    pub reserve: u64,
    /// What deposits credited it, and what withdrawals took from it.
    pub deposited: u64,
    /// What withdrawals took.
    pub withdrawn: u64,
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
    /// How many validators the hub has, and how many answer.
    pub validators: (usize, usize),
}

/// Internal units in drops, rounded up: a floor is never short.
const fn drops_up(units: u64) -> u64 {
    units.div_ceil(UNITS_PER_DROP)
}

impl Core {
    /// The hub account, while its scheme runs.
    fn hub_account(&self) -> Option<&HubAccount> {
        let active = self.schemes.get(&HUB)?;
        if active.failed.is_some() {
            return None;
        }
        let asset = self.assets.get(&active.asset?)?;
        let any: &dyn core::any::Any = asset.asset.as_ref();
        any.downcast_ref::<HubAccount>()
    }

    /// The hub peerings: each peer's node and its account in the hub.
    fn hub_peerings(&self, account: &HubAccount) -> Vec<(dassie_identity::NodeId, u32)> {
        let hub = account.hub();
        self.peer
            .active_peers()
            .filter(|(_, peering)| peering.scheme.as_str() == HUB.as_str())
            .filter_map(|(node, peering)| {
                let number = dassie_hub::member::peer_account(&peering.state, &hub)?;
                Some((*node, number.0))
            })
            .collect()
    }

    /// The floor and the ceiling, in internal units, for `peerings`.
    fn hub_limits(&self, peerings: usize) -> (u64, u64) {
        let max_packet = self.settle.max_packet;
        let in_flight = self.connector.config().max_in_flight();
        let peerings = u64::try_from(peerings).unwrap_or(u64::MAX);
        (
            policy::hub_floor(max_packet, in_flight, peerings),
            policy::hub_ceiling(max_packet),
        )
    }

    /// What a move is decided from, while the hub scheme runs.
    fn standing(&self) -> Option<Standing> {
        let account = self.hub_account()?;
        let terms = account.terms();
        let fees = terms.fees;
        let peerings = self.hub_peerings(account).len();
        let (floor, ceiling) = self.hub_limits(peerings);
        let (floor, ceiling) = (drops_up(floor), ceiling / UNITS_PER_DROP);
        let held = account
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
                let hold_reserves = fees
                    .hold_reserve
                    .saturating_mul(u64::try_from(paying).unwrap_or(u64::MAX));
                Held {
                    balance: view.balance,
                    exposure: view
                        .balance
                        .saturating_add(view.reserve)
                        .saturating_add(hold_reserves)
                        .saturating_add(toward_us),
                    deposited: view.deposited,
                }
            });
        Some(Standing {
            held,
            unfunded: matches!(
                self.assets
                    .get(&dassie_hub::member::FIRST)
                    .map(|a| a.asset.status()),
                Some(LedgerStatus::Unfunded)
            ),
            floor,
            ceiling,
            opening: fees
                .account_reserve
                .saturating_add(floor)
                .saturating_add(fees.starting_balance)
                .max(terms.new_account_minimum()),
            least_payout: fees.least_payout,
            withdrawal_fees: fees.operation.saturating_add(fees.ledger),
            least_deposit: drops_up(self.settle.max_packet),
            xrpl: self.xrpl_spendable(),
            xrpl_paying: self.paying(XRPL),
            hub_paying: self.paying(HUB),
        })
    }

    /// What the owner may move out of the XRPL account, in drops, after a
    /// fee; `None` while its scheme does not run, or its account is not up
    /// to date with its ledger and could sign nothing.
    fn xrpl_spendable(&self) -> Option<u64> {
        let active = self.schemes.get(&XRPL)?;
        let synced = self
            .assets
            .get(&active.asset?)
            .is_some_and(|asset| asset.asset.status() == LedgerStatus::Synced);
        if active.failed.is_some() || !active.scheme.has_synced() || !synced {
            return None;
        }
        let funds = self.owner_available(LedgerId::XrplTestnetXrp) / UNITS_PER_DROP;
        Some(funds.saturating_sub(XRPL_FEE_DROPS))
    }

    fn paying(&self, scheme: Name) -> OwnerPaymentState {
        self.schemes
            .get(&scheme)
            .map_or(OwnerPaymentState::Idle, |active| {
                active.scheme.owner_payment()
            })
    }

    /// Follows the move on its way, and makes the next automatic one when
    /// it is due, at each funding check.
    pub(super) fn fund_hub(&mut self) {
        let now = self.now_ms;
        if !self.funding.check_due(now) {
            return;
        }
        let Some(standing) = self.standing() else {
            return;
        };
        self.funding.follow(now, &standing);
        if let Some(moving) = self.funding.due(now, &standing)
            && self.make_move(moving, &standing).is_err()
        {
            self.funding.failed(now);
        }
    }

    /// When the next funding check is, while the hub scheme runs.
    pub(super) fn funding_deadline(&self) -> Option<u64> {
        self.hub_account().map(|_| self.funding.at_ms)
    }

    /// Makes `moving`: pays out of the side it leaves.
    fn make_move(&mut self, moving: Move, standing: &Standing) -> Result<(), SchemeError> {
        let account = self.hub_account().ok_or(SchemeError::Unavailable)?;
        let (scheme, payment) = match moving.direction {
            Direction::Deposit => {
                let escrow = dassie_xrpl::AccountId(account.terms().escrow);
                let (tag, memo) = match (standing.held, account.number()) {
                    (Some(_), Some(number)) => (number.0, None),
                    _ => (0, Some(account.key().as_bytes().to_vec())),
                };
                let payment = OwnerPayment {
                    destination: escrow.to_address(),
                    tag: Some(tag),
                    memo,
                    amount: moving.drops.saturating_mul(UNITS_PER_DROP),
                };
                (XRPL, payment)
            }
            Direction::Withdraw => {
                let own = super::schemes::own_xrpl_account(&self.key)?;
                let payment = OwnerPayment {
                    destination: own.to_address(),
                    tag: None,
                    memo: None,
                    amount: moving.drops.saturating_mul(UNITS_PER_DROP),
                };
                (HUB, payment)
            }
        };
        self.pay_owner(scheme, &payment)?;
        self.funding.start(self.now_ms, moving);
        Ok(())
    }

    /// Has `scheme` pay `payment` out of the owner's funds, and carries out
    /// what its asset asks.
    fn pay_owner(&mut self, scheme: Name, payment: &OwnerPayment) -> Result<(), SchemeError> {
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

/// The owner's moves and view.
impl Core {
    /// The node's hub account and its funding, while the hub scheme runs.
    pub fn hub_status(&self) -> Option<HubStatus> {
        let account = self.hub_account()?;
        let peers = self.hub_peerings(account);
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
        let mut paying = Vec::new();
        let mut taking = Vec::new();
        for (_, mirror) in account.holds() {
            if Some(mirror.payer) == number {
                paying.push(hold(mirror.payee, mirror.amount));
            } else {
                taking.push(hold(mirror.payer, mirror.amount));
            }
        }
        let now = self.now_ms;
        Some(HubStatus {
            hub_id: *account.hub().0.as_bytes(),
            account: number.map(|n| n.0),
            status: account.status(),
            balance: units(view.map_or(0, |v| v.balance)),
            reserve: units(view.map_or(0, |v| v.reserve)),
            deposited: units(view.map_or(0, |v| v.deposited)),
            withdrawn: units(view.map_or(0, |v| v.withdrawn)),
            paying,
            taking,
            floor,
            ceiling,
            automatic: self.funding.automatic,
            paused_until_ms: (self.funding.paused_until_ms > now)
                .then_some(self.funding.paused_until_ms),
            moving: self.funding.moving,
            validators: account.answering(),
        })
    }

    /// The owner deposits `amount` (internal units, whole drops) from the
    /// XRPL account into the hub, opening the account if the hub has none
    /// for us. Pauses the automatic moves.
    ///
    /// # Errors
    ///
    /// No hub scheme, no XRP scheme synced, a move on its way, too little
    /// to open the account, or more than the owner's funds there.
    pub fn hub_deposit(&mut self, amount: u64) -> Result<HubStatus, EngineError> {
        let standing = self.owner_standing()?;
        let drops = amount / UNITS_PER_DROP;
        let spendable = standing.xrpl.ok_or_else(|| {
            EngineError::Precondition("the XRP Ledger testnet scheme is not running synced".into())
        })?;
        if drops == 0 {
            return Err(EngineError::Invalid(
                "the amount is less than a drop".into(),
            ));
        }
        if drops > spendable {
            return Err(EngineError::Refused(
                "more than the owner's funds on the XRP Ledger, with its fee".into(),
            ));
        }
        let deposited = match standing.held {
            Some(held) => Some(held.deposited),
            None if standing.unfunded => None,
            None => {
                return Err(EngineError::Precondition(
                    "the hub's validators have not said yet whether the account exists".into(),
                ));
            }
        };
        if deposited.is_none() && drops < standing.opening {
            return Err(EngineError::Invalid(format!(
                "opening the account takes at least {} drops",
                standing.opening
            )));
        }
        self.owner_move(Move::deposit(drops, deposited, true), &standing)
    }

    /// The owner withdraws `amount` (internal units, whole drops) from the
    /// hub to the XRPL account. Pauses the automatic moves.
    ///
    /// # Errors
    ///
    /// No hub account, a move on its way, less than the hub pays out, or
    /// more than the account can spend with the fees.
    pub fn hub_withdraw(&mut self, amount: u64) -> Result<HubStatus, EngineError> {
        let standing = self.owner_standing()?;
        let held = standing.held.ok_or_else(|| {
            EngineError::Precondition("the node has no account in the hub yet".into())
        })?;
        let drops = amount / UNITS_PER_DROP;
        if drops < standing.least_payout {
            return Err(EngineError::Invalid(format!(
                "the hub pays out at least {} drops",
                standing.least_payout
            )));
        }
        if drops.saturating_add(standing.withdrawal_fees) > held.balance {
            return Err(EngineError::Refused(
                "more than the hub account can spend, with the fees".into(),
            ));
        }
        self.owner_move(Move::withdraw(drops, true), &standing)
    }

    /// Turns the automatic moves on or off; the database keeps it.
    ///
    /// # Errors
    ///
    /// No hub scheme runs.
    pub fn set_hub_automatic(&mut self, automatic: bool) -> Result<(), EngineError> {
        if self.hub_account().is_none() {
            return Err(EngineError::Precondition(
                "the settlement hub scheme is not running".into(),
            ));
        }
        self.funding.automatic = automatic;
        self.batch.hub_automatic = Some(automatic);
        Ok(())
    }

    /// What an owner's move is decided from: the move on its way followed
    /// first, so one that is done no longer stands in the way.
    fn owner_standing(&mut self) -> Result<Standing, EngineError> {
        let standing = self.standing().ok_or_else(|| {
            EngineError::Precondition("the settlement hub scheme is not running".into())
        })?;
        self.funding.follow(self.now_ms, &standing);
        if self.funding.moving.is_some() {
            return Err(EngineError::Exhausted(
                "a move between the hub and the XRP Ledger is on its way".into(),
            ));
        }
        Ok(standing)
    }

    fn owner_move(&mut self, moving: Move, standing: &Standing) -> Result<HubStatus, EngineError> {
        self.make_move(moving, standing)
            .map_err(|error| match error {
                SchemeError::Unavailable => EngineError::Exhausted(
                    "the ledger cannot take the payment now; try again shortly".into(),
                ),
                other => EngineError::Invalid(other.to_string()),
            })?;
        self.hub_status()
            .ok_or_else(|| EngineError::Internal("the hub scheme vanished".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hub account with a floor of 400 000 drops and a ceiling of
    /// 10 000 000, and plenty on the XRPL account.
    fn standing(balance: u64, exposure: u64) -> Standing {
        Standing {
            held: Some(Held {
                balance,
                exposure,
                deposited: 7,
            }),
            unfunded: false,
            floor: 400_000,
            ceiling: 10_000_000,
            opening: 2_401_000,
            least_payout: 1_000_000,
            withdrawal_fees: 110,
            least_deposit: 100_000,
            xrpl: Some(100_000_000),
            xrpl_paying: OwnerPaymentState::Idle,
            hub_paying: OwnerPaymentState::Idle,
        }
    }

    fn ready() -> Funding {
        let mut funding = Funding::new(0, 4_000);
        funding.not_before_ms = 0;
        funding
    }

    // Below the floor, a deposit up to the midpoint; above the ceiling, a
    // withdrawal down to it; in between, nothing.
    #[test]
    fn moves_go_back_to_the_midpoint() {
        let funding = ready();
        let middle = 5_200_000;
        let low = funding.due(1, &standing(100_000, 2_100_000)).unwrap();
        assert_eq!(low.direction, Direction::Deposit);
        assert_eq!(low.drops, middle - 100_000);
        let high = funding.due(1, &standing(12_000_000, 14_000_000)).unwrap();
        assert_eq!(high.direction, Direction::Withdraw);
        assert_eq!(high.drops, 14_000_000 - middle);
        assert_eq!(funding.due(1, &standing(3_000_000, 5_000_000)), None);
    }

    // A deposit never takes the hub past its ceiling, and a withdrawal never
    // the balance below the floor: holds toward us count toward the
    // ceiling, and only peers' settlements grow them.
    #[test]
    fn the_ceiling_and_the_floor_bound_each_move() {
        let funding = ready();
        let low = funding.due(1, &standing(100_000, 9_000_000)).unwrap();
        assert_eq!(low.drops, 1_000_000);
        assert_eq!(funding.due(1, &standing(100_000, 9_950_000)), None);
        let held = standing(1_500_000, 30_000_000);
        let high = funding.due(1, &held).unwrap();
        assert_eq!(high.drops, 1_500_000 - 400_000 - 110);
        assert_eq!(
            funding.due(1, &standing(600_000, 30_000_000)),
            None,
            "less than the hub pays out"
        );
    }

    // The first deposit opens the account once the hub says it has none.
    #[test]
    fn the_first_deposit_opens_the_account() {
        let funding = ready();
        let mut none = standing(0, 0);
        none.held = None;
        assert_eq!(funding.due(1, &none), None, "not before the hub says");
        none.unfunded = true;
        let open = funding.due(1, &none).unwrap();
        assert_eq!(
            (open.direction, open.drops),
            (Direction::Deposit, 2_401_000)
        );
        assert_eq!(open.deposited, None);
        none.xrpl = Some(2_000_000);
        assert_eq!(funding.due(1, &none), None, "too little to open it");
    }

    // One at a time: a deposit is done once the hub credited it, and one
    // that failed, or was never credited, waits a backoff that doubles.
    #[test]
    fn one_move_at_a_time_and_failures_back_off() {
        let mut funding = ready();
        let low = standing(100_000, 2_000_000);
        let moving = funding.due(1, &low).unwrap();
        funding.start(1, moving);
        assert_eq!(funding.due(2, &low), None, "one on its way");
        let mut applied = low;
        applied.xrpl_paying = OwnerPaymentState::Applied;
        funding.follow(10, &applied);
        assert!(funding.moving.is_some(), "applied, not credited yet");
        funding.follow(10 + CREDIT_WAIT_MS, &applied);
        assert!(funding.moving.is_none());
        assert_eq!(funding.due(11 + CREDIT_WAIT_MS, &low), None, "backing off");
        let after = 10 + CREDIT_WAIT_MS + FIRST_BACKOFF_MS;
        let again = funding.due(after, &low).unwrap();
        funding.start(after, again);
        let mut failed = low;
        failed.xrpl_paying = OwnerPaymentState::Failed;
        funding.follow(after, &failed);
        assert_eq!(
            funding.due(after + FIRST_BACKOFF_MS, &low),
            None,
            "the backoff doubled"
        );
        let later = after + 2 * FIRST_BACKOFF_MS;
        let third = funding.due(later, &low).unwrap();
        funding.start(later, third);
        let mut credited = low;
        credited.held = Some(Held {
            deposited: 8,
            ..low.held.unwrap()
        });
        funding.follow(later, &credited);
        assert!(funding.moving.is_none());
        assert_eq!(funding.backoff_ms, FIRST_BACKOFF_MS, "a success resets it");
    }

    // An owner's move pauses the automatic ones; turned off, none is made;
    // after a start, none for a backoff.
    #[test]
    fn the_owner_decides_first() {
        let low = standing(100_000, 2_000_000);
        let mut funding = ready();
        funding.start(5, Move::withdraw(1_000_000, true));
        let mut done = low;
        done.hub_paying = OwnerPaymentState::Applied;
        funding.follow(6, &done);
        assert_eq!(funding.due(7, &low), None);
        assert!(funding.due(5 + OWNER_PAUSE_MS, &low).is_some());
        funding.automatic = false;
        assert_eq!(funding.due(5 + OWNER_PAUSE_MS, &low), None);
        let started = Funding::new(1_000, 4_000);
        assert_eq!(started.due(1_000, &low), None);
        assert!(started.due(1_000 + FIRST_BACKOFF_MS, &low).is_some());
    }
}

//! Making the moves between the node's XRPL testnet account and its hub
//! account that `hub_funding.rs` decides on, keeping the one on its way in
//! the database, and the owner's view and moves
//! (spec/areas/hubs.md#in-the-node).

use dassie_hub::member::{HubAccount, UNITS_PER_DROP};
use dassie_ledger::{LedgerId, Name};
use dassie_settlement::{
    ExternalAsset, LedgerStatus, OwnerPayment, OwnerPaymentState, SchemeError, policy,
};

use super::hub_funding::{Direction, Held, Move, Planned, Standing};
use super::{Core, EngineError};
use crate::store::{HubFundingRow, HubMoveRow};

/// The most XRP Ledger fee a deposit is sized to leave room for, in drops.
const XRPL_FEE_DROPS: u64 = dassie_xrpl::account::MAX_FEE_DROPS;

const HUB: Name = dassie_ledger::name!(dassie_hub::member::SCHEME_ID);
const XRPL: Name = dassie_ledger::name!(dassie_xrpl::scheme::SCHEME_ID);

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
    /// The move the owner gave up, which way and how much in drops, while
    /// it holds the node's own moves back: the other side may yet show it.
    pub given_up: Option<(Direction, u64)>,
    /// How many validators the hub has, and how many answer.
    pub validators: (usize, usize),
}

/// Internal units in drops, rounded up: a floor is never short.
const fn drops_up(units: u64) -> u64 {
    units.div_ceil(UNITS_PER_DROP)
}

/// A move as the database keeps it.
fn row(funding: &super::hub_funding::Funding) -> HubFundingRow {
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

impl Core {
    /// Restores the moves' state from the database.
    pub(super) fn load_funding(&mut self) -> Result<(), String> {
        let (automatic, row) = self.store.hub_funding().map_err(|e| e.to_string())?;
        self.funding.automatic = automatic;
        self.funding.paused_until_ms = row.paused_until_ms;
        self.funding.moving = row.moving.as_ref().map(from_row).transpose()?;
        self.funding.given_up = row.given_up.map(|(deposit, drops)| {
            let direction = if deposit {
                Direction::Deposit
            } else {
                Direction::Withdraw
            };
            (direction, drops)
        });
        Ok(())
    }

    /// Keeps the moves' state with this batch.
    fn keep_funding(&mut self) {
        self.batch.hub_funding = Some(row(&self.funding));
    }

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

    /// What a move is decided from, while the hub scheme runs.
    fn standing(&self) -> Option<Standing> {
        let account = self.hub_account()?;
        let terms = account.terms();
        let fees = terms.fees;
        let (floor, ceiling) = self.hub_limits(self.hub_peerings(account).len());
        let (floor, ceiling) = (drops_up(floor), ceiling / UNITS_PER_DROP);
        let unfunded = self
            .assets
            .get(&dassie_hub::member::FIRST)
            .is_some_and(|asset| asset.asset.status() == LedgerStatus::Unfunded);
        Some(Standing {
            held: Self::held(account),
            unfunded,
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
            xrpl: self.xrpl_spendable(),
            hub_owner: self.owner_available(LedgerId::HubXrpTestnetXrp) / UNITS_PER_DROP,
            paying: self.paying(),
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

    /// What became of the payment of the move on its way, as the scheme of
    /// the side it leaves says; unknown while that scheme does not run.
    fn paying(&self) -> OwnerPaymentState {
        let Some(moving) = self.funding.moving else {
            return OwnerPaymentState::Unknown;
        };
        let scheme = match moving.direction {
            Direction::Deposit => XRPL,
            Direction::Withdraw => HUB,
        };
        self.schemes
            .get(&scheme)
            .filter(|active| active.failed.is_none())
            .map_or(OwnerPaymentState::Unknown, |active| {
                active.scheme.owner_payment(moving.payment)
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
        if self.funding.follow(now, &standing) {
            self.keep_funding();
        }
        if let Some(planned) = self.funding.due(now, &standing)
            && self.make_move(planned, &standing).is_err()
        {
            self.funding.failed(now);
        }
    }

    /// When the next funding check is, while the hub scheme runs.
    pub(super) fn funding_deadline(&self) -> Option<u64> {
        self.hub_account().map(|_| self.funding.at_ms)
    }

    /// Makes `planned`: pays out of the side it leaves, and keeps it, with
    /// its payment's ID, in the batch that keeps the payment.
    fn make_move(&mut self, planned: Planned, standing: &Standing) -> Result<(), SchemeError> {
        let (scheme, payment) = self.payment_for(planned, standing)?;
        // The most its fees may cost: the XRP Ledger's fee, or the hub's
        // operation fee and ledger fee.
        let fee_drops = match planned.direction {
            Direction::Deposit => XRPL_FEE_DROPS,
            Direction::Withdraw => standing.withdrawal_fees,
        };
        let fee = fee_drops.saturating_mul(UNITS_PER_DROP);
        self.check_owner_payment(scheme, payment.amount, fee)?;
        let id = self.pay_owner(scheme, &payment)?;
        self.hold_owner_payment(scheme, id, payment.amount, fee);
        self.funding.start(self.now_ms, planned, id);
        self.keep_funding();
        Ok(())
    }

    /// The payment that makes `planned`, and the scheme that pays it.
    fn payment_for(
        &self,
        planned: Planned,
        standing: &Standing,
    ) -> Result<(Name, OwnerPayment), SchemeError> {
        let account = self.hub_account().ok_or(SchemeError::Unavailable)?;
        let amount = planned.drops.saturating_mul(UNITS_PER_DROP);
        Ok(match planned.direction {
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
                    amount,
                };
                (XRPL, payment)
            }
            Direction::Withdraw => {
                let own = super::schemes::own_xrpl_account(&self.key)?;
                let payment = OwnerPayment {
                    destination: own.to_address(),
                    tag: None,
                    memo: None,
                    amount,
                };
                (HUB, payment)
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
    /// `destination`, with no tag: the payment that creates the escrow of a
    /// hub the node founds. Held against the owner's funds while it is on
    /// its way, as any owner's payment. Its ID, to follow it by.
    ///
    /// # Errors
    ///
    /// The XRP scheme does not run synced, the owner's funds there do not
    /// cover it with its fee, or the scheme has an owner's payment on its
    /// way already.
    pub fn pay_from_xrpl(&mut self, destination: &str, drops: u64) -> Result<Name, EngineError> {
        let spendable = self.xrpl_spendable().ok_or_else(|| {
            EngineError::Precondition("the XRP Ledger testnet scheme is not running synced".into())
        })?;
        if drops == 0 || drops > spendable {
            return Err(EngineError::Refused(
                "more than the owner's funds on the XRP Ledger, with its fee".into(),
            ));
        }
        let amount = drops.saturating_mul(UNITS_PER_DROP);
        let payment = OwnerPayment {
            destination: destination.into(),
            tag: None,
            memo: None,
            amount,
        };
        let fee = XRPL_FEE_DROPS.saturating_mul(UNITS_PER_DROP);
        let refused = |error: SchemeError| {
            EngineError::Exhausted(format!("the XRP scheme cannot pay now: {error:?}"))
        };
        self.check_owner_payment(XRPL, amount, fee)
            .map_err(refused)?;
        let id = self.pay_owner(XRPL, &payment).map_err(refused)?;
        self.hold_owner_payment(XRPL, id, amount, fee);
        Ok(id)
    }

    /// The rules of the hub the node is a member of, while its scheme
    /// runs.
    #[must_use]
    pub fn hub_rules(&self) -> Option<dassie_hub::Rules> {
        self.hub_account().map(|account| account.rules().clone())
    }

    /// What became of the owner's payment `id` out of the XRPL testnet
    /// account, as the XRP scheme says; unknown while it does not run.
    #[must_use]
    pub fn xrpl_owner_payment(&self, id: Name) -> OwnerPaymentState {
        self.schemes
            .get(&XRPL)
            .filter(|active| active.failed.is_none())
            .map_or(OwnerPaymentState::Unknown, |active| {
                active.scheme.owner_payment(id)
            })
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
        let (mut paying, mut taking) = (Vec::new(), Vec::new());
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
            returned: units(view.map_or(0, |v| v.returned)),
            paying,
            taking,
            floor,
            ceiling,
            automatic: self.funding.automatic,
            paused_until_ms: (self.funding.paused_until_ms > now)
                .then_some(self.funding.paused_until_ms),
            moving: self.funding.moving,
            late: self.funding.late(now),
            given_up: self.funding.given_up,
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
            Some(held) => held.deposited,
            None if standing.unfunded && drops >= standing.opening => 0,
            None if standing.unfunded => {
                return Err(EngineError::Invalid(format!(
                    "opening the account takes at least {} drops",
                    standing.opening
                )));
            }
            None => {
                return Err(EngineError::Precondition(
                    "the hub's validators have not said yet whether the account exists".into(),
                ));
            }
        };
        self.owner_move(Planned::deposit(drops, deposited, true), &standing)
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
        if drops.saturating_add(standing.withdrawal_fees) > standing.hub_owner {
            return Err(EngineError::Refused(
                "more than the owner's funds in the hub: the rest is owed to peers".into(),
            ));
        }
        let planned = Planned::withdraw(drops, standing.ledger_fee, held.withdrawn, true);
        self.owner_move(planned, &standing)
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
        if automatic && self.funding.given_up.is_some() {
            self.funding.owner_acted();
            self.keep_funding();
        }
        Ok(())
    }

    /// The owner gives up the move on its way, once it is late: the node
    /// forgets it and books nothing. Each side books its half when it sees
    /// it, so if the hub credits a given-up deposit later, or the XRPL
    /// account sees a given-up withdrawal arrive, it is booked then, once.
    /// The node's own moves wait until the owner turns them on again or
    /// moves funds, so it never pays again by itself what may yet arrive.
    ///
    /// # Errors
    ///
    /// No hub scheme runs, or no move is on its way, or it is not late.
    pub fn hub_give_up(&mut self) -> Result<HubStatus, EngineError> {
        let standing = self.standing().ok_or_else(|| {
            EngineError::Precondition("the settlement hub scheme is not running".into())
        })?;
        if self.funding.follow(self.now_ms, &standing) {
            self.keep_funding();
        }
        if self.funding.give_up(self.now_ms).is_none() {
            return Err(EngineError::Precondition(
                "only a move that is late can be given up".into(),
            ));
        }
        self.keep_funding();
        self.hub_status()
            .ok_or_else(|| EngineError::Internal("the hub scheme vanished".into()))
    }

    /// What an owner's move is decided from: the move on its way followed
    /// first, so one that is done no longer stands in the way.
    fn owner_standing(&mut self) -> Result<Standing, EngineError> {
        let standing = self.standing().ok_or_else(|| {
            EngineError::Precondition("the settlement hub scheme is not running".into())
        })?;
        if self.funding.follow(self.now_ms, &standing) {
            self.keep_funding();
        }
        if self.funding.moving.is_some() {
            return Err(EngineError::Exhausted(
                "a move between the hub and the XRP Ledger is on its way".into(),
            ));
        }
        Ok(standing)
    }

    fn owner_move(
        &mut self,
        planned: Planned,
        standing: &Standing,
    ) -> Result<HubStatus, EngineError> {
        self.make_move(planned, standing)
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

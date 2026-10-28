//! Moving the owner's funds between the owner book of a hub's asset, on the
//! node's XRPL testnet account, and its account in that settlement hub
//! (spec/areas/hubs.md#in-the-node,
//! spec/designs/settlement-hubs.md#11-choosing-a-hub): when a move is due,
//! and when one on its way is done. `hub_moves.rs` makes the moves.
//! Amounts are in the hub asset's smallest unit, as the hub counts it:
//! drops of XRP, or millionths of a token such as DSUSD.
//!
//! - **Between a floor and a ceiling.** What the hub account can spend
//!   stays above a floor of what its peerings over the hub need to fund
//!   their holds when asked, and what the node keeps in the hub (the
//!   balance, its reserves, its holds toward peers and theirs toward it)
//!   below a ceiling, both counted in largest packets
//!   (`policy::hub_floor`, `policy::hub_ceiling`). Below the floor, the node
//!   deposits from its XRPL account up to the midpoint, never past the
//!   ceiling; above the ceiling, it withdraws to the owner's own account
//!   down to the midpoint, never below the floor: the hub keeps a float, and
//!   the owner's funds are kept on the own account
//!   (spec/decisions/one-balance-per-asset.md). Holds toward us grow by our
//!   peers' settlements alone, so only they can take the hub past the
//!   ceiling, until they are taken and withdrawn.
//! - **Joining by deposit.** While the hub has no account for the node's
//!   key, the first deposit opens it: tag 0, the account key as the memo,
//!   of the hub's type for it (`dassie_hub::escrow::OPEN_MEMO_TYPE`),
//!   sized to the account reserve, the floor and the fees of its first
//!   operations.
//! - **One move at a time, done by the counters.** A move names the counter
//!   of the hub account it shows in, and the value it takes it to: a
//!   deposit, what deposits credited the account as agreed when it was made
//!   plus what it pays; a withdrawal, what withdrawals took plus what it
//!   takes with the ledger fee. It is done once an agreed state of the
//!   account reaches that, failed only once the side it leaves says its
//!   payment moved nothing, and on its way otherwise, however long, and
//!   across restarts: the database keeps it. So no move is ever paid twice.
//!   One that failed waits a backoff, doubling, before the next.
//! - **Late.** A move on its way past a while (`CREDIT_WAIT_MS`) since the
//!   XRP Ledger applied it, or since it was made, is late: the owner is told,
//!   and nothing is paid again for it. A deposit the XRP Ledger applied and
//!   the hub never credits stays on its way.
//! - **Giving up.** Once a move is late, the owner may give it up: the
//!   node forgets it and books nothing, since each side books its half
//!   when it sees it, so what the hub does with it later is booked then,
//!   once. Its automatic moves then stay paused until the owner turns them
//!   on again or moves funds himself: the node never pays again by itself
//!   what the hub may yet credit.
//! - **The owner's moves** go the same way, and pause the automatic ones
//!   for a while, so the node does not undo them at once. The owner may
//!   turn the automatic moves off. The database keeps both.
//! - **Nothing moves into or out of a hub that is unavailable**, such as a
//!   frozen token hub (spec/decisions/token-hubs.md): its scheme or its
//!   account is not in step with the hub.
//! - **A token hub's gas first.** Before the node asks a token hub for a
//!   withdrawal, it reads the escrow's XRP, proven, and if that is below
//!   the least the hub's crate fixes (`LEAST_ESCROW_GAS_DROPS`), sends the
//!   escrow the shortfall from its XRP owner book and waits until that
//!   applied ([`Gas`]). A top-up that fails holds the withdrawal back for
//!   a backoff of the check's own, which holds no other move back; one
//!   whose outcome the XRP book no longer knows, such as one another of
//!   its payments followed, is settled by reading the escrow's XRP again.
//!
//! Each side books its half when it sees it happen: the XRP scheme the
//! owner's withdrawal once its Payment applied, the hub scheme the owner's
//! deposit once the hub credited it, and the other way round
//! (spec/areas/hubs.md#the-members-side).

use dassie_ledger::Name;
use dassie_settlement::OwnerPaymentState;

/// How long an owner's own move pauses the automatic ones.
pub const OWNER_PAUSE_MS: u64 = 3_600_000;
/// The first wait after a move failed.
pub const FIRST_BACKOFF_MS: u64 = 60_000;
/// The longest wait after moves failed in a row.
const MAX_BACKOFF_MS: u64 = 3_600_000;
/// How long a move may be on its way, since the XRP Ledger applied it or
/// since it was made, before the owner is told it is late: the validators
/// attest a deposit within seconds of a validated ledger, and execute an
/// operation within a block.
pub const CREDIT_WAIT_MS: u64 = 600_000;

/// Which way a move goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// From the XRPL account into the hub.
    Deposit,
    /// From the hub to the XRPL account.
    Withdraw,
}

/// How long a check of a token hub's gas holds
/// ([`dassie_hub::member::GAS_CHECK_LASTS_MS`]).
pub const GAS_CHECK_LASTS_MS: u64 = dassie_hub::member::GAS_CHECK_LASTS_MS;

/// The least XRP, in drops, a token hub's escrow holds before a member
/// asks it for a withdrawal ([`dassie_hub::member::LEAST_ESCROW_GAS_DROPS`]).
pub const LEAST_ESCROW_GAS_DROPS: u64 = dassie_hub::member::LEAST_ESCROW_GAS_DROPS;

/// A move decided on, not made yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planned {
    /// Which way.
    pub direction: Direction,
    /// How much, in the hub asset's smallest unit (drops of XRP, or
    /// millionths of a token).
    pub drops: u64,
    /// Whether the owner asked for it.
    pub owner: bool,
    /// The value of the account's counter that shows it done.
    pub target: u64,
}

impl Planned {
    /// A deposit of `drops`, to an account deposits had credited
    /// `deposited` when it was made (0 for one it opens).
    pub const fn deposit(drops: u64, deposited: u64, owner: bool) -> Self {
        Self {
            direction: Direction::Deposit,
            drops,
            owner,
            target: deposited.saturating_add(drops),
        }
    }

    /// A withdrawal of `drops`, with `ledger_fee`, from an account
    /// withdrawals had taken `withdrawn` from when it was made.
    pub const fn withdraw(drops: u64, ledger_fee: u64, withdrawn: u64, owner: bool) -> Self {
        Self {
            direction: Direction::Withdraw,
            drops,
            owner,
            target: withdrawn.saturating_add(drops).saturating_add(ledger_fee),
        }
    }
}

/// A move on its way, as the database keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    /// Which way.
    pub direction: Direction,
    /// How much, in the hub asset's smallest unit.
    pub drops: u64,
    /// Whether the owner asked for it.
    pub owner: bool,
    /// The value of the account's counter (`deposited` for a deposit,
    /// `withdrawn` for a withdrawal) that shows it done.
    pub target: u64,
    /// Its payment's ID with the scheme of the side it leaves.
    pub payment: Name,
    /// When it was made.
    pub sent_ms: u64,
    /// When the XRP Ledger applied a deposit.
    pub applied_ms: Option<u64>,
}

/// The node's hub account, as last agreed, in the hub asset's smallest
/// unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// What it can spend.
    pub balance: u64,
    /// What the node keeps in the hub: the balance, the reserves and the
    /// holds toward it.
    pub exposure: u64,
    /// What deposits credited it.
    pub deposited: u64,
    /// What withdrawals took from it, ledger fees included.
    pub withdrawn: u64,
}

/// What the moves are decided from, in the hub asset's smallest unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Standing {
    /// The hub account, once agreed.
    pub held: Option<Held>,
    /// Whether the hub says it has no account for the node's key.
    pub unfunded: bool,
    /// Whether the hub is unavailable, as a frozen token hub is: nothing
    /// moves into or out of it.
    pub unavailable: bool,
    /// The floor and the ceiling.
    pub floor: u64,
    /// The ceiling.
    pub ceiling: u64,
    /// What a deposit that opens the account pays: its reserve, the floor
    /// and the fees of its first operations.
    pub opening: u64,
    /// The least the hub pays out.
    pub least_payout: u64,
    /// The hub's ledger fee on a withdrawal.
    pub ledger_fee: u64,
    /// What a withdrawal costs beyond its amount: the operation's fee and
    /// the ledger's.
    pub withdrawal_fees: u64,
    /// The least deposit worth its fees: a largest packet.
    pub least_deposit: u64,
    /// The owner's funds in the owner book of the hub's asset a deposit
    /// may take, after its fee; `None` while the book does not run synced.
    pub xrpl: Option<u64>,
    /// The owner's funds on the hub's ledger: what a withdrawal may take,
    /// with its fees. What the hub account holds beyond them is owed to
    /// peers.
    pub hub_owner: u64,
    /// What became of the payment of the move on its way, as the scheme of
    /// the side it leaves says.
    pub paying: OwnerPaymentState,
}

/// Where the check of a token hub's gas before a withdrawal stands
/// (spec/decisions/token-hubs.md). Not kept across restarts: a restart
/// checks again. A top-up still on its way then holds the XRP owner book,
/// and a read may not show it yet: a short read while the book is held
/// waits for the book ([`Gas::Waiting`]), and the escrow is read again once
/// it is free, so no second top-up goes for one shortfall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gas {
    /// Not checked for the next withdrawal.
    Unchecked,
    /// The escrow's XRP asked for, to be proven at ledger `after` or later.
    Reading {
        /// The XRP Ledger's newest ledger the node had proven when it
        /// asked, or the one after a read that showed the escrow short: a
        /// read proven there or later shows the escrow as it was when the
        /// node asked, or later.
        after: u32,
    },
    /// A read showed the escrow short while a payment of the XRP owner
    /// book was on its way, which may be a top-up the read does not show
    /// yet, as one from before a restart: once the book has none on its
    /// way, the escrow is read again, at the newest ledger proven then or
    /// later, which shows what that payment did.
    Waiting,
    /// The node's top-up of the escrow's XRP, on its way.
    ToppingUp {
        /// Its payment's ID with the XRP owner book.
        payment: Name,
        /// The ledger the read that showed the escrow short was proven at.
        short_at: u32,
    },
    /// The escrow holds the least gas, or the node's top-up applied, as of
    /// `at_ms`.
    Ready {
        /// When it was checked.
        at_ms: u64,
    },
}

/// What the gas check is decided from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GasSeen {
    /// The newest ledger the XRP owner book's account has proven, if it
    /// runs in step with its ledger.
    pub anchor: Option<u32>,
    /// The escrow's XRP as last proven, in drops (`None` for no account),
    /// and the ledger it was proven at.
    pub proven: Option<(Option<u64>, u32)>,
    /// What became of the node's top-up on its way, as the XRP owner book
    /// says.
    pub topping_up: OwnerPaymentState,
    /// Whether the XRP owner book has a payment of the owner's on its way:
    /// the book takes no top-up until it is done, and the escrow may not
    /// show it yet.
    pub book_held: bool,
}

/// What the gas check asks for next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GasNext {
    /// The escrow has its gas: the withdrawal may go.
    Ready,
    /// Nothing for now.
    Wait,
    /// Read the escrow's XRP.
    Read,
    /// Send the escrow `drops` of XRP, which a read proven at ledger `at`
    /// showed it short of.
    TopUp {
        /// The shortfall below the least gas.
        drops: u64,
        /// The ledger the read was proven at.
        at: u32,
    },
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
    /// The move the owner gave up last, which way and how much, which may
    /// still show on the other side: the automatic moves wait for the owner
    /// until it is cleared.
    pub given_up: Option<(Direction, u64)>,
    /// No automatic move before this: a failure's backoff.
    not_before_ms: u64,
    /// The wait after the next failure.
    backoff_ms: u64,
    /// The next check.
    pub at_ms: u64,
    interval_ms: u64,
    /// The check of a token hub's gas before its next withdrawal.
    pub gas: Gas,
    /// No check of the gas before this: a failed top-up's backoff, which
    /// holds back no other move.
    gas_not_before_ms: u64,
    /// The wait after the next failed top-up.
    gas_backoff_ms: u64,
}

impl Funding {
    /// Nothing on its way, checked as often as settlements are.
    pub const fn new(now_ms: u64, interval_ms: u64) -> Self {
        Self {
            automatic: true,
            paused_until_ms: 0,
            moving: None,
            given_up: None,
            not_before_ms: 0,
            backoff_ms: FIRST_BACKOFF_MS,
            at_ms: now_ms,
            interval_ms,
            gas: Gas::Unchecked,
            gas_not_before_ms: 0,
            gas_backoff_ms: FIRST_BACKOFF_MS,
        }
    }

    /// Follows the move on its way: done once the account's counter
    /// reaches its target, failed once the side it leaves says its payment
    /// moved nothing, and on its way otherwise. Returns whether the move
    /// changed, which the database keeps.
    pub fn follow(&mut self, now_ms: u64, standing: &Standing) -> bool {
        let Some(moving) = &mut self.moving else {
            return false;
        };
        let counter = standing.held.map(|held| match moving.direction {
            Direction::Deposit => held.deposited,
            Direction::Withdraw => held.withdrawn,
        });
        if counter.is_some_and(|counter| counter >= moving.target) {
            self.moving = None;
            self.backoff_ms = FIRST_BACKOFF_MS;
            return true;
        }
        match standing.paying {
            OwnerPaymentState::Failed(_) | OwnerPaymentState::Cancelled => {
                self.failed(now_ms);
                true
            }
            OwnerPaymentState::Applied
                if moving.direction == Direction::Deposit && moving.applied_ms.is_none() =>
            {
                moving.applied_ms = Some(now_ms);
                true
            }
            _ => false,
        }
    }

    /// Whether the move on its way is late: the owner is told, and nothing
    /// is paid again for it.
    pub fn late(&self, now_ms: u64) -> bool {
        self.moving.is_some_and(|moving| {
            let since = moving.applied_ms.unwrap_or(moving.sent_ms);
            now_ms >= since.saturating_add(CREDIT_WAIT_MS)
        })
    }

    /// The owner gives up the move on its way, which must be late: it is
    /// forgotten, nothing is booked, and the automatic moves wait for the
    /// owner. Returns it, or `None` if there is none or it is not late.
    pub fn give_up(&mut self, now_ms: u64) -> Option<Move> {
        if !self.late(now_ms) {
            return None;
        }
        let moving = self.moving.take()?;
        self.given_up = Some((moving.direction, moving.drops));
        Some(moving)
    }

    /// The owner turned the automatic moves on, or moved funds: the given-up
    /// move no longer holds them back.
    pub const fn owner_acted(&mut self) {
        self.given_up = None;
    }

    /// A move that failed, or could not be made: the next waits.
    pub fn failed(&mut self, now_ms: u64) {
        self.moving = None;
        self.back_off(now_ms);
    }

    /// The next automatic move waits the backoff, which doubles.
    fn back_off(&mut self, now_ms: u64) {
        self.not_before_ms = now_ms.saturating_add(self.backoff_ms);
        self.backoff_ms = self.backoff_ms.saturating_mul(2).min(MAX_BACKOFF_MS);
    }

    /// The automatic move due now, if any: none while one is on its way,
    /// the owner turned them off or paused them, gave one up and has not
    /// acted since, or a failure's backoff lasts.
    pub fn due(&self, now_ms: u64, standing: &Standing) -> Option<Planned> {
        let waiting = self.moving.is_some()
            || standing.unavailable
            || self.given_up.is_some()
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
                .then(|| Planned::deposit(drops, held.deposited, false));
        }
        if held.exposure > standing.ceiling {
            let spare = held
                .balance
                .saturating_sub(standing.floor)
                .saturating_sub(standing.withdrawal_fees)
                .min(standing.hub_owner.saturating_sub(standing.withdrawal_fees));
            let drops = (held.exposure - middle).min(spare);
            return (drops >= standing.least_payout)
                .then(|| Planned::withdraw(drops, standing.ledger_fee, held.withdrawn, false));
        }
        None
    }

    /// Starts `planned`, paid as `payment`; an owner's pauses the automatic
    /// moves.
    pub fn start(&mut self, now_ms: u64, planned: Planned, payment: Name) {
        assert!(self.moving.is_none(), "one move at a time");
        if planned.direction == Direction::Withdraw {
            self.gas = Gas::Unchecked;
        }
        if planned.owner {
            self.paused_until_ms = now_ms.saturating_add(OWNER_PAUSE_MS);
            self.owner_acted();
        }
        self.moving = Some(Move {
            direction: planned.direction,
            drops: planned.drops,
            owner: planned.owner,
            target: planned.target,
            payment,
            sent_ms: now_ms,
            applied_ms: None,
        });
    }

    /// Whether a token hub's escrow has its gas for a withdrawal now.
    pub const fn gas_ready(&self, now_ms: u64) -> bool {
        matches!(self.gas, Gas::Ready { at_ms } if now_ms < at_ms.saturating_add(GAS_CHECK_LASTS_MS))
    }

    /// Whether a check of a token hub's gas is under way: read or topped
    /// up, and not yet ready.
    pub const fn gas_checking(&self) -> bool {
        matches!(
            self.gas,
            Gas::Reading { .. } | Gas::Waiting | Gas::ToppingUp { .. }
        )
    }

    /// Until when a failed top-up holds the check of the gas back, while
    /// it does.
    pub const fn gas_backing_off(&self, now_ms: u64) -> Option<u64> {
        if now_ms < self.gas_not_before_ms && matches!(self.gas, Gas::Unchecked) {
            Some(self.gas_not_before_ms)
        } else {
            None
        }
    }

    /// The next step of the check of a token hub's gas before a
    /// withdrawal, from what the XRP owner book's account proved of the
    /// escrow: read its XRP, at the newest ledger proven or later, then
    /// send it the shortfall below [`LEAST_ESCROW_GAS_DROPS`], and wait
    /// until that applied. Its whole XRP counts, never a part of it kept
    /// for its reserve. A short read while another payment of the XRP book
    /// is on its way waits for that payment, and reads the escrow again
    /// once the book is free. A top-up that moved nothing fails: the check backs
    /// off, and starts again after. A top-up whose outcome the XRP book no
    /// longer knows (it keeps its last payment's alone) is neither on its
    /// way nor failed: the escrow's XRP, read again at a ledger after the
    /// one that showed it short, tells whether it arrived.
    pub fn gas_step(&mut self, now_ms: u64, seen: &GasSeen) -> GasNext {
        if self.gas_ready(now_ms) {
            return GasNext::Ready;
        }
        match self.gas {
            Gas::Unchecked | Gas::Ready { .. } if now_ms < self.gas_not_before_ms => GasNext::Wait,
            Gas::Unchecked | Gas::Ready { .. } => self.read_gas(seen.anchor, 0),
            Gas::Reading { after } => match seen.proven {
                Some((drops, at)) if at >= after => {
                    let held = drops.unwrap_or(0);
                    if held >= LEAST_ESCROW_GAS_DROPS {
                        self.gas_ready_now(now_ms);
                        GasNext::Ready
                    } else if seen.book_held {
                        self.gas = Gas::Waiting;
                        GasNext::Wait
                    } else {
                        GasNext::TopUp {
                            drops: LEAST_ESCROW_GAS_DROPS - held,
                            at,
                        }
                    }
                }
                _ => GasNext::Read,
            },
            // The book freed as of the anchor, so a read proven there or
            // later shows what its payment did.
            Gas::Waiting if seen.book_held => GasNext::Wait,
            Gas::Waiting => self.read_gas(seen.anchor, 0),
            Gas::ToppingUp { short_at, .. } => match seen.topping_up {
                OwnerPaymentState::Applied => {
                    self.gas_ready_now(now_ms);
                    GasNext::Ready
                }
                OwnerPaymentState::Failed(_) | OwnerPaymentState::Cancelled => {
                    self.gas_failed(now_ms);
                    GasNext::Wait
                }
                OwnerPaymentState::InFlight => GasNext::Wait,
                OwnerPaymentState::Unknown => {
                    self.read_gas(seen.anchor, short_at.saturating_add(1))
                }
            },
        }
    }

    /// Asks for the escrow's XRP, proven at the anchor `anchor` or later,
    /// and never at or before a ledger below `least`.
    fn read_gas(&mut self, anchor: Option<u32>, least: u32) -> GasNext {
        let Some(anchor) = anchor else {
            return GasNext::Wait;
        };
        self.gas = Gas::Reading {
            after: anchor.max(least),
        };
        GasNext::Read
    }

    /// The escrow has its gas as of `now_ms`: a later failure backs off
    /// from the first wait again.
    const fn gas_ready_now(&mut self, now_ms: u64) {
        self.gas = Gas::Ready { at_ms: now_ms };
        self.gas_backoff_ms = FIRST_BACKOFF_MS;
    }

    /// The node's top-up of the escrow's gas, of the shortfall a read
    /// proven at ledger `short_at` showed, went out as `payment`.
    pub fn topping_up(&mut self, payment: Name, short_at: u32) {
        assert!(
            matches!(self.gas, Gas::Reading { .. }),
            "a top-up follows a read"
        );
        self.gas = Gas::ToppingUp { payment, short_at };
    }

    /// The XRP book could not take the top-up a read proven at ledger
    /// `short_at` asked for now, as while another of its payments is on its
    /// way: the next decision rests on a read proven after it.
    pub fn top_up_refused(&mut self, short_at: u32) {
        assert!(
            matches!(self.gas, Gas::Reading { .. }),
            "a top-up follows a read"
        );
        self.gas = Gas::Reading {
            after: short_at.saturating_add(1),
        };
    }

    /// The top-up the gas check asked for could not be made, or moved
    /// nothing: the check waits its own backoff, which doubles, and starts
    /// again after. The automatic moves wait for nothing but the check.
    pub fn gas_failed(&mut self, now_ms: u64) {
        self.gas = Gas::Unchecked;
        self.gas_not_before_ms = now_ms.saturating_add(self.gas_backoff_ms);
        self.gas_backoff_ms = self.gas_backoff_ms.saturating_mul(2).min(MAX_BACKOFF_MS);
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
fn opening(standing: &Standing) -> Option<Planned> {
    let enough = standing.unfunded && standing.xrpl? >= standing.opening;
    enough.then(|| Planned::deposit(standing.opening, 0, false))
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
                withdrawn: 3,
            }),
            unfunded: false,
            unavailable: false,
            floor: 400_000,
            ceiling: 10_000_000,
            opening: 2_401_000,
            least_payout: 1_000_000,
            ledger_fee: 100,
            withdrawal_fees: 110,
            least_deposit: 100_000,
            xrpl: Some(100_000_000),
            hub_owner: 100_000_000,
            paying: OwnerPaymentState::Unknown,
        }
    }

    fn payment() -> Name {
        Name::new("abc").unwrap()
    }

    /// `low` with the account's counters at `deposited` and `withdrawn`.
    fn counted(mut standing: Standing, deposited: u64, withdrawn: u64) -> Standing {
        if let Some(held) = &mut standing.held {
            held.deposited = deposited;
            held.withdrawn = withdrawn;
        }
        standing
    }

    // Below the floor, a deposit up to the midpoint; above the ceiling, a
    // withdrawal down to it; in between, nothing. Each names the counter
    // value that shows it done.
    #[test]
    fn moves_go_back_to_the_midpoint() {
        let funding = Funding::new(0, 4_000);
        let middle = 5_200_000;
        let low = funding.due(1, &standing(100_000, 2_100_000)).unwrap();
        assert_eq!(low.direction, Direction::Deposit);
        assert_eq!(low.drops, middle - 100_000);
        assert_eq!(low.target, 7 + middle - 100_000);
        let high = funding.due(1, &standing(12_000_000, 14_000_000)).unwrap();
        assert_eq!(high.direction, Direction::Withdraw);
        assert_eq!(high.drops, 14_000_000 - middle);
        assert_eq!(high.target, 3 + 14_000_000 - middle + 100);
        assert_eq!(funding.due(1, &standing(3_000_000, 5_000_000)), None);
    }

    // A deposit never takes the hub past its ceiling, and a withdrawal never
    // the balance below the floor: holds toward us count toward the
    // ceiling, and only peers' settlements grow them.
    #[test]
    fn the_ceiling_and_the_floor_bound_each_move() {
        let funding = Funding::new(0, 4_000);
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

    // The first deposit opens the account once the hub says it has none,
    // and is done once the account it opened counts it.
    #[test]
    fn the_first_deposit_opens_the_account() {
        let funding = Funding::new(0, 4_000);
        let mut none = standing(0, 0);
        none.held = None;
        assert_eq!(funding.due(1, &none), None, "not before the hub says");
        none.unfunded = true;
        let open = funding.due(1, &none).unwrap();
        assert_eq!(
            (open.direction, open.drops),
            (Direction::Deposit, 2_401_000)
        );
        assert_eq!(open.target, 2_401_000);
        none.xrpl = Some(2_000_000);
        assert_eq!(funding.due(1, &none), None, "too little to open it");
    }

    // One at a time, done only once the counter reaches its target,
    // however long that takes: late after a while, never paid again. A
    // payment that moved nothing fails the move, and the next waits a
    // backoff that doubles.
    #[test]
    fn a_move_is_done_by_its_counter_and_never_paid_twice() {
        let mut funding = Funding::new(0, 4_000);
        let low = standing(100_000, 2_000_000);
        let planned = funding.due(1, &low).unwrap();
        funding.start(1, planned, payment());
        assert_eq!(funding.due(2, &low), None, "one on its way");
        let mut applied = low;
        applied.paying = OwnerPaymentState::Applied;
        assert!(funding.follow(10, &applied), "applied: kept");
        assert!(!funding.follow(20, &applied));
        assert!(!funding.late(9 + CREDIT_WAIT_MS));
        assert!(funding.late(10 + CREDIT_WAIT_MS));
        let much_later = 10 + 100 * CREDIT_WAIT_MS;
        let mut unknown = low;
        unknown.paying = OwnerPaymentState::Unknown;
        assert!(!funding.follow(much_later, &unknown));
        assert!(funding.moving.is_some(), "still on its way");
        assert_eq!(funding.due(much_later, &low), None, "never paid again");
        let short = counted(low, planned.target - 1, 3);
        assert!(!funding.follow(much_later, &short));
        let credited = counted(low, planned.target, 3);
        assert!(funding.follow(much_later, &credited));
        assert!(funding.moving.is_none());

        let planned = funding.due(much_later, &low).unwrap();
        funding.start(much_later, planned, payment());
        let mut failed = low;
        failed.paying = OwnerPaymentState::Failed(dassie_settlement::PaymentFailure::Other);
        assert!(funding.follow(much_later, &failed));
        assert_eq!(funding.due(much_later + 1, &low), None, "backing off");
        let after = much_later + FIRST_BACKOFF_MS;
        let again = funding.due(after, &low).unwrap();
        funding.start(after, again, payment());
        funding.follow(after, &failed);
        assert_eq!(
            funding.due(after + FIRST_BACKOFF_MS, &low),
            None,
            "the backoff doubled"
        );
        assert!(funding.due(after + 2 * FIRST_BACKOFF_MS, &low).is_some());
    }

    // A withdrawal is done once what withdrawals took reaches its target,
    // its ledger fee included.
    #[test]
    fn a_withdrawal_is_done_by_what_withdrawals_took() {
        let mut funding = Funding::new(0, 4_000);
        let high = standing(12_000_000, 14_000_000);
        let planned = funding.due(1, &high).unwrap();
        funding.start(1, planned, payment());
        let mut executed = high;
        executed.paying = OwnerPaymentState::Applied;
        assert!(!funding.follow(2, &executed), "not shown yet");
        assert!(!funding.follow(3, &counted(high, 7, planned.target - 100)));
        assert!(funding.follow(4, &counted(high, 7, planned.target)));
    }

    // A move may be given up only once late; then nothing moves by itself
    // until the owner acts, and the move is forgotten, whatever shows later.
    #[test]
    fn a_late_move_given_up_waits_for_the_owner() {
        let low = standing(100_000, 2_000_000);
        let mut funding = Funding::new(0, 4_000);
        let planned = funding.due(1, &low).unwrap();
        funding.start(1, planned, payment());
        assert_eq!(funding.give_up(2), None, "not late yet");
        let late = 1 + CREDIT_WAIT_MS;
        let given_up = funding.give_up(late).unwrap();
        assert_eq!(given_up.target, planned.target);
        assert!(funding.moving.is_none());
        assert_eq!(funding.due(late + 100 * OWNER_PAUSE_MS, &low), None);
        assert!(!funding.follow(late, &counted(low, planned.target, 3)));
        funding.owner_acted();
        assert!(funding.due(late, &low).is_some());
        funding.start(late, Planned::deposit(5, 7, true), payment());
        assert_eq!(funding.given_up, None);
    }

    // A withdrawal never takes more than the owner's funds on the hub's
    // ledger: what the account holds beyond them is owed to hub peers.
    #[test]
    fn a_withdrawal_takes_only_the_owners_funds() {
        let funding = Funding::new(0, 4_000);
        let mut high = standing(12_000_000, 14_000_000);
        high.hub_owner = 2_000_000;
        let planned = funding.due(1, &high).unwrap();
        assert_eq!(planned.drops, 2_000_000 - 110, "with its fees");
        high.hub_owner = 500_000;
        assert_eq!(funding.due(1, &high), None, "less than the hub pays out");
    }

    // An owner's move pauses the automatic ones; turned off, none is made.
    #[test]
    fn the_owner_decides_first() {
        let low = standing(100_000, 2_000_000);
        let mut funding = Funding::new(0, 4_000);
        funding.start(5, Planned::withdraw(1_000_000, 100, 3, true), payment());
        funding.follow(6, &counted(low, 7, 1_000_103));
        assert!(funding.moving.is_none());
        assert_eq!(funding.due(7, &low), None);
        assert!(funding.due(5 + OWNER_PAUSE_MS, &low).is_some());
        funding.automatic = false;
        assert_eq!(funding.due(5 + OWNER_PAUSE_MS, &low), None);
    }

    // spec/decisions/token-hubs.md: before a token hub's withdrawal, the
    // escrow's XRP is read at the newest ledger proven when asked, or
    // later; with enough, the withdrawal goes; short, the shortfall is
    // topped up, and the withdrawal waits for it to apply. A withdrawal
    // started takes the check with it.
    #[test]
    fn a_token_hubs_withdrawal_tops_up_the_gas_first() {
        let mut funding = Funding::new(0, 4_000);
        let mut seen = GasSeen {
            anchor: None,
            proven: None,
            topping_up: OwnerPaymentState::Unknown,
            book_held: false,
        };
        assert_eq!(funding.gas_step(1, &seen), GasNext::Wait, "nothing proven");
        seen.anchor = Some(10);
        assert_eq!(funding.gas_step(1, &seen), GasNext::Read);
        assert!(funding.gas_checking());
        seen.proven = Some((Some(LEAST_ESCROW_GAS_DROPS * 5), 9));
        assert_eq!(funding.gas_step(2, &seen), GasNext::Read, "as of before");
        seen.proven = Some((Some(LEAST_ESCROW_GAS_DROPS), 10));
        assert_eq!(funding.gas_step(3, &seen), GasNext::Ready, "at the anchor");
        assert!(funding.gas_ready(3) && !funding.gas_ready(3 + GAS_CHECK_LASTS_MS));
        funding.start(4, Planned::withdraw(1_000_000, 0, 3, true), payment());
        assert!(!funding.gas_ready(5), "each withdrawal checks again");
        funding.moving = None;

        // Short: topped up by the shortfall, ready once that applied.
        seen.anchor = Some(11);
        assert_eq!(funding.gas_step(5, &seen), GasNext::Read);
        seen.proven = Some((Some(300_000), 12));
        let short = GasNext::TopUp {
            drops: LEAST_ESCROW_GAS_DROPS - 300_000,
            at: 12,
        };
        assert_eq!(funding.gas_step(6, &seen), short);
        funding.topping_up(payment(), 12);
        seen.topping_up = OwnerPaymentState::InFlight;
        assert_eq!(funding.gas_step(7, &seen), GasNext::Wait);
        seen.topping_up = OwnerPaymentState::Applied;
        assert_eq!(funding.gas_step(8, &seen), GasNext::Ready);
    }

    // spec/decisions/token-hubs.md: a top-up whose outcome the XRP book no
    // longer knows, as when another of its payments followed it, never
    // holds the withdrawal back for good: the escrow's XRP is read again,
    // never from the read that showed it short, and decides.
    #[test]
    fn a_top_up_whose_outcome_is_lost_reads_the_escrow_again() {
        let mut funding = Funding::new(0, 4_000);
        let mut seen = GasSeen {
            anchor: Some(20),
            proven: None,
            topping_up: OwnerPaymentState::Unknown,
            book_held: false,
        };
        assert_eq!(funding.gas_step(1, &seen), GasNext::Read);
        seen.proven = Some((Some(100_000), 20));
        assert!(matches!(
            funding.gas_step(2, &seen),
            GasNext::TopUp { at: 20, .. }
        ));
        funding.topping_up(payment(), 20);
        seen.topping_up = OwnerPaymentState::InFlight;
        assert_eq!(funding.gas_step(3, &seen), GasNext::Wait);
        // Applied, then followed by another payment of the XRP book.
        seen.topping_up = OwnerPaymentState::Unknown;
        assert_eq!(funding.gas_step(4, &seen), GasNext::Read);
        assert_eq!(
            funding.gas,
            Gas::Reading { after: 21 },
            "after the short read"
        );
        assert_eq!(
            funding.gas_step(5, &seen),
            GasNext::Read,
            "not the short read"
        );
        seen.anchor = Some(24);
        seen.proven = Some((Some(LEAST_ESCROW_GAS_DROPS), 22));
        assert_eq!(funding.gas_step(6, &seen), GasNext::Ready, "it arrived");

        // Lost, and it never arrived: topped up again.
        funding.gas = Gas::ToppingUp {
            payment: payment(),
            short_at: 22,
        };
        assert_eq!(funding.gas_step(7, &seen), GasNext::Read);
        assert_eq!(funding.gas, Gas::Reading { after: 24 }, "at the anchor");
        seen.proven = Some((Some(100_000), 24));
        assert!(matches!(
            funding.gas_step(8, &seen),
            GasNext::TopUp { at: 24, .. }
        ));
    }

    // spec/decisions/token-hubs.md: a restart forgets the check, never the
    // XRP book's payment. With its top-up T1 still on its way, the node
    // reads the escrow short again; it sends no second top-up for that
    // shortfall, but waits for the book, and decides once it is free from
    // a read proven no earlier than then, which shows T1.
    #[test]
    fn a_restart_with_a_top_up_on_its_way_sends_no_second() {
        let mut funding = Funding::new(0, 4_000);
        let mut seen = GasSeen {
            anchor: Some(30),
            proven: None,
            topping_up: OwnerPaymentState::Unknown,
            book_held: true,
        };
        assert_eq!(funding.gas_step(1, &seen), GasNext::Read);
        seen.proven = Some((Some(100_000), 30));
        assert_eq!(
            funding.gas_step(2, &seen),
            GasNext::Wait,
            "T1 holds the book"
        );
        assert_eq!(funding.gas, Gas::Waiting);
        assert!(funding.gas_checking());
        seen.anchor = Some(31);
        seen.proven = Some((Some(100_000), 31));
        assert_eq!(funding.gas_step(3, &seen), GasNext::Wait, "still held");
        // T1 applied in ledger 33, which freed the book; the read at 31
        // was taken before, and decides nothing.
        seen.anchor = Some(34);
        seen.book_held = false;
        assert_eq!(funding.gas_step(4, &seen), GasNext::Read);
        assert_eq!(funding.gas, Gas::Reading { after: 34 });
        assert_eq!(
            funding.gas_step(5, &seen),
            GasNext::Read,
            "not the old read"
        );
        seen.proven = Some((Some(100_000 + LEAST_ESCROW_GAS_DROPS), 34));
        assert_eq!(funding.gas_step(6, &seen), GasNext::Ready, "T1 shows");

        // A top-up the book refuses all the same is decided from a later
        // read, never the one that asked for it.
        let mut funding = Funding::new(0, 4_000);
        seen.proven = Some((Some(100_000), 34));
        assert_eq!(funding.gas_step(7, &seen), GasNext::Read);
        assert!(matches!(
            funding.gas_step(8, &seen),
            GasNext::TopUp { at: 34, .. }
        ));
        funding.top_up_refused(34);
        assert_eq!(funding.gas, Gas::Reading { after: 35 });
        assert_eq!(funding.gas_step(9, &seen), GasNext::Read, "read again");
    }

    // spec/decisions/token-hubs.md: a top-up that failed backs off the
    // check alone, doubling, never the automatic moves; a check that
    // finds the gas starts the backoff from its first wait again.
    #[test]
    fn a_failed_top_up_backs_off_the_check_alone() {
        let mut funding = Funding::new(0, 4_000);
        let mut seen = GasSeen {
            anchor: Some(12),
            proven: Some((None, 12)),
            topping_up: OwnerPaymentState::Cancelled,
            book_held: false,
        };
        // An escrow with no account is short of all of it.
        assert_eq!(funding.gas_step(9, &seen), GasNext::Read);
        let all = GasNext::TopUp {
            drops: LEAST_ESCROW_GAS_DROPS,
            at: 12,
        };
        assert_eq!(funding.gas_step(9, &seen), all);
        funding.topping_up(payment(), 12);
        assert_eq!(funding.gas_step(10, &seen), GasNext::Wait);
        assert_eq!(funding.gas, Gas::Unchecked);
        assert_eq!(funding.gas_backing_off(11), Some(10 + FIRST_BACKOFF_MS));
        assert_eq!(funding.gas_step(11, &seen), GasNext::Wait, "backing off");
        let high = standing(12_000_000, 14_000_000);
        assert!(funding.due(11, &high).is_some(), "the moves go on");
        let again = 10 + FIRST_BACKOFF_MS;
        assert_eq!(funding.gas_step(again, &seen), GasNext::Read);
        assert_eq!(funding.gas_step(again, &seen), all);
        funding.topping_up(payment(), 12);
        seen.topping_up = OwnerPaymentState::Failed(dassie_settlement::PaymentFailure::Other);
        assert_eq!(funding.gas_step(again + 1, &seen), GasNext::Wait);
        assert_eq!(
            funding.gas_backing_off(again + 1),
            Some(again + 1 + 2 * FIRST_BACKOFF_MS),
            "doubled"
        );
        let later = again + 1 + 2 * FIRST_BACKOFF_MS;
        assert_eq!(funding.gas_step(later, &seen), GasNext::Read);
        seen.proven = Some((Some(LEAST_ESCROW_GAS_DROPS), 13));
        assert_eq!(funding.gas_step(later, &seen), GasNext::Ready);
        funding.gas_failed(later + 1);
        assert_eq!(
            funding.gas_backing_off(later + 1),
            Some(later + 1 + FIRST_BACKOFF_MS),
            "from the first wait again"
        );
    }

    // spec/decisions/token-hubs.md: nothing moves into or out of a hub that
    // is unavailable, as a frozen one is.
    #[test]
    fn nothing_moves_while_the_hub_is_unavailable() {
        let funding = Funding::new(0, 4_000);
        for mut standing in [
            standing(100_000, 2_000_000),
            standing(12_000_000, 14_000_000),
        ] {
            assert!(funding.due(1, &standing).is_some());
            standing.unavailable = true;
            assert_eq!(funding.due(1, &standing), None);
        }
    }
}

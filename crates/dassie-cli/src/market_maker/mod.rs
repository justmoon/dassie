//! `dassie market-maker` (spec/areas/rate-feed.md#market-maker): one node
//! of its own, a periphery node in the settlement hubs of both assets of
//! an AMM pool on the XRP Ledger, pinned to the core nodes it trades
//! through, its targets. It reads the targets' snapshots its node is
//! pushed, swaps through the one that pays best when its quotes and the
//! pool leave a margin, and moves its own funds back on the ledger, from
//! the owner book of one asset to the other's on the node's one account.
//!
//! - [`config`]: the config file, checked whole before anything connects;
//! - [`node`]: the node, over its IPC socket: what it holds, the targets'
//!   snapshots and the swaps it sends;
//! - [`xrpl`]: the pool, and the rebalancing Payment;
//! - [`strategy`]: what to do, as a pure function of all that.
//!
//! The loop decides once a tick, from the current snapshots, the pool and
//! the node's holdings, and logs one line per decision. Every wait is
//! bounded, and a tick does at most one thing.

pub mod config;
pub mod node;
pub mod strategy;
pub mod xrpl;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use dassie_node::clock::{Clock, SystemClock};
use dassie_node::env::{self, Env};
use dassie_node::log;
use dassie_node::outbound::OutboundClient;
use dassie_xrpl::Wallet;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::keys::AccountId;
use lexopt::prelude::*;
use tokio_util::sync::CancellationToken;

use self::config::Config;
use self::node::{LedgerState, Node, PeerSnapshot};
use self::strategy::{Decision, Holdings, Inputs, Quotes, Side, TargetQuote};
use self::xrpl::Xrpl;

/// What its log lines are under.
pub const TARGET: &str = dassie_node::log::target::MARKET_MAKER;

/// How often it decides.
const TICK: Duration = Duration::from_secs(10);

/// How long it waits, at most, for the node to book a rebalance that paid
/// from its account, or may have, before it decides again.
const UNBOOKED_WAIT_MS: u64 = 2 * 60 * 1_000;

/// The flags.
#[derive(Debug)]
pub struct Options {
    config: PathBuf,
}

/// Reads the flags after `market-maker`.
///
/// # Errors
///
/// A flag is unknown, or `--config` is missing.
pub fn args(parser: &mut lexopt::Parser) -> Result<Options, lexopt::Error> {
    let mut config = None;
    while let Some(argument) = parser.next()? {
        match argument {
            Long("config") => config = Some(PathBuf::from(parser.value()?)),
            other => return Err(other.unexpected()),
        }
    }
    let config = config.ok_or("market-maker needs --config <file>")?;
    Ok(Options { config })
}

/// Runs the market maker until SIGTERM or SIGINT.
pub fn run(options: &Options) -> ExitCode {
    match env::log_filter(&Env::from_process()) {
        Ok(filter) => log::init(filter),
        Err(error) => {
            eprintln!("dassie: {error}");
            return ExitCode::FAILURE;
        }
    }
    let config = match config::load(&options.config) {
        Ok(config) => config,
        Err(problem) => {
            eprintln!("dassie: {}: {problem}", options.config.display());
            return ExitCode::FAILURE;
        }
    };
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::FAILURE;
    };
    let result = runtime.block_on(async {
        let shutdown = CancellationToken::new();
        let Some(signals) = crate::listen_for_signals() else {
            return Err("no signal handlers".to_owned());
        };
        tokio::spawn(crate::stop_on_signal(signals, shutdown.clone()));
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let mut maker = MarketMaker::start(config, Arc::clone(&clock)).await?;
        maker.run(&shutdown).await;
        Ok(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            tracing::error!(target: TARGET, %problem, "could not start");
            ExitCode::FAILURE
        }
    }
}

/// A running market maker.
struct MarketMaker {
    config: Config,
    clock: Arc<dyn Clock>,
    xrpl: Xrpl,
    node: Node,
    /// The node's XRPL account's keys, derived from its passphrase.
    wallet: Wallet,
    unbooked: Option<Unbooked>,
    /// The target the last trade went through, so that targets that pay
    /// alike take turns.
    last_target: Option<dassie_identity::NodeId>,
}

/// A rebalance that paid, or may have, from the node's account, which the
/// node may not have booked yet. Until it has, the holding of the asset it
/// spent still counts what it spent, and a decision from it would spend
/// that again: below the asset's band. It counts as booked once that
/// holding fell by at least what it can have cost and what the node's own
/// move between the asset's hub and book has on its way, which lowers what
/// the books say while it is, by up to its amount ([`Unbooked::booked`]).
#[derive(Clone, Copy, Debug)]
struct Unbooked {
    /// The asset it spent.
    from: Side,
    /// What the node held of it, as its books had it, before.
    total: u64,
    /// The least the rebalance can have spent ([`strategy::least_spent`]).
    least: u64,
    /// When the market maker stops waiting: the rebalance never applied,
    /// or the node booked it as something else.
    until_ms: u64,
}

impl Unbooked {
    /// Whether the node booked the rebalance, its holding now `total`
    /// with `moving` of its own move on its way. That move may read short
    /// by all of its amount, or by none of it yet, and may have been
    /// on its way before too: so the fall counts for the rebalance only
    /// past all of it. A move that was on its way before, and has landed
    /// or still is, only holds the next decision back, at most until the
    /// wait is over; one taken for the rebalance would have it spend again.
    const fn booked(&self, total: u64, moving: u64) -> bool {
        self.total.saturating_sub(total) >= self.least.saturating_add(moving)
    }
}

impl MarketMaker {
    /// Derives the keys, checks that the node runs both owner books on the
    /// account they open and each hub's ledger in its book's asset, and
    /// that it reads its peers' snapshots.
    async fn start(config: Config, clock: Arc<dyn Clock>) -> Result<Self, String> {
        let wallet = config::wallet_from_file(&config.passphrase_file, clock.now_ms())?;
        let mut node = Node::new(&config);
        check_node(&mut node, wallet.account(), &config.token).await?;
        let client = OutboundClient::new(config.ca_file.as_deref(), config.allow_private)?;
        let mut xrpl = Xrpl::new(config.xrpl_server.clone(), client);
        xrpl.testnet_state().await?;
        tracing::info!(
            target: TARGET,
            account = %wallet.account(),
            "trading through the node's peers"
        );
        Ok(Self {
            config,
            clock,
            xrpl,
            node,
            wallet,
            unbooked: None,
            last_target: None,
        })
    }

    async fn run(&mut self, shutdown: &CancellationToken) {
        let mut ticks = tokio::time::interval(TICK);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = ticks.tick() => {}
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = self.tick() => {}
            }
            // A full tick after a decision, however long it took: the node
            // books what it did meanwhile.
            ticks.reset();
        }
    }

    /// Decides once, among the targets with a current snapshot, and does
    /// what it decided.
    async fn tick(&mut self) {
        let snapshots = match self.node.peer_snapshots().await {
            Ok(snapshots) => snapshots,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "hold: cannot read the targets' snapshots");
                return;
            }
        };
        let pool = match self.xrpl.pool(self.config.token).await {
            Ok(pool) => pool,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "hold: cannot read the pool");
                return;
            }
        };
        let holdings = match self.node.holdings().await {
            Ok(holdings) => holdings,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "hold: cannot read the node's holdings");
                return;
            }
        };
        let now_ms = self.clock.now_ms();
        if self.awaits_booking(now_ms, &holdings).await {
            return;
        }
        let targets = self.current(&snapshots, now_ms);
        let params = self.config.params;
        let inputs = Inputs {
            targets: &targets,
            last_target: self.last_target,
            pool,
            holdings,
            params: &params,
        };
        match strategy::decide(&inputs) {
            Decision::Hold(hold) => {
                tracing::info!(target: TARGET, ?hold, targets = ?targets, ?holdings, "hold");
            }
            Decision::Trade(trade) => {
                let through = targets.iter().find(|quote| quote.target == trade.target);
                assert!(through.is_some(), "a trade through a target it was given");
                if let Some(through) = through {
                    self.last_target = Some(trade.target);
                    self.trade(&trade, &through.quotes).await;
                }
            }
            Decision::Rebalance(rebalance) => self.rebalance(&rebalance, &holdings).await,
        }
    }

    /// The targets whose snapshot counts at `now_ms`, each priced between
    /// the two hubs' methods.
    fn current(&self, snapshots: &[PeerSnapshot], now_ms: u64) -> Vec<TargetQuote> {
        snapshots
            .iter()
            .filter(|listed| now_ms < listed.snapshot.valid_until_ms)
            .map(|listed| TargetQuote {
                target: listed.node,
                quotes: Quotes::new(
                    Arc::clone(&listed.snapshot),
                    &self.config.xrp.method,
                    &self.config.token_ledgers.method,
                ),
            })
            .collect()
    }

    /// Whether the last rebalance still waits for the node to book it, at
    /// `now_ms` with `holdings`: nothing is decided meanwhile. Once booked,
    /// or once the wait is over, it waits no more.
    async fn awaits_booking(&mut self, now_ms: u64, holdings: &Holdings) -> bool {
        let Some(unbooked) = self.unbooked else {
            return false;
        };
        // A move that cannot be read counts as one too large to pass by:
        // the rebalance waits, at most until the wait is over.
        let moving = match self.node.moving(unbooked.from).await {
            Ok(moving) => moving,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "cannot read the node's move on its way");
                u64::MAX
            }
        };
        let booked = unbooked.booked(holdings.of(unbooked.from).total, moving);
        if !booked && now_ms < unbooked.until_ms {
            tracing::info!(
                target: TARGET, from = unbooked.from.name(), ?holdings, moving,
                "hold: the node has not booked the last rebalance yet"
            );
            return true;
        }
        self.unbooked = None;
        false
    }

    /// Swaps through the trade's target, and logs what came back against
    /// what the snapshot it was decided by promised for what was paid: the
    /// node may have priced it by a newer one.
    async fn trade(&mut self, trade: &strategy::Trade, quotes: &Quotes) {
        let (pay_on, back_on) = (
            &self.config.ledgers(trade.from).method,
            &self.config.ledgers(trade.from.other()).method,
        );
        let outcome = self
            .node
            .swap(trade.target, pay_on, back_on, trade.debit, trade.rate())
            .await;
        let (target, from) = (trade.target, trade.from.name());
        match outcome {
            Ok(done) => {
                let promised = quotes.out(trade.from, done.paid);
                tracing::info!(
                    target: TARGET, %target, from, debit = trade.debit,
                    least = trade.least, snapshot_receive = trade.receive,
                    paid = done.paid, received = done.received, ?promised,
                    fulfilled = done.fulfilled, rejected = done.rejected,
                    ended = ?done.ended, "trade"
                );
            }
            Err(problem) => tracing::warn!(
                target: TARGET, %target, from, debit = trade.debit, %problem,
                "trade failed"
            ),
        }
    }

    async fn rebalance(&mut self, rebalance: &strategy::Rebalance, holdings: &Holdings) {
        // To itself: the Payment spends one asset and delivers the other,
        // each on its owner book of the node's one account.
        let account: AccountId = self.wallet.account();
        let outcome = self.xrpl.rebalance(&self.wallet, account, rebalance).await;
        if outcome.as_ref().is_ok_and(xrpl::Outcome::may_have_paid) {
            self.unbooked = Some(Unbooked {
                from: rebalance.from,
                total: holdings.of(rebalance.from).total,
                least: strategy::least_spent(rebalance, self.config.params.slippage_ppm),
                until_ms: self.clock.now_ms().saturating_add(UNBOOKED_WAIT_MS),
            });
        }
        let from = rebalance.from.name();
        match outcome {
            Ok(outcome) => tracing::info!(
                target: TARGET, from, deliver = ?rebalance.deliver,
                send_max = ?rebalance.send_max, ?holdings, ?outcome, "rebalance"
            ),
            Err(problem) => tracing::warn!(
                target: TARGET, from, %problem, "rebalance failed"
            ),
        }
    }
}

/// Checks that the node runs both owner books on the account `account`,
/// the one its passphrase opens, so a rebalance pays from and to it: XRP's,
/// and the pool's token's of the issuer its book holds, so what a
/// rebalance delivers is booked there; that each hub's ledger holds its
/// book's asset ([`Node::ledger_states`]), and the node funds the hub from
/// that book; and that the node reads its peers' snapshots, its owner's
/// `swapping`, which a new node has on.
async fn check_node(
    node: &mut Node,
    account: AccountId,
    token: &IssuedAsset,
) -> Result<(), String> {
    let [xrp, token_state] = node.ledger_states().await?;
    check_ledger(Side::Xrp, &xrp, account, token)?;
    check_ledger(Side::Token, &token_state, account, token)?;
    if !node.swapping().await? {
        return Err(
            "the node reads no snapshot of its peers: its owner turned swapping off".into(),
        );
    }
    tracing::info!(target: TARGET, socket = %node.socket().display(), "node ready");
    Ok(())
}

/// What [`check_node`] checks of what the node says of `side`'s owner
/// book.
fn check_ledger(
    side: Side,
    state: &LedgerState,
    account: AccountId,
    token: &IssuedAsset,
) -> Result<(), String> {
    if state.deposit_address != account.to_address() {
        return Err(format!(
            "the node's {} book is not on the account its passphrase opens",
            side.name()
        ));
    }
    let of_xrp = state.asset_code == "XRP" && state.issuer.is_empty();
    match side {
        Side::Xrp if !of_xrp => Err("the XRP owner book is not a ledger of XRP".into()),
        Side::Token if of_xrp || state.issuer != token.issuer.to_address() => {
            Err("the token's owner book is not a ledger of the pool's token's issuer".into())
        }
        Side::Xrp | Side::Token => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use dassie_xrpl::amount::Currency;

    use super::*;
    use crate::market_maker::strategy::Holding;

    // spec/areas/rate-feed.md#bands: a rebalance counts as booked once
    // the holding of the asset it spent fell by the least it can have
    // cost, and by whatever the node's own move between hub and book has on
    // its way besides, which lowers the holding by up to its amount: a
    // withdrawal from the hub that the book has not seen arrive never
    // passes for it.
    #[test]
    fn a_move_on_its_way_never_passes_for_a_rebalance() {
        let unbooked = Unbooked {
            from: Side::Xrp,
            total: 1_000,
            least: 100,
            until_ms: 0,
        };
        // Nothing fell, nothing on its way: not yet.
        assert!(!unbooked.booked(1_000, 0));
        // The node's own withdrawal of 150 executed, not yet in the book:
        // the holding reads 150 short, and the rebalance is not booked.
        assert!(!unbooked.booked(850, 150));
        // Booked, the move still on its way: its fall and the move's.
        assert!(unbooked.booked(750, 150));
        // The move landed: the rebalance's fall alone.
        assert!(unbooked.booked(900, 0));
        assert!(!unbooked.booked(901, 0));
    }

    #[test]
    fn both_books_are_on_the_account_the_passphrase_opens() {
        let account = AccountId([1; 20]);
        let token = IssuedAsset {
            currency: Currency::parse("USD").unwrap(),
            issuer: AccountId([2; 20]),
        };
        let state = |asset: &str, issuer: String| LedgerState {
            asset_code: asset.into(),
            holding: Holding {
                total: 0,
                on_ledger: 0,
            },
            deposit_address: account.to_address(),
            issuer,
        };
        let xrp = state("XRP", String::new());
        let usd = state("USD", token.issuer.to_address());
        let check = |side, state: &LedgerState| check_ledger(side, state, account, &token);
        assert!(check(Side::Xrp, &xrp).is_ok());
        assert!(check(Side::Token, &usd).is_ok());
        assert!(check(Side::Xrp, &usd).is_err());
        assert!(check(Side::Token, &xrp).is_err());
        // Another issuer's token: what the pool delivers would not be
        // booked on the node's ledger.
        let other = state("USD", AccountId([3; 20]).to_address());
        assert!(check(Side::Token, &other).is_err());
        // Another account than the passphrase opens.
        let mut elsewhere = usd.clone();
        elsewhere.deposit_address = AccountId([4; 20]).to_address();
        assert!(check(Side::Token, &elsewhere).is_err());
    }
}

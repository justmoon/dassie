//! `dassie market-maker` (spec/areas/rate-feed.md#market-maker): reads the
//! snapshots of one or more core nodes, its targets, trades through the
//! one that pays best with two nodes of its own, each a member of the
//! settlement hub of its asset, when its quotes and an AMM pool on the XRP
//! Ledger leave a margin, and moves its own funds back on the ledger, from
//! the owner book that funds one node's hub to the other's.
//!
//! - [`config`]: the config file, checked whole before anything connects;
//! - [`target`]: who each target is, from its discovery document;
//! - [`feed`]: each target's snapshot over UDP, asked for and verified;
//! - [`xrpl`]: the pool, and the rebalancing Payment;
//! - [`nodes`]: the two nodes, over their IPC sockets;
//! - [`strategy`]: what to do, as a pure function of all that.
//!
//! The loop decides once a tick, from the current snapshots, the pool, both
//! nodes' holdings and the targets each is peered with, and logs one line
//! per decision. Every wait is bounded, and a tick does at most one thing.

pub mod config;
pub mod feed;
pub mod nodes;
pub mod strategy;
pub mod target;
pub mod xrpl;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use dassie_feed::Snapshot;
use dassie_identity::NodeId;
use dassie_node::clock::{Clock, SystemClock};
use dassie_node::env::{self, Env};
use dassie_node::log;
use dassie_node::outbound::OutboundClient;
use dassie_xrpl::Wallet;
use dassie_xrpl::amount::IssuedAsset;
use dassie_xrpl::keys::AccountId;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use lexopt::prelude::*;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use self::config::{Config, MAX_TARGETS};
use self::nodes::{LedgerState, Node, TradeOutcome};
use self::strategy::{Decision, Holdings, Inputs, Quotes, Side, TargetQuote};
use self::target::TargetInfo;
use self::xrpl::Xrpl;

/// What its log lines are under.
pub const TARGET: &str = dassie_node::log::target::MARKET_MAKER;

/// How often it decides.
const TICK: Duration = Duration::from_secs(10);

/// How long it waits, at most, for a node to book a rebalance that paid
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
        let mut maker = MarketMaker::start(config, Arc::clone(&clock), shutdown.clone()).await?;
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
    /// One per target, in the config's order.
    targets: Vec<Following>,
    xrpl: Xrpl,
    xrp: Node,
    token: Node,
    wallets: Wallets,
    unbooked: Option<Unbooked>,
    /// The target, in the config's order, the last trade went through, so
    /// that targets that pay alike take turns.
    last_target: Option<usize>,
}

/// A target the market maker follows ([`target::follow`]).
struct Following {
    /// Its URL, for log lines.
    url: String,
    /// Who it is, once its discovery document answered.
    found: watch::Receiver<Option<TargetInfo>>,
    /// Its newest snapshot.
    feed: watch::Receiver<Option<Arc<Snapshot>>>,
    /// Whether it was found to be a node an earlier target is too, and
    /// said so.
    duplicate: bool,
}

/// A target with a current snapshot, at a tick.
struct Current {
    /// Which, in the config's order.
    index: usize,
    /// Its node ID.
    node: NodeId,
    /// Its URL.
    url: String,
    /// Its quotes between the two nodes' methods, their hubs.
    quotes: Quotes,
}

/// A rebalance that paid, or may have, from a node's account, which the
/// node may not have booked yet. Until it has, its holding still counts
/// what the rebalance spent, and a decision from it would spend that
/// again: below the node's band. It counts as booked once the node's
/// holding fell by at least what it can have cost and what the node's own
/// move between hub and book has on its way, which lowers what the books
/// say while it is, by up to its amount ([`Unbooked::booked`]).
#[derive(Clone, Copy, Debug)]
struct Unbooked {
    /// The node whose account paid.
    from: Side,
    /// What the node held, as its books had it, before.
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

/// The nodes' XRPL accounts' keys, derived from their passphrases.
struct Wallets {
    xrp: Wallet,
    token: Wallet,
}

impl Wallets {
    const fn of(&self, side: Side) -> &Wallet {
        match side {
            Side::Xrp => &self.xrp,
            Side::Token => &self.token,
        }
    }
}

impl MarketMaker {
    /// Derives the keys, checks that each node runs its owner book on the
    /// account they open, and its hub's ledger in the same asset, and
    /// starts following each target: a target that
    /// does not answer yet is asked again, while the others trade.
    async fn start(
        config: Config,
        clock: Arc<dyn Clock>,
        shutdown: CancellationToken,
    ) -> Result<Self, String> {
        let now_ms = clock.now_ms();
        let wallets = Wallets {
            xrp: config::wallet_from_file(&config.xrp.passphrase_file, now_ms)?,
            token: config::wallet_from_file(&config.token_node.passphrase_file, now_ms)?,
        };
        let mut xrp = Node::new(Side::Xrp, &config.xrp);
        let mut token = Node::new(Side::Token, &config.token_node);
        check_node(&mut xrp, wallets.xrp.account(), &config.token).await?;
        check_node(&mut token, wallets.token.account(), &config.token).await?;
        let client = OutboundClient::new(config.ca_file.as_deref(), config.allow_private)?;
        let mut xrpl = Xrpl::new(config.xrpl_server.clone(), client.clone());
        xrpl.testnet_state().await?;
        // A task per target, as many as the config may name.
        assert!(!config.targets.is_empty() && config.targets.len() <= MAX_TARGETS);
        let mut targets = Vec::with_capacity(config.targets.len());
        for target in &config.targets {
            let (found_sender, found) = watch::channel(None);
            let (feed_sender, feed) = watch::channel(None);
            tokio::spawn(target::follow(
                client.clone(),
                target.clone(),
                Arc::clone(&clock),
                found_sender,
                feed_sender,
                shutdown.clone(),
            ));
            targets.push(Following {
                url: target.url.clone(),
                found,
                feed,
                duplicate: false,
            });
        }
        tracing::info!(
            target: TARGET,
            targets = targets.len(),
            xrp = %wallets.xrp.account(),
            token = %wallets.token.account(),
            "trading through the targets"
        );
        Ok(Self {
            config,
            clock,
            targets,
            xrpl,
            xrp,
            token,
            wallets,
            unbooked: None,
            last_target: None,
        })
    }

    async fn run(&mut self, shutdown: &CancellationToken) {
        let mut ticks = tokio::time::interval(TICK);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Whether the last decision held for want of a current snapshot:
        // then, having done nothing, it decides again as soon as one
        // arrives, rather than a tick on, as at the start.
        let mut unfed = false;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = ticks.tick() => {}
                () = self.feed_arrives(), if unfed => {}
            }
            let now_ms = self.clock.now_ms();
            let current = self.current(now_ms);
            unfed = current.is_empty();
            if unfed {
                tracing::info!(target: TARGET, "hold: no current snapshot from any target");
            } else {
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = self.tick(now_ms, current) => {}
                }
            }
            // A full tick after a decision, however long it took: the
            // nodes book what it did meanwhile.
            ticks.reset();
        }
    }

    /// Resolves once a target's snapshot changes, marking it seen; never once
    /// every target's task has ended.
    async fn feed_arrives(&mut self) {
        let mut changes: FuturesUnordered<_> = self
            .targets
            .iter_mut()
            .map(|following| following.feed.changed())
            .collect();
        while let Some(changed) = changes.next().await {
            if changed.is_ok() {
                return;
            }
        }
        std::future::pending().await
    }

    /// Decides once, at `now_ms`, among the targets with a current snapshot,
    /// and does what it decided.
    async fn tick(&mut self, now_ms: u64, current: Vec<Current>) {
        let pool = match self.xrpl.pool(self.config.token).await {
            Ok(pool) => pool,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "hold: cannot read the pool");
                return;
            }
        };
        let holdings = match self.holdings().await {
            Ok(holdings) => holdings,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "hold: cannot read a node's holding");
                return;
            }
        };
        if self.awaits_booking(now_ms, &holdings).await {
            return;
        }
        // A rebalance needs no target, so peers it cannot read only keep it
        // from trading: every target counts as not peered this tick.
        let peered = match self.peered().await {
            Ok(peered) => peered,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "cannot read a node's peers: no trade this tick");
                Default::default()
            }
        };
        let quotes: Vec<TargetQuote> = current
            .iter()
            .map(|target| TargetQuote {
                target: target.index,
                quotes: target.quotes.clone(),
                peered: peered.contains(&target.node),
            })
            .collect();
        let params = self.config.params;
        let inputs = Inputs {
            targets: &quotes,
            last_target: self.last_target,
            pool,
            holdings,
            params: &params,
        };
        match strategy::decide(&inputs) {
            Decision::Hold(hold) => {
                tracing::info!(target: TARGET, ?hold, targets = ?quotes, ?holdings, "hold");
            }
            Decision::Trade(trade) => {
                let through = current.iter().find(|target| target.index == trade.target);
                assert!(through.is_some(), "a trade through a target it was given");
                if let Some(through) = through {
                    self.last_target = Some(trade.target);
                    self.trade(&trade, through, &inputs, now_ms).await;
                }
            }
            Decision::Rebalance(rebalance) => self.rebalance(&rebalance, &holdings).await,
        }
    }

    /// Whether the last rebalance still waits for its node to book it, at
    /// `now_ms` with `holdings`: nothing is decided meanwhile. Once booked,
    /// or once the wait is over, it waits no more.
    async fn awaits_booking(&mut self, now_ms: u64, holdings: &Holdings) -> bool {
        let Some(unbooked) = self.unbooked else {
            return false;
        };
        // A move that cannot be read counts as one too large to pass by:
        // the rebalance waits, at most until the wait is over.
        let moving = match self.node(unbooked.from).moving().await {
            Ok(moving) => moving,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "cannot read a node's move on its way");
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

    /// The node of `side`.
    const fn node(&mut self, side: Side) -> &mut Node {
        match side {
            Side::Xrp => &mut self.xrp,
            Side::Token => &mut self.token,
        }
    }

    /// What each node holds.
    async fn holdings(&mut self) -> Result<Holdings, String> {
        Ok(Holdings {
            xrp: self.xrp.ledger_state().await?.holding,
            token: self.token.ledger_state().await?.holding,
        })
    }

    /// The targets both nodes are confirmed with now. A trade goes through
    /// a target only where both are, so that the payee's credentials can
    /// name the address beneath it and the payer's first hop is it.
    async fn peered(&mut self) -> Result<BTreeSet<NodeId>, String> {
        let xrp = self.xrp.confirmed_uplinks().await?;
        let token = self.token.confirmed_uplinks().await?;
        Ok(xrp.intersection(&token).copied().collect())
    }

    /// The targets found, each node once, whose snapshot is current at
    /// `now_ms`, in the config's order. A target that is a node an earlier
    /// one is too is passed over, and said so once.
    fn current(&mut self, now_ms: u64) -> Vec<Current> {
        let mut nodes = BTreeSet::new();
        let mut current = Vec::with_capacity(self.targets.len());
        for (index, following) in self.targets.iter_mut().enumerate() {
            let Some(node) = following.found.borrow().as_ref().map(|info| info.node_id) else {
                continue;
            };
            if !nodes.insert(node) {
                if !following.duplicate {
                    following.duplicate = true;
                    tracing::warn!(
                        target: TARGET, url = %following.url, %node,
                        "a target is a node another target is too: trading through it once"
                    );
                }
                continue;
            }
            // Seen, so that waiting for a snapshot waits for a newer one.
            let snapshot = following.feed.borrow_and_update();
            let Some(snapshot) = snapshot
                .as_ref()
                .filter(|snapshot| now_ms < snapshot.valid_until_ms)
            else {
                continue;
            };
            current.push(Current {
                index,
                node,
                url: following.url.clone(),
                quotes: Quotes::new(Arc::clone(snapshot), &self.xrp.method, &self.token.method),
            });
        }
        assert!(current.len() <= MAX_TARGETS);
        current
    }

    async fn trade(
        &mut self,
        trade: &strategy::Trade,
        through: &Current,
        inputs: &Inputs<'_>,
        now_ms: u64,
    ) {
        let (from, to) = match trade.from {
            Side::Xrp => (&mut self.xrp, &mut self.token),
            Side::Token => (&mut self.token, &mut self.xrp),
        };
        let outcome = nodes::trade(
            from,
            to,
            through.node,
            trade,
            &inputs.pool,
            inputs.params,
            now_ms,
        )
        .await;
        let from = trade.from.name();
        let (target, url) = (through.node, &through.url);
        match outcome {
            Ok(TradeOutcome::Declined { receive }) => tracing::info!(
                target: TARGET, %target, %url, from, debit = trade.debit,
                snapshot_receive = trade.receive, quote_receive = receive,
                "trade declined: the quote is below the margin"
            ),
            Ok(TradeOutcome::Paid {
                state,
                sent,
                receive,
            }) => tracing::info!(
                target: TARGET, %target, %url, from, debit = trade.debit, sent,
                quote_receive = receive, back = trade.back, %state, "trade"
            ),
            Err(problem) => tracing::warn!(
                target: TARGET, %target, %url, from, debit = trade.debit, %problem,
                "trade failed"
            ),
        }
    }

    async fn rebalance(&mut self, rebalance: &strategy::Rebalance, holdings: &Holdings) {
        let wallet = self.wallets.of(rebalance.from);
        let destination: AccountId = self.wallets.of(rebalance.from.other()).account();
        let outcome = self.xrpl.rebalance(wallet, destination, rebalance).await;
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

/// Checks that `node` runs its owner book on the account `account`, the
/// one its passphrase opens, so a rebalance pays from and to the nodes'
/// accounts: the XRP node's in XRP, and the token node's in the pool's
/// token, of the issuer its book holds, so what a rebalance delivers to it
/// is booked there. Its hub's ledger holds the same asset
/// ([`Node::ledger_state`]), and the node funds the hub from that book.
async fn check_node(
    node: &mut Node,
    account: AccountId,
    token: &IssuedAsset,
) -> Result<(), String> {
    let state = node.ledger_state().await?;
    check_ledger(node.side, &node.owner_book, &state, account, token)?;
    tracing::info!(target: TARGET, side = node.side.name(), socket = %node.socket().display(), "node ready");
    Ok(())
}

/// What [`check_node`] checks of what the node of `side` says of its
/// owner book `ledger`.
fn check_ledger(
    side: Side,
    ledger: &str,
    state: &LedgerState,
    account: AccountId,
    token: &IssuedAsset,
) -> Result<(), String> {
    if state.deposit_address != account.to_address() {
        return Err(format!(
            "the {} node's account is not the one its passphrase opens",
            side.name()
        ));
    }
    let of_xrp = state.asset_code == "XRP" && state.issuer.is_empty();
    match side {
        Side::Xrp if !of_xrp => Err(format!("{ledger} is not a ledger of XRP")),
        Side::Token if of_xrp || state.issuer != token.issuer.to_address() => Err(format!(
            "{ledger} is not a ledger of the pool's token's issuer"
        )),
        Side::Xrp | Side::Token => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use dassie_xrpl::amount::Currency;

    use super::*;
    use crate::market_maker::strategy::Holding;

    // spec/areas/rate-feed.md#bands: a rebalance counts as booked once
    // the paying node's holding fell by the least it can have cost, and by
    // whatever the node's own move between hub and book has on its way
    // besides, which lowers the holding by up to its amount: a withdrawal
    // from the hub that the book has not seen arrive never passes for it.
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
    fn each_node_runs_its_asset_on_the_account_its_passphrase_opens() {
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
        let check = |side, state: &LedgerState| check_ledger(side, "l", state, account, &token);
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

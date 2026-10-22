//! `dassie market-maker` (spec/areas/rate-feed.md#market-maker): watches a
//! core node's feed, trades through it with two nodes of its own when its
//! rates and an AMM pool on the XRP Ledger leave a margin, and moves its
//! own funds back on the ledger.
//!
//! - [`config`]: the config file, checked whole before anything connects;
//! - [`target`]: who the target is, from its `/stats`;
//! - [`feed`]: its feed over UDP, verified;
//! - [`xrpl`]: the pool, and the rebalancing Payment;
//! - [`nodes`]: the two nodes, over their IPC sockets;
//! - [`strategy`]: what to do, as a pure function of all that.
//!
//! The loop decides once a tick, from the current feed, the pool and both
//! nodes' holdings, and logs one line per decision. Every wait is bounded,
//! and a tick does at most one thing.

pub mod config;
pub mod feed;
pub mod nodes;
pub mod strategy;
pub mod target;
pub mod xrpl;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use dassie_node::clock::{Clock, SystemClock};
use dassie_node::env::{self, Env};
use dassie_node::log;
use dassie_node::peer_client::PeerClient;
use dassie_xrpl::Wallet;
use dassie_xrpl::keys::AccountId;
use lexopt::prelude::*;
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use self::config::Config;
use self::feed::{FeedClient, FeedView};
use self::nodes::{Node, TradeOutcome};
use self::strategy::{Decision, Holdings, Inputs, Rates, Side};
use self::xrpl::Xrpl;

/// What its log lines are under.
pub const TARGET: &str = "das:market-maker";

/// How often it decides.
const TICK: Duration = Duration::from_secs(10);

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
    feeds: watch::Receiver<Option<FeedView>>,
    xrpl: Xrpl,
    xrp: Node,
    token: Node,
    wallets: Wallets,
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
    /// Derives the keys, checks that each node runs its ledger on the
    /// account they open, finds the target and subscribes to its feed.
    async fn start(
        config: Config,
        clock: Arc<dyn Clock>,
        shutdown: CancellationToken,
    ) -> Result<Self, String> {
        let wallets = Wallets {
            xrp: config::wallet_from_file(&config.xrp.passphrase_file)?,
            token: config::wallet_from_file(&config.token_node.passphrase_file)?,
        };
        let mut xrp = Node::new(
            Side::Xrp,
            config.xrp.socket.clone(),
            config.xrp.ledger.clone(),
        );
        let mut token = Node::new(
            Side::Token,
            config.token_node.socket.clone(),
            config.token_node.ledger.clone(),
        );
        check_node(&mut xrp, wallets.xrp.account(), Some("XRP")).await?;
        check_node(&mut token, wallets.token.account(), None).await?;
        let client = PeerClient::new(config.ca_file.as_deref(), config.allow_private)?;
        let info = target::fetch(&client, &config.target.url).await?;
        let port = info
            .feed_port
            .or(config.target.feed_port)
            .ok_or("the target names no feed port, and the config none")?;
        let address = client
            .resolve(&config.target.host, port)
            .await
            .map_err(|e| format!("{}: {e}", config.target.host))?
            .into_iter()
            .next()
            .ok_or("the target's name resolves to no address")?;
        let socket = UdpSocket::bind(unspecified(address))
            .await
            .map_err(|e| format!("cannot open a UDP socket: {e}"))?;
        let feed_client = FeedClient::new(address, info.node_id, info.public_key, clock.now_ms());
        let (sender, feeds) = watch::channel(None);
        tokio::spawn(feed::run(
            socket,
            feed_client,
            Arc::clone(&clock),
            sender,
            shutdown,
        ));
        tracing::info!(
            target: TARGET,
            node = %info.node_id,
            feed = %address,
            xrp = %wallets.xrp.account(),
            token = %wallets.token.account(),
            "trading through the target"
        );
        let xrpl = Xrpl::new(config.xrpl_server.clone(), client);
        Ok(Self {
            config,
            clock,
            feeds,
            xrpl,
            xrp,
            token,
            wallets,
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
        }
    }

    /// Decides once, and does what it decided.
    async fn tick(&mut self) {
        let now_ms = self.clock.now_ms();
        let feed = self
            .feeds
            .borrow()
            .clone()
            .filter(|feed| feed.is_current(now_ms));
        let Some(feed) = feed else {
            tracing::info!(target: TARGET, "hold: no current feed from the target");
            return;
        };
        let rates = Rates {
            fee_ppm: feed.fee_ppm,
            xrp_to_token_e15: feed.rate(&self.xrp.ledger, &self.token.ledger),
            token_to_xrp_e15: feed.rate(&self.token.ledger, &self.xrp.ledger),
        };
        let pool = match self.xrpl.pool(self.config.token).await {
            Ok(pool) => pool,
            Err(problem) => {
                tracing::warn!(target: TARGET, %problem, "hold: cannot read the pool");
                return;
            }
        };
        let holdings = match (
            self.xrp.ledger_state().await,
            self.token.ledger_state().await,
        ) {
            (Ok(xrp), Ok(token)) => Holdings {
                xrp: xrp.holding,
                token: token.holding,
            },
            (Err(problem), _) | (_, Err(problem)) => {
                tracing::warn!(target: TARGET, %problem, "hold: cannot read a node's holding");
                return;
            }
        };
        let params = self.config.params;
        let inputs = Inputs {
            rates,
            pool,
            holdings,
            params: &params,
        };
        match strategy::decide(&inputs) {
            Decision::Hold(hold) => {
                tracing::info!(target: TARGET, ?hold, ?rates, ?holdings, "hold");
            }
            Decision::Trade(trade) => self.trade(&trade, &inputs, now_ms).await,
            Decision::Rebalance(rebalance) => self.rebalance(&rebalance, &holdings).await,
        }
    }

    async fn trade(&mut self, trade: &strategy::Trade, inputs: &Inputs<'_>, now_ms: u64) {
        let (from, to) = match trade.from {
            Side::Xrp => (&mut self.xrp, &mut self.token),
            Side::Token => (&mut self.token, &mut self.xrp),
        };
        let outcome = nodes::trade(from, to, trade, &inputs.pool, inputs.params, now_ms).await;
        let from = trade.from.name();
        match outcome {
            Ok(TradeOutcome::Declined { receive }) => tracing::info!(
                target: TARGET, from, debit = trade.debit, feed_receive = trade.receive,
                quote_receive = receive, "trade declined: the quote is below the margin"
            ),
            Ok(TradeOutcome::Paid {
                state,
                sent,
                receive,
            }) => tracing::info!(
                target: TARGET, from, debit = trade.debit, sent, quote_receive = receive,
                back = trade.back, %state, "trade"
            ),
            Err(problem) => tracing::warn!(
                target: TARGET, from, debit = trade.debit, %problem, "trade failed"
            ),
        }
    }

    async fn rebalance(&mut self, rebalance: &strategy::Rebalance, holdings: &Holdings) {
        let wallet = self.wallets.of(rebalance.from);
        let destination: AccountId = self.wallets.of(rebalance.from.other()).account();
        let outcome = self.xrpl.rebalance(wallet, destination, rebalance).await;
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

/// Checks that `node` runs its ledger on the account `account`, the one
/// its passphrase opens, so a rebalance pays from and to the nodes'
/// accounts: the XRP node's in XRP.
async fn check_node(
    node: &mut Node,
    account: AccountId,
    asset: Option<&str>,
) -> Result<(), String> {
    let state = node.ledger_state().await?;
    if state.deposit_address != account.to_address() {
        return Err(format!(
            "the {} node's account is not the one its passphrase opens",
            node.side.name()
        ));
    }
    if asset.is_some_and(|asset| asset != state.asset_code) {
        return Err(format!("{} is not a ledger of XRP", node.ledger));
    }
    if node.side == Side::Token && state.asset_code == "XRP" {
        return Err(format!("{} is not a ledger of the token", node.ledger));
    }
    tracing::info!(target: TARGET, side = node.side.name(), socket = %node.socket().display(), "node ready");
    Ok(())
}

/// The unspecified address of `address`'s family, on any port.
const fn unspecified(address: SocketAddr) -> SocketAddr {
    let ip = match address {
        SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    SocketAddr::new(ip, 0)
}

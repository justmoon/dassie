//! Setting up a founded hub's escrow on the XRP Ledger, as its sponsor
//! (spec/areas/hubs.md#founding-a-hub): the payment from the node's XRPL
//! account on the hub's network that creates it, then what is missing of
//! its setup, signed by its master key, which the node derives from its
//! own key and the hub's nonce, the master key disabled last. What to do next is
//! `dassie_hub::escrow::setup_step`'s to say, from what the ledger shows;
//! this carries it out, over a WebSocket of its own to the XRPL server.
//!
//! A token hub's issuer cannot stop it: the payment that funds the escrow
//! is in XRP, and the signer list, `RequireDest`, the line to the issuer
//! and disabling the master key move no token, so a freeze, a global
//! freeze or a transfer fee lets each through. A hub founded while the
//! issuer freezes its escrow's line starts frozen once the validators
//! attest it. A transfer fee is for the validators' check of the escrow
//! to refuse, not for founding (spec/areas/hubs.md#what-validators-attest,
//! spec/decisions/token-hubs.md).

use std::sync::Arc;
use std::time::Duration;

use dassie_hub::Rules;
use dassie_hub::escrow::{EscrowSeen, SetupStep, escrow_master, setup_step};
use dassie_identity::NodeKey;
use dassie_settlement::{OwnerPaymentState, PaymentFailure};
use dassie_xrpl::{AccountId, Wallet, api};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::super::xrpl::Session;
use crate::log::target;
use crate::node::Node;

/// How often the escrow is read while it is being set up.
const POLL: Duration = Duration::from_secs(2);

/// How long after a connection failed it is made again.
const RETRY: Duration = Duration::from_secs(5);

/// What the setup tells the founding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The payment that funds the escrow went out: its ID.
    Paid(String),
    /// The escrow is set up as the rules say.
    SetUp,
    /// The payment that funds it failed on the ledger: none is made again.
    Stopped(Stop),
    /// What stops it for now, for the owner.
    Problem(String),
}

/// What the setup works with.
pub struct Setup {
    /// The node.
    pub node: Arc<Node>,
    /// Its key, from which the escrow's master key derives.
    pub key: Arc<NodeKey>,
    /// The hub's rules.
    pub rules: Rules,
    /// The XRPL server.
    pub xrpl: String,
    /// What funds the escrow, in drops.
    pub funding_drops: u64,
    /// The payment that funds it, if one went out already.
    pub payment: Option<String>,
    /// Why no payment funds it again, if one failed on the ledger: the same
    /// payment would fail alike and cost a fee each time. The owner drops
    /// the founding and founds again.
    pub stop: Option<Stop>,
    /// Where it reports.
    pub events: mpsc::Sender<Event>,
}

impl Setup {
    /// Sets the escrow up, until it is or `stop`.
    pub async fn run(mut self, stop: CancellationToken) {
        let master = escrow_master(&self.key, self.rules.terms.network, &self.rules.terms.nonce);
        if master.account().0 != self.rules.terms.escrow {
            let problem = "the hub's escrow is not one the node's key makes".into();
            let _ = self.events.send(Event::Problem(problem)).await;
            return;
        }
        loop {
            let client = self.node.outbound().cloned();
            match Session::connect(client.as_ref(), &self.xrpl).await {
                Ok(mut session) => loop {
                    match self.step(&mut session, &master).await {
                        Ok(true) => {
                            let _ = self.events.send(Event::SetUp).await;
                            return;
                        }
                        Ok(false) => {}
                        Err(problem) => {
                            self.report(problem).await;
                            break;
                        }
                    }
                    if !pause(POLL, &stop).await {
                        return;
                    }
                },
                Err(error) => {
                    self.report(format!("cannot reach the XRPL server: {error}"))
                        .await;
                }
            }
            if !pause(RETRY, &stop).await {
                return;
            }
        }
    }

    async fn report(&self, problem: String) {
        tracing::warn!(target: target::HUB, url = %self.xrpl, %problem, "the hub's escrow is not set up yet");
        let _ = self.events.send(Event::Problem(problem)).await;
    }

    /// Reads the escrow and does the next step: whether it is set up.
    async fn step(&mut self, session: &mut Session, master: &Wallet) -> Result<bool, String> {
        let escrow = AccountId(self.rules.terms.escrow);
        let seen = read(session, escrow).await?;
        // The fee an escrow transaction names: an XRP hub's ledger fee, or
        // a token hub's gas (its ledger fee is none).
        let fee = self.rules.terms.transaction_fee();
        match setup_step(&self.rules, master, seen.as_ref(), fee) {
            SetupStep::Fund => self.fund(escrow).await?,
            SetupStep::Wait => {}
            SetupStep::Submit(transactions) => {
                for transaction in transactions {
                    let answer = session
                        .call(|id| api::submit(id, transaction.blob()))
                        .await?;
                    let result = answer
                        .ok()
                        .as_ref()
                        .and_then(api::submit_result)
                        .unwrap_or_default();
                    tracing::info!(target: target::HUB, escrow = %escrow.to_address(), %result, "submitted a step of the hub's escrow's setup");
                }
            }
            SetupStep::Done => return Ok(true),
            SetupStep::Broken(fault) => {
                return Err(format!(
                    "the escrow is not as the rules say, and its master key cannot mend it: {fault:?}"
                ));
            }
        }
        Ok(false)
    }

    /// Funds the escrow from the node's XRPL account on the hub's network,
    /// the testnet's or mainnet's as its realm is, unless a payment that
    /// does is still on its way or may yet arrive: never twice. One the XRP
    /// scheme cancelled moved nothing, and is paid again; after one the
    /// ledger recorded as failed, none is.
    async fn fund(&mut self, escrow: AccountId) -> Result<(), String> {
        if let Some(stop) = self.stop {
            return Err(stop.problem().into());
        }
        let engine = self.node.engine().ok_or("the ledger is not running")?;
        if let Some(payment) = &self.payment {
            let state = engine
                .xrpl_owner_payment(payment.clone())
                .await
                .map_err(|error| error.to_string())?;
            match state {
                OwnerPaymentState::Cancelled => {}
                OwnerPaymentState::Failed(failure) => {
                    let stop = match failure {
                        PaymentFailure::TooLittle => Stop::TooLittle,
                        PaymentFailure::Other => Stop::Failed,
                    };
                    self.stop = Some(stop);
                    let _ = self.events.send(Event::Stopped(stop)).await;
                    return Err(stop.problem().into());
                }
                OwnerPaymentState::InFlight | OwnerPaymentState::Applied => return Ok(()),
                OwnerPaymentState::Unknown => {
                    return Err("the payment that funds the escrow is not known any more: \
                         the owner looks, and founds again if it never arrived"
                        .into());
                }
            }
        }
        let payment = engine
            .pay_from_xrpl(escrow.to_address(), self.funding_drops)
            .await
            .map_err(|error| format!("cannot fund the escrow: {error}"))?;
        tracing::info!(target: target::HUB, escrow = %escrow.to_address(), drops = self.funding_drops, "funding the hub's escrow");
        self.payment = Some(payment.clone());
        let _ = self.events.send(Event::Paid(payment)).await;
        Ok(())
    }
}

/// Why no payment funds the escrow again: the ledger recorded the last as
/// failed, and the same payment would fail alike, at a fee each time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Too little to create the account: the network's reserve is more
    /// than the founding funds it with.
    TooLittle,
    /// A failure of another kind.
    Failed,
}

impl Stop {
    /// What the founding row keeps of it.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::TooLittle => "too-little",
            Self::Failed => "failed",
        }
    }

    /// The stop a founding row keeps as `code`.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "too-little" => Some(Self::TooLittle),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// What it means, for the owner.
    #[must_use]
    pub const fn problem(self) -> &'static str {
        match self {
            Self::TooLittle => {
                "the payment that funds the escrow was too little to create its account \
                 on the XRP Ledger: drop the founding and found again with more"
            }
            Self::Failed => {
                "the payment that funds the escrow failed on the XRP Ledger: \
                 drop the founding and found again once the owner knows why"
            }
        }
    }
}

/// What the ledger shows of the escrow as of a validated ledger: `None`
/// while it has no account.
async fn read(session: &mut Session, escrow: AccountId) -> Result<Option<EscrowSeen>, String> {
    let info = match session.call(|id| api::account_info(id, escrow)).await? {
        Ok(result) => result,
        Err(error) if error.error == "actNotFound" => return Ok(None),
        Err(error) => return Err(format!("the XRPL server refused: {}", error.error)),
    };
    let objects = session
        .call(|id| api::account_objects(id, escrow, None))
        .await?
        .map_err(|error| format!("the XRPL server refused: {}", error.error))?;
    let unreadable = || "the XRPL server's answer does not read".to_string();
    Ok(Some(EscrowSeen {
        sequence: api::account_info_result(&info)
            .ok_or_else(unreadable)?
            .sequence,
        settings: api::account_settings_result(&info).ok_or_else(unreadable)?,
        objects: api::account_objects_result(&objects).ok_or_else(unreadable)?,
    }))
}

/// Waits `wait`: false if `stop` came first.
async fn pause(wait: Duration, stop: &CancellationToken) -> bool {
    tokio::select! {
        () = stop.cancelled() => false,
        () = tokio::time::sleep(wait) => true,
    }
}

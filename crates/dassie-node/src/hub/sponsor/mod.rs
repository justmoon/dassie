//! Founding hubs as a sponsor (spec/areas/hubs.md#founding-a-hub): the
//! owner proposes terms and picks validators ([`found`]), on the XRP
//! Ledger network of the node's realm ([`founding_network`]); the node asks
//! each validator over the hub's founding stream, as
//! `dassie_hub::founding::Sponsor` says, persisting the founding after
//! every answer that changed it; once a quorum runs the hub, it sets up
//! the escrow ([`escrow`]) and asks the validators until f + 1 say a
//! quorum checked it.
//!
//! The service also keeps the payout bundles of the hub the node is a
//! member of ([`super::payouts`]).

pub mod escrow;
pub mod terms;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use dassie_hub::HubId;
use dassie_hub::founding::{FoundingAnswer, FoundingRequest, Refusal, Sponsor, Stage};
use dassie_identity::{IdentityKey, NodeId};
use dassie_oer::{Decode, Encode};
use dassie_xrpl::AccountId;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::HubClient;
use super::driver::hex;
use crate::clock::clock_moved;
use crate::log::target;
use crate::node::Node;
use crate::store::{FoundingRow, StoreError};

/// What funds a new hub's escrow unless the owner says otherwise, in
/// drops: the XRP Ledger's reserve for the account, its signer list and,
/// for a token hub, its trust line, the fees of its setup, and room for
/// the fees of the hub's own transactions (at least [`least_funding`]).
pub const ESCROW_FUNDING_DROPS: u64 = 2_000_000;

/// The XRP Ledger's reserve for each object an account owns, in drops,
/// the testnet's and mainnet's alike: the escrow's signer list is one, and
/// a token hub's trust line another.
pub const LEDGER_OWNER_RESERVE: u64 = 200_000;

/// The least that funds the escrow of a hub of `terms`, in drops: the XRP
/// Ledger's reserves for the account and its signer list, and the fees of
/// its three setup transactions; a token hub's also the reserve of its
/// trust line and the fee of the `TrustSet` that makes it, at its gas.
/// Less, and the payment that should create it fails, or the setup cannot
/// be paid.
#[must_use]
pub fn least_funding(terms: &dassie_hub::rules::Terms) -> u64 {
    let (objects, transactions) = match terms.asset {
        dassie_hub::rules::Asset::Xrp => (1, 3),
        dassie_hub::rules::Asset::Token { .. } => (2, 4),
    };
    dassie_hub::rules::LEDGER_BASE_RESERVE
        .saturating_add(LEDGER_OWNER_RESERVE.saturating_mul(objects))
        .saturating_add(terms.transaction_fee().saturating_mul(transactions))
}

/// The longest one request to a validator takes.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest a founding waits before it looks at the system clock again,
/// which may jump; a clock moved by hand wakes it when it moves.
const MAX_WAIT: Duration = Duration::from_millis(500);

/// The hubs the node founds: what wakes the service when the owner starts
/// or drops one.
#[derive(Debug)]
pub struct Sponsoring {
    changed: watch::Sender<u64>,
}

impl Default for Sponsoring {
    fn default() -> Self {
        Self {
            changed: watch::channel(0).0,
        }
    }
}

impl Sponsoring {
    fn wake(&self) {
        self.changed
            .send_modify(|count| *count = count.wrapping_add(1));
    }
}

/// What the owner asks for.
#[derive(Clone, Debug)]
pub struct FoundRequest {
    /// The validators, each by its node and where it is reached.
    pub validators: Vec<(NodeId, String)>,
    /// The terms that differ from the starting ones, as JSON
    /// ([`terms::terms`]); empty for none.
    pub terms: String,
    /// What funds the escrow, in drops; the default if `None`.
    pub funding_drops: Option<u64>,
    /// The XRPL server the escrow is set up on; the XRP scheme's if `None`.
    pub xrpl: Option<String>,
}

/// Why a founding does not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FoundError {
    /// What the owner asked for is wrong.
    Invalid(String),
    /// The node cannot found now: no key yet, or as many foundings as it
    /// keeps.
    Precondition(String),
}

/// A validator of a founding, as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitedStatus {
    /// Its node.
    pub node: NodeId,
    /// Where the node reaches it.
    pub endpoint: String,
    /// Whether it accepted the terms.
    pub accepted: bool,
    /// Whether it signed the genesis.
    pub signed: bool,
    /// Whether it runs the hub.
    pub running: bool,
    /// Whether it says a quorum checked the escrow.
    pub escrow_checked: bool,
    /// Its last refusal.
    pub refusal: Option<Refusal>,
}

/// A founding, as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundingStatus {
    /// The terms' nonce.
    pub nonce: [u8; 32],
    /// Where it stands.
    pub stage: Stage,
    /// The hub, once its rules are fixed, and the rules, encoded.
    pub hub: Option<(HubId, Vec<u8>)>,
    /// The escrow's account.
    pub escrow: AccountId,
    /// The payment that funds it, once it went out.
    pub escrow_payment: Option<String>,
    /// Whether it is set up.
    pub escrow_set_up: bool,
    /// The validators.
    pub validators: Vec<InvitedStatus>,
    /// What stops it for now.
    pub problem: Option<String>,
    /// When the owner asked for it.
    pub created_ms: u64,
}

/// The XRP Ledger network a node of `realm` founds hubs on, whose XRP
/// funds their escrows: the testnet in the test realm, mainnet in the live
/// realm, which runs only where it may (spec/decisions/real-money.md).
#[must_use]
pub const fn founding_network(
    realm: dassie_node_core::config::Realm,
) -> &'static dassie_xrpl::network::Network {
    match realm {
        dassie_node_core::config::Realm::Test => &dassie_xrpl::network::TESTNET,
        dassie_node_core::config::Realm::Live => &dassie_xrpl::network::MAINNET,
    }
}

/// Refuses terms no node would join: a hub whose network and asset name no
/// hub scheme (spec/decisions/hub-scheme-names.md), as a token other than
/// DSUSD on the testnet or RLUSD on mainnet; or one whose margin and
/// allowance do not fit the connector's message window, which a node
/// peers over a hub only within. The escrow would fund a hub nobody uses.
fn joinable(terms: &dassie_hub::rules::Terms, validators: usize) -> Result<(), FoundError> {
    if dassie_hub::member::scheme_of(terms).is_none() {
        return Err(FoundError::Invalid(
            "no settlement scheme joins a hub of this asset on this network: XRP, or the \
             testnet's DSUSD, or mainnet's RLUSD"
                .into(),
        ));
    }
    let window_ms = dassie_connector::Config::DEFAULT.message_window_ms;
    let (margin, allowance) = (terms.timers.margin, terms.timers.allowance);
    if margin.saturating_add(allowance) > window_ms {
        return Err(FoundError::Invalid(format!(
            "the margin of {margin} ms that {validators} validators need, with the \
             allowance of {allowance} ms, exceeds the {window_ms} ms nodes leave for a \
             hub, so no node would join it: found it with fewer validators or a smaller \
             allowance"
        )));
    }
    Ok(())
}

/// Starts founding a hub with the validators and terms the owner asked for.
///
/// # Errors
///
/// See [`FoundError`].
pub fn found(node: &Node, request: FoundRequest) -> Result<FoundingStatus, FoundError> {
    let key = node
        .node_key()
        .ok_or_else(|| FoundError::Precondition("the node is not set up".into()))?;
    let nonce: [u8; 32] = node.with_rng(|rng| {
        let mut nonce = [0; 32];
        rng.fill_bytes(&mut nonce);
        nonce
    });
    // Keys at the path that names the network
    // (spec/decisions/hub-keys-by-network.md).
    let network = founding_network(node.config().realm);
    let master = dassie_hub::escrow::escrow_master(&key, network.id, &nonce);
    let base =
        dassie_hub::founding::starting_terms(nonce, key.identity(), network.id, master.account().0);
    let mut terms = terms::terms(base, &request.terms).map_err(FoundError::Invalid)?;
    // The margin covers finality with as many validators faulty as these
    // tolerate, or no validator would sign the genesis of these rules.
    let least_margin = dassie_hub::rules::least_margin_ms(request.validators.len());
    terms.timers.margin = terms.timers.margin.max(least_margin);
    joinable(&terms, request.validators.len())?;
    let funding_drops = request.funding_drops.unwrap_or(ESCROW_FUNDING_DROPS);
    let least = least_funding(&terms);
    if funding_drops < least {
        return Err(FoundError::Invalid(format!(
            "the escrow needs at least {least} drops to be created and set up"
        )));
    }
    let sponsor = Sponsor::new(terms, request.validators).map_err(|error| {
        FoundError::Invalid(format!(
            "{error:?}: 4 to 16 validators, each once, at a host:port"
        ))
    })?;
    let xrpl = match request.xrpl {
        Some(url) if url.starts_with("wss://") || url.starts_with("ws://") => url,
        Some(_) => {
            return Err(FoundError::Invalid(
                "the XRPL server is no WebSocket URL".into(),
            ));
        }
        None => xrpl_server(node, network),
    };
    let now = node.now_ms();
    let row = FoundingRow {
        nonce,
        state: sponsor.to_vec(),
        xrpl,
        funding_drops,
        escrow_payment: None,
        problem: None,
        escrow_stop: None,
        created_at_ms: now,
        updated_at_ms: now,
    };
    let kept = node
        .store_handle()
        .put_hub_founding(&row)
        .map_err(|error| FoundError::Precondition(error.to_string()))?;
    if !kept {
        return Err(FoundError::Precondition(format!(
            "the node keeps {} foundings: drop one first",
            crate::store::MAX_FOUNDINGS
        )));
    }
    tracing::info!(target: target::HUB, nonce = %hex(&nonce), "founding a hub");
    node.sponsoring().wake();
    status_of(&row).map_err(|error| FoundError::Precondition(error.to_string()))
}

/// The XRPL server of the node's account on `network`, whose XRP funds
/// the escrow: the owner's config of it, or the network's server.
fn xrpl_server(node: &Node, network: &dassie_xrpl::network::Network) -> String {
    let configs = node.store_handle().asset_configs().unwrap_or_default();
    account_server(&configs, network)
}

/// The XRPL server that the owner's config of the node's account on
/// `network`, among `configs`, names, or the network's
/// (spec/areas/settlement.md#external-assets).
fn account_server(configs: &[(String, String)], network: &dassie_xrpl::network::Network) -> String {
    let account = network.first().to_string();
    configs
        .iter()
        .find(|(asset, _)| *asset == account)
        .and_then(|(_, config)| dassie_xrpl::account::Config::parse_on(config, network).ok())
        .map_or_else(|| network.server.into(), |config| config.server)
}

/// Drops the founding of `nonce`: the node asks nothing more for it, and
/// sets up no escrow. Whether there was one.
///
/// # Errors
///
/// The database failed.
pub fn abandon(node: &Node, nonce: &[u8; 32]) -> Result<bool, StoreError> {
    let removed = node.store_handle().remove_hub_founding(nonce)?;
    if removed {
        tracing::info!(target: target::HUB, nonce = %hex(nonce), "dropped a hub founding");
        node.sponsoring().wake();
    }
    Ok(removed)
}

/// The node's foundings, as the owner sees them.
///
/// # Errors
///
/// The database failed, or holds a founding that does not read.
pub fn statuses(node: &Node) -> Result<Vec<FoundingStatus>, StoreError> {
    node.store_handle()
        .hub_foundings()?
        .iter()
        .map(status_of)
        .collect()
}

fn sponsor_of(row: &FoundingRow) -> Result<Sponsor, StoreError> {
    Sponsor::decode_exact(&row.state).map_err(|_| StoreError::Corrupt("a hub founding"))
}

fn status_of(row: &FoundingRow) -> Result<FoundingStatus, StoreError> {
    let sponsor = sponsor_of(row)?;
    let validators = sponsor
        .invited()
        .iter()
        .map(|invited| InvitedStatus {
            node: invited.node,
            endpoint: invited.endpoint.clone(),
            accepted: invited.acceptance.is_some(),
            signed: invited.signature.is_some(),
            running: invited.running,
            escrow_checked: invited.escrow_checked,
            refusal: invited.refusal,
        })
        .collect();
    Ok(FoundingStatus {
        nonce: row.nonce,
        stage: sponsor.stage(),
        hub: sponsor.rules().map(|rules| (rules.id(), rules.to_vec())),
        escrow: AccountId(sponsor.terms().escrow),
        escrow_payment: row.escrow_payment.clone(),
        escrow_set_up: sponsor.escrow_set_up(),
        validators,
        problem: row.problem.clone(),
        created_ms: row.created_at_ms,
    })
}

/// Drives the node's foundings until `shutdown`, once it has its key, and
/// keeps the payout bundles of the hub it is a member of.
pub async fn run(node: Arc<Node>, shutdown: CancellationToken) {
    tokio::join!(
        foundings(Arc::clone(&node), shutdown.clone()),
        super::payouts::run(node, shutdown),
    );
}

async fn foundings(node: Arc<Node>, shutdown: CancellationToken) {
    let mut identity = node.identity();
    let key = tokio::select! {
        () = shutdown.cancelled() => return,
        key = identity.wait_for(Option::is_some) => key.ok().and_then(|key| key.clone()),
    };
    let Some(key) = key else {
        return shutdown.cancelled().await;
    };
    let client = HubClient::of(&node);
    let tasks = TaskTracker::new();
    let mut driving: BTreeMap<[u8; 32], CancellationToken> = BTreeMap::new();
    let mut changed = node.sponsoring().changed.subscribe();
    loop {
        changed.mark_unchanged();
        let rows = match node.store_handle().hub_foundings() {
            Ok(rows) => rows,
            Err(error) => {
                tracing::error!(target: target::HUB, %error, "cannot read the hub foundings");
                Vec::new()
            }
        };
        driving.retain(|nonce, stop| {
            let kept = rows.iter().any(|row| row.nonce == *nonce);
            if !kept {
                stop.cancel();
            }
            kept && !stop.is_cancelled()
        });
        for row in rows {
            if driving.contains_key(&row.nonce) {
                continue;
            }
            let Ok(sponsor) = sponsor_of(&row) else {
                tracing::error!(target: target::HUB, nonce = %hex(&row.nonce), "a hub founding does not read");
                continue;
            };
            let done = sponsor.stage() == Stage::Founded
                && sponsor.invited().iter().all(|invited| invited.running);
            if done {
                continue;
            }
            let stop = shutdown.child_token();
            driving.insert(row.nonce, stop.clone());
            let founding = Founding {
                node: Arc::clone(&node),
                key: Arc::clone(&key),
                client: client.clone(),
                row,
                sponsor,
            };
            tasks.spawn(founding.drive(stop));
        }
        tokio::select! {
            () = shutdown.cancelled() => break,
            result = changed.changed() => if result.is_err() { break },
        }
    }
    tasks.close();
    tasks.wait().await;
}

/// One founding at work.
struct Founding {
    node: Arc<Node>,
    key: Arc<IdentityKey>,
    client: HubClient,
    row: FoundingRow,
    sponsor: Sponsor,
}

impl Founding {
    /// Asks the validators, and sets up the escrow, until the hub is
    /// founded and every validator runs it, the owner drops it, or `stop`.
    async fn drive(mut self, stop: CancellationToken) {
        let (answered, mut answers) = mpsc::channel(2 * dassie_hub::MAX_VALIDATORS);
        let (events, mut escrow) = mpsc::channel(8);
        let mut setting_up = false;
        let asks = TaskTracker::new();
        let clock = self.node.clock();
        let mut moves = clock.subscribe();
        loop {
            let now = clock.now_ms();
            for (index, request) in self.sponsor.due(now) {
                self.ask(index, &request, &answered, &asks, &stop);
            }
            if !setting_up && self.sponsor.stage() >= Stage::Escrow && !self.sponsor.escrow_set_up()
            {
                setting_up = self.set_up_escrow(&events, &asks, &stop);
            }
            if self.done() {
                break;
            }
            let wait = self
                .sponsor
                .deadline()
                .map_or(MAX_WAIT, |at| Duration::from_millis(at.saturating_sub(now)))
                .min(MAX_WAIT);
            let kept = tokio::select! {
                () = stop.cancelled() => break,
                Some((index, answer)) = answers.recv() => {
                    let changed = self.sponsor.answer(index, clock.now_ms(), answer);
                    !changed || self.save()
                }
                Some(event) = escrow.recv() => self.escrow(event),
                () = tokio::time::sleep(wait) => true,
                () = clock_moved(&mut moves) => true,
            };
            if !kept {
                break;
            }
        }
        stop.cancel();
        asks.close();
        asks.wait().await;
    }

    /// Whether the founding is done: the hub founded, and every validator
    /// runs it.
    fn done(&self) -> bool {
        self.sponsor.stage() == Stage::Founded
            && self.sponsor.invited().iter().all(|invited| invited.running)
    }

    /// Asks validator `index`; its answer, or that none came, goes to
    /// `answered`.
    fn ask(
        &self,
        index: usize,
        request: &FoundingRequest,
        answered: &mpsc::Sender<(usize, Option<FoundingAnswer>)>,
        asks: &TaskTracker,
        stop: &CancellationToken,
    ) {
        let Some(invited) = self.sponsor.invited().get(index) else {
            return;
        };
        let (node, endpoint) = (invited.node, invited.endpoint.clone());
        let (client, answered, stop) = (self.client.clone(), answered.clone(), stop.clone());
        let request = request.to_vec();
        asks.spawn(async move {
            let exchanged = tokio::select! {
                () = stop.cancelled() => return,
                exchanged = client.found(node, &endpoint, &request, EXCHANGE_TIMEOUT) => exchanged,
            };
            let answer = match exchanged {
                Ok(bytes) => FoundingAnswer::decode_exact(&bytes).ok(),
                Err(error) => {
                    tracing::debug!(target: target::HUB, %node, %error, "a validator did not answer the founding");
                    None
                }
            };
            let _ = answered.send((index, answer)).await;
        });
    }

    /// Starts setting up the escrow: whether it started.
    fn set_up_escrow(
        &self,
        events: &mpsc::Sender<escrow::Event>,
        asks: &TaskTracker,
        stop: &CancellationToken,
    ) -> bool {
        let Some(rules) = self.sponsor.rules() else {
            return false;
        };
        let setup = escrow::Setup {
            node: Arc::clone(&self.node),
            key: Arc::clone(&self.key),
            rules,
            xrpl: self.row.xrpl.clone(),
            funding_drops: self.row.funding_drops,
            payment: self.row.escrow_payment.clone(),
            stop: self
                .row
                .escrow_stop
                .as_deref()
                .and_then(escrow::Stop::from_code),
            events: events.clone(),
        };
        asks.spawn(setup.run(stop.clone()));
        true
    }

    /// Takes what the escrow's setup reports: whether the founding is
    /// still kept.
    fn escrow(&mut self, event: escrow::Event) -> bool {
        match event {
            escrow::Event::Paid(payment) => {
                self.row.escrow_payment = Some(payment);
                self.row.problem = None;
            }
            escrow::Event::SetUp => {
                tracing::info!(target: target::HUB, nonce = %hex(&self.row.nonce), "the hub's escrow is set up");
                self.sponsor.set_escrow_set_up(self.node.now_ms());
                self.row.problem = None;
            }
            escrow::Event::Stopped(stop) => {
                tracing::warn!(target: target::HUB, nonce = %hex(&self.row.nonce), problem = stop.problem(), "funding the hub's escrow stopped");
                self.row.escrow_stop = Some(stop.code().into());
                self.row.problem = Some(stop.problem().into());
            }
            escrow::Event::Problem(problem) => {
                let mut problem = problem;
                problem.truncate(500);
                self.row.problem = Some(problem);
            }
        }
        self.save()
    }

    /// Writes the founding: whether the owner still keeps it.
    fn save(&mut self) -> bool {
        self.row.state = self.sponsor.to_vec();
        self.row.updated_at_ms = self.node.now_ms();
        match self.node.store_handle().update_hub_founding(&self.row) {
            Ok(kept) => kept,
            Err(error) => {
                tracing::error!(target: target::HUB, %error, "cannot keep a hub founding");
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dassie_node_core::config::Realm;
    use dassie_xrpl::network::{MAINNET, TESTNET};

    // spec/areas/settlement.md#external-assets: the escrow is funded over
    // the server the owner's config of the account on the hub's network
    // names, whichever schemes run on it; the other network's config
    // counts for nothing.
    #[test]
    fn the_escrow_goes_over_the_accounts_server() {
        let testnet = (
            "xrpl-testnet-account/0".to_string(),
            r#"{"faucet":null,"server":"ws://ledger.test"}"#.to_string(),
        );
        let mainnet = (
            "xrpl-account/0".to_string(),
            r#"{"server":"ws://a"}"#.to_string(),
        );
        let both = [testnet.clone(), mainnet.clone()];
        assert_eq!(super::account_server(&both, &TESTNET), "ws://ledger.test");
        assert_eq!(super::account_server(&both, &MAINNET), "ws://a");
        assert_eq!(
            super::account_server(&[mainnet], &TESTNET),
            dassie_xrpl::account::DEFAULT_SERVER
        );
        assert_eq!(super::account_server(&[testnet], &MAINNET), MAINNET.server);
        assert_eq!(super::account_server(&[], &MAINNET), MAINNET.server);
    }

    // spec/areas/hubs.md#founding-a-hub: a node founds hubs on its realm's
    // network: the testnet in the test realm, mainnet in the live realm.
    #[test]
    fn a_node_founds_on_its_realms_network() {
        assert_eq!(super::founding_network(Realm::Test), &TESTNET);
        assert_eq!(super::founding_network(Realm::Live), &MAINNET);
    }

    // spec/decisions/hub-scheme-names.md: a hub no scheme joins is not
    // founded: a token other than the network's dollar token.
    #[test]
    fn a_hub_no_scheme_joins_is_not_founded() {
        use dassie_hub::rules::Asset;

        let key = dassie_identity::TableKeygen::identity_key(&[3; 32]);
        let terms = |network: &dassie_xrpl::network::Network, asset| {
            let mut terms =
                dassie_hub::founding::starting_terms([1; 32], key.identity(), network.id, [2; 20]);
            terms.asset = asset;
            terms
        };
        let dsusd = Asset::Token {
            currency: dassie_xrpl::dsusd::CURRENCY,
            issuer: dassie_xrpl::dsusd::ISSUER,
        };
        let rlusd = Asset::Token {
            currency: dassie_xrpl::rlusd::CURRENCY,
            issuer: dassie_xrpl::rlusd::ISSUER,
        };
        for (network, asset) in [
            (&TESTNET, Asset::Xrp),
            (&TESTNET, dsusd),
            (&MAINNET, Asset::Xrp),
            (&MAINNET, rlusd),
        ] {
            assert_eq!(super::joinable(&terms(network, asset), 4), Ok(()));
        }
        for (network, asset) in [(&TESTNET, rlusd), (&MAINNET, dsusd)] {
            assert!(matches!(
                super::joinable(&terms(network, asset), 4),
                Err(super::FoundError::Invalid(why)) if why.contains("no settlement scheme")
            ));
        }
    }
}

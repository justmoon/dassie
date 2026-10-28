//! `dassie setup`: sets a node up from its owner's passphrase, with no
//! browser, for an owner who keeps the phrase on the node's own host, as a
//! headless node or a market maker's nodes must
//! (spec/areas/node.md#the-dassie-command,
//! spec/decisions/headless-setup.md).
//!
//! It does what the wallet does: it derives the node key from the phrase,
//! and posts it with the setup token to `/api/setup`
//! (spec/areas/http.md#setup). So the node checks a setup from
//! here exactly as one from a browser, and the phrase never reaches the
//! node, only the key does, with the realm the phrase names, which is the
//! node's for good then (spec/decisions/mainnet-schemes.md); a node that
//! may not run that realm refuses the setup, as it refuses the wallet's.
//! Then it waits for the engine, sets
//! the config of each external asset it is given, and adds each settlement
//! scheme it is given that is not active. Then it joins settlement hubs
//! (spec/decisions/hubs-only.md): each one `--hub` names by the scheme over
//! it, or, with no `--hub`, the realm's default hub of XRP in the test
//! realm and none in the live realm, since on mainnet a node joins a hub
//! only once its owner turns hubs on
//! (spec/designs/settlement-hubs.md#11-choosing-a-hub); `--hub none` joins
//! none. For each it adds the owner books the hub funds from, peering with
//! nobody, unless they are active, then the hub's scheme with no config,
//! which joins the default hub of its asset. With `--keep-funds`, it
//! first sets where the node keeps what it receives in each asset a hub
//! holds: in the hub, the default, or on the owner's own account
//! (spec/areas/payments.md#where-received-money-is-kept).
//!
//! A phrase names its realm (spec/decisions/phrase-format.md), so no flag
//! does: one could only agree with the phrase or refuse it. A phrase that
//! needs correcting, or that says it was made more than a day after the
//! host's clock, is refused, since no one is here to confirm or to set the
//! clock.
//!
//! Running it again changes nothing, and a node set up with another phrase,
//! or in another realm than the phrase's, is refused, untouched. Every
//! call is bounded in time, every wait in attempts, and each step prints
//! one line.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use dassie_identity::NodeKey;
use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response};
use dassie_node::tls;
use dassie_node_core::config::Realm;
use dassie_node_core::hex;
use dassie_node_core::setup::SetupRequest;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;
use lexopt::prelude::*;
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest as _, Sha256};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::client::{CallError, Client};
use crate::passphrase;

/// Most `--scheme` flags.
const MAX_SCHEMES: usize = 16;
/// Most `--asset` flags.
const MAX_ASSETS: usize = 8;
/// Exit code for a node that is set up with another passphrase.
const OTHER_NODE: u8 = 3;
/// How long one call over the IPC socket may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long connecting to the node's HTTPS listener at one address, with
/// the TLS handshake, may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the setup request may take, from the first connection to the
/// answer's last byte.
const SETUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Most bytes of the node's answer to the setup request read: a refusal's
/// sentence.
const MAX_ANSWER_BYTES: usize = 4 * 1024;
/// Most characters of a refusal shown.
const MAX_REFUSAL_CHARS: usize = 200;
/// Time between two polls while waiting for the engine or a scheme.
const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Polls before giving up on the engine or a scheme: a minute.
const MAX_POLLS: u32 = 300;
/// Bytes of a setup token, as hex.
const TOKEN_HEX_CHARS: usize = 2 * dassie_node_core::setup::SETUP_TOKEN_BYTES;
/// A settlement hub's scheme, which `--hub` names: the node lists it only
/// while its build has a default hub of the scheme's asset built in, or
/// runs it (spec/areas/hubs.md#in-the-node).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubScheme {
    /// The scheme over the hub.
    scheme: &'static str,
    /// The realm whose ledgers it books on, the only one it runs in.
    realm: Realm,
    /// The owner books it funds from (spec/areas/settlement.md#owner-books),
    /// in the order setup adds them: a token hub's XRP book, which pays
    /// its escrow's gas, before the book of its token.
    books: &'static [&'static str],
}

/// Every settlement hub's scheme, one per network and asset
/// (spec/decisions/hub-scheme-names.md): the testnet's XRP hub first, the
/// test realm's default, and mainnet's XRP hub third, the live realm's.
const HUB_SCHEMES: [HubScheme; 4] = [
    HubScheme {
        scheme: "hub-xrpl-testnet-xrp",
        realm: Realm::Test,
        books: &[dassie_xrpl::scheme::SCHEME_ID],
    },
    HubScheme {
        scheme: "hub-xrpl-testnet-dsusd",
        realm: Realm::Test,
        books: &[
            dassie_xrpl::scheme::SCHEME_ID,
            dassie_xrpl::dsusd::SCHEME_ID,
        ],
    },
    HubScheme {
        scheme: "hub-xrpl-xrp",
        realm: Realm::Live,
        books: &[dassie_xrpl::scheme::MAINNET_SCHEME_ID],
    },
    HubScheme {
        scheme: "hub-xrpl-rlusd",
        realm: Realm::Live,
        books: &[
            dassie_xrpl::scheme::MAINNET_SCHEME_ID,
            dassie_xrpl::rlusd::SCHEME_ID,
        ],
    },
];

impl HubScheme {
    /// The scheme over the default hub of XRP of `realm`, which setup joins
    /// with no `--hub` in the test realm: on the testnet
    /// `hub-xrpl-testnet-xrp`, funded from `xrpl-testnet-xrp`; on mainnet
    /// `hub-xrpl-xrp`, funded from `xrpl-xrp`.
    const fn xrp_of(realm: Realm) -> Self {
        match realm {
            Realm::Test => HUB_SCHEMES[0],
            Realm::Live => HUB_SCHEMES[2],
        }
    }

    /// The hub scheme `id`, if it is one.
    fn named(id: &str) -> Option<Self> {
        HUB_SCHEMES.into_iter().find(|hub| hub.scheme == id)
    }
}

/// The settlement hubs `dassie setup` joins (`--hub`).
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Hubs {
    /// No `--hub`: the realm's default, which is the default hub of XRP in
    /// the test realm and none in the live realm, where a node joins a hub
    /// only once its owner turns hubs on.
    #[default]
    Default,
    /// `--hub none`: none.
    None,
    /// Those `--hub` names, by their schemes, in order.
    Named(Vec<HubScheme>),
}

impl Hubs {
    /// Adds what one `--hub` names: a hub's scheme, or `none`, which takes
    /// no other.
    fn add(&mut self, name: &str) -> Result<(), lexopt::Error> {
        let hub = if name == "none" {
            None
        } else {
            Some(HubScheme::named(name).ok_or_else(|| {
                let schemes: Vec<&str> = HUB_SCHEMES.iter().map(|hub| hub.scheme).collect();
                format!(
                    "--hub {name} names no settlement hub's scheme: name one of {}, or none",
                    schemes.join(", ")
                )
            })?)
        };
        match (&mut *self, hub) {
            (Self::Default, None) => *self = Self::None,
            (Self::Default, Some(hub)) => *self = Self::Named(vec![hub]),
            (Self::None, None) => return Err("--hub none given twice".into()),
            (Self::None, Some(hub)) => return Err(none_and(hub.scheme)),
            (Self::Named(named), None) => {
                let first = named.first().map_or("", |hub| hub.scheme);
                return Err(none_and(first));
            }
            (Self::Named(named), Some(hub)) => {
                if named.contains(&hub) {
                    return Err(format!("--hub {} given twice", hub.scheme).into());
                }
                named.push(hub);
            }
        }
        Ok(())
    }
}

/// Why `--hub none` and `--hub <scheme>` are refused together.
fn none_and(scheme: &str) -> lexopt::Error {
    format!("--hub none joins no settlement hub, and --hub {scheme} one").into()
}

/// What `dassie setup` was asked to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// The file that holds the owner's passphrase.
    pub passphrase_file: PathBuf,
    /// The settlement schemes the node should have, in order.
    pub schemes: Vec<Scheme>,
    /// The external assets' configs, set before any scheme is added.
    pub assets: Vec<Scheme>,
    /// The settlement hubs to join.
    pub hubs: Hubs,
    /// Where the node keeps what it receives over Interledger in each
    /// asset a settlement hub holds (`--keep-funds`), if the owner says;
    /// otherwise the node's choice stays as it is, in the hub for a new
    /// node.
    pub keep_funds: Option<KeepFunds>,
}

/// Where the node keeps what it receives in an asset a settlement hub
/// holds (spec/areas/payments.md#where-received-money-is-kept).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepFunds {
    /// In the hub, where it arrives (`--keep-funds hub`).
    Hub,
    /// On the owner's own account on the hub's underlying ledger: the node
    /// withdraws what the hub holds beyond its ceiling
    /// (`--keep-funds account`).
    OwnAccount,
}

/// A settlement scheme to add, or an external asset to configure, with
/// its config.
#[derive(Debug, PartialEq, Eq)]
pub struct Scheme {
    /// Its ID, for example `stub` or `xrpl-testnet-account/0`.
    pub id: String,
    /// Its config, JSON; empty for none.
    pub config: String,
}

/// Reads `dassie setup`'s flags.
pub fn args(parser: &mut lexopt::Parser) -> Result<Options, lexopt::Error> {
    let mut passphrase_file = None;
    let mut schemes: Vec<Scheme> = Vec::new();
    let mut assets: Vec<Scheme> = Vec::new();
    let mut hubs = Hubs::Default;
    let mut keep_funds = None;
    while let Some(argument) = parser.next()? {
        match argument {
            Long("passphrase-file") => passphrase_file = Some(PathBuf::from(parser.value()?)),
            Long("no-hub") => {
                return Err("--no-hub is gone: pass --hub none to join no settlement hub".into());
            }
            Long("hub") => hubs.add(&parser.value()?.string()?)?,
            Long("keep-funds") => {
                keep_funds = Some(match parser.value()?.string()?.as_str() {
                    "hub" => KeepFunds::Hub,
                    "account" => KeepFunds::OwnAccount,
                    other => {
                        return Err(format!(
                            "--keep-funds {other:?}: say hub, or account for your own account"
                        )
                        .into());
                    }
                });
            }
            Long("scheme") => {
                let scheme = scheme("scheme", &parser.value()?.string()?)?;
                add_once("scheme", &mut schemes, scheme, MAX_SCHEMES)?;
            }
            Long("asset") => {
                let asset = scheme("asset", &parser.value()?.string()?)?;
                add_once("asset", &mut assets, asset, MAX_ASSETS)?;
            }
            other => return Err(other.unexpected()),
        }
    }
    let passphrase_file = passphrase_file.ok_or("--passphrase-file is required")?;
    Ok(Options {
        passphrase_file,
        schemes,
        assets,
        hubs,
        keep_funds,
    })
}

/// Adds what `--<flag>` names to `known`, at most `most` of them, and each
/// once.
fn add_once(
    flag: &str,
    known: &mut Vec<Scheme>,
    new: Scheme,
    most: usize,
) -> Result<(), lexopt::Error> {
    if known.len() == most {
        return Err(format!("more than {most} --{flag} flags").into());
    }
    if known.iter().any(|known| known.id == new.id) {
        return Err(format!("--{flag} {} given twice", new.id).into());
    }
    known.push(new);
    Ok(())
}

/// `<id>` or `<id>=<config>`, as `--<flag>` gives it: the config is
/// everything after the first `=`.
fn scheme(flag: &str, text: &str) -> Result<Scheme, lexopt::Error> {
    let (id, config) = text.split_once('=').unwrap_or((text, ""));
    if id.is_empty() {
        return Err(format!("--{flag} {text:?} names no {flag}").into());
    }
    Ok(Scheme {
        id: id.to_owned(),
        config: config.to_owned(),
    })
}

/// Why `dassie setup` stopped.
#[derive(Debug)]
enum Failure {
    /// The node is set up with another passphrase; nothing was changed.
    OtherNode(String),
    /// Anything else.
    Failed(String),
}

impl From<String> for Failure {
    fn from(problem: String) -> Self {
        Self::Failed(problem)
    }
}

impl From<CallError> for Failure {
    fn from(error: CallError) -> Self {
        Self::Failed(error.message)
    }
}

/// Runs `dassie setup`.
pub fn run(options: &Options) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::FAILURE;
    };
    let env = Env::from_process();
    match runtime.block_on(setup(&env, options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::OtherNode(problem)) => {
            eprintln!("{problem}");
            ExitCode::from(OTHER_NODE)
        }
        Err(Failure::Failed(problem)) => {
            eprintln!("{problem}");
            ExitCode::FAILURE
        }
    }
}

/// The realm the passphrase file's phrase names, which the node is set up
/// in, and the node key of the phrase in it.
fn node_key(options: &Options) -> Result<(Realm, NodeKey), String> {
    let (realm, key) =
        passphrase::own_realm_key_from_file(&options.passphrase_file, passphrase::now_ms())?;
    let realm = match realm {
        dassie_identity::Realm::Test => Realm::Test,
        dassie_identity::Realm::Live => Realm::Live,
    };
    Ok((realm, key))
}

async fn setup(env: &Env, options: &Options) -> Result<(), Failure> {
    let (realm, key) = node_key(options)?;
    let id = key.identity().node_id();
    println!(
        "The passphrase in {} is node {id}, of the {} realm.",
        options.passphrase_file.display(),
        realm.as_str()
    );
    check_hub_realms(&options.hubs, realm, &options.passphrase_file)?;
    let mut client = tokio::time::timeout(CALL_TIMEOUT, Client::connect(env))
        .await
        .map_err(|_| "The Dassie daemon did not answer on its socket".to_owned())??;
    let before = basic_state(&mut client).await?;
    // A node set up before runs its realm for good, and a phrase of the
    // other is not its owner's.
    if !is_uninitialized(&before) {
        let runs = node_realm(&mut client).await?;
        if runs != realm {
            return Err(Failure::Failed(format!(
                "The node runs the {} realm, and the passphrase in {} is a phrase of the {} \
                 realm: a node's realm is fixed when it is set up. Nothing was changed.",
                runs.as_str(),
                options.passphrase_file.display(),
                realm.as_str()
            )));
        }
    }
    let claimed = is_uninitialized(&before) && claim(&mut client, &key, realm).await?;
    drop(key);
    // Again after a claim, to see it took; and after one that found the
    // node set up meanwhile, to see by whom.
    let after = if is_uninitialized(&before) {
        basic_state(&mut client).await?
    } else {
        before
    };
    if is_uninitialized(&after) {
        return Err("The node did not take the setup, and is still not set up"
            .to_owned()
            .into());
    }
    if after.node_id != id.as_str() {
        return Err(Failure::OtherNode(format!(
            "The node is set up as {}, not as {id}, the node of the passphrase in {}. \
             Nothing was changed.",
            after.node_id,
            options.passphrase_file.display()
        )));
    }
    if claimed {
        println!("Set the node up as {id}, in the {} realm.", realm.as_str());
    } else {
        println!("The node is set up already as {id}.");
    }
    wait_for_engine(&mut client).await?;
    println!("The node's engine is running.");
    for asset in &options.assets {
        set_asset_config(&mut client, asset).await?;
    }
    for scheme in &options.schemes {
        add_scheme(&mut client, scheme, &after.active_settlement_schemes).await?;
    }
    if let Some(keep) = options.keep_funds {
        keep_funds(&mut client, keep).await?;
    }
    settle_hubs(&mut client, &options.hubs, realm).await
}

/// Keeps what the node receives in every asset a settlement hub holds
/// where `keep` says: the node keeps the choice, for hubs it joins later
/// too.
async fn keep_funds(client: &mut Client, keep: KeepFunds) -> Result<(), Failure> {
    let funds_kept = match keep {
        KeepFunds::Hub => proto::FundsKept::Hub,
        KeepFunds::OwnAccount => proto::FundsKept::OwnAccount,
    };
    let request = proto::SetFundsKeptRequest {
        name: String::new(),
        funds_kept: funds_kept.into(),
    };
    match call(client, Method::SetFundsKept(request)).await? {
        response::Method::SetFundsKept(_) => {}
        _ => return Err(unexpected()),
    }
    println!(
        "{}",
        match keep {
            KeepFunds::Hub => "The node keeps what it receives in its settlement hubs.",
            KeepFunds::OwnAccount => {
                "The node keeps what it receives on your own account: it withdraws from its \
                 settlement hubs what is beyond their ceiling."
            }
        }
    );
    Ok(())
}

/// Refuses a hub `--hub` names of another realm than `realm`, the
/// phrase's, in `file`, before anything is changed: a node runs only its
/// realm's schemes (spec/decisions/mainnet-schemes.md).
fn check_hub_realms(hubs: &Hubs, realm: Realm, file: &std::path::Path) -> Result<(), Failure> {
    let Hubs::Named(named) = hubs else {
        return Ok(());
    };
    match named.iter().find(|hub| hub.realm != realm) {
        None => Ok(()),
        Some(hub) => Err(Failure::Failed(format!(
            "--hub {} names a settlement hub of the {} realm, and the passphrase in {} is a \
             phrase of the {} realm. Nothing was changed.",
            hub.scheme,
            hub.realm.as_str(),
            file.display(),
            realm.as_str()
        ))),
    }
}

/// Joins the settlement hubs `hubs` says: those it names, failing if the
/// node offers any of them not; with none named, the default hub of XRP in
/// the test realm, saying so where the node offers none, and in the live
/// realm none, saying how to join while the node offers a hub and runs
/// none.
async fn settle_hubs(client: &mut Client, hubs: &Hubs, realm: Realm) -> Result<(), Failure> {
    match (hubs, realm) {
        (Hubs::None, _) => Ok(()),
        (Hubs::Named(named), _) => join_hubs(client, named, realm, true).await,
        (Hubs::Default, Realm::Test) => {
            join_hubs(client, &[HubScheme::xrp_of(realm)], realm, false).await
        }
        (Hubs::Default, Realm::Live) => {
            if let Some(hint) = live_hint(&list_schemes(client).await?) {
                println!("{hint}");
            }
            Ok(())
        }
    }
}

/// What setup says in the live realm with no `--hub`, while the node
/// offers a hub's scheme of the realm and runs none: how to join one.
fn live_hint(schemes: &[proto::SettlementSchemeInfo]) -> Option<String> {
    let live = || {
        HUB_SCHEMES
            .iter()
            .filter(|hub| hub.realm == Realm::Live)
            .filter_map(|hub| schemes.iter().find(|scheme| scheme.id == hub.scheme))
    };
    if live().any(|scheme| scheme.active) {
        return None;
    }
    let offered: Vec<&str> = live().map(|scheme| scheme.id.as_str()).collect();
    (!offered.is_empty()).then(|| {
        format!(
            "In the live realm, the node joins a settlement hub only once its owner turns hubs \
             on: pass --hub <scheme> to join the default hub of its asset, with <scheme> one of \
             {}.",
            offered.join(", ")
        )
    })
}

/// The settlement schemes the node lists.
async fn list_schemes(client: &mut Client) -> Result<Vec<proto::SettlementSchemeInfo>, Failure> {
    match call(
        client,
        Method::ListSettlementSchemes(proto::ListSettlementSchemesRequest {}),
    )
    .await?
    {
        response::Method::ListSettlementSchemes(list) => Ok(list.schemes),
        _ => Err(unexpected()),
    }
}

/// How `dassie setup` joins a settlement hub, from the schemes the node
/// lists.
#[derive(Debug, PartialEq, Eq)]
enum HubPlan {
    /// The node offers no default hub of the scheme's asset: its build has
    /// none built in, or the node's realm is not the scheme's.
    NoneOffered,
    /// The node runs the hub's scheme already, over the default hub or the
    /// one its config names.
    Joined,
    /// Add these schemes, in order: the owner books the hub funds from that
    /// are not active, then the hub's scheme, with no config, for the
    /// default hub of its asset, whose rules the node stores as the
    /// scheme's config.
    Join(Vec<Scheme>),
}

/// The plan for the hub of `hub`, from the node's `ListSettlementSchemes`:
/// it lists the hub's scheme while it offers the default hub of its asset,
/// or runs the scheme (spec/areas/node.md#the-dassie-command).
fn hub_plan(hub: HubScheme, schemes: &[proto::SettlementSchemeInfo]) -> HubPlan {
    let listed = |id: &str| schemes.iter().find(|scheme| scheme.id == id);
    match listed(hub.scheme) {
        None => HubPlan::NoneOffered,
        Some(scheme) if scheme.active => HubPlan::Joined,
        Some(_) => {
            let books = hub
                .books
                .iter()
                .filter(|book| !listed(book).is_some_and(|book| book.active));
            let steps = books
                .chain([&hub.scheme])
                .map(|id| Scheme {
                    id: (*id).to_owned(),
                    config: String::new(),
                })
                .collect();
            HubPlan::Join(steps)
        }
    }
}

/// What `dassie setup` says when the node offers no default hub for the
/// schemes `missing`, of `realm`: a line where it joins the realm's
/// default hub, or a failure where the owner named them (`asked`,
/// `--hub`), which the node cannot join, before it joins any hub.
fn none_offered(realm: Realm, missing: &[&str], asked: bool) -> Result<String, Failure> {
    if asked {
        return Err(Failure::Failed(format!(
            "This build has no default settlement hub built in for {}, so --hub joins none of \
             them, and setup joined no hub. The node is set up: add a hub's scheme with the \
             hub's rules as its config (--scheme <id>=<config>) to join one, or run setup \
             again without that --hub.",
            missing.join(", ")
        )));
    }
    Ok(format!(
        "This build has no default settlement hub for the {} realm yet: joining none.",
        realm.as_str()
    ))
}

/// Joins the settlement hubs of `hubs`, in order, as the node offers them:
/// for each, adds the owner books it funds from that are not active, then
/// its scheme, each step with a line, and leaves one whose scheme is
/// active as it is. Where the owner named them (`asked`), it fails before
/// joining any if the node offers any of them not; otherwise it joins
/// those the node offers, and says so of the others.
async fn join_hubs(
    client: &mut Client,
    hubs: &[HubScheme],
    realm: Realm,
    asked: bool,
) -> Result<(), Failure> {
    let schemes = list_schemes(client).await?;
    let plans: Vec<(HubScheme, HubPlan)> = hubs
        .iter()
        .map(|hub| (*hub, hub_plan(*hub, &schemes)))
        .collect();
    let missing: Vec<&str> = plans
        .iter()
        .filter(|(_, plan)| *plan == HubPlan::NoneOffered)
        .map(|(hub, _)| hub.scheme)
        .collect();
    if !missing.is_empty() {
        println!("{}", none_offered(realm, &missing, asked)?);
    }
    // A book two hubs fund from is added once, for the first.
    let mut added: Vec<String> = Vec::with_capacity(2 * hubs.len());
    for (hub, plan) in plans {
        match plan {
            HubPlan::NoneOffered => {}
            HubPlan::Joined => {
                println!("The settlement scheme {} is active already.", hub.scheme);
            }
            HubPlan::Join(steps) => {
                for step in steps {
                    add_scheme(client, &step, &added).await?;
                    added.push(step.id);
                }
            }
        }
    }
    Ok(())
}

/// One call over the IPC socket, bounded in time.
async fn call(client: &mut Client, method: Method) -> Result<response::Method, CallError> {
    tokio::time::timeout(CALL_TIMEOUT, client.call(method))
        .await
        .unwrap_or_else(|_| {
            Err(CallError {
                code: None,
                message: format!(
                    "The daemon did not answer within {} s",
                    CALL_TIMEOUT.as_secs()
                ),
            })
        })
}

fn unexpected() -> Failure {
    Failure::Failed("The daemon answered something this CLI does not understand".into())
}

async fn basic_state(client: &mut Client) -> Result<proto::GetBasicStateResponse, Failure> {
    match call(
        client,
        Method::GetBasicState(proto::GetBasicStateRequest {}),
    )
    .await?
    {
        response::Method::GetBasicState(basic) => Ok(basic),
        _ => Err(unexpected()),
    }
}

/// The realm the node runs.
async fn node_realm(client: &mut Client) -> Result<Realm, Failure> {
    match call(client, Method::GetConfig(proto::GetConfigRequest {})).await? {
        response::Method::GetConfig(proto::GetConfigResponse {
            config: Some(config),
            ..
        }) => match proto::Realm::try_from(config.realm) {
            Ok(proto::Realm::Test) => Ok(Realm::Test),
            Ok(proto::Realm::Live) => Ok(Realm::Live),
            _ => Err(unexpected()),
        },
        _ => Err(unexpected()),
    }
}

fn is_uninitialized(basic: &proto::GetBasicStateResponse) -> bool {
    basic.state == i32::from(proto::NodeState::Uninitialized)
}

/// Claims the node with `key` in `realm`, as the wallet does. False if it
/// was set up meanwhile, by whoever.
async fn claim(client: &mut Client, key: &NodeKey, realm: Realm) -> Result<bool, Failure> {
    let url = match call(client, Method::GetSetupUrl(proto::GetSetupUrlRequest {})).await {
        Ok(response::Method::GetSetupUrl(setup)) => setup.url,
        Err(CallError {
            code: Some(proto::ErrorCode::FailedPrecondition),
            ..
        }) => return Ok(false),
        Err(error) => return Err(error.into()),
        Ok(_) => return Err(unexpected()),
    };
    let token = url
        .rsplit_once("/setup/")
        .map(|(_, token)| token)
        .filter(|token| is_token(token))
        .ok_or_else(|| "The daemon's setup URL holds no setup token".to_owned())?;
    let listener = match call(client, Method::GetStatus(proto::GetStatusRequest {})).await? {
        response::Method::GetStatus(status) => Listener::of(&status)?,
        _ => return Err(unexpected()),
    };
    // The key is what the node stores in its database once set up; the
    // phrase and the master seed, which it never gets, are gone already.
    let body = serde_json::to_string(&SetupRequest {
        setup_token: token.into(),
        node_key: hex::encode(key.as_bytes()),
        realm: Some(realm.as_str().into()),
    })
    .map_err(|error| error.to_string())?;
    post_setup(&listener, body, realm).await
}

/// The node's HTTPS listener, as the daemon describes it over the owner's
/// socket: its port, and the certificate it serves there.
struct Listener {
    port: u16,
    leaf_sha256: [u8; 32],
}

impl Listener {
    fn of(status: &proto::GetStatusResponse) -> Result<Self, String> {
        let port = u16::try_from(status.https_port)
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| format!("The daemon names no HTTPS port ({})", status.https_port))?;
        let leaf_sha256 = status
            .certificate
            .as_ref()
            .and_then(|certificate| <[u8; 32]>::try_from(certificate.leaf_sha256.as_slice()).ok())
            .ok_or_else(|| {
                "The daemon does not say which certificate it serves: restart it, so that it \
                 runs the version of this command"
                    .to_owned()
            })?;
        Ok(Self { port, leaf_sha256 })
    }
}

/// 64 lowercase hex digits: a token that is safe to put in JSON as it is.
fn is_token(text: &str) -> bool {
    text.len() == TOKEN_HEX_CHARS && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Posts `body`, a setup in `realm`, to `/api/setup` on the node's HTTPS
/// listener on this host. True once set up; false if the route is gone,
/// which it is once the node has an identity.
async fn post_setup(listener: &Listener, body: String, realm: Realm) -> Result<bool, Failure> {
    let port = listener.port;
    let origin = format!("https://localhost:{port}");
    let exchange = async {
        let tls = connect_local(listener).await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tls))
            .await
            .map_err(|e| format!("No HTTP with the node at {origin}: {e}"))?;
        let driver = tokio::spawn(connection);
        let request = http::Request::post("/api/setup")
            .header(http::header::HOST, format!("localhost:{port}"))
            .header(http::header::ORIGIN, &origin)
            .header(http::header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| format!("Could not make the setup request: {e}"))?;
        let answer = async {
            let response = sender.send_request(request).await?;
            let status = response.status();
            let text = Limited::new(response.into_body(), MAX_ANSWER_BYTES)
                .collect()
                .await
                .map(|body| body.to_bytes())
                .unwrap_or_default();
            Ok::<_, hyper::Error>((status, text))
        }
        .await;
        driver.abort();
        answer.map_err(|e| format!("The setup request to {origin} failed: {e}"))
    };
    let (status, text) = tokio::time::timeout(SETUP_TIMEOUT, exchange)
        .await
        .map_err(|_| {
            format!(
                "The node did not answer the setup request within {} s",
                SETUP_TIMEOUT.as_secs()
            )
        })??;
    match status {
        http::StatusCode::OK => Ok(true),
        http::StatusCode::NOT_FOUND => Ok(false),
        // The realm is the phrase's, which the node may not run: the
        // refusal says so, in the node's words.
        other => Err(format!(
            "The node refused setup in the {} realm, which the passphrase names ({other}): {}",
            realm.as_str(),
            shown(&text)
        )
        .into()),
    }
}

/// What the node said, as one short line of plain text.
fn shown(text: &[u8]) -> String {
    String::from_utf8_lossy(text)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_REFUSAL_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A TLS connection to the node's listener on loopback, IPv4 first, then
/// IPv6, as the daemon binds every address of one or both, or `--listen`
/// one of them. Only a listener that proves it holds the key of the
/// certificate the daemon named counts: whatever else answers at an
/// address is passed over, and is sent nothing.
async fn connect_local(listener: &Listener) -> Result<TlsStream<TcpStream>, String> {
    let config = rustls::ClientConfig::builder_with_provider(tls::provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS: {e}"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NodeCertificate(listener.leaf_sha256)))
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from("localhost").map_err(|e| format!("TLS: {e}"))?;
    let addresses = [
        std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, listener.port)),
        std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, listener.port)),
    ];
    let mut problems = Vec::with_capacity(addresses.len());
    for address in addresses {
        let attempt = async {
            let stream = TcpStream::connect(address).await?;
            connector.connect(name.clone(), stream).await
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(error)) => problems.push(format!("{address}: {error}")),
            Err(_) => problems.push(format!("{address}: no answer")),
        }
    }
    Err(format!(
        "Cannot reach the node's HTTPS listener on this host ({}). `dassie setup` runs on \
         the node's own host, and needs the listener on loopback or on every address.",
        problems.join("; ")
    ))
}

/// Accepts exactly the certificate whose SHA-256 the daemon named over the
/// owner's socket, and checks that the handshake is signed with its key.
///
/// No CA can vouch for a node that has no hostname, and its self-signed
/// certificate names `localhost`, which anyone can put in one. The pin
/// does not depend on who holds the port: another user may bind a
/// loopback address the node leaves free, but cannot sign with the key of
/// the node's certificate. Only the node holds that key, and whoever can
/// read it from the node's database can read the node key there too
/// (spec/decisions/headless-setup.md).
#[derive(Debug)]
struct NodeCertificate([u8; 32]);

impl ServerCertVerifier for NodeCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if Sha256::digest(end_entity.as_ref()).as_slice() == self.0 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "not the certificate the daemon says it serves".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = tls::provider().signature_verification_algorithms;
        rustls::crypto::verify_tls12_signature(message, certificate, signature, &algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        certificate: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = tls::provider().signature_verification_algorithms;
        rustls::crypto::verify_tls13_signature(message, certificate, signature, &algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        tls::provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The node's ledgers, or none while its engine is not running: before it
/// has started after setup, or for a moment after a failure.
async fn ledgers(client: &mut Client) -> Result<Option<proto::ListLedgersResponse>, Failure> {
    match call(client, Method::ListLedgers(proto::ListLedgersRequest {})).await {
        Ok(response::Method::ListLedgers(list)) => Ok(Some(list)),
        Ok(_) => Err(unexpected()),
        Err(CallError {
            code: Some(proto::ErrorCode::Unavailable),
            ..
        }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Polls until the engine answers, a bounded number of times.
async fn wait_for_engine(client: &mut Client) -> Result<(), Failure> {
    for _ in 0..MAX_POLLS {
        if ledgers(client).await?.is_some() {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(Failure::Failed(format!(
        "The node's engine did not start within {} s: `dassie status` and the daemon's \
         log may say why",
        wait_seconds()
    )))
}

fn wait_seconds() -> u64 {
    POLL_INTERVAL.saturating_mul(MAX_POLLS).as_secs()
}

/// Sets the owner's config of the external asset `asset` names: the same
/// config again changes nothing (spec/areas/settlement.md#external-assets).
async fn set_asset_config(client: &mut Client, asset: &Scheme) -> Result<(), Failure> {
    let id = &asset.id;
    let method = Method::SetAssetConfig(proto::SetAssetConfigRequest {
        asset: id.clone(),
        config: asset.config.clone(),
    });
    match call(client, method).await {
        Ok(response::Method::SetAssetConfig(set)) if set.in_effect => {
            println!("Set the config of {id}.");
            Ok(())
        }
        Ok(response::Method::SetAssetConfig(_)) => {
            println!(
                "Set the config of {id}; the node takes it at its next start, since schemes \
                 settle through it now."
            );
            Ok(())
        }
        Ok(_) => Err(unexpected()),
        Err(error) => Err(Failure::Failed(format!(
            "Could not set the config of {id}: {}",
            error.message
        ))),
    }
}

/// Adds `scheme` unless it is among `active`, and polls, a bounded number
/// of times, until the node lists a ledger of it.
async fn add_scheme(
    client: &mut Client,
    scheme: &Scheme,
    active: &[String],
) -> Result<(), Failure> {
    let id = &scheme.id;
    if active.contains(id) {
        println!("The settlement scheme {id} is active already.");
        return Ok(());
    }
    let method = Method::AddSettlementScheme(proto::AddSettlementSchemeRequest {
        id: id.clone(),
        config: scheme.config.clone(),
    });
    match call(client, method).await {
        Ok(response::Method::AddSettlementScheme(_)) => {}
        Ok(_) => return Err(unexpected()),
        Err(error) => {
            return Err(Failure::Failed(format!(
                "Could not add the settlement scheme {id}: {}",
                error.message
            )));
        }
    }
    for _ in 0..MAX_POLLS {
        let listed = ledgers(client).await?.is_some_and(|list| {
            list.ledgers
                .iter()
                .any(|ledger| ledger.settlement_scheme == *id)
        });
        if listed {
            println!("Added the settlement scheme {id}.");
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    Err(Failure::Failed(format!(
        "The node took the settlement scheme {id} but lists no ledger of it after {} s",
        wait_seconds()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Result<Options, String> {
        let mut parser = lexopt::Parser::from_args(words.iter().copied());
        args(&mut parser).map_err(|e| e.to_string())
    }

    #[test]
    fn flags() {
        let options = parse(&[
            "--passphrase-file",
            "/run/phrase",
            "--scheme",
            "stub",
            "--scheme",
            r#"xrpl-testnet-xrp={"a":"b=c"}"#,
            "--asset",
            r#"xrpl-testnet-account/0={"server":"wss://a"}"#,
        ])
        .unwrap();
        assert_eq!(options.passphrase_file, PathBuf::from("/run/phrase"));
        assert_eq!(
            options.schemes,
            [
                Scheme {
                    id: "stub".into(),
                    config: String::new()
                },
                Scheme {
                    id: "xrpl-testnet-xrp".into(),
                    config: r#"{"a":"b=c"}"#.into()
                },
            ]
        );
        assert!(
            parse(&["--scheme", "stub"])
                .unwrap_err()
                .contains("--passphrase-file")
        );
        let twice = parse(&[
            "--passphrase-file",
            "p",
            "--scheme",
            "stub",
            "--scheme",
            "stub=x",
        ]);
        assert!(twice.unwrap_err().contains("twice"));
        assert_eq!(
            options.assets,
            [Scheme {
                id: "xrpl-testnet-account/0".into(),
                config: r#"{"server":"wss://a"}"#.into()
            }]
        );
        let unnamed = parse(&["--passphrase-file", "p", "--scheme", "={}"]);
        assert!(unnamed.unwrap_err().contains("names no scheme"));
        let unnamed = parse(&["--passphrase-file", "p", "--asset", "={}"]);
        assert!(unnamed.unwrap_err().contains("names no asset"));
        let twice = parse(&[
            "--passphrase-file",
            "p",
            "--asset",
            "a/0",
            "--asset",
            "a/0={}",
        ]);
        assert!(twice.unwrap_err().contains("--asset a/0 given twice"));
        let mut many = vec!["--passphrase-file".to_owned(), "p".to_owned()];
        for n in 0..=MAX_SCHEMES {
            many.extend(["--scheme".to_owned(), format!("s{n}")]);
        }
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(parse(&many).unwrap_err().contains("more than"));
        // The realm is the phrase's, so no flag names it.
        assert!(parse(&["--passphrase-file", "p", "--realm", "live"]).is_err());
    }

    fn hub(id: &str) -> HubScheme {
        HubScheme::named(id).unwrap()
    }

    // spec/areas/node.md#the-dassie-command: `--hub` names each hub by its
    // scheme, repeatable, or `none`; with none given, the realm's default.
    // `--no-hub` is gone, and points to `--hub none`.
    #[test]
    fn hubs_are_named_by_their_schemes() {
        let plain = parse(&["--passphrase-file", "p"]).unwrap();
        assert_eq!(plain.hubs, Hubs::Default);
        let none = parse(&["--passphrase-file", "p", "--hub", "none"]).unwrap();
        assert_eq!(none.hubs, Hubs::None);
        let two = parse(&[
            "--passphrase-file",
            "p",
            "--hub",
            "hub-xrpl-testnet-xrp",
            "--hub=hub-xrpl-testnet-dsusd",
        ])
        .unwrap();
        assert_eq!(
            two.hubs,
            Hubs::Named(vec![
                hub("hub-xrpl-testnet-xrp"),
                hub("hub-xrpl-testnet-dsusd")
            ])
        );
        // A mainnet hub parses in any realm; setup refuses it for a phrase
        // of the test realm before it changes anything.
        let live = parse(&["--passphrase-file", "p", "--hub", "hub-xrpl-rlusd"]).unwrap();
        assert_eq!(live.hubs, Hubs::Named(vec![hub("hub-xrpl-rlusd")]));

        let refused = |words: &[&str]| {
            let mut all = vec!["--passphrase-file", "p"];
            all.extend_from_slice(words);
            parse(&all).unwrap_err()
        };
        let gone = refused(&["--no-hub"]);
        assert!(gone.contains("--hub none"), "{gone}");
        assert!(refused(&["--hub"]).contains("--hub"));
        let unknown = refused(&["--hub", "xrpl-testnet-xrp"]);
        assert!(
            unknown.contains("names no settlement hub's scheme"),
            "{unknown}"
        );
        assert!(unknown.contains("hub-xrpl-testnet-dsusd"), "{unknown}");
        assert!(refused(&["--hub", "stub"]).contains("no settlement hub"));
        for both in [
            ["--hub", "none", "--hub", "hub-xrpl-testnet-xrp"],
            ["--hub", "hub-xrpl-testnet-xrp", "--hub", "none"],
        ] {
            let both = refused(&both);
            assert!(
                both.contains("--hub none joins no settlement hub"),
                "{both}"
            );
        }
        let twice = refused(&["--hub", "hub-xrpl-xrp", "--hub", "hub-xrpl-xrp"]);
        assert!(twice.contains("--hub hub-xrpl-xrp given twice"), "{twice}");
        assert!(refused(&["--hub", "none", "--hub", "none"]).contains("twice"));
    }

    // A hub of another realm than the phrase's is refused before anything
    // is changed, naming both realms.
    #[test]
    fn a_hub_of_another_realm_is_refused() {
        let file = std::path::Path::new("/run/phrase");
        for hubs in [
            Hubs::Default,
            Hubs::None,
            Hubs::Named(vec![
                hub("hub-xrpl-testnet-xrp"),
                hub("hub-xrpl-testnet-dsusd"),
            ]),
        ] {
            assert!(check_hub_realms(&hubs, Realm::Test, file).is_ok());
        }
        let mixed = Hubs::Named(vec![hub("hub-xrpl-testnet-xrp"), hub("hub-xrpl-xrp")]);
        let Err(Failure::Failed(refused)) = check_hub_realms(&mixed, Realm::Test, file) else {
            panic!("a mainnet hub for a test phrase");
        };
        assert!(refused.contains("--hub hub-xrpl-xrp"), "{refused}");
        assert!(refused.contains("of the live realm"), "{refused}");
        assert!(refused.contains("a phrase of the test realm"), "{refused}");
        assert!(refused.contains("Nothing was changed"), "{refused}");
        let testnet = Hubs::Named(vec![hub("hub-xrpl-testnet-dsusd")]);
        assert!(check_hub_realms(&testnet, Realm::Live, file).is_err());
    }

    // spec/areas/payments.md#where-received-money-is-kept: what the node
    // receives stays in the hub unless the owner says otherwise; without
    // the flag, setup leaves the node's choice alone.
    #[test]
    fn keeping_funds_is_the_owners_choice() {
        assert_eq!(parse(&["--passphrase-file", "p"]).unwrap().keep_funds, None);
        let hub = parse(&["--passphrase-file", "p", "--keep-funds", "hub"]).unwrap();
        assert_eq!(hub.keep_funds, Some(KeepFunds::Hub));
        let own = parse(&["--passphrase-file", "p", "--keep-funds", "account"]).unwrap();
        assert_eq!(own.keep_funds, Some(KeepFunds::OwnAccount));
        let wrong = parse(&["--passphrase-file", "p", "--keep-funds", "ledger"]);
        assert!(wrong.unwrap_err().contains("--keep-funds"));
        assert!(parse(&["--passphrase-file", "p", "--keep-funds"]).is_err());
    }

    fn listed(id: &str, active: bool) -> proto::SettlementSchemeInfo {
        proto::SettlementSchemeInfo {
            id: id.into(),
            active,
            ..Default::default()
        }
    }

    fn scheme(id: &str, config: &str) -> Scheme {
        Scheme {
            id: id.into(),
            config: config.into(),
        }
    }

    // The node lists a hub's scheme only while it offers the default hub of
    // its asset, or runs the scheme; the plan adds the owner books it funds
    // from first, peering with nobody, unless they are active, and changes
    // nothing the second time.
    #[test]
    fn a_hub_is_joined_when_the_node_offers_it() {
        for realm in [Realm::Test, Realm::Live] {
            let default = HubScheme::xrp_of(realm);
            let ([book], hub) = (default.books, default.scheme) else {
                panic!("an XRP hub funds from its XRP book alone");
            };
            let plan = |schemes: &[proto::SettlementSchemeInfo]| hub_plan(default, schemes);
            assert_eq!(plan(&[]), HubPlan::NoneOffered);
            assert_eq!(
                plan(&[listed("stub", true), listed(book, false)]),
                HubPlan::NoneOffered
            );
            assert_eq!(
                plan(&[listed(book, false), listed(hub, false)]),
                HubPlan::Join(vec![scheme(book, ""), scheme(hub, "")])
            );
            assert_eq!(
                plan(&[listed(book, true), listed(hub, false)]),
                HubPlan::Join(vec![scheme(hub, "")])
            );
            // Run again, with what the first run added active; or a hub
            // scheme the owner added with rules of its own.
            assert_eq!(
                plan(&[listed(book, true), listed(hub, true)]),
                HubPlan::Joined
            );
        }
        // A token hub funds from its token's book, and pays its escrow's
        // gas from the XRP book of its network, which comes first.
        let dsusd = hub("hub-xrpl-testnet-dsusd");
        let (xrp, token) = ("xrpl-testnet-xrp", "xrpl-testnet-dsusd");
        assert_eq!(
            hub_plan(dsusd, &[listed(xrp, false), listed(dsusd.scheme, false)]),
            HubPlan::Join(vec![
                scheme(xrp, ""),
                scheme(token, ""),
                scheme(dsusd.scheme, "")
            ])
        );
        assert_eq!(
            hub_plan(dsusd, &[listed(xrp, true), listed(dsusd.scheme, false)]),
            HubPlan::Join(vec![scheme(token, ""), scheme(dsusd.scheme, "")])
        );
        // The other realm's hub is not this realm's.
        let live = HubScheme::xrp_of(Realm::Live);
        let test = HubScheme::xrp_of(Realm::Test);
        assert_eq!(
            hub_plan(
                live,
                &[listed(test.books[0], true), listed(test.scheme, false)]
            ),
            HubPlan::NoneOffered
        );
    }

    // spec/areas/node.md#the-dassie-command: a node that offers no default
    // hub joins none by default; for a hub `--hub` named, setup fails and
    // says why, rather than exit 0 as if it joined.
    #[test]
    fn no_default_hub_fails_only_where_it_was_named() {
        for realm in [Realm::Test, Realm::Live] {
            let hub = HubScheme::xrp_of(realm).scheme;
            let said = none_offered(realm, &[hub], false).unwrap();
            assert!(said.contains("no default settlement hub"), "{said}");
            assert!(said.contains(realm.as_str()), "{said}");
            let Err(Failure::Failed(failed)) = none_offered(realm, &[hub, "other"], true) else {
                panic!("named a hub the node offers no default of");
            };
            assert!(failed.contains("--hub joins none"), "{failed}");
            assert!(failed.contains(&format!("{hub}, other")), "{failed}");
        }
    }

    // In the live realm with no `--hub`, setup says how to join while the
    // node offers a hub of the realm and runs none.
    #[test]
    fn the_live_realm_says_how_to_join() {
        assert_eq!(live_hint(&[listed("xrpl-xrp", true)]), None);
        let hint = live_hint(&[
            listed("hub-xrpl-xrp", false),
            listed("hub-xrpl-rlusd", false),
        ])
        .unwrap();
        assert!(hint.contains("--hub <scheme>"), "{hint}");
        assert!(hint.contains("hub-xrpl-xrp, hub-xrpl-rlusd"), "{hint}");
        let joined = [
            listed("hub-xrpl-xrp", false),
            listed("hub-xrpl-rlusd", true),
        ];
        assert_eq!(live_hint(&joined), None);
        assert_eq!(live_hint(&[listed("hub-xrpl-testnet-xrp", false)]), None);
    }

    // The IDs are the node's: every hub's scheme it runs is one `--hub`
    // names, and each hub's scheme and the owner books it funds from are
    // schemes of its realm's ledgers.
    #[test]
    fn the_hubs_name_schemes_the_node_runs() {
        assert_eq!(
            HubScheme::xrp_of(Realm::Test).scheme,
            "hub-xrpl-testnet-xrp"
        );
        assert_eq!(HubScheme::xrp_of(Realm::Live).scheme, "hub-xrpl-xrp");
        assert_eq!(HubScheme::xrp_of(Realm::Live).books, ["xrpl-xrp"]);
        for info in dassie_node::engine::SCHEMES {
            let named = HubScheme::named(info.id).is_some();
            assert_eq!(named, info.id.starts_with("hub-"), "{}", info.id);
        }
        for hub in HUB_SCHEMES {
            for id in hub.books.iter().chain([&hub.scheme]) {
                let info = dassie_node::engine::SCHEMES
                    .iter()
                    .find(|info| info.id == *id)
                    .unwrap_or_else(|| panic!("{id}"));
                let ledger = format!("{:?}", info.ledger.realm());
                assert_eq!(ledger, format!("{:?}", hub.realm), "{id}");
            }
        }
    }

    #[test]
    fn tokens() {
        assert!(is_token(&"0a".repeat(32)));
        assert!(!is_token(&"0A".repeat(32)));
        assert!(!is_token(&"0a".repeat(31)));
        assert!(!is_token(&format!("{}\"", "0".repeat(63))));
    }

    #[test]
    fn only_the_certificate_the_daemon_names_is_accepted() {
        let served = CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0x01]);
        let other = CertificateDer::from(vec![0x30, 0x03, 0x02, 0x01, 0x02]);
        let pinned = NodeCertificate(Sha256::digest(served.as_ref()).into());
        let check = |certificate: &CertificateDer<'_>| {
            pinned.verify_server_cert(
                certificate,
                &[],
                &ServerName::try_from("localhost").unwrap(),
                &[],
                UnixTime::now(),
            )
        };
        assert!(check(&served).is_ok());
        assert!(check(&other).is_err());
    }

    #[test]
    fn a_daemon_that_names_no_certificate_is_sent_nothing() {
        let mut status = proto::GetStatusResponse {
            https_port: 5101,
            certificate: Some(proto::Certificate {
                leaf_sha256: vec![7; 32],
                ..Default::default()
            }),
            ..Default::default()
        };
        let listener = Listener::of(&status).unwrap();
        assert_eq!((listener.port, listener.leaf_sha256), (5101, [7; 32]));
        status.certificate = Some(proto::Certificate::default());
        assert!(Listener::of(&status).is_err());
        status.certificate = None;
        assert!(Listener::of(&status).is_err());
        status.https_port = 0;
        assert!(Listener::of(&status).is_err());
    }

    #[test]
    fn refusals_are_one_short_line() {
        assert_eq!(shown(b"Invalid setup token"), "Invalid setup token");
        assert_eq!(shown(b"a\nb\x1b[31m"), "a b [31m");
        assert_eq!(shown(&[b'x'; 1_000]).len(), MAX_REFUSAL_CHARS);
    }
}

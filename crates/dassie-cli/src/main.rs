//! `dassie`: the daemon and its command-line tools.
//!
//! - `dassie daemon [--runtime standalone|systemd]` runs the node.
//! - `dassie init` configures it, over the IPC socket.
//! - `dassie status` says how it is doing, `dassie setup-url` prints the
//!   link that completes setup, and `dassie pair` a code that pairs an owner
//!   app, over the IPC socket too.
//! - `dassie hub` shows the node's account in its settlement hub, and moves
//!   the owner's funds between it and the node's XRP Ledger account;
//!   `dassie hub found` founds a hub, and `dassie hub payout` shows its
//!   payout bundles.
//! - `dassie setup` sets the node up from its owner's passphrase, without
//!   a browser, and adds settlement schemes.
//! - `dassie update` installs a newer release from the mirror.
//! - `dassie verify-install` checks that this binary works here.
//! - `dassie front-door` runs a front door, which is no node.
//! - `dassie market-maker` trades through a core node with two nodes of
//!   its own, from the node's rate feed.

mod client;
mod founding;
mod front_door;
mod hub;
mod init;
mod market_maker;
mod passphrase;
mod setup;
mod status;
mod update;

use std::process::ExitCode;
use std::sync::Arc;

use dassie_node::clock::{SystemClock, os_rng};
use dassie_node::daemon::{self, DaemonOptions, Prepared};
use dassie_node::env::{Env, Settings};
use dassie_node::node::{Policies, Runtime};
use dassie_node::store::Store;
use dassie_node::{http, log, tls};
use dassie_node_core::certificate;
use dassie_node_core::config::Ports;
use lexopt::prelude::*;
use tokio_util::sync::CancellationToken;

const USAGE: &str = "\
Usage: dassie <command> [options]

Commands:
  daemon            Run the Dassie daemon in the foreground, for a process
                    manager like systemd.
                      --runtime standalone|systemd   how to get the sockets
                                                     (default: systemd if it
                                                     passed them, else bind)
                      --http-port <port>             standalone: this port
                      --https-port <port>            instead of the configured
                      --listen <address>             standalone: only this
                                                     address (default: all)
                      --feed-listen <addr:port>      standalone: the rate
                                                     feed's UDP socket (the
                                                     HTTPS address, port 7768)
  init              Configure the node: hostname and TLS certificate.
                      --hostname <name>      the node's public DNS name
                      --email <address>      contact for the ACME account
                      --agree-tos            agree to the CA's terms
                      --acme-directory <url> another ACME directory
                      --acme-staging         Let's Encrypt's staging directory
                      --skip-acme            keep the current certificate
  status            Show how the node is doing: version, address,
                    certificate, setup, network, ledgers and updates.
  setup-url         Print the link that completes setup in a browser.
  hub [status]      Show the node's account in its settlement hub, and the
                    floor and ceiling the node keeps it between.
  hub deposit <XRP> Move XRP from the node's XRP Ledger account into the
                    hub, opening the account there if it has none.
  hub withdraw <XRP>
                    Move XRP from the hub to the node's XRP Ledger account.
                    Either move pauses the node's own moves for an hour.
  hub auto on|off   Let the node move funds to and from the hub by itself,
                    or not.
  hub give-up       Give up a move the hub has not shown for long; the node
                    then moves nothing by itself until you act.
  hub found <node ID>@<host:port>...
                    Found a settlement hub with these validators, as its
                    sponsor; every one of them must accept.
                      --terms <file>            JSON of the terms that
                                                differ from the starting
                                                ones
                      --escrow-funding <XRP>    what funds the escrow
                      --xrpl <url>              the XRPL server to set the
                                                escrow up on (default: the
                                                XRP Ledger testnet scheme's)
  hub foundings     Show each hub the node founds: which validators
                    accepted, signed and run it, and its escrow.
  hub drop <nonce>  Drop a founding.
  hub payout        Show the payout bundles kept of the node's hub, and
                    once a quorum signed one set, its transactions to
                    submit.
  setup             Set the node up from its owner's passphrase, on the
                    node's own host, without a browser, and add the
                    settlement schemes it lacks. Running it again changes
                    nothing; it exits 3, changing nothing, if the node is
                    another passphrase's.
                      --passphrase-file <file>  the passphrase, alone
                      --scheme <id>[=<config>]  a settlement scheme, with
                                                its JSON config; repeatable
  pair              Print a code that pairs an owner app with the node, for
                    the next five minutes.
  update           Update Dassie to the newest release, once its signature
                    and sums check out.
                      -f, --force                  install even if not newer, or
                                                   if it was rolled back
                      -t, --target-version <v>     this version instead
                      --mirror <url>               another mirror (default:
                                                   DASSIE_MIRROR_URL, or
                                                   https://get.dassie.land)
                      --no-restart                 leave systemd alone
                      --unit <name>.service=<dir>  restart this unit, whose
                                                   node keeps its data in
                                                   <dir>, rather than
                                                   dassie.service, and
                                                   install none of the
                                                   release's units;
                                                   repeatable
  verify-install    Verify that Dassie and its dependencies work here.
  front-door        Run a front door: Open Payments at <label>.<domain> for
                    nodes that name it as their relay, and at
                    <name>.<domain> for the holder of a name, as the
                    registrars in DASSIE_BOOTSTRAP_NODES list it. Each flag
                    overrides its DASSIE_FRONT_DOOR_* variable.
                      --domain <name>           the domain (dassiepay.net)
                      --https-listen <addr:port> HTTPS ([::]:443)
                      --quic-listen <addr:port>  QUIC, on UDP ([::]:443)
                      --certificate <file>      PEM chain for *.<domain>
                      --certificate-key <file>  its PEM key
                      --key-file <file>         the front door's own key
                      --relay-url <url>         where nodes dial it
                                                (https://relay.<domain>)
                      --make-key                make the key file if there
                                                is none, print the entry for
                                                DASSIE_RELAY, and exit
  market-maker      Trade through a core node with two nodes of your own
                    when its rate feed and an AMM pool on the XRP Ledger
                    leave a margin, and move your funds back on the ledger.
                      --config <file>           the JSON config

Options:
  -h, --help        Show this help.
  -V, --version     Show the version.
";

/// Exit code for a command line that could not be understood.
const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dassie: {error}\n\n{USAGE}");
            ExitCode::from(USAGE_ERROR)
        }
    }
}

fn run() -> Result<ExitCode, lexopt::Error> {
    let mut parser = lexopt::Parser::from_env();
    let Some(argument) = parser.next()? else {
        print!("{USAGE}");
        return Ok(ExitCode::from(USAGE_ERROR));
    };
    match argument {
        Short('h') | Long("help") => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Short('V') | Long("version") => {
            println!("dassie {}", dassie_node::VERSION);
            Ok(ExitCode::SUCCESS)
        }
        Value(command) => match command.string()?.as_str() {
            "daemon" => Ok(daemon(daemon_args(&mut parser)?)),
            "init" => Ok(init(init_args(&mut parser)?)),
            "status" => {
                no_more(&mut parser)?;
                Ok(over_ipc(status::status))
            }
            "setup" => Ok(setup::run(&setup::args(&mut parser)?)),
            "hub" => {
                let command = hub::args(&mut parser)?;
                Ok(over_ipc(async |env: &Env| hub::run(env, command).await))
            }
            "setup-url" => {
                no_more(&mut parser)?;
                Ok(over_ipc(status::setup_url))
            }
            "pair" => {
                no_more(&mut parser)?;
                Ok(over_ipc(status::pair))
            }
            "update" => Ok(update::run(update_args(&mut parser)?)),
            "verify-install" => {
                no_more(&mut parser)?;
                Ok(verify_install())
            }
            "front-door" => Ok(front_door::run(front_door::args(&mut parser)?)),
            "market-maker" => Ok(market_maker::run(&market_maker::args(&mut parser)?)),
            other => Err(format!("unknown command {other:?}").into()),
        },
        other => Err(other.unexpected()),
    }
}

fn no_more(parser: &mut lexopt::Parser) -> Result<(), lexopt::Error> {
    match parser.next()? {
        Some(argument) => Err(argument.unexpected()),
        None => Ok(()),
    }
}

fn daemon_args(parser: &mut lexopt::Parser) -> Result<DaemonOptions, lexopt::Error> {
    let mut options = DaemonOptions::default();
    let (mut http, mut https) = (None, None);
    while let Some(argument) = parser.next()? {
        match argument {
            Long("runtime") => {
                options.runtime = Some(match parser.value()?.string()?.as_str() {
                    "standalone" => Runtime::Standalone,
                    "systemd" => Runtime::Systemd,
                    other => return Err(format!("unknown runtime {other:?}").into()),
                });
            }
            Long("http-port") => http = Some(parser.value()?.parse::<u32>()?),
            Long("https-port") => https = Some(parser.value()?.parse::<u32>()?),
            Long("listen") => {
                options.listen = Some(parser.value()?.parse::<std::net::IpAddr>()?);
            }
            Long("feed-listen") => {
                options.feed = Some(parser.value()?.parse::<std::net::SocketAddr>()?);
            }
            other => return Err(other.unexpected()),
        }
    }
    options.ports = match (http, https) {
        (None, None) => None,
        (Some(http), Some(https)) => Some(Ports::new(http, https).map_err(|e| e.to_string())?),
        _ => return Err("--http-port and --https-port go together".into()),
    };
    Ok(options)
}

fn init_args(parser: &mut lexopt::Parser) -> Result<init::Options, lexopt::Error> {
    let mut options = init::Options::default();
    while let Some(argument) = parser.next()? {
        match argument {
            Long("hostname") => options.hostname = Some(parser.value()?.string()?),
            Long("email") => options.email = Some(parser.value()?.string()?),
            Long("agree-tos") => options.agree_tos = true,
            Long("acme-directory") => options.acme_directory = Some(parser.value()?.string()?),
            Long("acme-staging") => options.use_staging(),
            Long("skip-acme") => options.skip_acme = true,
            other => return Err(other.unexpected()),
        }
    }
    Ok(options)
}

fn update_args(parser: &mut lexopt::Parser) -> Result<update::Options, lexopt::Error> {
    let mut options = update::Options::default();
    while let Some(argument) = parser.next()? {
        match argument {
            Short('f') | Long("force") => options.force = true,
            Short('t') | Long("target-version") => options.target = Some(parser.value()?.string()?),
            Long("mirror") => options.mirror = Some(parser.value()?.string()?),
            Long("no-restart") => options.no_restart = true,
            Long("unit") => {
                let unit = update::NodeUnit::parse(&parser.value()?.string()?)?;
                if options.units.iter().any(|named| named.name == unit.name) {
                    return Err(format!("--unit names {} twice", unit.name).into());
                }
                if options.units.len() == update::MAX_NODE_UNITS {
                    return Err(format!("at most {} --unit", update::MAX_NODE_UNITS).into());
                }
                options.units.push(unit);
            }
            other => return Err(other.unexpected()),
        }
    }
    if options.no_restart && !options.units.is_empty() {
        return Err("--unit restarts what --no-restart leaves alone: choose one".into());
    }
    Ok(options)
}

/// Exit code for a daemon that could not start.
const START_FAILED: u8 = 1;

fn daemon(options: DaemonOptions) -> ExitCode {
    let env = Env::from_process();
    match Settings::from_env(&env) {
        Ok(settings) => log::init(settings.log_filter),
        Err(error) => {
            eprintln!("dassie: {error}");
            return ExitCode::from(START_FAILED);
        }
    }
    // Before the runtime, so that taking systemd's sockets happens while
    // this is the only thread (see dassie_node::systemd).
    let prepared = match Prepared::new(&env, options) {
        Ok(prepared) => prepared,
        Err(error) => {
            tracing::error!(target: log::target::DAEMON, %error, "could not start");
            return ExitCode::from(START_FAILED);
        }
    };
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::from(START_FAILED);
    };
    let result = runtime.block_on(async {
        let shutdown = CancellationToken::new();
        // The handlers exist before the daemon publishes its socket or
        // reports readiness, so no SIGTERM finds the process without one.
        let Some(signals) = listen_for_signals() else {
            return Err(daemon::DaemonError::Signals);
        };
        tokio::spawn(stop_on_signal(signals, shutdown.clone()));
        daemon::run(
            prepared,
            Arc::new(SystemClock),
            os_rng(),
            Policies::DEFAULT,
            shutdown,
        )
        .await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(target: log::target::DAEMON, %error, "could not start");
            ExitCode::from(START_FAILED)
        }
    }
}

type Signals = (tokio::signal::unix::Signal, tokio::signal::unix::Signal);

/// SIGTERM and SIGINT, handled from now on.
fn listen_for_signals() -> Option<Signals> {
    use tokio::signal::unix::{SignalKind, signal};
    let terminate = signal(SignalKind::terminate()).ok()?;
    let interrupt = signal(SignalKind::interrupt()).ok()?;
    Some((terminate, interrupt))
}

async fn stop_on_signal(signals: Signals, shutdown: CancellationToken) {
    let (mut terminate, mut interrupt) = signals;
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    shutdown.cancel();
}

fn init(options: init::Options) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::FAILURE;
    };
    let env = Env::from_process();
    match runtime.block_on(init::run(&env, options)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(init::Stopped) => ExitCode::FAILURE,
    }
}

/// Runs a command that talks to the daemon, and shows its problem if any.
fn over_ipc(command: impl AsyncFnOnce(&Env) -> Result<(), String>) -> ExitCode {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::FAILURE;
    };
    let env = Env::from_process();
    match runtime.block_on(command(&env)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("{problem}");
            ExitCode::FAILURE
        }
    }
}

/// One check of `verify-install`: a problem, or none.
type Check = fn() -> Result<(), String>;

/// Checks what the original checked, the database, and more: the schema
/// check it skipped, TLS, that the wallet is built into the binary, and
/// that its version is one the updater understands.
fn verify_install() -> ExitCode {
    let checks: [(&str, Check); 4] = [
        ("Verifying that SQLite is working...", check_database),
        ("Verifying that TLS is working...", check_tls),
        ("Verifying that the wallet is built in...", check_wallet),
        ("Verifying the version...", check_version),
    ];
    for (title, check) in checks {
        println!("{title}");
        if let Err(problem) = check() {
            eprintln!("Error: {problem}");
            return ExitCode::FAILURE;
        }
    }
    match update::release_key() {
        Ok(key) => println!("Updates are verified with release key {}.", key.id()),
        Err(problem) => {
            eprintln!("Error: {problem}");
            return ExitCode::FAILURE;
        }
    }
    println!("Success! Dassie appears to be installed correctly.");
    ExitCode::SUCCESS
}

/// The version names a release, as the updater needs.
fn check_version() -> Result<(), String> {
    dassie_release::Version::parse(dassie_node::VERSION)
        .map(|_| ())
        .map_err(|e| format!("{:?}: {e}", dassie_node::VERSION))
}

fn check_database() -> Result<(), String> {
    let store = Store::open_in_memory().map_err(|e| e.to_string())?;
    match store.self_test().map_err(|e| e.to_string())? {
        2 => Ok(()),
        other => Err(format!("SQLite computed 1 + 1 = {other}")),
    }
}

fn check_tls() -> Result<(), String> {
    let names = certificate::self_signed_names(None);
    let stored = tls::self_signed(&names, SystemClock.now_ms()).map_err(|e| e.to_string())?;
    tls::load(&stored).map(|_| ()).map_err(|e| e.to_string())
}

fn check_wallet() -> Result<(), String> {
    if http::assets::wallet_built() {
        Ok(())
    } else {
        Err("this binary was built without the wallet (DASSIE_WALLET_DIST was not set)".into())
    }
}

use dassie_node::clock::Clock as _;

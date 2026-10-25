//! `dassie front-door`: runs a front door (spec/areas/http.md#front-doors),
//! with its settings from the environment and flags over them.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use dassie_node::clock::{SystemClock, os_rng};
use dassie_node::env::Env;
use dassie_node::front_door::{self, Settings, key, settings};
use dassie_node::log;
use lexopt::prelude::*;
use tokio_util::sync::CancellationToken;

/// The flags, each over its variable.
#[derive(Debug, Default)]
pub struct Options {
    /// Make the key file if there is none, print the `DASSIE_RELAY` entry,
    /// and exit.
    make_key: bool,
    domain: Option<String>,
    https_listen: Option<std::net::SocketAddr>,
    quic_listen: Option<std::net::SocketAddr>,
    certificate: Option<PathBuf>,
    certificate_key: Option<PathBuf>,
    key_file: Option<PathBuf>,
    relay_url: Option<String>,
}

/// Reads the flags after `front-door`.
///
/// # Errors
///
/// A flag is unknown, or its value cannot be read.
pub fn args(parser: &mut lexopt::Parser) -> Result<Options, lexopt::Error> {
    let mut options = Options::default();
    while let Some(argument) = parser.next()? {
        match argument {
            Long("make-key") => options.make_key = true,
            Long("domain") => {
                let text = parser.value()?.string()?;
                settings::parse_domain(&text)?;
                options.domain = Some(text);
            }
            Long("https-listen") => options.https_listen = Some(parser.value()?.parse()?),
            Long("quic-listen") => options.quic_listen = Some(parser.value()?.parse()?),
            Long("certificate") => options.certificate = Some(parser.value()?.into()),
            Long("certificate-key") => options.certificate_key = Some(parser.value()?.into()),
            Long("key-file") => options.key_file = Some(parser.value()?.into()),
            Long("relay-url") => {
                let text = parser.value()?.string()?;
                options.relay_url = Some(settings::parse_relay_url(&text)?);
            }
            other => return Err(other.unexpected()),
        }
    }
    Ok(options)
}

/// The settings from the environment, with the flags over them.
fn settings(options: Options) -> Result<Settings, String> {
    let mut settings = Settings::from_env(&Env::from_process()).map_err(|e| e.to_string())?;
    if let Some(domain) = options.domain {
        settings.domain = settings::parse_domain(&domain)?;
    }
    settings.https_listen = options.https_listen.unwrap_or(settings.https_listen);
    settings.quic_listen = options.quic_listen.unwrap_or(settings.quic_listen);
    settings.certificate = options.certificate.or(settings.certificate);
    settings.certificate_key = options.certificate_key.or(settings.certificate_key);
    settings.key_file = options.key_file.or(settings.key_file);
    settings.relay_url = options.relay_url.or(settings.relay_url);
    Ok(settings)
}

/// Runs the front door, or makes its key.
pub fn run(options: Options) -> ExitCode {
    let make_key = options.make_key;
    let settings = match settings(options) {
        Ok(settings) => settings,
        Err(problem) => {
            eprintln!("dassie: {problem}");
            return ExitCode::FAILURE;
        }
    };
    if make_key {
        return make(&settings);
    }
    log::init(settings.log_filter.clone());
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        eprintln!("dassie: could not start the async runtime");
        return ExitCode::FAILURE;
    };
    let result = runtime.block_on(async {
        let shutdown = CancellationToken::new();
        let Some(signals) = crate::listen_for_signals() else {
            return Err(front_door::StartError("no signal handlers".into()));
        };
        tokio::spawn(crate::stop_on_signal(signals, shutdown.clone()));
        front_door::run(&settings, (Arc::new(SystemClock), os_rng()), shutdown).await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(target: log::target::FRONT_DOOR, %error, "could not start");
            ExitCode::FAILURE
        }
    }
}

/// Makes the key file if there is none, and prints the `DASSIE_RELAY`
/// entry that names this front door.
fn make(settings: &Settings) -> ExitCode {
    let Some(path) = settings.key_file.as_ref() else {
        eprintln!("dassie: no key file: set DASSIE_FRONT_DOOR_KEY_FILE or --key-file");
        return ExitCode::FAILURE;
    };
    match key::read_or_make(path) {
        Ok(key) => {
            println!("{}", key::relay_entry(&key, &settings.relay_url()));
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("dassie: {problem}");
            ExitCode::FAILURE
        }
    }
}

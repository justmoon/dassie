//! `dassie init`: the configuration wizard (spec 2 §4.5).
//!
//! The same steps and prompts as the original: the realm (test only), the
//! public hostname, the CA's terms and an email address, then the setup
//! URL. The difference is that the daemon now runs the ACME order and the
//! wizard only shows its progress, so there is one ACME client, not two.
//!
//! Every prompt has a flag, so scripts and tests can run it unattended:
//! a prompt that would be needed without a terminal is an error instead.

use std::io::IsTerminal;

use dassie_node::env::Env;
use dassie_node::rpc::proto::{self, request::Method, response, stream_item};
use dassie_node_core::config::{Hostname, LETS_ENCRYPT_STAGING};
use dassie_node_core::urls;

use crate::client::{CallError, Client};

/// The flags of `dassie init`.
#[derive(Debug, Default)]
pub struct Options {
    /// The public hostname.
    pub hostname: Option<String>,
    /// The ACME account's email.
    pub email: Option<String>,
    /// Agree to the CA's terms without asking.
    pub agree_tos: bool,
    /// Another ACME directory than the configured one.
    pub acme_directory: Option<String>,
    /// Keep the current certificate, whatever it is.
    pub skip_acme: bool,
}

impl Options {
    /// `--acme-staging`.
    pub fn use_staging(&mut self) {
        self.acme_directory = Some(LETS_ENCRYPT_STAGING.into());
    }
}

/// A step failed or was cancelled; the message has been shown.
pub struct Stopped;

fn shown<T>(result: std::io::Result<T>) -> Result<T, Stopped> {
    result.map_err(|_| {
        let _ = cliclack::outro_cancel("Canceled. Exiting...");
        Stopped
    })
}

fn failed(error: &CallError) -> Stopped {
    let _ = cliclack::log::error(error);
    let _ = cliclack::outro_cancel("Configuration did not complete.");
    Stopped
}

/// Runs the wizard.
pub async fn run(env: &Env, options: Options) -> Result<(), Stopped> {
    let interactive = std::io::stdin().is_terminal();
    shown(cliclack::intro("Dassie Configuration"))?;
    let mut client = Client::connect(env).await.map_err(|error| failed(&error))?;
    let Ok(response::Method::GetConfig(config)) = client
        .call(Method::GetConfig(proto::GetConfigRequest {}))
        .await
    else {
        return Err(failed(&CallError {
            code: None,
            message: "Could not read the configuration".into(),
        }));
    };
    shown(cliclack::note(
        "Realm",
        "Setting realm to 'test'. This preview supports the test network only.",
    ))?;
    call(
        &mut client,
        Method::SetRealm(proto::SetRealmRequest {
            realm: proto::Realm::Test.into(),
        }),
    )
    .await?;
    let current = config
        .config
        .map(|config| config.hostname)
        .unwrap_or_default();
    let hostname = ask_hostname(options.hostname.clone(), &current, interactive)?;
    if hostname != current {
        call(
            &mut client,
            Method::SetHostname(proto::SetHostnameRequest {
                hostname: hostname.clone(),
            }),
        )
        .await?;
    }
    if !options.skip_acme && wants_acme(config.has_tls, interactive)? {
        obtain_certificate(&mut client, &options, interactive).await?;
    }
    finish(&mut client, &hostname).await
}

async fn call(client: &mut Client, method: Method) -> Result<response::Method, Stopped> {
    client.call(method).await.map_err(|error| failed(&error))
}

fn ask_hostname(
    given: Option<String>,
    current: &str,
    interactive: bool,
) -> Result<String, Stopped> {
    shown(cliclack::note(
        "Public HTTPS Setup",
        "Dassie needs a domain name that points at this host, with port 80 reachable from the \
         internet, to get a certificate from Let's Encrypt.",
    ))?;
    let hostname = match given {
        Some(hostname) => hostname,
        None if !interactive => return Err(failed(&missing("--hostname"))),
        None => shown(
            cliclack::input("What is the publicly accessible domain name of this host?")
                .default_input(current)
                .validate(|text: &String| {
                    Hostname::parse(text)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                })
                .interact(),
        )?,
    };
    match Hostname::parse(&hostname) {
        Ok(hostname) => Ok(hostname.as_str().to_string()),
        Err(error) => Err(failed(&CallError {
            code: None,
            message: format!("{hostname}: {error}"),
        })),
    }
}

fn wants_acme(has_tls: bool, interactive: bool) -> Result<bool, Stopped> {
    if !has_tls || !interactive {
        return Ok(!has_tls);
    }
    let skip = shown(
        cliclack::confirm(
            "An existing TLS certificate was found, would you like to skip ACME/LetsEncrypt setup?",
        )
        .initial_value(true)
        .interact(),
    )?;
    Ok(!skip)
}

fn missing(flag: &str) -> CallError {
    CallError {
        code: None,
        message: format!("{flag} is required when not running in a terminal"),
    }
}

/// Whether the owner agrees to the CA's terms: by flag, or when asked.
fn agree(options: &Options, terms_url: &str, interactive: bool) -> Result<bool, Stopped> {
    if options.agree_tos {
        return Ok(true);
    }
    if !interactive {
        return Err(failed(&missing("--agree-tos")));
    }
    let url = if terms_url.is_empty() {
        "(none given)"
    } else {
        terms_url
    };
    shown(cliclack::note(
        "Terms of Service",
        format!("The CA's terms of service: {url}"),
    ))?;
    shown(
        cliclack::confirm("Do you agree?")
            .initial_value(false)
            .interact(),
    )
}

async fn obtain_certificate(
    client: &mut Client,
    options: &Options,
    interactive: bool,
) -> Result<(), Stopped> {
    let directory_url = options.acme_directory.clone().unwrap_or_default();
    let terms = call(
        client,
        Method::GetAcmeTerms(proto::GetAcmeTermsRequest {
            directory_url: directory_url.clone(),
        }),
    );
    let Ok(response::Method::GetAcmeTerms(terms)) = terms.await else {
        return Err(failed(&CallError {
            code: None,
            message: "Could not read the CA's directory".into(),
        }));
    };
    if !agree(options, &terms.terms_of_service_url, interactive)? {
        let _ = cliclack::log::error(
            "A certificate from an ACME CA such as Let's Encrypt is required in this preview.",
        );
        let _ = cliclack::outro_cancel("Configuration did not complete.");
        return Err(Stopped);
    }
    let email = match &options.email {
        Some(email) => email.clone(),
        None if !interactive => String::new(),
        None => shown(
            cliclack::input("What is your email address?")
                .placeholder("The CA may write to it about your certificate")
                .required(false)
                .interact(),
        )?,
    };
    let request = proto::ObtainCertificateRequest {
        email,
        agree_to_terms_of_service: true,
        directory_url,
    };
    let spinner = cliclack::spinner();
    spinner.start("Ordering certificate...");
    let result = client
        .stream(Method::ObtainCertificate(request), |item| {
            let stream_item::Method::ObtainCertificate(progress) = item;
            spinner.set_message(step_message(progress.step()));
        })
        .await;
    match result {
        Ok(()) => {
            spinner.stop("Certificate installed");
            Ok(())
        }
        Err(error) => {
            spinner.error("The certificate could not be obtained");
            Err(failed(&error))
        }
    }
}

const fn step_message(step: proto::AcmeStep) -> &'static str {
    match step {
        proto::AcmeStep::CreatingAccount => "Creating account...",
        proto::AcmeStep::Ordering | proto::AcmeStep::Unspecified => "Ordering certificate...",
        proto::AcmeStep::PreparingChallenge => "Preparing challenge...",
        proto::AcmeStep::Validating => "Waiting for confirmation from ACME provider...",
        proto::AcmeStep::Finalizing => "Finalizing order...",
        proto::AcmeStep::Downloading => "Fetching certificate...",
        proto::AcmeStep::Installed => "Installing certificate...",
    }
}

async fn finish(client: &mut Client, hostname: &str) -> Result<(), Stopped> {
    match client
        .call(Method::GetSetupUrl(proto::GetSetupUrlRequest {}))
        .await
    {
        Ok(response::Method::GetSetupUrl(setup)) => {
            println!("{}", setup.url);
            shown(cliclack::outro(format!(
                "Configuration completed! You may now continue setup in your browser by visiting {}",
                setup.url
            )))
        }
        Err(CallError {
            code: Some(proto::ErrorCode::FailedPrecondition),
            ..
        }) => {
            let url = node_url(client, hostname).await?;
            shown(cliclack::outro(format!(
                "Configuration completed! You may now visit your Dassie node at {url}/"
            )))
        }
        Err(error) => Err(failed(&error)),
        Ok(_) => Err(failed(&CallError {
            code: None,
            message: "The daemon answered out of turn".into(),
        })),
    }
}

/// The node's URL, with the port its HTTPS listener actually has, which
/// under systemd may not be the configured one (spec 7 §4.1).
async fn node_url(client: &mut Client, hostname: &str) -> Result<String, Stopped> {
    let status = client
        .call(Method::GetStatus(proto::GetStatusRequest {}))
        .await;
    let Ok(response::Method::GetStatus(status)) = status else {
        return Err(failed(&CallError {
            code: None,
            message: "Could not read the node's status".into(),
        }));
    };
    let port = u16::try_from(status.https_port).unwrap_or(443);
    let hostname = Hostname::parse(hostname).map_err(|error| {
        failed(&CallError {
            code: None,
            message: format!("{hostname}: {error}"),
        })
    })?;
    Ok(urls::node_url(Some(&hostname), port).unwrap_or_default())
}

//! Serves the fake XRP Ledger, for the end-to-end tests and
//! `node --run dev -- --xrpl fake`:
//!
//! ```text
//! fake_ledger [--listen 127.0.0.1:0] [--close-ms 1000] [--network mainnet]
//!             [--faucet-listen 127.0.0.1:0 --cert cert.pem --key key.pem]
//! ```
//!
//! Once up, it prints one line of JSON, `{"url": "ws://…", "faucet":
//! "https://…" | null, "tokens": "https://…" | null, "deposit": "https://…"
//! | null}`, and serves until stdin closes or it is killed. It is the
//! testnet, or with `--network mainnet` mainnet, which has no faucet.
//! `tokens` is where the issuer of its network's dollar, DSUSD or RLUSD,
//! mints, freezes and claws back on request, and `deposit` where XRP
//! arrives from outside.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// A process for the end-to-end tests: it watches stdin from a thread, and
// prints its address.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

use std::time::Duration;

use dassie_node::fake_ledger::{Options, Server};
use dassie_node::store::StoredCertificate;
use dassie_node_core::certificate::CertificateSource;

fn argument(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1).cloned())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let listen = argument(&args, "--listen").unwrap_or_else(|| "127.0.0.1:0".into());
    let close_ms: u64 = argument(&args, "--close-ms").map_or(1_000, |ms| ms.parse().unwrap());
    let network = match argument(&args, "--network").as_deref() {
        None | Some("testnet") => Some(&dassie_xrpl::network::TESTNET),
        Some("mainnet") => Some(&dassie_xrpl::network::MAINNET),
        Some(_) => None,
    }
    .expect("--network is testnet or mainnet");
    let faucet = match (
        argument(&args, "--faucet-listen"),
        argument(&args, "--cert"),
        argument(&args, "--key"),
    ) {
        (Some(listen), Some(cert), Some(key)) => Some((
            listen.parse().unwrap(),
            StoredCertificate {
                source: CertificateSource::Manual,
                chain_pem: std::fs::read_to_string(cert).unwrap(),
                key_pem: std::fs::read_to_string(key).unwrap(),
            },
        )),
        _ => None,
    };
    let server = Server::start(Options {
        network,
        listen: listen.parse().unwrap(),
        faucet,
        close_every: Duration::from_millis(close_ms),
    })
    .await
    .unwrap();
    println!(
        "{}",
        serde_json::json!({
            "url": server.url,
            "faucet": server.faucet_url,
            "tokens": server.tokens_url,
            "deposit": server.deposit_url,
        })
    );
    // Until stdin closes (the parent went away), or a signal.
    let (closed, stdin_closed) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        let _ = closed.send(());
    });
    tokio::select! {
        _ = stdin_closed => {}
        _ = tokio::signal::ctrl_c() => {}
    }
    server.stop();
}

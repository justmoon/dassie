//! Generates the RPC contract's messages: `proto/dassie/owner/v1` at the
//! repo root becomes `dassie.owner.v1.rs`. protox parses it in Rust, so no
//! `protoc` binary is needed. The wallet is embedded by
//! `dassie-wallet-assets`.

// Cargo hands a build script its inputs in the environment, and takes its
// instructions from what it prints.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

type BuildResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn main() -> BuildResult {
    generate_contract()
}

fn generate_contract() -> BuildResult {
    let root = "../../proto";
    println!("cargo:rerun-if-changed={root}/dassie/owner/v1");
    let file = format!("{root}/dassie/owner/v1/transport.proto");
    let descriptors = protox::compile([file], [root])?;
    prost_build::Config::new().compile_fds(descriptors)?;
    Ok(())
}

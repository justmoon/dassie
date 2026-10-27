//! The wallet (apps/wallet), built into the binary: [`WALLET`], which the
//! CLI and the desktop app hand the daemon to serve. A crate of its own, on
//! top of the node, so that a change to the wallet compiles this table
//! again and relinks the binary, rather than compiling the node again
//! (spec/areas/node.md).

use dassie_node::http::assets::Wallet;

include!(concat!(env!("OUT_DIR"), "/wallet.rs"));

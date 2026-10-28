//! What the `dassie` binary's commands share, and what its tests reach:
//! the client of the node's IPC socket, reading a passphrase from its file,
//! stopping on a signal, and the market maker, whose loop the tests run
//! beside daemons on tokio's paused time
//! (spec/areas/testing.md#the-market-maker-and-peer-swaps-on-paused-time).
//! The commands themselves are the binary's (`main.rs`).

pub mod client;
pub mod market_maker;
pub mod passphrase;
pub mod signals;

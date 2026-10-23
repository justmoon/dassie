//! Settlement hubs in the node (spec/areas/hubs.md#the-validator-in-the-node).
//!
//! - [`transport`]: the hub's stream, on which validators and members
//!   exchange the hub's messages over QUIC, or `POST /hub` for members:
//!   its bytes, the client that sends on it ([`HubClient`]), and how the
//!   node hands what arrives to the hubs it validates ([`Hubs`]).
//! - [`config`]: which hubs the owner has the node validate, and for which
//!   sponsors.
//! - [`driver`]: one hub's validator, persisted before it acts.
//! - [`service`]: the validator role, a supervised service with a task per
//!   hub.
//! - [`escrow`]: following a hub's escrow on the XRP Ledger, without IO,
//!   and `xrpl`, its WebSocket.

pub mod config;
pub mod driver;
pub mod escrow;
pub mod service;
pub mod transport;
mod xrpl;

pub use transport::{HubAnswer, HubClient, HubExchangeError, HubStatus, Hubs, Sender};

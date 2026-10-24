//! Settlement hubs in the node (spec/areas/hubs.md#the-validator-in-the-node).
//!
//! - [`transport`]: the hub's stream, on which validators and members
//!   exchange the hub's messages over QUIC: its bytes, the client that sends on it ([`HubClient`]), and how the
//!   node hands what arrives to the hubs it validates ([`Hubs`]).
//! - [`config`]: which hubs the owner has the node validate, and for which
//!   sponsors.
//! - [`driver`]: one hub's validator, persisted before it acts.
//! - [`service`]: the validator role, a supervised service with a task per
//!   hub.
//! - [`escrow`]: following a hub's escrow on the XRP Ledger, without IO,
//!   and `xrpl`, its WebSocket.
//! - [`founding`]: founding hubs on the validator's side: a sponsor's
//!   requests, decided and written before they are answered.
//! - [`sponsor`]: founding hubs as a sponsor: asking the validators, and
//!   setting up the escrow.
//! - [`payouts`]: a member's copies of its hub's validators' payout
//!   bundles.

pub mod config;
pub mod driver;
pub mod escrow;
pub mod founding;
pub mod payouts;
pub mod service;
pub mod sponsor;
pub mod transport;
mod xrpl;

pub use transport::{HubAccess, HubAnswer, HubClient, HubExchangeError, HubStatus, Hubs, Sender};

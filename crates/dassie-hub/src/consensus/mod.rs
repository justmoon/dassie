//! Simplex consensus (Chan and Pass, 2023) for a hub's validators
//! (spec/designs/settlement-hubs.md#10-consensus), as a sans-IO state
//! machine, generic over an opaque payload that the application, the hub's
//! ledger, builds and checks ([`Application`]).
//!
//! **A view** has one leader, the view's number modulo the size of the set
//! that runs it. The leader proposes a block on the highest notarized block
//! whose later views are all nullified, stamped with its clock. A validator
//! notarizes the leader's first valid proposal: its parent is the last
//! finalized block or a notarized block above it, every view between them
//! is nullified, its stamp grows along the chain and is within the precision
//! of the validator's clock, allowing for the message delay (CometBFT's
//! proposer-based timestamps), and the application accepts its payload.
//! q = n − f notarize votes make a notarization: every validator multicasts
//! it, enters the next view, and votes to finalize the block unless it
//! nullified the view. q finalize votes make the block final, with all its
//! ancestors. A view whose proposal or notarization is late is nullified;
//! q nullify votes make a nullification, and the next view begins. Commonware's
//! `consensus::simplex` is the reference: its leader and advance timeouts,
//! skipping a leader that is gone, treating a leader's own nullify as a
//! timeout, and rebroadcasting a stuck view's nullify with the certificate
//! that entered it.
//!
//! **Epochs:** each validator set runs an epoch of its own, with views from
//! 1, and every block, vote and certificate names its epoch. The next set
//! starts once its predecessor's last block is final; while that block is
//! notarized and not final, the old set's leaders propose repeats of it,
//! whose finalization proves it (spec/decisions/epoch-handover.md).
//!
//! **Safety across crashes:** every vote we sign goes into the step's
//! [`Batch`], which the host persists before it sends the step's outputs,
//! and [`Engine::new`] restores from the [`Stored`] state the batches built.
//!
//! **Evidence:** two conflicting votes of one validator, two conflicting
//! certificates of one view, and two finalized blocks at one height each
//! name validators no honest one would be ([`Evidence`]).
//!
//! **Bounds:** rounds above the last finalized block, votes and proposals
//! ahead of our view, blocks held, finalized blocks kept, events and every
//! count on the wire have a limit; `Engine::check_invariants` checks them.
//!
//! Timings are in [`Config`], derived from the regional assumption.

mod config;
mod engine;
mod epochs;
mod hooks;
mod io;
mod persist;
mod types;
pub mod wire;

pub use config::{
    Config, MAX_BACKOFF_AFTER_VIEWS, MAX_STALL_MS, MAX_VIEW_TIMEOUT_MS, MIN_STALL_MS,
};
pub use engine::{
    BLOCKS_PER_VIEW, Engine, MAX_BLOCKS, MAX_BOUNDARIES, MAX_ROUNDS, MAX_UNFINALIZED_HEIGHTS,
    MAX_VIEWS_AHEAD, RETAINED_BLOCKS,
};
pub use epochs::{Epochs, MAX_EPOCHS, ReconfigureError};
pub use hooks::{Application, Declined, ProposeContext, VerifyContext};
pub use io::{Event, Evidence, Output, PeerId, Recipient, Stats};
pub use persist::{Batch, Epoch, MAX_STORED_ITEMS, Stored};
pub use types::{
    Block, Certificate, Finalized, Invalid, MAX_PAYLOAD_BYTES, Vote, VoteKind, verify_certificate,
    vote_digest,
};

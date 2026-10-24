//! Names (spec/designs/names.md): names people can say, held for a node ID
//! by a quorum of the bootstrap nodes' lists.
//!
//! A registrant commits to a name sealed at every registrar, and unseals it
//! once it holds every receipt; at each registrar the commitment it logged
//! first wins (spec/decisions/names-by-earliest-commitment.md). This module
//! holds what every side shares, what a name is and the commitment that
//! seals one, the registrar's state, and every node's view of who holds a
//! name, from a quorum of the registrars' lists. Like the rest of the
//! crate, it never reads a clock, an RNG or a socket
//! (spec/decisions/tigerstyle.md).

mod commitment;
pub mod holders;
mod name;
pub mod registrar;

pub use commitment::{COMMITMENT_BYTES, COMMITMENT_OID, Commitment, SALT_BYTES, Salt};
pub use holders::{HeldName, HolderChange, Holders, HoldersConfig, MAX_HOLDERS, MAX_REGISTRARS};
pub use name::{MAX_NAME_BYTES, MIN_NAME_BYTES, Name, NameError, RESERVED};
pub(crate) use name::{name_len, read_name, write_name};
pub use registrar::{Granted, LogHead, Logged, Registrar, RegistrarConfig, Unsealing};

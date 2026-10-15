//! What a release of Dassie is, and how the updater and the installer trust
//! one ([D11](../../spec/08-decisions.md#d11-release-integrity)), sans-IO.
//!
//! - [`Version`]: a release (`0.1.0`) or a canary
//!   (`canary-2026-10-15-041530-15bd7aa`), and the [`Channel`] it is published on.
//! - [`layout`]: where the mirror keeps what: `meta/<channel>`, and
//!   `<version>/` with the bundles, `SHA256SUMS` and `SHA256SUMS.minisig`.
//! - [`Sums`]: a release's `SHA256SUMS`, as `sha256sum` writes it.
//! - [`PublicKey`] and [`verify_sums`]: the minisign signature of
//!   `SHA256SUMS`, whose trusted comment names the version, so that the sums
//!   of one release cannot pass for another's.
//!
//! Everything read here comes from the network, so every parser is bounded
//! and refuses what it does not expect.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    clippy::too_many_lines
)]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod layout;
mod signature;
mod sums;
mod version;

pub use signature::{PublicKey, SignatureError, VerifyError, trusted_comment, verify_sums};
pub use sums::{Sums, SumsError};
pub use version::{Channel, Version, VersionError};

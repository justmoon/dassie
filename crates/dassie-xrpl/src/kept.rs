//! What the crate keeps across restarts, written one way: the account's
//! state, a scheme's, and the parts they share (spec/areas/xrpl.md#state-and-restarts).
//!
//! Each state starts with a version byte, then every field of that version
//! in a fixed order, in canonical OER with fixed-width integers, as a
//! settlement hub member's state is (`dassie-hub`'s `member/state.rs`). A
//! state has one encoding: no field is optional to a reader, an option is
//! a presence byte (`00` or `FF`), a list is its count (four bytes) and
//! its entries, and a state with bytes left over is refused. A new layout
//! is a new version. A reader reads the version it writes, and an older
//! one only where it says so: a scheme reads the state it wrote while it
//! peered, and keeps of it what an owner book needs
//! (spec/areas/xrpl.md#state-and-restarts).

use alloc::vec::Vec;

use dassie_ledger::Name;
use dassie_oer::Reader;

use crate::amount::TokenValue;
use crate::keys::AccountId;

/// The version of the account's state, and of a verifier's, which is written
/// as the account's is.
pub(crate) const ACCOUNT_VERSION: u8 = 1;
/// The version of a scheme's state, the owner book's.
pub(crate) const SCHEME_VERSION: u8 = 2;
/// The version of a scheme's state while the XRPL schemes peered: it
/// held the peers' addresses, the settlements a restart left to look up
/// and the entry payments held. Read, never written.
pub(crate) const PEERING_SCHEME_VERSION: u8 = 1;

/// Bytes being written.
pub(crate) struct Out(pub(crate) Vec<u8>);

impl Out {
    /// A state of `version`, its version written.
    pub(crate) fn state(version: u8) -> Self {
        let mut out = Self(Vec::new());
        out.u8(version);
        out
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    /// As canonical OER writes it: `FF` for true.
    pub(crate) fn bool(&mut self, value: bool) {
        self.u8(if value { 0xff } else { 0 });
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    /// A count of entries that follow. Every list a state keeps is bounded
    /// far below what four bytes count.
    pub(crate) fn count(&mut self, count: usize) {
        let count = u32::try_from(count);
        assert!(count.is_ok(), "a kept list is bounded");
        self.u32(count.unwrap_or(u32::MAX));
    }

    /// Bytes of a fixed length, which the reader knows.
    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// Bytes of any length up to a bound the reader knows, after their
    /// length in four bytes.
    pub(crate) fn octets(&mut self, bytes: &[u8]) {
        self.count(bytes.len());
        self.bytes(bytes);
    }

    /// A name: its length in one byte, and its characters.
    pub(crate) fn name(&mut self, name: Name) {
        let text = name.as_str().as_bytes();
        let length = u8::try_from(text.len());
        assert!(length.is_ok(), "a name is short");
        self.u8(length.unwrap_or(0));
        self.bytes(text);
    }

    /// A token's value as the ledger writes an amount's first eight bytes.
    pub(crate) fn token(&mut self, value: TokenValue) {
        self.u64(value.to_bits());
    }

    pub(crate) fn option<T>(&mut self, value: Option<T>, write: impl FnOnce(&mut Self, T)) {
        self.bool(value.is_some());
        if let Some(value) = value {
            write(self, value);
        }
    }
}

/// Reads what [`Out`] wrote: each `None` is a state no writer of this
/// version writes.
pub(crate) struct In<'a>(pub(crate) Reader<'a>);

impl<'a> In<'a> {
    /// A state of `version`: `None` for any other.
    pub(crate) fn state(state: &'a [u8], version: u8) -> Option<Self> {
        let (input, read) = Self::versioned(state)?;
        (read == version).then_some(input)
    }

    /// A state of any version, and its version.
    pub(crate) fn versioned(state: &'a [u8]) -> Option<(Self, u8)> {
        let mut input = Self(Reader::new(state));
        let version = input.u8()?;
        Some((input, version))
    }

    /// Whether every byte was read.
    pub(crate) fn finish(&self) -> Option<()> {
        self.0.finish().ok()
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.0.read_u8().ok()
    }

    pub(crate) fn bool(&mut self) -> Option<bool> {
        self.0.read_bool().ok()
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.0.read_u32().ok()
    }

    pub(crate) fn u64(&mut self) -> Option<u64> {
        self.0.read_u64().ok()
    }

    /// A count of at most `most` entries, each at least `least` bytes, that
    /// the rest of the input can hold: a count from no state of ours
    /// allocates nothing.
    pub(crate) fn count(&mut self, least: usize, most: usize) -> Option<usize> {
        let count = usize::try_from(self.u32()?).ok()?;
        (count <= most && count.saturating_mul(least) <= self.0.remaining()).then_some(count)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.0.read_array::<N>().ok().copied()
    }

    /// Bytes of at most `most`, after their length.
    pub(crate) fn octets(&mut self, most: usize) -> Option<&'a [u8]> {
        let length = self.count(1, most)?;
        self.0.read_bytes(length).ok()
    }

    pub(crate) fn name(&mut self) -> Option<Name> {
        let length = usize::from(self.u8()?);
        let text = self.0.read_bytes(length).ok()?;
        Name::new(core::str::from_utf8(text).ok()?).ok()
    }

    pub(crate) fn account(&mut self) -> Option<AccountId> {
        Some(AccountId(self.array()?))
    }

    pub(crate) fn token(&mut self) -> Option<TokenValue> {
        TokenValue::from_bits(self.u64()?)
    }

    pub(crate) fn option<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Option<T>,
    ) -> Option<Option<T>> {
        if self.bool()? {
            Some(Some(read(self)?))
        } else {
            Some(None)
        }
    }
}

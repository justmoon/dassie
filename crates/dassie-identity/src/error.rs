use core::fmt;

use crate::phrase::{Positions, Realm};

/// Why an input was rejected.
///
/// Every variant describes bad input, never a bug in the caller: programming
/// errors are assertions instead. The variants carry no secret material, so
/// they are safe to log and to show to the user: a phrase's errors name
/// positions and counts, never a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The phrase is longer than any phrase could be, even with generous
    /// whitespace.
    PhraseTooLong,
    /// The text has 12, 15, 18, 21 or 24 words: a BIP-39 phrase, another
    /// wallet's or a Dassie phrase from before the current format, which
    /// Dassie no longer reads.
    Bip39Phrase {
        /// How many words it has.
        words: usize,
    },
    /// The text does not have the 20 words of a phrase.
    WrongWordCount {
        /// How many words it has.
        words: usize,
    },
    /// Too many words are wrong or unreadable for the phrase to be
    /// restored: more than two wrong at places not known, or more than
    /// `2 × wrong + unreadable` five.
    TooDamaged,
    /// The phrase's kind is 0, which no phrase has.
    InvalidKind,
    /// The phrase's kind is not one this version knows: a newer Dassie
    /// made it.
    NewerKind {
        /// The kind it names.
        kind: u8,
    },
    /// The phrase is of another realm than the one expected.
    OtherRealm {
        /// The phrase's realm.
        realm: Realm,
    },
    /// The phrase was read only by correcting the words at these
    /// positions, and the reader has no one to confirm the correction.
    NeedsConfirmation {
        /// The positions of the words corrected.
        changed: Positions,
    },
    /// The clock is outside the hours a phrase can record, so no phrase
    /// can be made now.
    ClockOutOfRange,
    /// A segment of a seed path is empty, longer than
    /// [`MAX_SEGMENT_BYTES`](crate::MAX_SEGMENT_BYTES), or has a character
    /// outside printable ASCII, a space or `/`.
    InvalidSeedPath,
    /// A node ID is not 39 characters of base32 whose unused last bits are
    /// zero.
    InvalidNodeId,
    /// Thirty-two bytes that are not the encoding of a curve point.
    InvalidPublicKey,
    /// A signature does not verify.
    InvalidSignature,
    /// A DER key encoding has the wrong length or preamble.
    InvalidDer,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::PhraseTooLong => "phrase is too long",
            Self::Bip39Phrase { words } => {
                return write!(
                    f,
                    "a phrase of {words} words is a BIP-39 phrase, which Dassie no longer reads"
                );
            }
            Self::WrongWordCount { words } => {
                return write!(f, "a phrase has 20 words, not {words}");
            }
            Self::TooDamaged => "too many words of the phrase are wrong or unreadable",
            Self::InvalidKind => "not a phrase: its kind is 0",
            Self::NewerKind { kind } => {
                return write!(f, "phrase of kind {kind}, made by a newer Dassie");
            }
            Self::OtherRealm { realm } => {
                return write!(f, "phrase of the {realm} realm");
            }
            Self::NeedsConfirmation { changed } => {
                // One-based, since this text is meant for people.
                let words = if changed.len() == 1 { "word" } else { "words" };
                return write!(
                    f,
                    "the phrase needs correcting at {words} {changed}, which only its owner can confirm"
                );
            }
            Self::ClockOutOfRange => "the clock is outside the hours a phrase can record",
            Self::InvalidSeedPath => "invalid seed path",
            Self::InvalidNodeId => "invalid node ID",
            Self::InvalidPublicKey => "invalid public key",
            Self::InvalidSignature => "invalid signature",
            Self::InvalidDer => "invalid DER key encoding",
        };
        f.write_str(text)
    }
}

impl core::error::Error for Error {}

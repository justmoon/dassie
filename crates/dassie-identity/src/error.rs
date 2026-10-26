use core::fmt;

/// Why an input was rejected.
///
/// Every variant describes bad input, never a bug in the caller: programming
/// errors are assertions instead. The variants carry no secret material, so
/// they are safe to log and to show to the user, so a mistyped phrase is
/// reported as one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// BIP-39 entropy must be 16, 20, 24, 28 or 32 bytes.
    InvalidEntropyLength,
    /// The phrase is longer than any valid phrase could be, even with
    /// generous whitespace.
    PhraseTooLong,
    /// The phrase contains a character outside ASCII. Only the English
    /// wordlist is supported, so this is never a valid phrase.
    NonAsciiPhrase,
    /// The phrase does not have 12, 15, 18, 21 or 24 words.
    InvalidWordCount,
    /// A word is not on the BIP-39 English wordlist.
    UnknownWord {
        /// Zero-based position of the word in the phrase.
        position: usize,
    },
    /// All words are valid but the checksum does not match, so at least one
    /// of them was mistyped or the order is wrong.
    InvalidChecksum,
    /// A segment of a seed path is empty, longer than
    /// [`MAX_SEGMENT_BYTES`](crate::MAX_SEGMENT_BYTES), or has a character
    /// outside printable ASCII, a space or `/`.
    InvalidSeedPath,
    /// A node ID is not 2 to 45 characters of `[A-Za-z0-9_-]`.
    InvalidNodeId,
    /// Thirty-two bytes that are not the encoding of a curve point.
    InvalidPublicKey,
    /// A signature does not verify.
    InvalidSignature,
    /// A DER key encoding has the wrong length or preamble.
    InvalidDer,
    /// A Diffie-Hellman result was all zeros: the peer's key has small order
    /// and would let it force a known shared secret.
    NonContributory,
    /// A node label is not 39 base32 characters naming a node ID.
    InvalidLabel,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::InvalidEntropyLength => "entropy must be 16, 20, 24, 28 or 32 bytes",
            Self::PhraseTooLong => "phrase is too long",
            Self::NonAsciiPhrase => "phrase contains a non-ASCII character",
            Self::InvalidWordCount => "phrase must have 12, 15, 18, 21 or 24 words",
            Self::UnknownWord { position } => {
                // One-based, since this text is meant for people.
                return write!(f, "word {} is not on the wordlist", position + 1);
            }
            Self::InvalidChecksum => "phrase checksum does not match",
            Self::InvalidSeedPath => "invalid seed path",
            Self::InvalidNodeId => "invalid node ID",
            Self::InvalidPublicKey => "invalid public key",
            Self::InvalidSignature => "invalid signature",
            Self::InvalidDer => "invalid DER key encoding",
            Self::NonContributory => "non-contributory Diffie-Hellman result",
            Self::InvalidLabel => "invalid node label",
        };
        f.write_str(text)
    }
}

impl core::error::Error for Error {}

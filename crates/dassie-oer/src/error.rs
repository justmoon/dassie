use core::fmt;

/// Why a decode failed, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeError {
    /// Byte offset into the reader's input at which the problem was found.
    pub offset: usize,
    /// What was wrong.
    pub kind: DecodeErrorKind,
}

/// The kinds of decode failure.
///
/// Decoding accepts only canonical OER (X.696 clause 8 with the canonical
/// rules of clause 11), so every malformed or merely non-canonical input maps
/// to one of these instead of being normalized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeErrorKind {
    /// The input ended before the value did.
    UnexpectedEnd,
    /// Bytes were left over after a value that must fill its container.
    TrailingBytes,
    /// A length determinant used the long form where the short form fits, or
    /// had a leading zero octet.
    NonCanonicalLength,
    /// A length determinant does not fit in `usize`.
    LengthOverflow,
    /// A variable-length integer had a redundant leading zero octet or no
    /// octets at all.
    NonCanonicalInteger,
    /// An integer does not fit in the target type.
    IntegerOverflow,
    /// A size-constrained value is shorter or longer than allowed.
    SizeOutOfRange,
    /// A string contains a character outside its permitted alphabet.
    InvalidCharacter,
    /// A UTF-8 string is not valid UTF-8.
    InvalidUtf8,
    /// A boolean octet is neither `0x00` nor `0xFF`.
    InvalidBoolean,
    /// An object identifier is malformed or not canonical.
    InvalidObjectIdentifier,
    /// A tag is not canonical or not one the type allows.
    UnexpectedTag,
    /// A value differs from the constant the type requires.
    UnexpectedValue,
    /// A count exceeds the limit the caller set.
    LimitExceeded,
}

impl DecodeError {
    /// Creates an error of `kind` at `offset`.
    #[must_use]
    pub const fn new(offset: usize, kind: DecodeErrorKind) -> Self {
        Self { offset, kind }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at offset {}", self.kind, self.offset)
    }
}

impl fmt::Display for DecodeErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::UnexpectedEnd => "unexpected end of input",
            Self::TrailingBytes => "trailing bytes",
            Self::NonCanonicalLength => "non-canonical length determinant",
            Self::LengthOverflow => "length determinant overflows",
            Self::NonCanonicalInteger => "non-canonical integer encoding",
            Self::IntegerOverflow => "integer out of range",
            Self::SizeOutOfRange => "size out of range",
            Self::InvalidCharacter => "character outside the permitted alphabet",
            Self::InvalidUtf8 => "invalid UTF-8",
            Self::InvalidBoolean => "invalid boolean",
            Self::InvalidObjectIdentifier => "invalid object identifier",
            Self::UnexpectedTag => "unexpected tag",
            Self::UnexpectedValue => "unexpected value",
            Self::LimitExceeded => "limit exceeded",
        };
        f.write_str(text)
    }
}

impl core::error::Error for DecodeError {}

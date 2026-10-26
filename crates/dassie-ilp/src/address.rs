use core::fmt;

/// Longest permitted ILP address, in octets (RFC 15).
pub const MAX_ADDRESS_LENGTH: usize = 1023;

/// The allocation schemes of RFC 15 §"Allocation Schemes".
const SCHEMES: [&str; 10] = [
    "g", "private", "example", "peer", "self", "test", "test1", "test2", "test3", "local",
];

/// An Interledger address, validated against the RFC 15 grammar:
///
/// ```text
/// address   = scheme 1*( "." segment )
/// segment   = 1*( ALPHA / DIGIT / "_" / "~" / "-" )
/// ```
///
/// The governing standard is stricter than any 7-bit string, and following
/// it keeps malformed destinations out of the routing table.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address<'a>(&'a str);

/// Why a string is not a valid ILP address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidAddress {
    /// Empty, or longer than [`MAX_ADDRESS_LENGTH`].
    Length,
    /// The first segment is not an RFC 15 allocation scheme.
    Scheme,
    /// The address has no segment after the scheme.
    NoSegments,
    /// A segment is empty or contains a character outside its alphabet.
    Segment,
}

impl<'a> Address<'a> {
    /// Validates `text` as an ILP address.
    pub const fn parse(text: &'a str) -> Result<Self, InvalidAddress> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_ADDRESS_LENGTH {
            return Err(InvalidAddress::Length);
        }
        let scheme_end = match first_dot(bytes) {
            Some(index) => index,
            None => return Err(InvalidAddress::NoSegments),
        };
        let (scheme, rest) = bytes.split_at(scheme_end);
        if !is_scheme(scheme) {
            return Err(InvalidAddress::Scheme);
        }
        // `rest` starts with the dot; every dot must be followed by a
        // non-empty segment.
        let mut segment_length = 0;
        let mut remaining = rest;
        while let [byte, tail @ ..] = remaining {
            if *byte == b'.' {
                if segment_length == 0 && remaining.len() != rest.len() {
                    return Err(InvalidAddress::Segment);
                }
                segment_length = 0;
            } else if is_segment_char(*byte) {
                segment_length += 1;
            } else {
                return Err(InvalidAddress::Segment);
            }
            remaining = tail;
        }
        if segment_length == 0 {
            return Err(InvalidAddress::Segment);
        }
        Ok(Self(text))
    }

    /// The address as text.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.0
    }

    /// Whether `prefix` is this address or one of its ancestors, comparing
    /// whole segments: `test.alice` is a prefix of `test.alice.bob` but not of
    /// `test.alicebob`.
    #[must_use]
    pub fn has_prefix(&self, prefix: Address<'_>) -> bool {
        match self.0.strip_prefix(prefix.0) {
            Some(rest) => rest.is_empty() || rest.starts_with('.'),
            None => false,
        }
    }

    /// The part after `prefix` and its separating dot, if `prefix` is a
    /// proper ancestor of this address.
    #[must_use]
    pub fn strip_prefix(&self, prefix: Address<'_>) -> Option<&'a str> {
        self.0.strip_prefix(prefix.0)?.strip_prefix('.')
    }
}

const fn first_dot(bytes: &[u8]) -> Option<usize> {
    let mut index = 0;
    let mut rest = bytes;
    while let [byte, tail @ ..] = rest {
        if *byte == b'.' {
            return Some(index);
        }
        index += 1;
        rest = tail;
    }
    None
}

const fn is_scheme(candidate: &[u8]) -> bool {
    let mut schemes: &[&str] = &SCHEMES;
    while let [scheme, tail @ ..] = schemes {
        if bytes_equal(scheme.as_bytes(), candidate) {
            return true;
        }
        schemes = tail;
    }
    false
}

const fn bytes_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut left = left;
    let mut right = right;
    while let ([a, left_tail @ ..], [b, right_tail @ ..]) = (left, right) {
        if *a != *b {
            return false;
        }
        left = left_tail;
        right = right_tail;
    }
    true
}

const fn is_segment_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'~' | b'-')
}

impl fmt::Display for Address<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl fmt::Debug for Address<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Address({:?})", self.0)
    }
}

impl fmt::Display for InvalidAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Length => "ILP address is empty or too long",
            Self::Scheme => "ILP address has an unknown allocation scheme",
            Self::NoSegments => "ILP address has no segment after the scheme",
            Self::Segment => "ILP address has an invalid segment",
        };
        f.write_str(text)
    }
}

impl core::error::Error for InvalidAddress {}

/// Validates a string literal as an ILP address at compile time.
#[macro_export]
macro_rules! address {
    ($text:expr) => {{
        const ADDRESS: $crate::Address<'static> = match $crate::Address::parse($text) {
            Ok(address) => address,
            Err(_) => panic!("invalid ILP address literal"),
        };
        ADDRESS
    }};
}

use crate::constraint::{Alphabet, Size};
use crate::error::{DecodeError, DecodeErrorKind};

/// A cursor over canonical OER input.
///
/// Every `read_*` method either consumes exactly one encoded value and
/// returns it, or fails without a meaningful position afterwards: callers
/// stop at the first error. Values borrow from the input, so decoding never
/// allocates.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    input: &'a [u8],
    position: usize,
    // Offset of `input` within the outermost input, so that errors from
    // nested readers (open types, CONTAINING) report absolute offsets.
    base: usize,
}

impl<'a> Reader<'a> {
    /// Starts reading `input` from its beginning.
    #[must_use]
    pub const fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            position: 0,
            base: 0,
        }
    }

    /// Bytes consumed so far.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Bytes left to read.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.input.len() - self.position
    }

    /// Whether all input has been consumed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// An error of `kind` at the current position.
    #[must_use]
    pub const fn error(&self, kind: DecodeErrorKind) -> DecodeError {
        DecodeError::new(self.base + self.position, kind)
    }

    /// Fails with [`DecodeErrorKind::TrailingBytes`] unless all input has been
    /// consumed.
    pub const fn finish(&self) -> Result<(), DecodeError> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(self.error(DecodeErrorKind::TrailingBytes))
        }
    }

    /// The bytes consumed since `start`, a value returned by
    /// [`position`](Self::position) earlier. Used to capture the exact
    /// encoding of a value, for example to verify a signature over it.
    #[must_use]
    pub fn consumed_since(&self, start: usize) -> &'a [u8] {
        assert!(start <= self.position, "capture start is after the cursor");
        self.input.get(start..self.position).unwrap_or_default()
    }

    /// Reads `count` raw octets.
    pub fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if count > self.remaining() {
            return Err(self.error(DecodeErrorKind::UnexpectedEnd));
        }
        let end = self.position + count;
        let bytes = self.input.get(self.position..end).unwrap_or_default();
        self.position = end;
        Ok(bytes)
    }

    /// Reads exactly `N` raw octets (a fixed-size OCTET STRING).
    pub fn read_array<const N: usize>(&mut self) -> Result<&'a [u8; N], DecodeError> {
        let bytes = self.read_bytes(N)?;
        // `read_bytes` returned exactly `N` bytes, so this cannot fail.
        bytes
            .try_into()
            .map_err(|_| self.error(DecodeErrorKind::UnexpectedEnd))
    }

    /// Reads one octet.
    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        let [byte] = *self.read_array::<1>()?;
        Ok(byte)
    }

    /// Reads a fixed-size 16-bit unsigned integer.
    pub fn read_u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(*self.read_array()?))
    }

    /// Reads a fixed-size 32-bit unsigned integer.
    pub fn read_u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(*self.read_array()?))
    }

    /// Reads a fixed-size 64-bit unsigned integer.
    pub fn read_u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(*self.read_array()?))
    }

    /// Reads a length determinant (X.696 §8.6), canonical form only.
    pub fn read_length(&mut self) -> Result<usize, DecodeError> {
        let start = self.position;
        let first = self.read_u8()?;
        if first < 0x80 {
            return Ok(usize::from(first));
        }
        let octets = usize::from(first & 0x7f);
        let bytes = self.read_bytes(octets)?;
        let at_start = |kind| DecodeError::new(self.base + start, kind);
        match bytes.first() {
            None | Some(0) => return Err(at_start(DecodeErrorKind::NonCanonicalLength)),
            Some(_) => {}
        }
        if octets > size_of::<usize>() {
            return Err(at_start(DecodeErrorKind::LengthOverflow));
        }
        let length = bytes
            .iter()
            .fold(0usize, |value, &byte| (value << 8) | usize::from(byte));
        if length < 0x80 {
            return Err(at_start(DecodeErrorKind::NonCanonicalLength));
        }
        Ok(length)
    }

    /// Reads the content octets of an unconstrained unsigned integer: a
    /// length determinant followed by the minimal big-endian octets.
    fn read_uint_octets(&mut self) -> Result<&'a [u8], DecodeError> {
        let start = self.position;
        let length = self.read_length()?;
        let octets = self.read_bytes(length)?;
        match octets {
            [] | [0, _, ..] => Err(DecodeError::new(
                self.base + start,
                DecodeErrorKind::NonCanonicalInteger,
            )),
            _ => Ok(octets),
        }
    }

    /// Reads an unconstrained unsigned integer that must fit in a `u64`.
    pub fn read_var_uint(&mut self) -> Result<u64, DecodeError> {
        let start = self.position;
        let octets = self.read_uint_octets()?;
        if octets.len() > size_of::<u64>() {
            return Err(DecodeError::new(
                self.base + start,
                DecodeErrorKind::IntegerOverflow,
            ));
        }
        Ok(octets
            .iter()
            .fold(0u64, |value, &byte| (value << 8) | u64::from(byte)))
    }

    /// Reads an unconstrained unsigned integer, mapping values above
    /// `u64::MAX` to `u64::MAX`. STREAM (RFC 29 §5.2) requires this for amounts
    /// and limits, which peers may send as arbitrarily large integers.
    pub fn read_var_uint_saturating(&mut self) -> Result<u64, DecodeError> {
        let octets = self.read_uint_octets()?;
        if octets.len() > size_of::<u64>() {
            return Ok(u64::MAX);
        }
        Ok(octets
            .iter()
            .fold(0u64, |value, &byte| (value << 8) | u64::from(byte)))
    }

    /// Reads a SEQUENCE OF quantity field and checks it against `max`, the
    /// caller's limit on the number of elements.
    pub fn read_quantity(&mut self, max: usize) -> Result<usize, DecodeError> {
        let start = self.position;
        let quantity = self.read_var_uint()?;
        match usize::try_from(quantity) {
            Ok(count) if count <= max => Ok(count),
            _ => Err(DecodeError::new(
                self.base + start,
                DecodeErrorKind::LimitExceeded,
            )),
        }
    }

    /// Reads a length-prefixed OCTET STRING whose size must satisfy `size`.
    pub fn read_octets(&mut self, size: Size) -> Result<&'a [u8], DecodeError> {
        let start = self.position;
        let length = self.read_length()?;
        if !size.contains(length) {
            return Err(DecodeError::new(
                self.base + start,
                DecodeErrorKind::SizeOutOfRange,
            ));
        }
        self.read_bytes(length)
    }

    /// Reads a length-prefixed 7-bit string (IA5String, VisibleString and
    /// restrictions of them). For these types characters are octets, so `size`
    /// counts both.
    pub fn read_ascii(&mut self, size: Size, alphabet: Alphabet) -> Result<&'a str, DecodeError> {
        let start = self.position;
        let bytes = self.read_octets(size)?;
        self.check_ascii(start, bytes, alphabet)
    }

    /// Reads a fixed-size 7-bit string of exactly `length` characters, which
    /// OER encodes without a length determinant.
    pub fn read_fixed_ascii(
        &mut self,
        length: usize,
        alphabet: Alphabet,
    ) -> Result<&'a str, DecodeError> {
        let start = self.position;
        let bytes = self.read_bytes(length)?;
        self.check_ascii(start, bytes, alphabet)
    }

    fn check_ascii(
        &self,
        start: usize,
        bytes: &'a [u8],
        alphabet: Alphabet,
    ) -> Result<&'a str, DecodeError> {
        let error = |kind| DecodeError::new(self.base + start, kind);
        if !alphabet.contains_all(bytes) {
            return Err(error(DecodeErrorKind::InvalidCharacter));
        }
        // Every byte is 7-bit, which is always valid UTF-8.
        core::str::from_utf8(bytes).map_err(|_| error(DecodeErrorKind::InvalidCharacter))
    }

    /// Reads a length-prefixed UTF8String. `size` limits its length in octets:
    /// the only UTF8String constraint in Dassie's protocols (the ILP reject
    /// message) is specified in octets.
    pub fn read_utf8(&mut self, size: Size) -> Result<&'a str, DecodeError> {
        let start = self.position;
        let bytes = self.read_octets(size)?;
        core::str::from_utf8(bytes)
            .map_err(|_| DecodeError::new(self.base + start, DecodeErrorKind::InvalidUtf8))
    }

    /// Reads a BOOLEAN, which canonical OER encodes as `0x00` or `0xFF`.
    pub fn read_bool(&mut self) -> Result<bool, DecodeError> {
        match self.read_u8()? {
            0x00 => Ok(false),
            0xff => Ok(true),
            _ => Err(DecodeError::new(
                self.base + self.position - 1,
                DecodeErrorKind::InvalidBoolean,
            )),
        }
    }

    /// Reads raw octets and fails unless they equal `expected`. Used for
    /// constant fields such as fixed object identifiers.
    pub fn expect_bytes(&mut self, expected: &[u8]) -> Result<(), DecodeError> {
        let start = self.position;
        if self.read_bytes(expected.len())? == expected {
            Ok(())
        } else {
            Err(DecodeError::new(
                self.base + start,
                DecodeErrorKind::UnexpectedValue,
            ))
        }
    }

    /// Reads a context-specific tag (X.696 §8.7) and returns its number.
    pub fn read_context_tag(&mut self) -> Result<u64, DecodeError> {
        let start = self.position;
        let first = self.read_u8()?;
        let unexpected = DecodeError::new(self.base + start, DecodeErrorKind::UnexpectedTag);
        if first & 0xc0 != 0x80 {
            return Err(unexpected);
        }
        let number = first & 0x3f;
        if number < 0x3f {
            return Ok(u64::from(number));
        }
        let number = self.read_base128().map_err(|error| match error.kind {
            DecodeErrorKind::UnexpectedEnd => error,
            _ => unexpected,
        })?;
        if number < 0x3f {
            return Err(unexpected);
        }
        Ok(number)
    }

    /// Reads one base-128 subidentifier: big-endian groups of seven bits, all
    /// but the last with the high bit set, without a leading `0x80` octet.
    pub fn read_base128(&mut self) -> Result<u64, DecodeError> {
        let start = self.position;
        let invalid = DecodeError::new(self.base + start, DecodeErrorKind::InvalidObjectIdentifier);
        let mut value: u64 = 0;
        // A u64 needs at most ten groups of seven bits; the bound also keeps
        // the loop finite on hostile input.
        for index in 0..10 {
            let byte = self.read_u8()?;
            if index == 0 && byte == 0x80 {
                return Err(invalid);
            }
            if value.leading_zeros() < 7 {
                return Err(DecodeError::new(
                    self.base + start,
                    DecodeErrorKind::IntegerOverflow,
                ));
            }
            value = (value << 7) | u64::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(invalid)
    }

    /// Reads a length-prefixed value (an open type or an OCTET STRING
    /// CONTAINING another type) with `decode`, which must consume the
    /// content exactly. The inner reader cannot see past the content.
    pub fn read_prefixed<T>(
        &mut self,
        decode: impl FnOnce(&mut Reader<'a>) -> Result<T, DecodeError>,
    ) -> Result<T, DecodeError> {
        let length = self.read_length()?;
        let content_start = self.position;
        let content = self.read_bytes(length)?;
        let mut inner = Reader {
            input: content,
            position: 0,
            base: self.base + content_start,
        };
        let value = decode(&mut inner)?;
        inner.finish()?;
        Ok(value)
    }
}

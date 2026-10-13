use crate::constraint::{Alphabet, Size};
use crate::size;

/// A cursor that writes canonical OER into a caller-provided buffer.
///
/// Callers size the buffer with [`Encode::encoded_len`](crate::Encode) first,
/// so running out of space is a programming error, and the writer asserts
/// against it rather than returning an error. The same goes for values that
/// violate their type's constraints: the typed wrappers that callers encode
/// validate on construction, so a violation here is a bug, not bad input.
#[derive(Debug)]
pub struct Writer<'a> {
    buffer: &'a mut [u8],
    position: usize,
}

impl<'a> Writer<'a> {
    /// Starts writing at the beginning of `buffer`.
    #[must_use]
    pub const fn new(buffer: &'a mut [u8]) -> Self {
        Self {
            buffer,
            position: 0,
        }
    }

    /// Bytes written so far.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// Writes raw octets.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        let end = self.position + bytes.len();
        match self.buffer.get_mut(self.position..end) {
            Some(destination) => destination.copy_from_slice(bytes),
            None => panic!("OER writer overflow: buffer smaller than encoded_len"),
        }
        self.position = end;
    }

    /// Writes one octet.
    pub fn write_u8(&mut self, value: u8) {
        self.write_bytes(&[value]);
    }

    /// Writes a fixed-size 16-bit unsigned integer.
    pub fn write_u16(&mut self, value: u16) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Writes a fixed-size 32-bit unsigned integer.
    pub fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Writes a fixed-size 64-bit unsigned integer.
    pub fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Writes a length determinant in canonical form.
    pub fn write_length(&mut self, length: usize) {
        if length < 0x80 {
            self.write_u8(low_byte(length as u64));
            return;
        }
        let octets = size::uint_octets(length as u64);
        // At most eight octets, so the count fits the low seven bits.
        self.write_u8(0x80 | low_byte(octets as u64));
        self.write_minimal_uint(length as u64, octets);
    }

    fn write_minimal_uint(&mut self, value: u64, octets: usize) {
        assert!(
            (1..=8).contains(&octets),
            "integer octet count out of range"
        );
        let bytes = value.to_be_bytes();
        self.write_bytes(bytes.get(8 - octets..).unwrap_or_default());
    }

    /// Writes an unconstrained unsigned integer with minimal octets.
    pub fn write_var_uint(&mut self, value: u64) {
        let octets = size::uint_octets(value);
        self.write_length(octets);
        self.write_minimal_uint(value, octets);
    }

    /// Writes a SEQUENCE OF quantity field.
    pub fn write_quantity(&mut self, count: usize) {
        self.write_var_uint(count as u64);
    }

    /// Writes a length-prefixed OCTET STRING whose size must satisfy `size`.
    pub fn write_octets(&mut self, bytes: &[u8], size: Size) {
        assert!(
            size.contains(bytes.len()),
            "octet string size violates its constraint"
        );
        self.write_length(bytes.len());
        self.write_bytes(bytes);
    }

    /// Writes a length-prefixed 7-bit string.
    pub fn write_ascii(&mut self, text: &str, size: Size, alphabet: Alphabet) {
        assert!(
            alphabet.contains_all(text.as_bytes()),
            "string violates its alphabet"
        );
        self.write_octets(text.as_bytes(), size);
    }

    /// Writes a fixed-size 7-bit string of exactly `length` characters.
    pub fn write_fixed_ascii(&mut self, text: &str, length: usize, alphabet: Alphabet) {
        assert!(
            text.len() == length,
            "fixed-size string has the wrong length"
        );
        assert!(
            alphabet.contains_all(text.as_bytes()),
            "string violates its alphabet"
        );
        self.write_bytes(text.as_bytes());
    }

    /// Writes a length-prefixed UTF8String, `size` counting octets.
    pub fn write_utf8(&mut self, text: &str, size: Size) {
        self.write_octets(text.as_bytes(), size);
    }

    /// Writes a BOOLEAN.
    pub fn write_bool(&mut self, value: bool) {
        self.write_u8(if value { 0xff } else { 0x00 });
    }

    /// Writes a context-specific tag with tag number `number`.
    pub fn write_context_tag(&mut self, number: u64) {
        if number < 0x3f {
            self.write_u8(0x80 | low_byte(number));
        } else {
            self.write_u8(0xbf);
            self.write_base128(number);
        }
    }

    /// Writes `value` as one base-128 subidentifier.
    pub fn write_base128(&mut self, value: u64) {
        let groups = size::base128(value);
        for index in (0..groups).rev() {
            let shift = 7 * index;
            let continuation = if index == 0 { 0 } else { 0x80 };
            self.write_u8(low_byte((value >> shift) & 0x7f) | continuation);
        }
    }

    /// Writes a length-prefixed value (an open type or an OCTET STRING
    /// CONTAINING another type) whose content is `length` octets, produced by
    /// `encode`.
    pub fn write_prefixed(&mut self, length: usize, encode: impl FnOnce(&mut Self)) {
        self.write_length(length);
        let start = self.position;
        encode(self);
        assert!(
            self.position - start == length,
            "prefixed content length differs from its declared length"
        );
    }
}

/// The least significant octet of `value`. Callers use it where the value is
/// known to fit, and the assertion checks it.
#[allow(clippy::cast_possible_truncation)]
const fn low_byte(value: u64) -> u8 {
    assert!(value <= 0xff, "value does not fit in one octet");
    value as u8
}

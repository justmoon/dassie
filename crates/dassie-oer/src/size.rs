//! Encoded sizes, for implementing [`Encode::encoded_len`](crate::Encode).

/// Octets needed for the unsigned big-endian value `value`, at least one.
#[must_use]
pub const fn uint_octets(value: u64) -> usize {
    let significant_bits = u64::BITS - value.leading_zeros();
    if significant_bits == 0 {
        1
    } else {
        significant_bits.div_ceil(8) as usize
    }
}

/// Size of a length determinant for `length` (X.696 §8.6).
#[must_use]
pub const fn length_determinant(length: usize) -> usize {
    if length < 0x80 {
        1
    } else {
        1 + uint_octets(length as u64)
    }
}

/// Size of an unconstrained unsigned integer: its length determinant (always
/// one octet, since a `u64` needs at most eight) and its minimal octets.
#[must_use]
pub const fn var_uint(value: u64) -> usize {
    1 + uint_octets(value)
}

/// Size of a variable-length octet string with `length` octets of content,
/// and of any other length-prefixed value (open types, CONTAINING).
#[must_use]
pub const fn prefixed(length: usize) -> usize {
    length_determinant(length) + length
}

/// Size of a SEQUENCE OF quantity field for `count` elements.
#[must_use]
pub const fn quantity(count: usize) -> usize {
    var_uint(count as u64)
}

/// Size of a context-specific tag with tag number `number` (X.696 §8.7).
#[must_use]
pub const fn tag(number: u64) -> usize {
    if number < 63 { 1 } else { 1 + base128(number) }
}

/// Size of `value` as base-128 subidentifier octets.
#[must_use]
pub const fn base128(value: u64) -> usize {
    let significant_bits = u64::BITS - value.leading_zeros();
    if significant_bits == 0 {
        1
    } else {
        significant_bits.div_ceil(7) as usize
    }
}

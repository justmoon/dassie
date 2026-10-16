//! Properties: values round-trip, decoding never panics on arbitrary input,
//! and whatever decodes is canonical, so re-encoding it reproduces the
//! consumed bytes exactly.

#![allow(clippy::unwrap_used)]

use dassie_oer::{Alphabet, DecodeError, ObjectIdentifier, Reader, Size, Writer, oid, size};
use proptest::collection::vec;
use proptest::prelude::*;

fn write_with(capacity: usize, write: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut buffer = vec![0; capacity];
    let mut writer = Writer::new(&mut buffer);
    write(&mut writer);
    let length = writer.position();
    buffer.truncate(length);
    buffer
}

/// Arbitrary bytes, mixed with bytes that favour the octets the decoders
/// single out (`0x00`, `0xFF`, printable ASCII), bytes that start with a
/// short length, and well-formed strings, ASCII or not, followed by a few
/// more bytes. Random bytes alone rarely get a read past its prefix, and
/// almost never make a string that decodes.
fn input() -> impl Strategy<Value = Vec<u8>> {
    let octet = || prop_oneof![any::<u8>(), 0x20u8..0x7f, Just(0x00), Just(0xff)];
    let text = prop_oneof!["[ -~]{0,16}", "[a-z0-9_-]{0,16}", ".{0,16}"];
    prop_oneof![
        any::<Vec<u8>>(),
        vec(octet(), 0..32),
        (0u8..16, vec(octet(), 0..32)).prop_map(|(length, rest)| [vec![length], rest].concat()),
        (text, vec(octet(), 0..4)).prop_map(|(text, rest)| {
            // At most 64 octets, so the length fits the short form.
            let length = u8::try_from(text.len()).unwrap();
            [vec![length], text.into_bytes(), rest].concat()
        }),
    ]
}

/// Base-128 input: a run of octets with the high bit set, often eight or
/// nine long, which the overflow check and the ten-group bound need, led by
/// `0x80`, `0x81` (the largest leading group that fits ten groups) or more.
fn base128_input() -> impl Strategy<Value = Vec<u8>> {
    let first = prop_oneof![Just(0x80u8), Just(0x81), 0x82u8..=0xff, any::<u8>()];
    let run = prop_oneof![vec(0x80u8..=0xff, 0..10), vec(0x80u8..=0xff, 8..=9)];
    (first, run, any::<Vec<u8>>()).prop_map(|(first, run, rest)| [vec![first], run, rest].concat())
}

/// Size constraints: none, exact and ranged ones around the lengths
/// `input` produces, and arbitrary ones.
fn size_constraint() -> impl Strategy<Value = Size> {
    let range = |(a, b): (usize, usize)| Size::range(a.min(b), a.max(b));
    prop_oneof![
        Just(Size::ANY),
        (0usize..40).prop_map(Size::exact),
        (0usize..40, 0usize..40).prop_map(range),
        any::<(usize, usize)>().prop_map(range),
    ]
}

/// The crate's alphabets, alphabets built as the protocol crates build
/// theirs (node IDs, seed path segments and scheme IDs in dassie-identity),
/// and alphabets built from arbitrary ranges and characters.
fn alphabet() -> impl Strategy<Value = Alphabet> {
    prop_oneof![
        Just(Alphabet::IA5),
        Just(Alphabet::VISIBLE),
        Just(Alphabet::EMPTY),
        Just(
            Alphabet::EMPTY
                .with_range(b'a', b'z')
                .with_range(b'A', b'Z')
                .with_range(b'0', b'9')
                .with_chars(b"-_")
        ),
        Just(Alphabet::EMPTY.with_range(0x21, 0x7e)),
        Just(
            Alphabet::EMPTY
                .with_range(b'a', b'z')
                .with_range(b'0', b'9')
                .with_chars(b"-")
        ),
        (0u8..0x80, 0u8..0x80, vec(0u8..0x80, 0..8)).prop_map(|(a, b, chars)| {
            Alphabet::EMPTY
                .with_range(a.min(b), a.max(b))
                .with_chars(&chars)
        }),
    ]
}

/// Counts for `read_bytes`, `read_fixed_ascii` and `read_quantity`: small
/// ones that the input can hold, and arbitrary ones that it cannot.
fn count() -> impl Strategy<Value = usize> {
    prop_oneof![0usize..40, any::<usize>()]
}

/// One `Reader` call.
#[derive(Clone, Debug)]
enum Read {
    U8,
    U16,
    U32,
    U64,
    Bytes(usize),
    ExpectBytes(Vec<u8>),
    Length,
    VarUint,
    VarUintSaturating,
    Quantity(usize),
    Octets(Size),
    Ascii(Size, Alphabet),
    FixedAscii(usize, Alphabet),
    Utf8(Size),
    Bool,
    ContextTag,
    Base128,
    ObjectIdentifier,
    Prefixed(Vec<Self>),
}

/// Any read, open types holding up to four reads and nesting two deep.
fn read() -> impl Strategy<Value = Read> {
    let leaf = prop_oneof![
        Just(Read::U8),
        Just(Read::U16),
        Just(Read::U32),
        Just(Read::U64),
        count().prop_map(Read::Bytes),
        vec(any::<u8>(), 0..3).prop_map(Read::ExpectBytes),
        Just(Read::Length),
        Just(Read::VarUint),
        Just(Read::VarUintSaturating),
        count().prop_map(Read::Quantity),
        size_constraint().prop_map(Read::Octets),
        (size_constraint(), alphabet()).prop_map(|(size, alphabet)| Read::Ascii(size, alphabet)),
        (count(), alphabet()).prop_map(|(length, alphabet)| Read::FixedAscii(length, alphabet)),
        size_constraint().prop_map(Read::Utf8),
        Just(Read::Bool),
        Just(Read::ContextTag),
        Just(Read::Base128),
        Just(Read::ObjectIdentifier),
    ];
    leaf.prop_recursive(2, 16, 4, |inner| vec(inner, 0..4).prop_map(Read::Prefixed))
}

/// Runs `read` and returns the canonical encoding of the value it read, or
/// `None` where there is nothing to compare: raw bytes, saturated integers
/// and open types, whose content `run_all` checks read by read.
fn run(reader: &mut Reader<'_>, read: &Read) -> Result<Option<Vec<u8>>, DecodeError> {
    let encoded = match read {
        Read::U8 => {
            let value = reader.read_u8()?;
            write_with(1, |writer| writer.write_u8(value))
        }
        Read::U16 => {
            let value = reader.read_u16()?;
            write_with(2, |writer| writer.write_u16(value))
        }
        Read::U32 => {
            let value = reader.read_u32()?;
            write_with(4, |writer| writer.write_u32(value))
        }
        Read::U64 => {
            let value = reader.read_u64()?;
            write_with(8, |writer| writer.write_u64(value))
        }
        Read::Bytes(count) => {
            reader.read_bytes(*count)?;
            return Ok(None);
        }
        Read::ExpectBytes(expected) => {
            reader.expect_bytes(expected)?;
            expected.clone()
        }
        Read::Length => {
            let length = reader.read_length()?;
            write_with(9, |writer| writer.write_length(length))
        }
        Read::VarUint => {
            let value = reader.read_var_uint()?;
            write_with(9, |writer| writer.write_var_uint(value))
        }
        Read::VarUintSaturating => {
            reader.read_var_uint_saturating()?;
            return Ok(None);
        }
        Read::Quantity(max) => {
            let count = reader.read_quantity(*max)?;
            assert!(count <= *max, "quantity {count} exceeds its limit {max}");
            write_with(9, |writer| writer.write_quantity(count))
        }
        Read::Octets(_) | Read::Ascii(..) | Read::FixedAscii(..) | Read::Utf8(_) => {
            return run_string(reader, read);
        }
        Read::Bool => {
            let value = reader.read_bool()?;
            write_with(1, |writer| writer.write_bool(value))
        }
        Read::ContextTag => {
            let number = reader.read_context_tag()?;
            write_with(size::tag(number), |writer| writer.write_context_tag(number))
        }
        Read::Base128 => {
            let value = reader.read_base128()?;
            write_with(size::base128(value), |writer| writer.write_base128(value))
        }
        Read::ObjectIdentifier => {
            let decoded = ObjectIdentifier::decode(reader)?;
            let mut arcs = [0; oid::MAX_ARCS];
            let count = decoded.arcs(&mut arcs);
            let arcs = arcs.get(..count).unwrap_or_default();
            write_with(oid::encoded_len(arcs), |writer| oid::write(writer, arcs))
        }
        Read::Prefixed(reads) => {
            reader.read_prefixed(|inner| run_all(inner, reads))?;
            return Ok(None);
        }
    };
    Ok(Some(encoded))
}

/// [`run`] for the string reads.
fn run_string(reader: &mut Reader<'_>, read: &Read) -> Result<Option<Vec<u8>>, DecodeError> {
    let encoded = match read {
        Read::Octets(size) => {
            let bytes = reader.read_octets(*size)?;
            let capacity = size::prefixed(bytes.len());
            write_with(capacity, |writer| writer.write_octets(bytes, *size))
        }
        Read::Ascii(size, alphabet) => {
            let text = reader.read_ascii(*size, *alphabet)?;
            let capacity = size::prefixed(text.len());
            write_with(capacity, |writer| {
                writer.write_ascii(text, *size, *alphabet);
            })
        }
        Read::FixedAscii(length, alphabet) => {
            let text = reader.read_fixed_ascii(*length, *alphabet)?;
            write_with(*length, |writer| {
                writer.write_fixed_ascii(text, *length, *alphabet);
            })
        }
        Read::Utf8(size) => {
            let text = reader.read_utf8(*size)?;
            let capacity = size::prefixed(text.len());
            write_with(capacity, |writer| writer.write_utf8(text, *size))
        }
        // `run` handles the other reads.
        _ => return Ok(None),
    };
    Ok(Some(encoded))
}

/// Runs `reads` one after another and returns the first error. It carries
/// on past errors: callers stop at the first, but a failed read leaves the
/// cursor inside the input, and nothing a caller does with it may panic.
///
/// Every read that succeeds must have consumed exactly the canonical
/// encoding of its value, and every error must point into the bytes its
/// read looked at.
fn run_all(reader: &mut Reader<'_>, reads: &[Read]) -> Result<(), DecodeError> {
    let mut first_error = None;
    for read in reads {
        let start = reader.position();
        let start_offset = reader.offset();
        match run(reader, read) {
            Ok(Some(encoded)) => assert_eq!(
                reader.consumed_since(start),
                encoded.as_slice(),
                "{read:?} did not consume a canonical encoding"
            ),
            Ok(None) => {}
            Err(error) => {
                assert!(
                    (start_offset..=reader.offset()).contains(&error.offset),
                    "{read:?} reported {error} outside the bytes it read"
                );
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

proptest! {
    #[test]
    fn var_uint_round_trips(value: u64) {
        let encoded = write_with(size::var_uint(value), |writer| writer.write_var_uint(value));
        prop_assert_eq!(encoded.len(), size::var_uint(value));
        let mut reader = Reader::new(&encoded);
        prop_assert_eq!(reader.read_var_uint(), Ok(value));
        prop_assert!(reader.is_empty());
    }

    #[test]
    fn length_round_trips(length: usize) {
        let encoded = write_with(9, |writer| writer.write_length(length));
        prop_assert_eq!(encoded.len(), size::length_determinant(length));
        prop_assert_eq!(Reader::new(&encoded).read_length(), Ok(length));
    }

    #[test]
    fn tag_round_trips(number: u64) {
        let encoded = write_with(size::tag(number), |writer| writer.write_context_tag(number));
        prop_assert_eq!(encoded.len(), size::tag(number));
        prop_assert_eq!(Reader::new(&encoded).read_context_tag(), Ok(number));
    }

    #[test]
    fn decoded_var_uints_are_canonical(input: Vec<u8>) {
        let mut reader = Reader::new(&input);
        if let Ok(value) = reader.read_var_uint() {
            let consumed = &input[..reader.position()];
            let encoded = write_with(9, |writer| writer.write_var_uint(value));
            prop_assert_eq!(consumed, encoded.as_slice());
        }
        // The saturating variant must not panic either.
        let _ = Reader::new(&input).read_var_uint_saturating();
    }

    #[test]
    fn decoded_lengths_are_canonical(input: Vec<u8>) {
        let mut reader = Reader::new(&input);
        if let Ok(length) = reader.read_length() {
            let consumed = &input[..reader.position()];
            let encoded = write_with(9, |writer| writer.write_length(length));
            prop_assert_eq!(consumed, encoded.as_slice());
        }
    }

    #[test]
    fn decoded_tags_are_canonical(input: Vec<u8>) {
        let mut reader = Reader::new(&input);
        if let Ok(number) = reader.read_context_tag() {
            let consumed = &input[..reader.position()];
            let encoded = write_with(11, |writer| writer.write_context_tag(number));
            prop_assert_eq!(consumed, encoded.as_slice());
        }
    }

    #[test]
    fn decoded_object_identifiers_are_canonical(input: Vec<u8>) {
        let mut reader = Reader::new(&input);
        if let Ok(decoded) = ObjectIdentifier::decode(&mut reader) {
            let consumed = &input[..reader.position()];
            let mut arcs = [0; oid::MAX_ARCS];
            let count = decoded.arcs(&mut arcs);
            let arcs = &arcs[..count];
            let encoded = write_with(oid::encoded_len(arcs), |writer| oid::write(writer, arcs));
            prop_assert_eq!(consumed, encoded.as_slice());
        }
    }

    #[test]
    fn object_identifiers_round_trip(
        first in 0u64..=2,
        second in 0u64..40,
        rest in proptest::collection::vec(any::<u64>(), 0..20),
    ) {
        let mut arcs = vec![first, second];
        arcs.extend(rest);
        let encoded = write_with(oid::encoded_len(&arcs), |writer| oid::write(writer, &arcs));
        let decoded = ObjectIdentifier::decode(&mut Reader::new(&encoded)).unwrap();
        let mut output = [0; oid::MAX_ARCS];
        let count = decoded.arcs(&mut output);
        prop_assert_eq!(&output[..count], arcs.as_slice());
    }

    #[test]
    fn prefixed_reads_never_panic(input: Vec<u8>, inner_reads in 0usize..4) {
        let mut reader = Reader::new(&input);
        let _ = reader.read_prefixed(|inner| -> Result<(), dassie_oer::DecodeError> {
            for _ in 0..inner_reads {
                inner.read_var_uint()?;
            }
            Ok(())
        });
    }

    #[test]
    fn decoded_booleans_are_canonical(input in input()) {
        let _ = run_all(&mut Reader::new(&input), &[Read::Bool]);
    }

    #[test]
    fn decoded_quantities_are_canonical(input in input(), max in count()) {
        let _ = run_all(&mut Reader::new(&input), &[Read::Quantity(max)]);
    }

    #[test]
    fn decoded_base128_is_canonical(input in base128_input()) {
        let _ = run_all(&mut Reader::new(&input), &[Read::Base128]);
    }

    #[test]
    fn fixed_strings_never_panic_and_re_encode(
        input in input(),
        length in count(),
        alphabet in alphabet(),
    ) {
        let _ = run_all(&mut Reader::new(&input), &[Read::FixedAscii(length, alphabet)]);
    }

    #[test]
    fn strings_never_panic_and_re_encode(
        input in input(),
        size in size_constraint(),
        alphabet in alphabet(),
    ) {
        for read in [Read::Octets(size), Read::Ascii(size, alphabet), Read::Utf8(size)] {
            let _ = run_all(&mut Reader::new(&input), &[read]);
        }
    }

    #[test]
    fn read_sequences_never_panic(input in input(), reads in vec(read(), 0..8)) {
        let mut reader = Reader::new(&input);
        let _ = run_all(&mut reader, &reads);
        prop_assert_eq!(reader.finish().is_ok(), reader.position() == input.len());
    }
}

//! Properties: values round-trip, decoding never panics on arbitrary input,
//! and whatever decodes is canonical, so re-encoding it reproduces the
//! consumed bytes exactly.

use dassie_oer::{Alphabet, ObjectIdentifier, Reader, Size, Writer, oid, size};
use proptest::prelude::*;

fn write_with(capacity: usize, write: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut buffer = vec![0; capacity];
    let mut writer = Writer::new(&mut buffer);
    write(&mut writer);
    let length = writer.position();
    buffer.truncate(length);
    buffer
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
    fn strings_never_panic_and_re_encode(input: Vec<u8>) {
        let mut reader = Reader::new(&input);
        if let Ok(text) = reader.read_ascii(Size::ANY, Alphabet::VISIBLE) {
            let consumed = &input[..reader.position()];
            let encoded = write_with(consumed.len(), |writer| {
                writer.write_ascii(text, Size::ANY, Alphabet::VISIBLE);
            });
            prop_assert_eq!(consumed, encoded.as_slice());
        }
        let mut reader = Reader::new(&input);
        if let Ok(text) = reader.read_utf8(Size::ANY) {
            let consumed = &input[..reader.position()];
            let encoded = write_with(consumed.len(), |writer| writer.write_utf8(text, Size::ANY));
            prop_assert_eq!(consumed, encoded.as_slice());
        }
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
}

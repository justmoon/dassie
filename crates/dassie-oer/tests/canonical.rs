//! Cases where the original lib-oer was wrong
//! (spec/areas/encoding.md#canonical-only, OER-q1 to OER-q3,
//! ILP-12). Each test fails against the original's behaviour.

mod support;

use dassie_oer::{Alphabet, DecodeErrorKind, Reader, Size, Writer, oid};
use support::hex;

fn kind<T>(result: Result<T, dassie_oer::DecodeError>) -> Option<DecodeErrorKind> {
    result.err().map(|error| error.kind)
}

#[test]
fn oer_q1_fixed_octet_string_checks_bounds() {
    // The original returned a short slice and claimed the full length.
    let input = [1, 2, 3];
    let mut reader = Reader::new(&input);
    assert_eq!(
        kind(reader.read_array::<32>()),
        Some(DecodeErrorKind::UnexpectedEnd)
    );
}

#[test]
fn oer_q2_containing_is_bounded_by_its_content() {
    // Content of length 2 followed by more bytes: the inner decoder must not
    // see past its two octets...
    let input = hex("020102ff");
    let mut reader = Reader::new(&input);
    let result = reader.read_prefixed(|inner| inner.read_u32());
    assert_eq!(kind(result), Some(DecodeErrorKind::UnexpectedEnd));

    // ...and must consume all of them.
    let mut reader = Reader::new(&input);
    let result = reader.read_prefixed(|inner| inner.read_u8());
    assert_eq!(kind(result), Some(DecodeErrorKind::TrailingBytes));
}

#[test]
fn oer_q3_object_identifier_written_at_an_offset() {
    // The original wrote the content at an absolute offset, which only
    // worked at the start of the buffer.
    let arcs = [1, 3, 6, 1, 4, 1, 54392, 5, 1399];
    let mut buffer = vec![0; 3 + oid::encoded_len(&arcs)];
    let mut writer = Writer::new(&mut buffer);
    writer.write_bytes(&[0xaa, 0xbb, 0xcc]);
    oid::write(&mut writer, &arcs);
    assert_eq!(buffer, hex("aabbcc0b2b0601040183a878058a77"));
}

#[test]
fn length_determinants_must_be_minimal() {
    // The original's canonical check was off by one and accepted `82 00 ff`.
    for input in ["8200ff", "817f", "8100", "80", "820080"] {
        let bytes = hex(input);
        assert_eq!(
            kind(Reader::new(&bytes).read_length()),
            Some(DecodeErrorKind::NonCanonicalLength),
            "{input}"
        );
    }
    assert_eq!(Reader::new(&hex("8180")).read_length(), Ok(0x80));
    assert_eq!(Reader::new(&hex("820100")).read_length(), Ok(0x100));
}

#[test]
fn length_determinants_do_not_overflow() {
    // The original accumulated five- and six-octet lengths in 32 bits.
    let bytes = hex("89010000000000000000");
    assert_eq!(
        kind(Reader::new(&bytes).read_length()),
        Some(DecodeErrorKind::LengthOverflow)
    );
    // A huge but representable length fails on the missing content, without
    // allocating anything.
    let bytes = hex("88ffffffffffffffff");
    let mut reader = Reader::new(&bytes);
    assert_eq!(
        kind(reader.read_octets(Size::ANY)),
        Some(DecodeErrorKind::UnexpectedEnd)
    );
}

#[test]
fn variable_integers_must_be_minimal() {
    for input in ["020001", "00", "03000001"] {
        let bytes = hex(input);
        assert_eq!(
            kind(Reader::new(&bytes).read_var_uint()),
            Some(DecodeErrorKind::NonCanonicalInteger),
            "{input}"
        );
    }
    assert_eq!(Reader::new(&hex("0100")).read_var_uint(), Ok(0));
    assert_eq!(Reader::new(&hex("0180")).read_var_uint(), Ok(0x80));
}

#[test]
fn visible_string_allows_tilde() {
    // NET-30: the original's VisibleString alphabet left out `~`.
    let bytes = hex("017e");
    assert_eq!(
        Reader::new(&bytes).read_ascii(Size::ANY, Alphabet::VISIBLE),
        Ok("~")
    );
    let bytes = hex("017f");
    assert_eq!(
        kind(Reader::new(&bytes).read_ascii(Size::ANY, Alphabet::VISIBLE)),
        Some(DecodeErrorKind::InvalidCharacter)
    );
}

#[test]
#[should_panic(expected = "string violates its alphabet")]
fn ia5_serialization_checks_the_alphabet() {
    // The original never applied the character filter when serializing, so
    // an IA5String could carry octets of 0x80 and above.
    let mut buffer = [0; 8];
    Writer::new(&mut buffer).write_ascii("é", Size::ANY, Alphabet::IA5);
}

#[test]
fn booleans_are_canonical() {
    assert_eq!(Reader::new(&[0xff]).read_bool(), Ok(true));
    assert_eq!(Reader::new(&[0x00]).read_bool(), Ok(false));
    assert_eq!(
        kind(Reader::new(&[0x01]).read_bool()),
        Some(DecodeErrorKind::InvalidBoolean)
    );
}

#[test]
fn tags_are_canonical() {
    assert_eq!(Reader::new(&hex("80")).read_context_tag(), Ok(0));
    assert_eq!(Reader::new(&hex("be")).read_context_tag(), Ok(62));
    assert_eq!(Reader::new(&hex("bf3f")).read_context_tag(), Ok(63));
    assert_eq!(Reader::new(&hex("bf8100")).read_context_tag(), Ok(128));
    // Long form for a number that fits the short form, a leading 0x80
    // group, and the wrong tag class are all rejected.
    for input in ["bf3e", "bf803f", "41", "c0"] {
        let bytes = hex(input);
        assert_eq!(
            kind(Reader::new(&bytes).read_context_tag()),
            Some(DecodeErrorKind::UnexpectedTag),
            "{input}"
        );
    }
}

#[test]
fn quantities_respect_the_callers_limit() {
    let bytes = hex("0105");
    assert_eq!(Reader::new(&bytes).read_quantity(5), Ok(5));
    assert_eq!(
        kind(Reader::new(&bytes).read_quantity(4)),
        Some(DecodeErrorKind::LimitExceeded)
    );
}

#[test]
fn errors_in_nested_values_report_absolute_offsets() {
    let input = hex("aa03010280");
    let mut reader = Reader::new(&input);
    reader.read_u8().unwrap();
    let error = reader
        .read_prefixed(|inner| {
            inner.read_u16()?;
            inner.read_bool()
        })
        .unwrap_err();
    assert_eq!(error.kind, DecodeErrorKind::InvalidBoolean);
    assert_eq!(error.offset, 4);
}

#[test]
fn object_identifiers_reject_malformed_content() {
    // Empty content, a leading 0x80 group, and a truncated subidentifier.
    for input in ["00", "02802b", "022b8f"] {
        let bytes = hex(input);
        let result = dassie_oer::ObjectIdentifier::decode(&mut Reader::new(&bytes));
        assert!(result.is_err(), "{input}");
    }
}

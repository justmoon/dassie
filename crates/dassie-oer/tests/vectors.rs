//! Golden vectors produced by the original lib-oer (testdata/original).

mod support;

use dassie_oer::{DecodeErrorKind, Reader, Size, Writer, oid};
use support::{hex, str_field, to_hex, vectors};

fn write_with(capacity: usize, write: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut buffer = vec![0; capacity];
    let mut writer = Writer::new(&mut buffer);
    write(&mut writer);
    let length = writer.position();
    buffer.truncate(length);
    buffer
}

#[test]
fn variable_unsigned_integers() {
    let vectors = vectors("vectors.json");
    let samples = vectors["variableUnsigned"].as_array().unwrap();
    let mut in_range = 0;
    for sample in samples {
        let encoded = hex(str_field(sample, "encoded"));
        let decimal = str_field(sample, "value");
        let mut reader = Reader::new(&encoded);
        if let Ok(value) = decimal.parse::<u64>() {
            in_range += 1;
            assert_eq!(reader.read_var_uint(), Ok(value), "decoding {decimal}");
            assert!(reader.is_empty());
            let written = write_with(9, |writer| writer.write_var_uint(value));
            assert_eq!(to_hex(&written), to_hex(&encoded), "encoding {decimal}");
        } else {
            // Larger than a u64: an overflow for the strict reader, and the
            // maximum for the saturating one that STREAM uses.
            assert_eq!(
                reader.read_var_uint().map_err(|error| error.kind),
                Err(DecodeErrorKind::IntegerOverflow),
                "decoding {decimal}"
            );
            let mut reader = Reader::new(&encoded);
            assert_eq!(reader.read_var_uint_saturating(), Ok(u64::MAX));
            assert!(reader.is_empty());
        }
    }
    assert!(in_range > 30, "the sample table should cover the u64 range");
    assert!(
        samples.len() > in_range,
        "the sample table should exceed u64"
    );
}

fn parse_arcs(text: &str) -> Vec<u64> {
    text.split('.').map(|arc| arc.parse().unwrap()).collect()
}

#[test]
fn object_identifiers() {
    let vectors = vectors("vectors.json");
    for sample in vectors["objectIdentifiers"].as_array().unwrap() {
        let text = str_field(sample, "value");
        let encoded = hex(str_field(sample, "encoded"));
        let arcs = parse_arcs(text);

        assert_eq!(oid::encoded_len(&arcs), encoded.len(), "length of {text}");
        let written = write_with(encoded.len(), |writer| oid::write(writer, &arcs));
        assert_eq!(to_hex(&written), to_hex(&encoded), "encoding {text}");

        let mut reader = Reader::new(&encoded);
        let decoded = dassie_oer::ObjectIdentifier::decode(&mut reader).unwrap();
        assert!(reader.is_empty());
        let mut buffer = [0; oid::MAX_ARCS];
        let count = decoded.arcs(&mut buffer);
        assert_eq!(&buffer[..count], arcs.as_slice(), "decoding {text}");
    }
}

#[test]
fn dassie_object_identifiers_are_constant_encodings() {
    let vectors = vectors("vectors.json");
    let peer = &vectors["peer"]["peerNodeInfoOid"];
    const PEER_NODE_INFO: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 1, 2]);
    assert_eq!(to_hex(&PEER_NODE_INFO), str_field(peer, "encoded"));
    assert_eq!(str_field(peer, "value"), "1.3.6.1.4.1.54392.5.1399.1.2");
}

#[test]
fn utf8_strings() {
    let vectors = vectors("vectors.json");
    for sample in vectors["utf8"].as_array().unwrap() {
        let text = str_field(sample, "value");
        let encoded = hex(str_field(sample, "encoded"));
        let mut reader = Reader::new(&encoded);
        assert_eq!(reader.read_utf8(Size::ANY), Ok(text));
        assert!(reader.is_empty());
        let written = write_with(encoded.len(), |writer| writer.write_utf8(text, Size::ANY));
        assert_eq!(written, encoded);
    }
}

#[test]
fn length_determinants() {
    let vectors = vectors("vectors.json");
    for sample in vectors["octetLengths"].as_array().unwrap() {
        let length = usize::try_from(sample["length"].as_u64().unwrap()).unwrap();
        let prefix = hex(str_field(sample, "prefix"));
        let written = write_with(9, |writer| writer.write_length(length));
        assert_eq!(written, prefix, "encoding {length}");
        assert_eq!(dassie_oer::size::length_determinant(length), prefix.len());
        assert_eq!(Reader::new(&prefix).read_length(), Ok(length));
    }
}

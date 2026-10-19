# Encoding (OER)

Every Dassie wire protocol (ILPv4, ILDCP, BTP, STREAM and the peer protocol) is encoded in the Octet Encoding Rules, [ITU-T X.696](https://www.itu.int/rec/T-REC-X.696). `dassie-oer` implements the subset these protocols use, and each protocol crate writes its message codecs by hand on its `Reader` and `Writer`.

**Paths:** `crates/dassie-oer/`
**Read with:** [ILP](ilp.md), [STREAM](stream.md), [Peer protocol](peer-protocol.md), [Identity](identity.md)

## Scope

`dassie-oer` is not a general ASN.1 library and has no schema compiler. It supports only what Dassie's protocols use. OPTIONAL and DEFAULT fields, extensible SEQUENCEs, signed integers, ENUMERATED, BIT STRING and REAL are absent, so no SEQUENCE has a preamble: a SEQUENCE is its fields in order. A protocol that needs one of these types adds it here first, with its canonical rules.

## Canonical only

The codec always writes canonical OER and reads nothing else: lengths, unconstrained integers, booleans, tags and OIDs in any but their one minimal form are refused, and everything decoded re-encodes to the bytes it came from (a property test). This matters beyond tidiness:

- **Signatures are checked over the bytes received** (`Reader::consumed_since`), as link states are ([Peer protocol](peer-protocol.md)). With one encoding per value, nobody can rewrite a message in transit into different bytes with the same meaning, so a hash or signature over the bytes identifies the value.
- **Every byte string decodes one way,** so two implementations cannot disagree about what a hostile message means.

The original's `lib-oer` accepted non-canonical forms and had encoding bugs of its own; each case has a test in `tests/canonical.rs` that fails against its behaviour.

Values are validated when the typed wrappers a protocol encodes are built, so writing never fails at run time: a size or alphabet violation while writing is a bug, and fails an assertion.

## Bounded decoding

Decoding never allocates and never panics, whatever the input ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)):

- strings and octets are borrowed from the input;
- every SEQUENCE OF is read with a limit the protocol passes (`read_quantity`), checked before any element is read, so a count field cannot make a reader loop or reserve beyond what the protocol allows;
- an open type or CONTAINING gives its inner decoder a reader that ends where its content ends, and the inner value must fill it exactly;
- a failure is a `DecodeError` whose offset is the position in the outermost input, even inside nested open types, so a protocol error points at the right byte.

**Saturating integers.** RFC 29 asks STREAM receivers to read amounts and limits larger than they can hold as their maximum. `read_var_uint_saturating` does so, after the same canonical checks; every other integer above 64 bits is an error.

## What pins the encoding

Golden vectors generated from the original (`tests/data/vectors.json`, by `testdata/original/`), including the original peer protocol's captured signed bytes, pin the wire bytes; the canonical cases above are the deliberate exceptions.

## Decisions

- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): deterministic, bounded, no allocation per message; every count has a caller's limit.

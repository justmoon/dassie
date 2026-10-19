# Encoding (OER)

Every Dassie protocol on the wire, ILPv4, ILDCP, BTP, STREAM and the peer protocol, is encoded in the Octet Encoding Rules of ITU-T X.696. `dassie-oer` implements the subset these protocols use, canonically: it always writes canonical OER and reads nothing else.

**Paths:** `crates/dassie-oer/`
**Read with:** [ILP](ilp.md), [STREAM](stream.md), [Peer protocol](peer-protocol.md), [Identity](identity.md)

## Scope

`dassie-oer` is not a general ASN.1 library. It provides a `Reader` and a `Writer` for the types below, and each protocol crate writes its message codecs by hand with them (traits `Encode` and `Decode`).

- **Supported:** length determinants; fixed-size unsigned integers of 8, 16, 32 and 64 bits; unconstrained unsigned integers up to 64 bits; fixed-size and variable-size OCTET STRINGs; IA5String, VisibleString and restrictions of them to an alphabet, fixed-size or variable; UTF8String; BOOLEAN; SEQUENCE OF quantities; context-specific CHOICE tags; open types and OCTET STRING CONTAINING; OBJECT IDENTIFIER.
- **Not supported:** OPTIONAL and DEFAULT fields, extensible SEQUENCEs (so no SEQUENCE has a preamble), signed integers, ENUMERATED, BIT STRING, REAL, and tags of other classes. No Dassie protocol uses them.
- **A SEQUENCE** is the concatenation of its fields in declaration order, with no preamble, since none of Dassie's has optional fields or extensions.
- **The codec does not allocate.** Decoding borrows strings and bytes from the input; encoding writes into a buffer the caller sizes with `encoded_len` first. A `to_vec` helper (feature `alloc`, on by default) serves API edges such as WASM bindings and tests.
- The crate is `no_std` and deterministic ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)).

## Length determinant

X.696 §8.6:

```
length ≤ 127:  one octet, the length
length ≥ 128:  0x80 | n, then the length in n big-endian octets, n minimal
```

- **Writing** always uses the short form when it fits, and the fewest octets otherwise.
- **Reading** refuses, as `NonCanonicalLength`:
  - the long form for a length that fits the short form (below 128);
  - a long form whose first length octet is zero, or with no length octets (`0x80`).

  A long form with more octets than a `usize` holds is `LengthOverflow`. The length announced must be present in the input (`UnexpectedEnd`).

## Integers

| Type | Encoding |
|---|---|
| `u8`, `u16`, `u32`, `u64` | 1, 2, 4 or 8 octets, big-endian, no length |
| unconstrained unsigned | a length determinant, then the value in minimal big-endian octets: `0` is `01 00`, `255` is `01 ff`, `256` is `02 01 00` |

- **Reading an unconstrained integer** refuses, as `NonCanonicalInteger`, zero content octets and a redundant leading `00` octet. A value above `2^64 − 1` is `IntegerOverflow`.
- **Saturating read.** STREAM amounts and limits may be sent as arbitrarily large integers, and RFC 29 asks receivers to treat a larger value as the maximum. `read_var_uint_saturating` reads such a value as `2^64 − 1`, after the same canonical checks ([STREAM](stream.md#packets)).

## Octet strings

- **Fixed size** (`SIZE(n)`): `n` raw octets with no length. Reading checks that `n` octets are present.
- **Variable size:** a length determinant, then the octets. The caller gives an inclusive size range (`Size`); a length outside it is `SizeOutOfRange`, checked before the content is read.

## Strings

- **7-bit strings** (IA5String, VisibleString and restrictions of them) are octet strings whose every octet is in an alphabet, a set of 7-bit code points. For them characters are octets, so sizes count both.
  - `IA5` is all of `0x00`–`0x7F`.
  - `VISIBLE` is `0x20`–`0x7E`, `~` included.
  - Protocols build restricted alphabets, for example node IDs (`[A-Za-z0-9_-]`) and settlement scheme IDs (`[a-z0-9-]`).
  - A character outside the alphabet is `InvalidCharacter` on reading. Writing one is a bug and fails an assertion.
- **Fixed-size 7-bit strings** (such as the 17-character Interledger timestamp and the 3-character ILP error code) have no length determinant.
- **UTF8String** is an octet string holding valid UTF-8 (`InvalidUtf8` otherwise). Its size is counted in octets, the only way any Dassie protocol constrains one (the ILP reject message).

## BOOLEAN

One octet: `0xFF` for true, `0x00` for false. Any other octet is `InvalidBoolean`.

## SEQUENCE OF

The element count, the *quantity*, is an unconstrained unsigned integer (`01 00` for none, `01 02` for two), followed by the elements. Reading a quantity takes the caller's limit on the number of elements, and a count above it is `LimitExceeded`, before any element is read. Every protocol sets one (for example at most 16 BTP protocol data entries, at most 512 STREAM frames).

## CHOICE tags

Dassie's CHOICEs use context-specific tags (X.696 §8.7):

```
tag < 63:  one octet, 0x80 | tag                 -- [0] is 0x80, [10] is 0x8a
tag ≥ 63:  0xbf, then the tag as base-128 groups
```

Reading refuses, as `UnexpectedTag`: a class other than context-specific, and the long form for a tag below 63. Which tags a CHOICE allows is the protocol's to check. An alternative follows its tag with no length of its own.

## Open types and CONTAINING

An open type, or an OCTET STRING CONTAINING another type, is a length determinant followed by the inner value's encoding.

- **Reading** gives the inner decoder a reader that ends where the content ends, so it cannot read past it, and requires it to consume the content exactly (`TrailingBytes` otherwise).
- **Writing** asserts that the inner encoder wrote exactly the length it declared.

## Object identifiers

A length determinant, then the BER content octets (X.690 §8.19): base-128 subidentifiers, the first combining the first two arcs as `40·a + b`.

- **Reading** validates the content: at least one subidentifier, at most 32 arcs, no subidentifier starting with a padding octet `0x80`, none above `2^64 − 1`, and none left unfinished. Anything else is `InvalidObjectIdentifier`.
- **Constants** are encoded at compile time with `oid::encode`, and matched byte for byte when read (`expect_bytes`, `UnexpectedValue` on a mismatch). Dassie's OIDs are all constants ([Identity](identity.md#object-identifiers)).
- Writing an OID at any offset gives the same bytes.

## Decoding a whole value

`decode_exact` decodes a value that must occupy all of its input: bytes left over are `TrailingBytes`. Protocols use it for every top-level message. Inside a value, `consumed_since` returns the exact bytes a part took, so a signature can be checked over the bytes received (link states, [Peer protocol](peer-protocol.md)).

## Errors

A failure is a value, `DecodeError { offset, kind }`, never a panic, whatever the input. The offset is the position in the outermost input, also inside open types and CONTAINING, so an error deep in a message points at the right byte. Protocol crates wrap it in their own error type, so they can report failures OER knows nothing about (an invalid ILP address, say) the same way.

| Kind | Meaning |
|---|---|
| `UnexpectedEnd` | the input ended before the value did |
| `TrailingBytes` | bytes left after a value that must fill its container |
| `NonCanonicalLength` | a length in the long form where the short fits, or with a leading zero octet |
| `LengthOverflow` | a length that does not fit a `usize` |
| `NonCanonicalInteger` | an unconstrained integer with no octets or a redundant leading zero |
| `IntegerOverflow` | an integer too large for its type |
| `SizeOutOfRange` | a sized value shorter or longer than allowed |
| `InvalidCharacter` | a character outside the string's alphabet |
| `InvalidUtf8` | a UTF8String that is not UTF-8 |
| `InvalidBoolean` | a boolean octet other than `00` or `ff` |
| `InvalidObjectIdentifier` | a malformed or non-canonical OID |
| `UnexpectedTag` | a tag that is not canonical or not allowed |
| `UnexpectedValue` | a value that differs from the constant required |
| `LimitExceeded` | a count above the caller's limit |

Writing never fails at run time. Values are validated when the typed wrappers protocols encode are built, so a size, alphabet or length violation while writing is a bug, and fails an assertion.

## What the original got wrong

The original's `lib-oer` accepted non-canonical forms and had several encoding bugs. `dassie-oer` follows X.696 instead; each of these has a test named after it:

- a fixed-size octet string is bounds-checked (OER-q1);
- CONTAINING is bounded by its content and must fill it (OER-q2);
- an OID writes correctly at any offset (OER-q3);
- length determinants must be minimal, with no off-by-one, and do not overflow at 5 or 6 octets;
- unconstrained integers must be minimal;
- VisibleString accepts `~`;
- writing a 7-bit string checks its alphabet;
- booleans and tags are canonical;
- quantities respect the caller's limit.

The original's signed-range, preamble, extension, ENUMERATED, BIT STRING and NumericString bugs have no counterpart, since none of these types is implemented.

## Tests

- **Golden vectors** (`tests/data/vectors.json`, from the original by `testdata/original/`): unconstrained integers, OIDs, UTF-8 strings and length determinants from `lib-oer`'s sample tables, and the peer protocol structures of the original's [3 §5.5](../original/03-peer-network.md) (node info, signed node info with its captured bytes, node lists, message envelopes).
- **Property tests:** integers, lengths, tags and OIDs round-trip; everything decoded re-encodes to the same bytes (canonical-only parse); no sequence of reads panics on arbitrary input.

## Decisions

- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): deterministic, bounded, no allocation per message; every count has a caller's limit.

## Original

- [3 §4](../original/03-peer-network.md): the OER subset the peer protocol used, and OER-q1 to OER-q3.
- [4 §2](../original/04-interledger.md): the original's `lib-oer` in full, with its bugs (ILP-12).

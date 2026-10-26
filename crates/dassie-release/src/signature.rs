//! The minisign signature of a release's `SHA256SUMS`.
//!
//! A minisign public key is `Ed`, an 8-byte key ID and the Ed25519 key, in
//! base64. A signature file has four lines:
//!
//! ```text
//! untrusted comment: <anything>
//! base64(ED ‖ key ID ‖ Ed25519 signature of BLAKE2b-512(file))
//! trusted comment: dassie <version>
//! base64(Ed25519 signature of (the signature above ‖ the trusted comment))
//! ```
//!
//! The release workflow signs with `minisign -S -t "dassie <version>"`. The
//! trusted comment binds the sums to their version: the signed sums of an
//! old release, served under a new version's path, are refused. Legacy
//! signatures (`Ed`, of the file itself rather than its hash) are refused
//! too, since nothing here makes them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use blake2::{Blake2b512, Digest as _};
use ed25519_dalek::{Signature, VerifyingKey};

use crate::{Sums, SumsError, Version};

/// The key's algorithm: Ed25519.
const KEY_ALGORITHM: [u8; 2] = *b"Ed";
/// A signature of the file's BLAKE2b-512, which minisign makes by default.
const PREHASHED: [u8; 2] = *b"ED";
/// The bytes of a key ID.
const KEY_ID: usize = 8;
/// The largest signature file read.
const MAX_SIGNATURE: usize = 1024;
/// What each comment line starts with.
const UNTRUSTED: &str = "untrusted comment: ";
const TRUSTED: &str = "trusted comment: ";

/// A minisign public key: the release key, compiled into the binary and
/// printed in the install script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    id: [u8; KEY_ID],
    key: VerifyingKey,
}

/// Why a key or a signature was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// Not what minisign writes.
    Encoding,
    /// Not Ed25519, or a legacy signature of the file itself.
    Algorithm,
    /// Made with another key.
    KeyId,
    /// The signature does not verify.
    Invalid,
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Encoding => "it is not a minisign key or signature",
            Self::Algorithm => "it uses an algorithm releases are not signed with",
            Self::KeyId => "it was made with another key than the release key",
            Self::Invalid => "the signature does not match",
        })
    }
}

/// Why a release's sums were refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The signature is not the release key's, or not of these sums.
    Signature(SignatureError),
    /// The signature is good, but it signs the sums of another version.
    Version {
        /// The trusted comment the signature carries.
        signed: String,
    },
    /// The sums are signed, but malformed.
    Sums(SumsError),
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signature(error) => write!(f, "SHA256SUMS.minisig was refused: {error}"),
            Self::Version { signed } => {
                write!(f, "SHA256SUMS is signed for another version ({signed:?})")
            }
            Self::Sums(error) => error.fmt(f),
        }
    }
}

impl PublicKey {
    /// Reads a public key as `minisign -G` writes it (`minisign.pub`), or
    /// just the base64 line of it.
    pub fn parse(text: &str) -> Result<Self, SignatureError> {
        let mut lines = text.lines();
        let first = lines.next().ok_or(SignatureError::Encoding)?;
        let encoded = match first.strip_prefix(UNTRUSTED) {
            Some(_) => lines.next().ok_or(SignatureError::Encoding)?,
            None => first,
        };
        if lines.any(|line| !line.is_empty()) {
            return Err(SignatureError::Encoding);
        }
        let bytes = decode::<{ 2 + KEY_ID + 32 }>(encoded)?;
        let (algorithm, rest) = bytes
            .split_first_chunk::<2>()
            .ok_or(SignatureError::Encoding)?;
        let (id, key) = rest
            .split_first_chunk::<KEY_ID>()
            .ok_or(SignatureError::Encoding)?;
        let key: &[u8; 32] = key.try_into().map_err(|_| SignatureError::Encoding)?;
        if *algorithm != KEY_ALGORITHM {
            return Err(SignatureError::Algorithm);
        }
        let key = VerifyingKey::from_bytes(key).map_err(|_| SignatureError::Encoding)?;
        if key.is_weak() {
            return Err(SignatureError::Encoding);
        }
        Ok(Self { id: *id, key })
    }

    /// The key's ID as minisign prints it: 16 uppercase hex digits.
    pub fn id(&self) -> String {
        self.id
            .iter()
            .rev()
            .map(|byte| format!("{byte:02X}"))
            .collect()
    }
}

/// The trusted comment a release's signature carries.
pub fn trusted_comment(version: &Version) -> String {
    format!("dassie {version}")
}

/// Checks that `signature` is the release key's signature of `sums` for
/// this version, then reads the sums.
pub fn verify_sums(
    key: &PublicKey,
    sums: &[u8],
    signature: &str,
    version: &Version,
) -> Result<Sums, VerifyError> {
    if sums.len() > crate::sums::MAX_SIZE {
        return Err(VerifyError::Sums(SumsError::TooLarge));
    }
    let signed = verify(key, sums, signature).map_err(VerifyError::Signature)?;
    if signed != trusted_comment(version) {
        return Err(VerifyError::Version { signed });
    }
    Sums::parse(sums).map_err(VerifyError::Sums)
}

/// Verifies both signatures, and returns the trusted comment.
fn verify(key: &PublicKey, file: &[u8], signature: &str) -> Result<String, SignatureError> {
    if signature.len() > MAX_SIGNATURE {
        return Err(SignatureError::Encoding);
    }
    let lines: Vec<&str> = signature.lines().collect();
    let [untrusted, encoded, trusted, global] = lines.as_slice() else {
        return Err(SignatureError::Encoding);
    };
    let comment = trusted
        .strip_prefix(TRUSTED)
        .ok_or(SignatureError::Encoding)?;
    if !untrusted.starts_with(UNTRUSTED) {
        return Err(SignatureError::Encoding);
    }
    let bytes = decode::<{ 2 + KEY_ID + 64 }>(encoded)?;
    let (algorithm, rest) = bytes
        .split_first_chunk::<2>()
        .ok_or(SignatureError::Encoding)?;
    let (id, signed) = rest
        .split_first_chunk::<KEY_ID>()
        .ok_or(SignatureError::Encoding)?;
    let signed: &[u8; 64] = signed.try_into().map_err(|_| SignatureError::Encoding)?;
    if *algorithm != PREHASHED {
        return Err(SignatureError::Algorithm);
    }
    if *id != key.id {
        return Err(SignatureError::KeyId);
    }
    let hash = Blake2b512::digest(file);
    key.key
        .verify_strict(hash.as_slice(), &Signature::from_bytes(signed))
        .map_err(|_| SignatureError::Invalid)?;
    let global = decode::<64>(global)?;
    let mut message = Vec::with_capacity(signed.len().saturating_add(comment.len()));
    message.extend_from_slice(signed);
    message.extend_from_slice(comment.as_bytes());
    key.key
        .verify_strict(&message, &Signature::from_bytes(&global))
        .map_err(|_| SignatureError::Invalid)?;
    Ok(comment.into())
}

/// Exactly `N` bytes, in standard base64.
fn decode<const N: usize>(encoded: &str) -> Result<[u8; N], SignatureError> {
    let bytes = STANDARD
        .decode(encoded.trim_end_matches('\r'))
        .map_err(|_| SignatureError::Encoding)?;
    bytes.try_into().map_err(|_| SignatureError::Encoding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use proptest::prelude::*;

    const PUBLIC_KEY: &str = include_str!("../../../testdata/release/test.pub");
    const SUMS: &str = include_str!("../../../testdata/release/SHA256SUMS");
    const SIGNATURE: &str = include_str!("../../../testdata/release/SHA256SUMS.minisig");
    const LEGACY: &str = include_str!("../../../testdata/release/SHA256SUMS.legacy.minisig");

    fn key() -> PublicKey {
        PublicKey::parse(PUBLIC_KEY).unwrap_or_else(|e| panic!("the test key: {e}"))
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn verifies_what_minisign_signs() {
        let sums = verify_sums(&key(), SUMS.as_bytes(), SIGNATURE, &version("0.1.0"));
        assert_eq!(sums.map(|sums| sums.names().count()), Ok(4));
        assert_eq!(key().id(), "91CCFD418EEBE0EA");
    }

    #[test]
    fn reads_the_key_alone_too() {
        let Some(line) = PUBLIC_KEY.lines().nth(1) else {
            panic!("minisign.pub has two lines");
        };
        assert_eq!(PublicKey::parse(line), Ok(key()));
        // "ED" for "Ed": the algorithm of a signature, not of a key.
        assert_eq!(
            PublicKey::parse(&PUBLIC_KEY.replace("RWT", "RUT")),
            Err(SignatureError::Algorithm)
        );
        assert_eq!(
            PublicKey::parse(&PUBLIC_KEY.replace("RWT", "RW")),
            Err(SignatureError::Encoding)
        );
        assert_eq!(PublicKey::parse(""), Err(SignatureError::Encoding));
        assert_eq!(
            PublicKey::parse(&[PUBLIC_KEY, "more\n"].concat()),
            Err(SignatureError::Encoding)
        );
    }

    #[test]
    fn refuses_tampered_sums() {
        // Too large to be sums: refused before it is hashed.
        let huge = "0".repeat(crate::sums::MAX_SIZE + 1);
        assert_eq!(
            verify_sums(&key(), huge.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Sums(SumsError::TooLarge))
        );
        let tampered = SUMS.replacen('4', "5", 1);
        assert_eq!(
            verify_sums(&key(), tampered.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
        let longer = [SUMS, "0"].concat();
        assert_eq!(
            verify_sums(&key(), longer.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
    }

    #[test]
    fn refuses_the_sums_of_another_version() {
        assert_eq!(
            verify_sums(&key(), SUMS.as_bytes(), SIGNATURE, &version("0.2.0")),
            Err(VerifyError::Version {
                signed: "dassie 0.1.0".into()
            })
        );
        // A trusted comment changed to match: the global signature fails.
        let relabelled = SIGNATURE.replace("dassie 0.1.0", "dassie 0.2.0");
        assert_eq!(
            verify_sums(&key(), SUMS.as_bytes(), &relabelled, &version("0.2.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
    }

    #[test]
    fn refuses_other_keys_and_legacy_signatures() {
        assert_eq!(
            verify_sums(&key(), SUMS.as_bytes(), LEGACY, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Algorithm))
        );
        let mut other = key();
        other.id = [0; KEY_ID];
        assert_eq!(
            verify_sums(&other, SUMS.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::KeyId))
        );
        // The same ID with another key: the signature does not verify.
        let mut impostor = key();
        impostor.key = VerifyingKey::from_bytes(&[
            0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64,
            0x07, 0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68,
            0xf7, 0x07, 0x51, 0x1a,
        ])
        .unwrap_or_else(|e| panic!("RFC 8032's first key: {e}"));
        assert_eq!(
            verify_sums(&impostor, SUMS.as_bytes(), SIGNATURE, &version("0.1.0")),
            Err(VerifyError::Signature(SignatureError::Invalid))
        );
    }

    #[test]
    fn refuses_malformed_signatures() {
        let lines: Vec<&str> = SIGNATURE.lines().collect();
        let cases = [
            String::new(),
            lines.iter().take(3).map(|l| [l, "\n"].concat()).collect(),
            [SIGNATURE, "extra\n"].concat(),
            SIGNATURE.replace("untrusted comment:", "comment:"),
            SIGNATURE.replace("trusted comment: dassie", "comment: dassie"),
            SIGNATURE.replace("RUT", "RUTT"),
            "x".repeat(MAX_SIGNATURE + 1),
        ];
        for signature in cases {
            assert_eq!(
                verify_sums(&key(), SUMS.as_bytes(), &signature, &version("0.1.0")),
                Err(VerifyError::Signature(SignatureError::Encoding)),
                "{signature:?}"
            );
        }
    }

    #[test]
    fn signs_for_the_version() {
        assert_eq!(trusted_comment(&version("12.0.345")), "dassie 12.0.345");
        assert_eq!(
            VerifyError::Version { signed: "x".into() }.to_string(),
            "SHA256SUMS is signed for another version (\"x\")"
        );
    }

    /// Standard base64 of these bytes.
    fn base64(bytes: impl Strategy<Value = Vec<u8>>) -> impl Strategy<Value = String> {
        bytes.prop_map(|bytes| STANDARD.encode(bytes))
    }

    /// Bytes, mostly `N` of them, sometimes a few more or fewer.
    fn about<const N: usize>() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            4 => proptest::collection::vec(any::<u8>(), N),
            1 => proptest::collection::vec(any::<u8>(), N.saturating_sub(4)..N + 4),
        ]
    }

    /// The bytes of a key: `Ed` or other algorithm bytes, an ID, and the
    /// test key, a weak key or any 32 bytes; sometimes not 42 bytes.
    fn key_bytes() -> impl Strategy<Value = Vec<u8>> {
        let algorithm = prop_oneof![
            3 => Just(KEY_ALGORITHM),
            1 => Just(PREHASHED),
            1 => any::<[u8; 2]>(),
        ];
        let point = prop_oneof![
            Just(key().key.to_bytes()),
            // The identity and a point of order 4: weak keys.
            Just(core::array::from_fn(|i| u8::from(i == 0))),
            Just([0; 32]),
            any::<[u8; 32]>(),
        ];
        prop_oneof![
            4 => (algorithm, any::<[u8; KEY_ID]>(), point)
                .prop_map(|(algorithm, id, point)| [&algorithm[..], &id, &point].concat()),
            1 => about::<{ 2 + KEY_ID + 32 }>(),
        ]
    }

    /// A key as `minisign.pub` has it, or just its base64 line, with any
    /// line endings and what follows.
    fn key_text() -> impl Strategy<Value = String> {
        let line = prop_oneof![4 => base64(key_bytes()), 1 => "\\PC{0,60}"];
        let comment = prop_oneof![4 => "untrusted comment: \\PC{0,30}", 1 => "\\PC{0,30}"];
        let end = prop::sample::select(&["", "\n", "\r\n", "\n\n", "\nmore\n", "\n\r\n"][..]);
        (proptest::option::of(comment), line, end).prop_map(|(comment, line, end)| match comment {
            Some(comment) => format!("{comment}\n{line}{end}"),
            None => format!("{line}{end}"),
        })
    }

    /// The untrusted comment line.
    fn untrusted_line() -> impl Strategy<Value = String> {
        prop_oneof![
            8 => "untrusted comment: \\PC{0,30}",
            1 => "untrusted comment: [a-z]{900,1000}",
            1 => "\\PC{0,30}",
        ]
    }

    /// The signature line: `ED` or other algorithm bytes, the test key's
    /// ID or another, and 64 bytes, in base64; or not quite.
    fn signature_line() -> impl Strategy<Value = String> {
        let algorithm = prop_oneof![
            3 => Just(PREHASHED),
            1 => Just(KEY_ALGORITHM),
            1 => any::<[u8; 2]>(),
        ];
        let id = prop_oneof![3 => Just(key().id), 1 => any::<[u8; KEY_ID]>()];
        let bytes = (algorithm, id, about::<64>())
            .prop_map(|(algorithm, id, signed)| [&algorithm[..], &id, &signed].concat());
        prop_oneof![4 => base64(bytes), 1 => "\\PC{0,110}"]
    }

    /// The trusted comment line.
    fn trusted_line() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("trusted comment: dassie 0.1.0".into()),
            "trusted comment: \\PC{0,30}",
            "\\PC{0,30}",
        ]
    }

    /// The global signature line: 64 bytes in base64, or not quite.
    fn global_line() -> impl Strategy<Value = String> {
        prop_oneof![4 => base64(about::<64>()), 1 => "\\PC{0,100}"]
    }

    /// Any of a signature file's lines.
    fn any_line() -> impl Strategy<Value = String> {
        prop_oneof![
            untrusted_line(),
            signature_line(),
            trusted_line(),
            global_line()
        ]
    }

    /// A signature file as minisign writes it, but of anything: four
    /// lines mostly, sometimes fewer or more, with either line ending.
    fn signature_file() -> impl Strategy<Value = String> {
        let lines = (
            untrusted_line(),
            signature_line(),
            trusted_line(),
            global_line(),
        )
            .prop_map(|(a, b, c, d)| [a, b, c, d]);
        let count = prop_oneof![8 => Just(4usize), 1 => 0..=6usize];
        let end = prop::sample::select(&["\n", "\r\n"][..]);
        (lines, count, end, any::<bool>()).prop_map(|(lines, count, end, last)| {
            let mut file: String = lines
                .iter()
                .cycle()
                .take(count)
                .map(|line| [line, end].concat())
                .collect();
            if !last {
                file.truncate(file.trim_end_matches(['\r', '\n']).len());
            }
            file
        })
    }

    /// Sums to sign or verify: the test sums, sums of other files, or
    /// anything.
    fn sums_bytes() -> impl Strategy<Value = Vec<u8>> {
        prop_oneof![
            Just(SUMS.as_bytes().to_vec()),
            "([0-9a-f]{64}  [a-z]{1,8}\n){1,4}".prop_map(String::into_bytes),
            proptest::collection::vec(any::<u8>(), 0..300),
        ]
    }

    /// What is done to a good signature.
    #[derive(Clone, Debug)]
    enum Mutation {
        None,
        Flip(usize, u8),
        Replace(usize, String),
        Insert(usize, String),
        Remove(usize),
        CrLf,
    }

    /// Mostly a flipped bit, else a line replaced, added or removed, or
    /// other line endings.
    fn mutation() -> impl Strategy<Value = Mutation> {
        prop_oneof![
            1 => Just(Mutation::None),
            4 => (any::<usize>(), 0u8..7).prop_map(|(at, bit)| Mutation::Flip(at, bit)),
            2 => (any::<usize>(), any_line()).prop_map(|(at, line)| Mutation::Replace(at, line)),
            1 => (any::<usize>(), any_line()).prop_map(|(at, line)| Mutation::Insert(at, line)),
            1 => any::<usize>().prop_map(Mutation::Remove),
            1 => Just(Mutation::CrLf),
        ]
    }

    /// Applies a mutation, if what comes out is still text.
    fn mutate(signature: &str, mutation: &Mutation) -> Option<String> {
        let mut lines: Vec<String> = signature.lines().map(String::from).collect();
        let count = lines.len();
        match mutation {
            Mutation::None => return Some(signature.into()),
            Mutation::Flip(at, bit) => {
                let mut bytes = signature.as_bytes().to_vec();
                let at = at % bytes.len();
                bytes[at] ^= 1 << bit;
                return String::from_utf8(bytes).ok();
            }
            Mutation::CrLf => return Some(signature.replace('\n', "\r\n")),
            Mutation::Replace(at, line) => lines[at % count] = line.clone(),
            Mutation::Insert(at, line) => lines.insert(at % (count + 1), line.clone()),
            Mutation::Remove(at) => {
                lines.remove(at % count);
            }
        }
        Some(lines.iter().map(|line| [line, "\n"].concat()).collect())
    }

    /// The version a signature is checked for.
    fn any_version() -> impl Strategy<Value = Version> {
        prop::sample::select(&["0.1.0", "0.2.0", "10.0.1"][..]).prop_map(version)
    }

    proptest! {
        /// Whatever it accepts is a strong Ed25519 key.
        #[test]
        fn keys_never_panic(text in prop_oneof![1 => "\\PC{0,120}", 4 => key_text()]) {
            if let Ok(key) = PublicKey::parse(&text) {
                prop_assert!(!key.key.is_weak());
                prop_assert_eq!(key.id().len(), 2 * KEY_ID);
            }
        }

        /// Signed by no one: refused, whatever it looks like.
        #[test]
        fn never_panics(
            signature in prop_oneof![1 => "\\PC{0,400}", 4 => signature_file()],
            sums in sums_bytes(),
        ) {
            let result = verify_sums(&key(), &sums, &signature, &version("0.1.0"));
            prop_assert!(matches!(result, Err(VerifyError::Signature(_) | VerifyError::Sums(_))));
        }

        /// A good signature verifies for its version only, and changed
        /// past its untrusted comment, not at all.
        #[test]
        fn mutated_signatures_are_refused(
            sums in sums_bytes(),
            comment in prop_oneof![
                Just("dassie 0.1.0".into()),
                "dassie \\PC{0,20}",
                "\\PC{0,30}",
            ],
            version in any_version(),
            mutation in mutation(),
        ) {
            let signature = crate::testing::sign(crate::testing::TEST_SECRET_KEY, &sums, &comment);
            let good = verify_sums(&key(), &sums, &signature, &version);
            if comment == trusted_comment(&version) {
                prop_assert_eq!(&good, &Sums::parse(&sums).map_err(VerifyError::Sums));
            } else {
                prop_assert_eq!(&good, &Err(VerifyError::Version { signed: comment }));
            }
            if let Some(mutated) = mutate(&signature, &mutation) {
                let result = verify_sums(&key(), &sums, &mutated, &version);
                if mutated.lines().skip(1).eq(signature.lines().skip(1)) {
                    let broken = Err(VerifyError::Signature(SignatureError::Encoding));
                    prop_assert!(result == good || result == broken, "{:?}", result);
                } else {
                    prop_assert!(matches!(result, Err(VerifyError::Signature(_))), "{:?}", result);
                }
            }
        }

        /// Any change to a signed byte is caught.
        #[test]
        fn any_flipped_bit_is_refused(position in 0..SUMS.len(), bit in 0u8..8) {
            let mut sums = SUMS.as_bytes().to_vec();
            if let Some(byte) = sums.get_mut(position) {
                *byte ^= 1 << bit;
            }
            prop_assert!(verify_sums(&key(), &sums, SIGNATURE, &version("0.1.0")).is_err());
        }
    }
}

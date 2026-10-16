//! BIP-39 recovery phrases with the English wordlist.
//!
//! Implemented here rather than taken from the `bip39` crate, which pulls in
//! `bitcoin_hashes` and Unicode normalization for wordlists Dassie does not
//! use. With English only, a valid phrase is plain ASCII, so normalization
//! reduces to lowercasing and collapsing whitespace.

use core::fmt;

use rand_core::CryptoRng;
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroize;

use crate::error::Error;
use crate::seed::MasterSeed;
use crate::wordlist::ENGLISH;

/// Most words in a phrase.
const MAX_WORDS: usize = 24;

/// Longest word on the English list, in characters.
const MAX_WORD_LENGTH: usize = 8;

/// Bits each word encodes.
const BITS_PER_WORD: usize = 11;

/// Entropy and checksum packed together: 24 words of 11 bits is 264 bits.
const PACKED_BYTES: usize = 33;

/// Longest canonical phrase: 24 words of 8 characters and 23 spaces.
pub const MAX_PHRASE_LENGTH: usize = MAX_WORDS * MAX_WORD_LENGTH + MAX_WORDS - 1;

/// Longest input [`Mnemonic::parse`] looks at. Well above
/// [`MAX_PHRASE_LENGTH`] to allow sloppy whitespace in pasted phrases, but
/// bounded so that parsing work is too.
pub const MAX_PHRASE_INPUT_BYTES: usize = 1024;

/// PBKDF2 parameters fixed by BIP-39. Dassie never uses a BIP-39
/// passphrase, so the salt is just the prefix.
const SEED_SALT: &[u8] = b"mnemonic";
const SEED_ROUNDS: u32 = 2048;

/// The phrase lengths BIP-39 defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WordCount {
    /// 12 words, 128 bits of entropy. What the original setup generated.
    Twelve,
    /// 15 words, 160 bits of entropy.
    Fifteen,
    /// 18 words, 192 bits of entropy.
    Eighteen,
    /// 21 words, 224 bits of entropy.
    TwentyOne,
    /// 24 words, 256 bits of entropy.
    TwentyFour,
}

impl WordCount {
    /// Number of words.
    #[must_use]
    pub const fn words(self) -> usize {
        match self {
            Self::Twelve => 12,
            Self::Fifteen => 15,
            Self::Eighteen => 18,
            Self::TwentyOne => 21,
            Self::TwentyFour => 24,
        }
    }

    /// Entropy length in bytes: 32 bits for every 3 words.
    #[must_use]
    pub const fn entropy_bytes(self) -> usize {
        self.words() / 3 * 4
    }

    /// The count with `words` words, if BIP-39 defines one.
    #[must_use]
    pub const fn from_words(words: usize) -> Option<Self> {
        match words {
            12 => Some(Self::Twelve),
            15 => Some(Self::Fifteen),
            18 => Some(Self::Eighteen),
            21 => Some(Self::TwentyOne),
            24 => Some(Self::TwentyFour),
            _ => None,
        }
    }

    /// The count for `bytes` bytes of entropy, if BIP-39 defines one.
    #[must_use]
    pub const fn from_entropy_bytes(bytes: usize) -> Option<Self> {
        if !bytes.is_multiple_of(4) {
            return None;
        }
        Self::from_words(bytes / 4 * 3)
    }

    /// Checksum length in bits: one per 32 bits of entropy.
    const fn checksum_bits(self) -> usize {
        self.entropy_bytes() / 4
    }
}

/// A BIP-39 phrase, held as its entropy and checksum.
///
/// Every `Mnemonic` is valid: [`parse`](Self::parse) checks each word and the
/// checksum, which the original never did (gap UI-5), so a mistyped word is
/// reported instead of silently producing a different node.
pub struct Mnemonic {
    /// Entropy, then the checksum byte (`SHA-256(entropy)[0]`, of which the
    /// phrase uses the leading bits), then zeros.
    packed: [u8; PACKED_BYTES],
    words: WordCount,
}

impl Mnemonic {
    /// The phrase for `entropy`, which must be 16, 20, 24, 28 or 32 bytes.
    pub fn from_entropy(entropy: &[u8]) -> Result<Self, Error> {
        let words =
            WordCount::from_entropy_bytes(entropy.len()).ok_or(Error::InvalidEntropyLength)?;
        Ok(Self::from_valid_entropy(entropy, words))
    }

    /// A new phrase of `words` words with entropy from `rng`.
    #[must_use]
    pub fn generate<R: CryptoRng + ?Sized>(rng: &mut R, words: WordCount) -> Self {
        let mut entropy = [0u8; MAX_WORDS / 3 * 4];
        let slot = entropy.get_mut(..words.entropy_bytes()).unwrap_or_default();
        rng.fill_bytes(slot);
        let mnemonic = Self::from_valid_entropy(slot, words);
        entropy.zeroize();
        mnemonic
    }

    /// Parses a phrase.
    ///
    /// Words may be separated by any amount of ASCII whitespace and may be
    /// in any case; the seed is computed from the canonical form (lowercase,
    /// single spaces) either way. Anything outside ASCII is rejected, since
    /// no English word contains it.
    pub fn parse(phrase: &str) -> Result<Self, Error> {
        if phrase.len() > MAX_PHRASE_INPUT_BYTES {
            return Err(Error::PhraseTooLong);
        }
        if !phrase.is_ascii() {
            return Err(Error::NonAsciiPhrase);
        }
        let mut indices = WordIndices([0; MAX_WORDS]);
        let mut count = 0;
        for word in phrase.split_ascii_whitespace() {
            let slot = indices.0.get_mut(count).ok_or(Error::InvalidWordCount)?;
            *slot = lookup(word).ok_or(Error::UnknownWord { position: count })?;
            count += 1;
        }
        let words = WordCount::from_words(count).ok_or(Error::InvalidWordCount)?;
        Self::from_indices(indices.0.get(..count).unwrap_or_default(), words)
    }

    /// The entropy.
    #[must_use]
    pub fn entropy(&self) -> &[u8] {
        self.packed
            .get(..self.words.entropy_bytes())
            .unwrap_or_default()
    }

    /// How many words the phrase has.
    #[must_use]
    pub const fn word_count(&self) -> WordCount {
        self.words
    }

    /// The words in order.
    #[must_use]
    pub const fn words(&self) -> Words<'_> {
        Words {
            mnemonic: self,
            position: 0,
        }
    }

    /// The canonical phrase: lowercase words separated by single spaces.
    #[must_use]
    pub fn phrase(&self) -> Phrase {
        let mut phrase = Phrase {
            bytes: [0; MAX_PHRASE_LENGTH],
            length: 0,
        };
        for (position, word) in self.words().enumerate() {
            if position > 0 {
                phrase.push(b" ");
            }
            phrase.push(word.as_bytes());
        }
        phrase
    }

    /// The master seed: PBKDF2-HMAC-SHA512 of the canonical phrase with
    /// salt `"mnemonic"` and 2048 rounds (BIP-39 with an empty passphrase,
    /// which is what Dassie has always used).
    #[must_use]
    pub fn to_seed(&self) -> MasterSeed {
        let phrase = self.phrase();
        let mut seed = [0u8; 64];
        pbkdf2::pbkdf2_hmac::<Sha512>(
            phrase.as_str().as_bytes(),
            SEED_SALT,
            SEED_ROUNDS,
            &mut seed,
        );
        let master = MasterSeed::from_bytes(seed);
        seed.zeroize();
        master
    }

    fn from_valid_entropy(entropy: &[u8], words: WordCount) -> Self {
        assert!(
            entropy.len() == words.entropy_bytes(),
            "entropy length matches the word count"
        );
        let mut packed = [0u8; PACKED_BYTES];
        let (head, tail) = packed.split_at_mut(entropy.len());
        head.copy_from_slice(entropy);
        if let Some(checksum) = tail.first_mut() {
            *checksum = Sha256::digest(entropy).first().copied().unwrap_or_default();
        }
        Self { packed, words }
    }

    /// The mnemonic for `indices`, if their checksum bits are right.
    fn from_indices(indices: &[u16], words: WordCount) -> Result<Self, Error> {
        assert!(
            indices.len() == words.words(),
            "index count matches the word count"
        );
        let mut packed = Packed([0; PACKED_BYTES]);
        for (position, &index) in indices.iter().enumerate() {
            write_bits(&mut packed.0, position * BITS_PER_WORD, index);
        }
        let entropy_bytes = words.entropy_bytes();
        let mnemonic =
            Self::from_valid_entropy(packed.0.get(..entropy_bytes).unwrap_or_default(), words);
        // Only the leading checksum bits are in the phrase; the rest of the
        // byte is zero in `packed` and hash bits in `mnemonic`.
        let unused = 8 - words.checksum_bits();
        let stored = packed.0.get(entropy_bytes).copied().unwrap_or_default() >> unused;
        let expected = mnemonic
            .packed
            .get(entropy_bytes)
            .copied()
            .unwrap_or_default()
            >> unused;
        if stored != expected {
            return Err(Error::InvalidChecksum);
        }
        Ok(mnemonic)
    }

    // Panics only on a broken invariant: an index has 11 bits.
    #[allow(clippy::panic)]
    fn word(&self, position: usize) -> &'static str {
        let index = read_bits(&self.packed, position * BITS_PER_WORD);
        let Some(word) = ENGLISH.get(usize::from(index)) else {
            panic!("an 11-bit index is within the wordlist");
        };
        word
    }
}

impl Drop for Mnemonic {
    fn drop(&mut self) {
        self.packed.zeroize();
    }
}

impl zeroize::ZeroizeOnDrop for Mnemonic {}

impl fmt::Debug for Mnemonic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mnemonic")
            .field("words", &self.words.words())
            .finish_non_exhaustive()
    }
}

/// The words of a [`Mnemonic`], in order.
#[derive(Debug)]
pub struct Words<'a> {
    mnemonic: &'a Mnemonic,
    position: usize,
}

impl Iterator for Words<'_> {
    type Item = &'static str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.position >= self.mnemonic.words.words() {
            return None;
        }
        let word = self.mnemonic.word(self.position);
        self.position += 1;
        Some(word)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.mnemonic.words.words().saturating_sub(self.position);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Words<'_> {}

/// A canonical phrase, held inline and wiped on drop.
pub struct Phrase {
    bytes: [u8; MAX_PHRASE_LENGTH],
    length: usize,
}

impl Phrase {
    /// The phrase as text.
    #[must_use]
    // Panics only on a broken invariant: phrases hold ASCII words.
    #[allow(clippy::panic)]
    pub fn as_str(&self) -> &str {
        let bytes = self.bytes.get(..self.length).unwrap_or_default();
        let Ok(text) = core::str::from_utf8(bytes) else {
            panic!("phrases hold only ASCII words");
        };
        text
    }

    // Panics only on a broken invariant: canonical phrases fit.
    #[allow(clippy::panic)]
    fn push(&mut self, text: &[u8]) {
        let end = self.length + text.len();
        let Some(slot) = self.bytes.get_mut(self.length..end) else {
            panic!("canonical phrases fit in {MAX_PHRASE_LENGTH} bytes");
        };
        slot.copy_from_slice(text);
        self.length = end;
    }
}

impl Drop for Phrase {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl zeroize::ZeroizeOnDrop for Phrase {}

impl fmt::Debug for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Phrase(<redacted>)")
    }
}

/// Word indices during parsing, wiped however parsing ends.
struct WordIndices([u16; MAX_WORDS]);

impl Drop for WordIndices {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Packed entropy during parsing, wiped however parsing ends.
struct Packed([u8; PACKED_BYTES]);

impl Drop for Packed {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// The index of `word` on the wordlist, ignoring ASCII case.
fn lookup(word: &str) -> Option<u16> {
    if word.len() > MAX_WORD_LENGTH {
        return None;
    }
    let mut buffer = [0u8; MAX_WORD_LENGTH];
    let lowered = buffer.get_mut(..word.len())?;
    lowered.copy_from_slice(word.as_bytes());
    lowered.make_ascii_lowercase();
    let found = ENGLISH.binary_search_by(|candidate| candidate.as_bytes().cmp(lowered));
    buffer.zeroize();
    found.ok().and_then(|index| u16::try_from(index).ok())
}

/// Reads the 11 bits starting at bit `offset` (most significant first).
fn read_bits(packed: &[u8; PACKED_BYTES], offset: usize) -> u16 {
    let mut value = 0u16;
    for bit in offset..offset + BITS_PER_WORD {
        let byte = packed.get(bit / 8).copied();
        assert!(byte.is_some(), "bit offset within the packed entropy");
        let set = (byte.unwrap_or_default() >> (7 - bit % 8)) & 1;
        value = (value << 1) | u16::from(set);
    }
    value
}

/// Writes the low 11 bits of `value` starting at bit `offset`.
// Panics only on a broken invariant: offsets stay within the entropy.
#[allow(clippy::panic)]
fn write_bits(packed: &mut [u8; PACKED_BYTES], offset: usize, value: u16) {
    assert!(value < 1 << BITS_PER_WORD, "word index has 11 bits");
    for (shift, bit) in (0..BITS_PER_WORD).rev().zip(offset..) {
        if (value >> shift) & 1 == 1 {
            let Some(byte) = packed.get_mut(bit / 8) else {
                panic!("bit offset within the packed entropy");
            };
            *byte |= 0x80 >> (bit % 8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The BIP-39 reference vector with all-zero entropy.
    #[test]
    fn zero_entropy_is_abandon_about() {
        let Ok(mnemonic) = Mnemonic::from_entropy(&[0; 16]) else {
            panic!()
        };
        assert_eq!(
            mnemonic.phrase().as_str(),
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        );
        assert_eq!(mnemonic.words().len(), 12);
    }

    #[test]
    fn longest_phrase_fits() {
        let Ok(mnemonic) = Mnemonic::from_entropy(&[0xff; 32]) else {
            panic!()
        };
        assert_eq!(mnemonic.phrase().as_str().split(' ').count(), 24);
        assert!(MAX_PHRASE_LENGTH >= mnemonic.phrase().as_str().len());
    }

    #[test]
    fn parsing_normalizes_case_and_whitespace() {
        let messy = "  ABANDON abandon\tabandon\nabandon abandon  abandon abandon abandon abandon abandon abandon About\r\n";
        let Ok(mnemonic) = Mnemonic::parse(messy) else {
            panic!()
        };
        assert_eq!(mnemonic.entropy(), &[0; 16]);
    }

    #[test]
    fn overlong_and_non_ascii_input_is_rejected() {
        let long = [b' '; MAX_PHRASE_INPUT_BYTES + 1];
        let Ok(long) = core::str::from_utf8(&long) else {
            panic!()
        };
        assert_eq!(Mnemonic::parse(long).map(|_| ()), Err(Error::PhraseTooLong));
        assert_eq!(
            Mnemonic::parse("abandon\u{a0}abandon").map(|_| ()),
            Err(Error::NonAsciiPhrase)
        );
        assert_eq!(
            Mnemonic::parse("").map(|_| ()),
            Err(Error::InvalidWordCount)
        );
    }
}

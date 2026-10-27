//! The recovery phrase (spec/decisions/phrase-format.md): 20 words of the
//! BIP-39 English list that say the phrase's realm and when it was made,
//! hold a 128-bit secret, and correct their own errors.
//!
//! Each word is the index of its place in the list, an element of
//! GF(2¹¹). The 14 data words are 154 bits, most significant first: the
//! kind (6 bits), the hour it was made (20 bits) and the secret (128 bits).
//! Six Reed–Solomon check words follow ([`crate::reed_solomon`]).
//!
//! A phrase is read by [`Phrase::read`]: words by their first four letters,
//! unknown or unreadable words as erasures, only 20 of them, corrected
//! within the code's bound, and refused by kind, realm and a creation time
//! more than a day ahead of the reader's clock before any correction is
//! shown. A correction comes back as a [`Correction`], which
//! gives up its phrase only once the caller [confirms](Correction::confirm)
//! it. The master seed is HKDF-SHA256 of the secret, keyed by the realm.

use core::fmt;

use hkdf::Hkdf;
use rand_core::CryptoRng;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::error::Error;
use crate::reed_solomon::{self, DATA, LENGTH};
use crate::secret::secret_bytes;
use crate::seed::MasterSeed;
use crate::wordlist;

/// Words in a phrase.
pub const PHRASE_WORDS: usize = LENGTH;

/// Longest word on the English list, in characters.
const MAX_WORD_LENGTH: usize = 8;

/// Longest canonical phrase: 20 words of 8 characters and 19 spaces.
pub const MAX_PHRASE_LENGTH: usize = PHRASE_WORDS * MAX_WORD_LENGTH + PHRASE_WORDS - 1;

/// Longest input [`Phrase::read`] looks at. Well above
/// [`MAX_PHRASE_LENGTH`] to allow sloppy whitespace and words typed in
/// full, but bounded so that reading work is too.
pub const MAX_PHRASE_INPUT_BYTES: usize = 1024;

/// Word counts of BIP-39 phrases, refused as such.
const BIP39_WORD_COUNTS: [usize; 5] = [12, 15, 18, 21, 24];

/// Bits of the kind, the creation hour and the secret, in that order.
const KIND_BITS: usize = 6;
const CREATED_BITS: usize = 20;
const SECRET_BITS: usize = 128;

/// Bits in a word.
const WORD_BITS: usize = 11;

/// The data words' bits, packed most significant first, with six bits to
/// spare at the end.
const PACKED_BYTES: usize = 20;
const _: () = assert!(KIND_BITS + CREATED_BITS + SECRET_BITS == DATA * WORD_BITS);
const _: () = assert!(DATA * WORD_BITS <= PACKED_BYTES * 8);

/// The kinds this version knows: this layout in each realm. Kind 0 is never
/// valid, and the rest are unassigned.
const KIND_TEST: u8 = 1;
const KIND_LIVE: u8 = 2;

/// The HKDF salt of the master seed.
const SEED_SALT: &[u8] = b"dassie phrase";

/// The realm a phrase is of: the testnet or the live network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Realm {
    /// The test realm.
    Test,
    /// The live realm.
    Live,
}

impl Realm {
    /// The realm's name, `test` or `live`, which is also the HKDF info of
    /// its master seeds.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Live => "live",
        }
    }

    /// The realm named `test` or `live`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "test" => Some(Self::Test),
            "live" => Some(Self::Live),
            _ => None,
        }
    }

    const fn kind(self) -> u8 {
        match self {
            Self::Test => KIND_TEST,
            Self::Live => KIND_LIVE,
        }
    }
}

impl fmt::Display for Realm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// When a phrase was made: hours since 2026-01-01T00:00:00Z, rounded down.
///
/// The field has 20 bits, so it ends in August 2145. A phrase that says it
/// was made more than a day after the reader's clock is refused
/// ([`Error::CreatedInFuture`]), and a new phrase is shown with its date,
/// which [`Display`](fmt::Display) writes as the decision shows it:
/// `2026-02-19`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Created(u32);

impl Created {
    /// 2026-01-01T00:00:00Z in milliseconds since the Unix epoch.
    pub const EPOCH_UNIX_MS: u64 = 1_767_225_600_000;

    /// The last hour the field can hold.
    pub const MAX_HOURS: u32 = (1 << CREATED_BITS) - 1;

    const HOUR_MS: u64 = 3_600_000;

    const DAY_MS: u64 = 24 * Self::HOUR_MS;

    /// The hour `hours` after the epoch, if the field can hold it.
    #[must_use]
    pub const fn from_hours(hours: u32) -> Option<Self> {
        if hours > Self::MAX_HOURS {
            None
        } else {
            Some(Self(hours))
        }
    }

    /// The hour `now_ms` (milliseconds since the Unix epoch) lies in, if
    /// the field can hold it.
    #[must_use]
    pub fn at_unix_ms(now_ms: u64) -> Option<Self> {
        let since_ms = now_ms.checked_sub(Self::EPOCH_UNIX_MS)?;
        Self::from_hours(u32::try_from(since_ms / Self::HOUR_MS).ok()?)
    }

    /// Hours since the epoch.
    #[must_use]
    pub const fn hours(self) -> u32 {
        self.0
    }

    /// The start of the hour, in milliseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_ms(self) -> u64 {
        Self::EPOCH_UNIX_MS + self.0 as u64 * Self::HOUR_MS
    }

    /// Whether the hour starts more than a day after `now_ms`, the
    /// reader's clock: a phrase cannot be made in the future, so either
    /// that clock is behind, the words are not the phrase, or the clock
    /// that made the phrase ran ahead.
    #[must_use]
    pub const fn is_more_than_a_day_after(self, now_ms: u64) -> bool {
        self.unix_ms() > now_ms.saturating_add(Self::DAY_MS)
    }

    /// The day the hour lies in, as year, month and day of the month, in
    /// UTC.
    #[must_use]
    // The year is below 2146 and the month and day below 32, so the
    // narrowing conversions are exact.
    #[allow(clippy::cast_possible_truncation)]
    pub const fn date(self) -> (u16, u8, u8) {
        // Howard Hinnant's `civil_from_days`, on days counted from
        // 0000-03-01 of the proleptic Gregorian calendar, which puts the
        // leap day at the end of a year. Every hour of the field is after
        // 1970, so nothing here is negative.
        let days = self.unix_ms() / Self::DAY_MS + 719_468;
        let era = days / 146_097;
        let day_of_era = days % 146_097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_from_march = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month_from_march + 2) / 5 + 1;
        let month = if month_from_march < 10 {
            month_from_march + 3
        } else {
            month_from_march - 9
        };
        let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
        (year as u16, month as u8, day as u8)
    }
}

impl fmt::Display for Created {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = self.date();
        write!(f, "{year:04}-{month:02}-{day:02}")
    }
}

/// A set of word positions in a phrase, zero-based: words an owner marks
/// as unreadable, or words a correction changed.
///
/// It names places only, never words, so it may be logged and shown.
/// [`Display`](fmt::Display) counts from one, for people.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Positions(u32);

impl Positions {
    /// No position.
    pub const NONE: Self = Self(0);

    /// The positions whose bits are set, bit `i` for position `i`, if all
    /// are within a phrase.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits >> PHRASE_WORDS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// The positions as bits, bit `i` for position `i`.
    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// These positions and `position`, which must be below
    /// [`PHRASE_WORDS`].
    #[must_use]
    pub const fn with(self, position: usize) -> Self {
        assert!(position < PHRASE_WORDS, "a position within the phrase");
        Self(self.0 | 1 << position)
    }

    /// Whether `position` is one of these.
    #[must_use]
    pub const fn contains(self, position: usize) -> bool {
        position < PHRASE_WORDS && self.0 & (1 << position) != 0
    }

    /// How many positions there are.
    #[must_use]
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Whether there are none.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The positions in order, zero-based.
    pub fn iter(self) -> impl Iterator<Item = usize> {
        (0..PHRASE_WORDS).filter(move |&position| self.contains(position))
    }
}

impl fmt::Debug for Positions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl fmt::Display for Positions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, position) in self.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}", position + 1)?;
        }
        Ok(())
    }
}

/// The word of the list that `typed` names, as [`Phrase::read`] reads
/// each word: by its first four letters, or in full if it is shorter, in
/// any ASCII case; `None` for a word not on the list, which a reading
/// takes as an erasure. For a page that shows the owner what each typed
/// word reads as before the phrase is read.
#[must_use]
pub fn word_named(typed: &str) -> Option<&'static str> {
    wordlist::find(typed).map(wordlist::word)
}

secret_bytes!(
    /// A phrase's 128-bit secret: the key material of its master seed.
    PhraseSecret,
    16
);

/// A phrase: a codeword whose kind names this layout and a realm.
///
/// Every `Phrase` is valid. It comes from [`generate`](Self::generate),
/// [`new`](Self::new), or a [reading](Self::read) that needed no
/// correction or whose correction was confirmed.
pub struct Phrase {
    words: [u16; LENGTH],
}

impl Phrase {
    /// The phrase of `secret`, made at `created`, in `realm`.
    #[must_use]
    pub fn new(realm: Realm, created: Created, secret: &PhraseSecret) -> Self {
        let mut packed = Zeroizing::new([0u8; PACKED_BYTES]);
        write_bits(&mut packed, 0, KIND_BITS, u32::from(realm.kind()));
        write_bits(&mut packed, KIND_BITS, CREATED_BITS, created.hours());
        for (index, &byte) in secret.as_bytes().iter().enumerate() {
            write_bits(
                &mut packed,
                KIND_BITS + CREATED_BITS + 8 * index,
                8,
                u32::from(byte),
            );
        }
        let mut data = Zeroizing::new([0u16; DATA]);
        for (index, word) in data.iter_mut().enumerate() {
            *word = read_word(&packed, index);
        }
        Self {
            words: reed_solomon::encode(&data),
        }
    }

    /// A new phrase in `realm`, with a secret from `rng`, made at `now_ms`
    /// (milliseconds since the Unix epoch).
    ///
    /// # Errors
    ///
    /// [`Error::ClockOutOfRange`] if `now_ms` is before 2026 or after the
    /// last hour the phrase can record.
    pub fn generate<R: CryptoRng + ?Sized>(
        rng: &mut R,
        realm: Realm,
        now_ms: u64,
    ) -> Result<Self, Error> {
        let created = Created::at_unix_ms(now_ms).ok_or(Error::ClockOutOfRange)?;
        let mut secret = PhraseSecret::from_bytes([0; 16]);
        rng.fill_bytes(&mut secret.0);
        Ok(Self::new(realm, created, &secret))
    }

    /// Reads a phrase that a person typed, pasted or kept in a file,
    /// expecting one of `expected`'s realm (spec/decisions/phrase-format.md).
    ///
    /// Words may be separated by any whitespace and be in any case. A word
    /// is matched by its first four letters, or in full if it is shorter.
    /// A word not on the list, `?` say, or one at a position in
    /// `unreadable`, is an erasure: its place is known, its value is not.
    /// Up to two wrong words are restored, and erasures with them while
    /// `2 × wrong + erasures` is at most five. The kind, the realm and the
    /// creation time are checked before any correction is returned, so an
    /// owner never confirms a phrase that is refused next: a phrase made
    /// more than a day after `now_ms`, the reader's clock in milliseconds
    /// since the Unix epoch, is refused.
    ///
    /// # Errors
    ///
    /// [`Error::PhraseTooLong`] beyond [`MAX_PHRASE_INPUT_BYTES`];
    /// [`Error::Bip39Phrase`] for 12, 15, 18, 21 or 24 words, and
    /// [`Error::WrongWordCount`] for any count but 20;
    /// [`Error::TooDamaged`] beyond what the code restores;
    /// [`Error::InvalidKind`] and [`Error::NewerKind`] for kinds this
    /// version does not know; [`Error::OtherRealm`] for a phrase of the
    /// realm not expected; [`Error::CreatedInFuture`] for one made more
    /// than a day after `now_ms`.
    pub fn read(
        text: &str,
        unreadable: Positions,
        expected: Realm,
        now_ms: u64,
    ) -> Result<Reading, Error> {
        let (received, erasures) = read_words(text, unreadable)?;
        let (words, changed) =
            reed_solomon::decode(&received, erasures.bits()).map_err(|_| Error::TooDamaged)?;
        let words = Zeroizing::new(words);
        let phrase = Self::from_codeword(&words)?;
        let realm = phrase.realm();
        if realm != expected {
            return Err(Error::OtherRealm { realm });
        }
        let created = phrase.created();
        if created.is_more_than_a_day_after(now_ms) {
            return Err(Error::CreatedInFuture { created });
        }
        assert!(
            changed >> LENGTH == 0,
            "changes are positions in the phrase"
        );
        let changed = Positions(changed);
        Ok(if changed.is_empty() {
            Reading::Exact(phrase)
        } else {
            Reading::Corrected(Correction { phrase, changed })
        })
    }

    /// The phrase of a codeword, if its kind is one this version knows.
    fn from_codeword(words: &[u16; LENGTH]) -> Result<Self, Error> {
        assert!(reed_solomon::is_codeword(words), "a phrase is a codeword");
        let packed = pack(words);
        let kind = u8::try_from(read_bits(&packed, 0, KIND_BITS)).unwrap_or(u8::MAX);
        match kind {
            0 => Err(Error::InvalidKind),
            KIND_TEST | KIND_LIVE => Ok(Self { words: *words }),
            kind => Err(Error::NewerKind { kind }),
        }
    }

    /// The realm the phrase's kind names.
    #[must_use]
    pub fn realm(&self) -> Realm {
        let kind = read_bits(&pack(&self.words), 0, KIND_BITS);
        if kind == u32::from(KIND_LIVE) {
            Realm::Live
        } else {
            assert!(kind == u32::from(KIND_TEST), "a phrase's kind is known");
            Realm::Test
        }
    }

    /// When the phrase was made.
    #[must_use]
    // Panics only on a broken invariant: the field has 20 bits.
    #[allow(clippy::panic)]
    pub fn created(&self) -> Created {
        let hours = read_bits(&pack(&self.words), KIND_BITS, CREATED_BITS);
        let Some(created) = Created::from_hours(hours) else {
            panic!("a 20-bit hour is within the field");
        };
        created
    }

    /// The phrase's secret.
    #[must_use]
    pub fn secret(&self) -> PhraseSecret {
        let packed = pack(&self.words);
        let mut secret = PhraseSecret::from_bytes([0; 16]);
        for (index, byte) in secret.0.iter_mut().enumerate() {
            let bits = read_bits(&packed, KIND_BITS + CREATED_BITS + 8 * index, 8);
            *byte = u8::try_from(bits).unwrap_or_default();
        }
        secret
    }

    /// The master seed: HKDF-SHA256 with the secret as its key material,
    /// `dassie phrase` as its salt and the realm's name as its info, 64
    /// bytes. The realm is the caller's, so no caller derives a phrase of
    /// the other realm by mistake.
    ///
    /// # Errors
    ///
    /// [`Error::OtherRealm`] if the phrase is of another realm than
    /// `expected`.
    // Panics only on a broken invariant: HKDF-SHA256 gives up to 8160
    // bytes.
    #[allow(clippy::panic)]
    pub fn master_seed(&self, expected: Realm) -> Result<MasterSeed, Error> {
        let realm = self.realm();
        if realm != expected {
            return Err(Error::OtherRealm { realm });
        }
        let secret = self.secret();
        let hkdf = Hkdf::<Sha256>::new(Some(SEED_SALT), secret.as_bytes());
        let mut seed = Zeroizing::new([0u8; MasterSeed::LENGTH]);
        let Ok(()) = hkdf.expand(realm.as_str().as_bytes(), seed.as_mut_slice()) else {
            panic!("HKDF-SHA256 gives 64 bytes");
        };
        Ok(MasterSeed::from_bytes(*seed))
    }

    /// The words in order.
    #[must_use]
    pub fn words(&self) -> Words<'_> {
        Words {
            words: &self.words,
            position: 0,
        }
    }

    /// The canonical phrase: the words in full, lowercase, separated by
    /// single spaces.
    #[must_use]
    pub fn text(&self) -> PhraseText {
        let mut text = PhraseText {
            bytes: [0; MAX_PHRASE_LENGTH],
            length: 0,
        };
        for (position, word) in self.words().enumerate() {
            if position > 0 {
                text.push(b" ");
            }
            text.push(word.as_bytes());
        }
        text
    }
}

impl Drop for Phrase {
    fn drop(&mut self) {
        self.words.zeroize();
    }
}

impl zeroize::ZeroizeOnDrop for Phrase {}

impl fmt::Debug for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Phrase(<redacted>)")
    }
}

/// What reading a phrase found: the phrase as read, or a correction the
/// owner has to confirm first.
#[derive(Debug)]
#[must_use = "a correction must be shown and confirmed"]
pub enum Reading {
    /// Every word was read as written.
    Exact(Phrase),
    /// Some words were wrong or unreadable and have been restored.
    Corrected(Correction),
}

impl Reading {
    /// The phrase, for a reader with no one to confirm a correction, such
    /// as `dassie setup` reading a file.
    ///
    /// # Errors
    ///
    /// [`Error::NeedsConfirmation`], naming the positions of the words a
    /// correction would change, never the words.
    pub fn without_correction(self) -> Result<Phrase, Error> {
        match self {
            Self::Exact(phrase) => Ok(phrase),
            Self::Corrected(correction) => Err(Error::NeedsConfirmation {
                changed: correction.changed,
            }),
        }
    }
}

/// A phrase restored from wrong or unreadable words, to be shown with the
/// changed words marked and taken only once the owner confirms it.
///
/// It shows its words, but gives up the phrase, and with it the master
/// seed, only through [`confirm`](Self::confirm).
#[must_use = "a correction must be shown and confirmed"]
pub struct Correction {
    phrase: Phrase,
    changed: Positions,
}

impl Correction {
    /// The positions of the words that changed, unreadable ones included.
    #[must_use]
    pub const fn changed(&self) -> Positions {
        self.changed
    }

    /// The corrected words in order, to show.
    #[must_use]
    pub fn words(&self) -> Words<'_> {
        self.phrase.words()
    }

    /// The corrected phrase in canonical form, to show.
    #[must_use]
    pub fn text(&self) -> PhraseText {
        self.phrase.text()
    }

    /// When the corrected phrase was made, to show.
    #[must_use]
    pub fn created(&self) -> Created {
        self.phrase.created()
    }

    /// The realm of the corrected phrase.
    #[must_use]
    pub fn realm(&self) -> Realm {
        self.phrase.realm()
    }

    /// The phrase, once the owner has seen the correction and confirmed it.
    pub fn confirm(self) -> Phrase {
        self.phrase
    }
}

impl fmt::Debug for Correction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Correction")
            .field("changed", &self.changed)
            .finish_non_exhaustive()
    }
}

/// The words of a phrase, in order.
#[derive(Debug)]
pub struct Words<'a> {
    words: &'a [u16; LENGTH],
    position: usize,
}

impl Iterator for Words<'_> {
    type Item = &'static str;

    fn next(&mut self) -> Option<Self::Item> {
        let index = *self.words.get(self.position)?;
        self.position += 1;
        Some(wordlist::word(index))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = LENGTH.saturating_sub(self.position);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Words<'_> {}

/// A canonical phrase as text, held inline and wiped on drop.
pub struct PhraseText {
    bytes: [u8; MAX_PHRASE_LENGTH],
    length: usize,
}

impl PhraseText {
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

impl Drop for PhraseText {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl zeroize::ZeroizeOnDrop for PhraseText {}

impl fmt::Debug for PhraseText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PhraseText(<redacted>)")
    }
}

/// The words of `text`, with erasures for words not on the list and words
/// at `unreadable`, if it has 20 words.
fn read_words(
    text: &str,
    unreadable: Positions,
) -> Result<(Zeroizing<[u16; LENGTH]>, Positions), Error> {
    if text.len() > MAX_PHRASE_INPUT_BYTES {
        return Err(Error::PhraseTooLong);
    }
    let mut words = Zeroizing::new([0u16; LENGTH]);
    let mut erasures = Positions::NONE;
    let mut count = 0;
    for token in text.split_whitespace() {
        if let Some(slot) = words.get_mut(count) {
            match wordlist::find(token) {
                Some(index) if !unreadable.contains(count) => *slot = index,
                _ => erasures = erasures.with(count),
            }
        }
        count += 1;
    }
    if count == LENGTH {
        Ok((words, erasures))
    } else if BIP39_WORD_COUNTS.contains(&count) {
        Err(Error::Bip39Phrase { words: count })
    } else {
        Err(Error::WrongWordCount { words: count })
    }
}

/// The data words of `words`, packed into bits.
fn pack(words: &[u16; LENGTH]) -> Zeroizing<[u8; PACKED_BYTES]> {
    let mut packed = Zeroizing::new([0u8; PACKED_BYTES]);
    for (index, &word) in words.iter().take(DATA).enumerate() {
        write_bits(&mut packed, index * WORD_BITS, WORD_BITS, u32::from(word));
    }
    packed
}

/// The data word at `index`.
fn read_word(packed: &[u8; PACKED_BYTES], index: usize) -> u16 {
    let bits = read_bits(packed, index * WORD_BITS, WORD_BITS);
    u16::try_from(bits).unwrap_or_default()
}

/// Reads `width` bits starting at bit `offset`, most significant first.
fn read_bits(packed: &[u8; PACKED_BYTES], offset: usize, width: usize) -> u32 {
    assert!(
        width <= 32 && offset + width <= PACKED_BYTES * 8,
        "bits within the data"
    );
    let mut value = 0u32;
    for bit in offset..offset + width {
        let byte = packed.get(bit / 8).copied().unwrap_or_default();
        value = (value << 1) | u32::from((byte >> (7 - bit % 8)) & 1);
    }
    value
}

/// Writes the low `width` bits of `value` starting at bit `offset`, into
/// bits that are still zero.
fn write_bits(packed: &mut [u8; PACKED_BYTES], offset: usize, width: usize, value: u32) {
    assert!(
        width <= 32 && offset + width <= PACKED_BYTES * 8,
        "bits within the data"
    );
    assert!(
        width == 32 || value >> width == 0,
        "the value fits its field"
    );
    for (shift, bit) in (0..width).rev().zip(offset..) {
        if (value >> shift) & 1 == 1
            && let Some(byte) = packed.get_mut(bit / 8)
        {
            *byte |= 0x80 >> (bit % 8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(byte: u8) -> PhraseSecret {
        PhraseSecret::from_bytes([byte; 16])
    }

    fn created(hours: u32) -> Created {
        Created::from_hours(hours).unwrap()
    }

    /// A reader's clock past every phrase's creation time.
    const AFTER_EVERY_PHRASE: u64 = u64::MAX;

    #[test]
    fn words_are_named_as_a_reading_names_them() {
        assert_eq!(word_named("abandon"), Some("abandon"));
        assert_eq!(word_named("ABAN"), Some("abandon"));
        assert_eq!(word_named("abanxyz"), Some("abandon"));
        assert_eq!(word_named("act"), Some("act"));
        assert_eq!(word_named("acti"), Some("action"));
        assert_eq!(word_named("ac"), None);
        assert_eq!(word_named("?"), None);
        assert_eq!(word_named("zoo"), Some("zoo"));
    }

    // The fields land where the table puts them: kind, then hour, then
    // secret, most significant first.
    #[test]
    fn the_layout_is_kind_created_secret() {
        let phrase = Phrase::new(Realm::Live, created(Created::MAX_HOURS), &secret(0));
        // 000010, then twenty ones, then zeros.
        assert_eq!(
            &phrase.words[..3],
            &[0b000_0101_1111, 0x7ff, 0b111_1000_0000]
        );
        assert!(phrase.words[3..DATA].iter().all(|&w| w == 0));

        let phrase = Phrase::new(
            Realm::Test,
            created(0),
            &PhraseSecret::from_bytes([0xff; 16]),
        );
        // 000001, twenty zeros, then the secret's ones from bit 26.
        assert_eq!(phrase.words[0], 0b000_0010_0000);
        assert_eq!(phrase.words[1], 0);
        assert_eq!(phrase.words[2], 0b000_0111_1111);
        assert!(phrase.words[3..DATA].iter().all(|&w| w == 0x7ff));
    }

    #[test]
    fn fields_round_trip() {
        for (realm, hours, byte) in [
            (Realm::Test, 0, 0),
            (Realm::Live, 7_000, 0xa5),
            (Realm::Test, Created::MAX_HOURS, 0xff),
        ] {
            let phrase = Phrase::new(realm, created(hours), &secret(byte));
            assert_eq!(phrase.realm(), realm);
            assert_eq!(phrase.created(), created(hours));
            assert_eq!(phrase.secret().as_bytes(), &[byte; 16]);
            let reading = Phrase::read(
                phrase.text().as_str(),
                Positions::NONE,
                realm,
                AFTER_EVERY_PHRASE,
            )
            .unwrap();
            let Reading::Exact(read) = reading else {
                panic!("read as written")
            };
            assert_eq!(read.words, phrase.words);
        }
    }

    #[test]
    fn creation_hours_span_the_field() {
        assert_eq!(
            Created::at_unix_ms(Created::EPOCH_UNIX_MS),
            Some(created(0))
        );
        assert_eq!(Created::at_unix_ms(Created::EPOCH_UNIX_MS - 1), None);
        assert_eq!(
            Created::at_unix_ms(Created::EPOCH_UNIX_MS + 3_599_999),
            Some(created(0))
        );
        assert_eq!(
            Created::at_unix_ms(Created::EPOCH_UNIX_MS + 3_600_000),
            Some(created(1))
        );
        // 2145-08-15T15:00:00Z is the last hour.
        let last = Created::EPOCH_UNIX_MS + u64::from(Created::MAX_HOURS) * 3_600_000;
        assert_eq!(last, 5_542_095_600_000);
        assert_eq!(
            Created::at_unix_ms(last + 3_599_999),
            Some(created(Created::MAX_HOURS))
        );
        assert_eq!(Created::at_unix_ms(last + 3_600_000), None);
        assert_eq!(Created::at_unix_ms(u64::MAX), None);
        assert_eq!(Created::from_hours(Created::MAX_HOURS + 1), None);
        assert_eq!(created(5).unix_ms(), Created::EPOCH_UNIX_MS + 5 * 3_600_000);
    }

    #[test]
    fn a_time_more_than_a_day_ahead_is_refused() {
        const DAY_MS: u64 = 86_400_000;
        let hour = created(100);
        assert!(!hour.is_more_than_a_day_after(hour.unix_ms()));
        assert!(!hour.is_more_than_a_day_after(hour.unix_ms() - DAY_MS));
        assert!(hour.is_more_than_a_day_after(hour.unix_ms() - DAY_MS - 1));
        assert!(created(0).is_more_than_a_day_after(0));
        assert!(!created(Created::MAX_HOURS).is_more_than_a_day_after(u64::MAX));

        let phrase = Phrase::new(Realm::Test, hour, &secret(1));
        let text = phrase.text();
        let read = |now_ms| Phrase::read(text.as_str(), Positions::NONE, Realm::Test, now_ms);
        assert!(read(hour.unix_ms() - DAY_MS).is_ok());
        assert_eq!(
            read(hour.unix_ms() - DAY_MS - 1).map(|_| ()),
            Err(Error::CreatedInFuture { created: hour })
        );
    }

    #[test]
    fn dates_are_days_of_the_calendar() {
        assert_eq!(std::format!("{}", created(0)), "2026-01-01");
        assert_eq!(std::format!("{}", created(23)), "2026-01-01");
        assert_eq!(std::format!("{}", created(24)), "2026-01-02");
        // 2026-02-19, as the decision shows a new phrase's date.
        assert_eq!(
            std::format!("{}", created((31 + 18) * 24 + 13)),
            "2026-02-19"
        );
        // 2028 is a leap year: Feb 29, then Mar 1.
        let feb_29_2028 = (365 + 365 + 31 + 28) * 24;
        assert_eq!(created(feb_29_2028).date(), (2028, 2, 29));
        assert_eq!(created(feb_29_2028 + 24).date(), (2028, 3, 1));
        assert_eq!(created(Created::MAX_HOURS).date(), (2145, 8, 15));
        // 2100 is not a leap year.
        let mar_1_2100 =
            u32::try_from((4_107_542_400_000 - Created::EPOCH_UNIX_MS) / 3_600_000).unwrap();
        assert_eq!(created(mar_1_2100 - 1).date(), (2100, 2, 28));
        assert_eq!(created(mar_1_2100).date(), (2100, 3, 1));
    }

    #[test]
    fn positions_count_from_one_for_people() {
        let positions = Positions::NONE.with(2).with(19).with(0);
        assert_eq!(std::format!("{positions}"), "1, 3, 20");
        assert_eq!(std::format!("{positions:?}"), "[0, 2, 19]");
        assert_eq!(positions.len(), 3);
        assert!(positions.contains(19) && !positions.contains(20) && !positions.contains(1));
        assert_eq!(Positions::from_bits(1 << 20), None);
        assert_eq!(Positions::from_bits(positions.bits()), Some(positions));
    }

    #[test]
    fn secrets_are_redacted() {
        let phrase = Phrase::new(Realm::Test, created(0), &secret(9));
        assert_eq!(std::format!("{phrase:?}"), "Phrase(<redacted>)");
        assert_eq!(
            std::format!("{:?}", phrase.text()),
            "PhraseText(<redacted>)"
        );
        assert_eq!(
            std::format!("{:?}", phrase.secret()),
            "PhraseSecret(<redacted>)"
        );
        let mut text = std::string::String::from(phrase.text().as_str());
        text = text.replacen(phrase.words().next().unwrap(), "?", 1);
        let reading =
            Phrase::read(&text, Positions::NONE, Realm::Test, AFTER_EVERY_PHRASE).unwrap();
        assert_eq!(
            std::format!("{reading:?}"),
            "Corrected(Correction { changed: [0], .. })"
        );
    }

    #[test]
    fn the_longest_phrase_fits() {
        let phrase = Phrase::new(Realm::Live, created(Created::MAX_HOURS), &secret(0xff));
        assert!(phrase.text().as_str().len() <= MAX_PHRASE_LENGTH);
        assert_eq!(phrase.words().len(), PHRASE_WORDS);
    }
}

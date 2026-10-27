//! The phrase's Reed–Solomon code over GF(2¹¹)
//! (spec/decisions/phrase-format.md): 14 data words and 6 check words.
//!
//! The 20 words are the coefficients of a polynomial, the first word the
//! highest, so the word at position `i` is the coefficient of `x^(19 - i)`.
//! The generator is g(x) = (x − α)(x − α²)…(x − α⁶), and the check words
//! are the remainder of the data words, shifted up by six places, divided
//! by g(x). Every codeword is a multiple of g(x), so it vanishes at α¹ to
//! α⁶, and two codewords differ in at least seven words.
//!
//! Decoding corrects errors at unknown places and erasures at known ones:
//! Berlekamp–Massey, started from the erasures' locator, finds the
//! locator of both; a Chien search finds its roots; Forney's algorithm
//! gives the values. Whatever it finds is checked before it is taken: the
//! result must be a codeword, and within the bound the phrase format sets,
//! at most [`MAX_ERRORS`] errors and `2 × errors + erasures` at most
//! [`MAX_WEIGHT`], so one check word is always left to catch a mistake.

use zeroize::Zeroizing;

use crate::gf;

/// Words in a codeword.
pub(crate) const LENGTH: usize = 20;

/// Data words, first in a codeword.
pub(crate) const DATA: usize = 14;

/// Check words, last in a codeword.
pub(crate) const CHECK: usize = LENGTH - DATA;

/// Most errors at unknown places the decoder corrects.
pub(crate) const MAX_ERRORS: usize = 2;

/// Most `2 × errors + erasures` the decoder corrects: one less than the
/// six check words could, so one is always left to catch a mistake.
pub(crate) const MAX_WEIGHT: usize = 5;

/// Coefficients of a polynomial, lowest first. Long enough for every
/// polynomial Berlekamp–Massey forms over six syndromes: its locators have
/// degree at most six, and the shifted correction polynomial at most twelve.
const POLY: usize = 2 * CHECK + 2;
type Poly = [u16; POLY];

/// A set of positions in a codeword, as bits: bit `i` is position `i`.
pub(crate) type Mask = u32;

/// Why a word could not be decoded within the bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TooDamaged;

/// The word at `position`, or zero past the end. Positions are always in
/// range where this is used; the fallback only keeps it from panicking.
fn at(words: &[u16], position: usize) -> u16 {
    words.get(position).copied().unwrap_or_default()
}

/// Sets `poly[index]`, asserting it is in range.
// Panics only on a broken invariant: degrees stay below POLY.
#[allow(clippy::panic)]
fn set(poly: &mut [u16], index: usize, value: u16) {
    let Some(slot) = poly.get_mut(index) else {
        panic!("polynomial degree within bounds");
    };
    *slot = value;
}

/// The exponent of x at `position`.
const fn power_of(position: usize) -> usize {
    LENGTH - 1 - position
}

/// g(x), highest coefficient first: x⁶ + g₁x⁵ + … + g₆.
fn generator() -> [u16; CHECK + 1] {
    let mut product = [0u16; CHECK + 1];
    set(&mut product, 0, 1);
    for root in 1..=CHECK {
        // Multiply by (x − αʳ): each coefficient takes the one before it
        // times αʳ.
        let alpha = gf::exp(root);
        for index in (1..=root).rev() {
            let value = at(&product, index) ^ gf::mul(at(&product, index - 1), alpha);
            set(&mut product, index, value);
        }
    }
    product
}

/// The codeword of `data`: the data words, then the check words.
pub(crate) fn encode(data: &[u16; DATA]) -> [u16; LENGTH] {
    assert!(
        data.iter().all(|&word| usize::from(word) < gf::SIZE),
        "data words are field elements"
    );
    let generator = generator();
    // The remainder, highest coefficient first, by long division.
    let mut remainder = [0u16; CHECK];
    for &word in data {
        let feedback = word ^ at(&remainder, 0);
        for index in 0..CHECK {
            let next = if index + 1 < CHECK {
                at(&remainder, index + 1)
            } else {
                0
            };
            set(
                &mut remainder,
                index,
                next ^ gf::mul(feedback, at(&generator, index + 1)),
            );
        }
    }
    let mut codeword = [0u16; LENGTH];
    let (head, tail) = codeword.split_at_mut(DATA);
    head.copy_from_slice(data);
    tail.copy_from_slice(&remainder);
    codeword
}

/// S₁ … S₆: the word evaluated at α¹ … α⁶. All zero for a codeword.
fn syndromes(word: &[u16; LENGTH]) -> [u16; CHECK] {
    let mut syndromes = [0u16; CHECK];
    for (index, syndrome) in syndromes.iter_mut().enumerate() {
        let alpha = gf::exp(index + 1);
        *syndrome = word
            .iter()
            .fold(0, |sum, &coefficient| gf::mul(sum, alpha) ^ coefficient);
    }
    syndromes
}

/// Whether `word` is a codeword.
pub(crate) fn is_codeword(word: &[u16; LENGTH]) -> bool {
    syndromes(word).iter().all(|&syndrome| syndrome == 0)
}

/// p(x) at `x`.
fn evaluate(poly: &Poly, x: u16) -> u16 {
    poly.iter()
        .rev()
        .fold(0, |sum, &coefficient| gf::mul(sum, x) ^ coefficient)
}

/// The degree of `poly`, or zero for the zero polynomial.
fn degree(poly: &Poly) -> usize {
    poly.iter().rposition(|&c| c != 0).unwrap_or_default()
}

/// The erasures' locator: the product of (1 − Xₖx) over the erased
/// positions, where Xₖ = α to the exponent of x at the position.
fn erasure_locator(erasures: Mask) -> Poly {
    let mut locator = [0u16; POLY];
    set(&mut locator, 0, 1);
    for position in (0..LENGTH).filter(|&p| erasures & (1 << p) != 0) {
        let x = gf::exp(power_of(position));
        for index in (1..POLY).rev() {
            let value = at(&locator, index) ^ gf::mul(at(&locator, index - 1), x);
            set(&mut locator, index, value);
        }
    }
    locator
}

/// Berlekamp–Massey with erasures (Blahut): the locator of the errors and
/// the erasures together, and its length L, from the syndromes and the
/// erasures' locator.
fn locator(syndromes: &[u16; CHECK], erasures: Mask, count: usize) -> (Poly, usize) {
    let mut lambda = erasure_locator(erasures);
    let mut previous = lambda;
    let mut length = count;
    for step in count + 1..=CHECK {
        // The discrepancy: Σⱼ Λⱼ S_{step − j}, over the terms that exist.
        let discrepancy = (0..=length.min(step - 1)).fold(0, |sum, j| {
            sum ^ gf::mul(at(&lambda, j), at(syndromes, step - j - 1))
        });
        // x · B(x)
        let mut shifted = [0u16; POLY];
        for index in 1..POLY {
            set(&mut shifted, index, at(&previous, index - 1));
        }
        if discrepancy == 0 {
            previous = shifted;
            continue;
        }
        let mut next = lambda;
        for (index, coefficient) in next.iter_mut().enumerate() {
            *coefficient ^= gf::mul(discrepancy, at(&shifted, index));
        }
        if 2 * length < step + count {
            let inverse = gf::inverse(discrepancy);
            for (index, coefficient) in previous.iter_mut().enumerate() {
                *coefficient = gf::mul(inverse, at(&lambda, index));
            }
            length = step + count - length;
        } else {
            previous = shifted;
        }
        lambda = next;
    }
    (lambda, length)
}

/// Decodes `received`, whose words at `erasures` are unknown (their
/// values are ignored), within the bound: at most [`MAX_ERRORS`] errors,
/// and `2 × errors + erasures` at most [`MAX_WEIGHT`].
///
/// Returns the codeword and the positions whose word it changed, every
/// erasure among them; or [`TooDamaged`] when no codeword is within the
/// bound. The bound leaves the code distance to spare, so the codeword,
/// if there is one, is the only one.
pub(crate) fn decode(
    received: &[u16; LENGTH],
    erasures: Mask,
) -> Result<([u16; LENGTH], Mask), TooDamaged> {
    assert!(
        erasures >> LENGTH == 0,
        "erasures are positions in the word"
    );
    let count = erasures.count_ones() as usize;
    if count > MAX_WEIGHT {
        return Err(TooDamaged);
    }
    // The words are the phrase's, so the working copy is wiped too.
    let mut word = Zeroizing::new(*received);
    for (position, value) in word.iter_mut().enumerate() {
        assert!(usize::from(*value) < gf::SIZE, "words are field elements");
        if erasures & (1 << position) != 0 {
            *value = 0;
        }
    }
    let syndromes = syndromes(&word);
    if count == 0 && syndromes.iter().all(|&s| s == 0) {
        return Ok((*word, 0));
    }
    let (lambda, length) = locator(&syndromes, erasures, count);
    if degree(&lambda) != length || length > CHECK {
        return Err(TooDamaged);
    }
    correct(&mut word, &syndromes, &lambda)?;
    check(received, *word, erasures)
}

/// Finds the locator's roots among the positions (Chien) and corrects the
/// word there by Forney's values. Fails unless every root is a position.
fn correct(
    word: &mut [u16; LENGTH],
    syndromes: &[u16; CHECK],
    lambda: &Poly,
) -> Result<(), TooDamaged> {
    // Ω(x) = S(x) Λ(x) mod x⁶, with S(x) = S₁ + S₂x + … + S₆x⁵.
    let mut omega = [0u16; POLY];
    for (index, slot) in omega.iter_mut().enumerate().take(CHECK) {
        *slot = (0..=index).fold(0, |sum, j| {
            sum ^ gf::mul(at(syndromes, j), at(lambda, index - j))
        });
    }
    // Λ'(x): in characteristic 2 only the odd terms survive.
    let mut derivative = [0u16; POLY];
    for index in (1..POLY).step_by(2) {
        set(&mut derivative, index - 1, at(lambda, index));
    }
    let mut roots = 0;
    for (position, value) in word.iter_mut().enumerate() {
        let inverse = gf::exp(gf::ORDER - power_of(position));
        if evaluate(lambda, inverse) != 0 {
            continue;
        }
        roots += 1;
        let denominator = evaluate(&derivative, inverse);
        if denominator == 0 {
            return Err(TooDamaged);
        }
        // eₖ = −Xₖ^(1−b) Ω(Xₖ⁻¹) / Λ'(Xₖ⁻¹), with the first root b = 1.
        *value ^= gf::div(evaluate(&omega, inverse), denominator);
    }
    if roots == degree(lambda) {
        Ok(())
    } else {
        Err(TooDamaged)
    }
}

/// Takes `corrected` only if it is a codeword within the bound of
/// `received`, whatever the steps before found.
fn check(
    received: &[u16; LENGTH],
    corrected: [u16; LENGTH],
    erasures: Mask,
) -> Result<([u16; LENGTH], Mask), TooDamaged> {
    if !is_codeword(&corrected) {
        return Err(TooDamaged);
    }
    let mut changed = erasures;
    let mut errors = 0;
    for (position, (&old, &new)) in received.iter().zip(&corrected).enumerate() {
        if erasures & (1 << position) == 0 && old != new {
            changed |= 1 << position;
            errors += 1;
        }
    }
    let weight = 2 * errors + erasures.count_ones() as usize;
    if errors > MAX_ERRORS || weight > MAX_WEIGHT {
        return Err(TooDamaged);
    }
    Ok((corrected, changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic generator of test words (xorshift).
    struct Words(u64);

    impl Words {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        /// Any set of positions.
        fn mask(&mut self) -> Mask {
            Mask::try_from(self.next() & ((1 << LENGTH) - 1)).unwrap()
        }

        fn word(&mut self) -> u16 {
            u16::try_from(self.next() % 2048).unwrap()
        }

        /// A word other than `word`.
        fn other(&mut self, word: u16) -> u16 {
            word ^ u16::try_from(1 + self.next() % 2047).unwrap()
        }

        fn codeword(&mut self) -> [u16; LENGTH] {
            let mut data = [0u16; DATA];
            data.iter_mut().for_each(|word| *word = self.word());
            encode(&data)
        }
    }

    /// Every set of `size` positions out of `from`, as masks.
    fn subsets(from: Mask, size: usize) -> std::vec::Vec<Mask> {
        let positions: std::vec::Vec<usize> =
            (0..LENGTH).filter(|p| from & (1 << p) != 0).collect();
        let mut found = std::vec::Vec::new();
        if size > positions.len() {
            return found;
        }
        // Indices into `positions`, increasing, stepped like an odometer.
        let mut picks: std::vec::Vec<usize> = (0..size).collect();
        loop {
            found.push(
                picks
                    .iter()
                    .fold(0, |mask, &pick| mask | 1 << positions[pick]),
            );
            let Some(slot) = (0..size)
                .rev()
                .find(|&slot| picks[slot] < positions.len() - size + slot)
            else {
                return found;
            };
            picks[slot] += 1;
            for next in slot + 1..size {
                picks[next] = picks[next - 1] + 1;
            }
        }
    }

    /// `codeword` with every position in `errors` replaced by another
    /// word, and every position in `erasures` by any word.
    fn damage(
        words: &mut Words,
        codeword: &[u16; LENGTH],
        errors: Mask,
        erasures: Mask,
    ) -> [u16; LENGTH] {
        let mut received = *codeword;
        for (position, word) in received.iter_mut().enumerate() {
            if errors & (1 << position) != 0 {
                *word = words.other(*word);
            } else if erasures & (1 << position) != 0 {
                *word = words.word();
            }
        }
        received
    }

    #[test]
    fn the_generator_has_roots_alpha_one_to_six() {
        let generator = generator();
        assert_eq!(generator[0], 1);
        for root in 1..=CHECK {
            let x = gf::exp(root);
            let value = generator.iter().fold(0, |sum, &c| gf::mul(sum, x) ^ c);
            assert_eq!(value, 0, "g(α^{root})");
        }
    }

    #[test]
    fn codewords_are_systematic_and_vanish_at_the_roots() {
        let mut words = Words(1);
        for _ in 0..200 {
            let mut data = [0u16; DATA];
            data.iter_mut().for_each(|word| *word = words.word());
            let codeword = encode(&data);
            assert_eq!(&codeword[..DATA], &data);
            assert!(is_codeword(&codeword));
            assert_eq!(decode(&codeword, 0), Ok((codeword, 0)));
        }
        assert_eq!(encode(&[0; DATA]), [0; LENGTH]);
    }

    // A word of data changes at least seven words of the codeword: the
    // code's distance, on the codewords of one data word each.
    #[test]
    fn one_data_word_moves_seven_words() {
        for position in 0..DATA {
            for value in [1u16, 2, 0x400, 0x7ff] {
                let mut data = [0u16; DATA];
                data[position] = value;
                let weight = encode(&data).iter().filter(|&&w| w != 0).count();
                assert!(weight > CHECK, "weight {weight}");
            }
        }
    }

    // Every placement of errors and erasures within the bound, each with
    // random words, is corrected, and the changed positions are exactly
    // the damaged ones.
    #[test]
    fn every_damage_within_the_bound_is_corrected() {
        let mut words = Words(0x5eed);
        let all: Mask = (1 << LENGTH) - 1;
        let mut cases = 0;
        for erasure_count in 0..=MAX_WEIGHT {
            for erasures in subsets(all, erasure_count) {
                let error_count = MAX_ERRORS.min((MAX_WEIGHT - erasure_count) / 2);
                for errors in (0..=error_count).flat_map(|n| subsets(all & !erasures, n)) {
                    let codeword = words.codeword();
                    let received = damage(&mut words, &codeword, errors, erasures);
                    assert_eq!(
                        decode(&received, erasures),
                        Ok((codeword, errors | erasures)),
                        "errors {errors:#x}, erasures {erasures:#x}"
                    );
                    cases += 1;
                }
            }
        }
        assert!(cases > 40_000, "{cases}");
    }

    // One step past the bound, a word is always refused: no codeword is
    // within the bound of it, since the one it came from is at distance 7
    // from every other.
    #[test]
    fn every_damage_one_past_the_bound_is_refused() {
        let mut words = Words(0xdead);
        let all: Mask = (1 << LENGTH) - 1;
        for (error_count, erasure_count) in [(3, 0), (2, 2), (1, 4), (0, 6), (3, 1)] {
            for erasures in subsets(all, erasure_count) {
                for errors in subsets(all & !erasures, error_count) {
                    let codeword = words.codeword();
                    let received = damage(&mut words, &codeword, errors, erasures);
                    assert_eq!(
                        decode(&received, erasures),
                        Err(TooDamaged),
                        "errors {errors:#x}, erasures {erasures:#x}"
                    );
                }
            }
        }
    }

    // Further damage is mostly refused; whatever is taken is a codeword
    // within the bound of what was read.
    #[test]
    fn heavier_damage_is_refused_or_lands_within_the_bound() {
        let mut words = Words(0xbeef);
        let mut refused = 0;
        for round in 0..20_000u32 {
            let codeword = words.codeword();
            let erasures = words.mask() & if round % 2 == 0 { 0 } else { 0x0f0f };
            let errors = words.mask() & !erasures;
            let received = damage(&mut words, &codeword, errors, erasures);
            match decode(&received, erasures) {
                Ok((decoded, changed)) => {
                    assert!(is_codeword(&decoded));
                    let unknown = (changed & !erasures).count_ones() as usize;
                    assert!(unknown <= MAX_ERRORS);
                    assert!(2 * unknown + erasures.count_ones() as usize <= MAX_WEIGHT);
                }
                Err(TooDamaged) => refused += 1,
            }
        }
        assert!(refused > 19_900, "{refused}");
    }
}

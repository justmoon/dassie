//! GF(2¹¹), the field a phrase's words live in
//! (spec/decisions/phrase-format.md): every word is the index of its place
//! in the wordlist, an element of the field, and the field is built from
//! the primitive polynomial x¹¹ + x² + 1 with α = x.
//!
//! Elements are `u16`s below [`SIZE`]. Multiplication and division go
//! through log and antilog tables built at compile time.

/// Elements in the field: one per word of the list.
pub(crate) const SIZE: usize = 2048;

/// The order of α, which generates the multiplicative group.
pub(crate) const ORDER: usize = SIZE - 1;

/// x¹¹ + x² + 1.
const POLYNOMIAL: u16 = (1 << 11) | (1 << 2) | 1;

/// `exp[i]` is αⁱ; `log[a]` is the log of `a`, and `log[0]` is unused.
struct Tables {
    exp: [u16; ORDER],
    log: [u16; SIZE],
}

static TABLES: Tables = build();

// Const evaluation fails on an out-of-bounds index or a truncating cast
// that loses bits, so neither can happen at run time: the loop stays
// below ORDER, and every element is below SIZE.
#[allow(clippy::indexing_slicing, clippy::cast_possible_truncation)]
const fn build() -> Tables {
    let mut exp = [0u16; ORDER];
    let mut log = [0u16; SIZE];
    let mut element: u16 = 1;
    let mut power = 0;
    while power < ORDER {
        exp[power] = element;
        log[element as usize] = power as u16;
        element <<= 1;
        if element & (1 << 11) != 0 {
            element ^= POLYNOMIAL;
        }
        power += 1;
    }
    Tables { exp, log }
}

/// αⁱ, for any `power`.
// Panics only on a broken invariant: the index is reduced below ORDER.
#[allow(clippy::panic)]
pub(crate) fn exp(power: usize) -> u16 {
    let Some(&element) = TABLES.exp.get(power % ORDER) else {
        panic!("a reduced power is within the table");
    };
    element
}

/// The log of a nonzero `element`.
// Panics only on a broken invariant: callers never take the log of zero,
// and elements are below SIZE.
#[allow(clippy::panic)]
fn log(element: u16) -> usize {
    assert!(element != 0, "zero has no log");
    let Some(&power) = TABLES.log.get(usize::from(element)) else {
        panic!("a field element is below {SIZE}");
    };
    usize::from(power)
}

/// `a × b`.
pub(crate) fn mul(a: u16, b: u16) -> u16 {
    if a == 0 || b == 0 {
        return 0;
    }
    exp(log(a) + log(b))
}

/// `a ÷ b`, for a nonzero `b`.
pub(crate) fn div(a: u16, b: u16) -> u16 {
    assert!(b != 0, "division by zero in GF(2^11)");
    if a == 0 {
        return 0;
    }
    exp(log(a) + ORDER - log(b))
}

/// `a⁻¹`, for a nonzero `a`.
pub(crate) fn inverse(a: u16) -> u16 {
    div(1, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    // x¹¹ + x² + 1 is primitive: α has order 2047 = 23 × 89, so it reaches
    // every nonzero element, each once.
    #[test]
    fn alpha_generates_the_field() {
        assert_eq!(exp(0), 1);
        assert_eq!(exp(ORDER), 1);
        assert_ne!(exp(ORDER / 23), 1);
        assert_ne!(exp(ORDER / 89), 1);
        let mut seen = [false; SIZE];
        for power in 0..ORDER {
            let element = exp(power);
            assert!(usize::from(element) < SIZE && element != 0);
            assert!(!seen[usize::from(element)], "α^{power} repeats");
            seen[usize::from(element)] = true;
            assert_eq!(log(element), power);
        }
    }

    // α¹¹ = α² + 1 is what the polynomial says.
    #[test]
    fn alpha_is_x() {
        assert_eq!(exp(1), 2);
        assert_eq!(exp(10), 1 << 10);
        assert_eq!(exp(11), 0b101);
    }

    #[test]
    fn arithmetic_agrees_with_carryless_multiplication() {
        // Shift-and-add multiplication reduced by the polynomial, the
        // definition the tables must match.
        fn slow(a: u16, b: u16) -> u16 {
            let mut product: u32 = 0;
            for bit in 0..11 {
                if (b >> bit) & 1 == 1 {
                    product ^= u32::from(a) << bit;
                }
            }
            for bit in (11..22).rev() {
                if (product >> bit) & 1 == 1 {
                    product ^= u32::from(POLYNOMIAL) << (bit - 11);
                }
            }
            u16::try_from(product).unwrap()
        }
        for a in (0..2048u16).step_by(7) {
            for b in (0..2048u16).step_by(13) {
                assert_eq!(mul(a, b), slow(a, b), "{a} × {b}");
                if b != 0 {
                    assert_eq!(mul(div(a, b), b), a);
                }
            }
            if a != 0 {
                assert_eq!(mul(a, inverse(a)), 1);
            }
        }
    }
}

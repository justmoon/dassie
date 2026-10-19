/// An inclusive size constraint, in octets or in characters depending on the
/// type it is applied to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    /// Smallest permitted size.
    pub min: usize,
    /// Largest permitted size.
    pub max: usize,
}

impl Size {
    /// No constraint beyond what fits in memory.
    pub const ANY: Self = Self {
        min: 0,
        max: usize::MAX,
    };

    /// Exactly `size`.
    #[must_use]
    pub const fn exact(size: usize) -> Self {
        Self {
            min: size,
            max: size,
        }
    }

    /// From `min` to `max`, both inclusive.
    #[must_use]
    pub const fn range(min: usize, max: usize) -> Self {
        assert!(min <= max, "size range minimum exceeds maximum");
        Self { min, max }
    }

    /// Whether `size` satisfies the constraint.
    #[must_use]
    pub const fn contains(self, size: usize) -> bool {
        size >= self.min && size <= self.max
    }
}

/// A set of permitted characters for a 7-bit string type (IA5String,
/// VisibleString and restrictions of them), as a bit mask over code points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alphabet(u128);

impl Alphabet {
    /// IA5String: all of 7-bit ASCII.
    pub const IA5: Self = Self(u128::MAX);

    /// VisibleString: printable ASCII, `0x20` to `0x7E`. The original left out
    /// `~` (spec/original/04-interledger.md#25-strings-stringts, NET-30).
    pub const VISIBLE: Self = Self::EMPTY.with_range(0x20, 0x7e);

    /// The empty alphabet, to build others from.
    pub const EMPTY: Self = Self(0);

    /// Adds the inclusive range `from..=to`, both 7-bit code points.
    #[must_use]
    pub const fn with_range(self, from: u8, to: u8) -> Self {
        assert!(from <= to && to < 0x80, "alphabet range must be 7-bit");
        let mut mask = self.0;
        let mut code = from;
        loop {
            mask |= 1 << code;
            if code == to {
                break;
            }
            code += 1;
        }
        Self(mask)
    }

    /// Adds every character of `chars`, which must be 7-bit.
    #[must_use]
    pub const fn with_chars(self, chars: &[u8]) -> Self {
        let mut mask = self.0;
        let mut rest = chars;
        while let [code, tail @ ..] = rest {
            assert!(*code < 0x80, "alphabet characters must be 7-bit");
            mask |= 1 << *code;
            rest = tail;
        }
        Self(mask)
    }

    /// Whether `byte` is in the alphabet. Bytes of `0x80` and above never are.
    #[must_use]
    pub const fn contains(self, byte: u8) -> bool {
        byte < 0x80 && self.0 & (1 << byte) != 0
    }

    /// Whether every byte of `bytes` is in the alphabet.
    #[must_use]
    pub fn contains_all(self, bytes: &[u8]) -> bool {
        bytes.iter().all(|&byte| self.contains(byte))
    }
}

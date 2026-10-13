//! A seeded RNG for tests and simulations.

use core::convert::Infallible;

use rand_core::{TryCryptoRng, TryRng};

/// xoshiro256**, seeded with SplitMix64. Fast and reproducible; **not** a
/// cryptographic generator, despite implementing [`TryCryptoRng`] so that
/// it can stand in for one in tests.
#[derive(Clone, Debug)]
pub struct SeededRng {
    state: [u64; 4],
}

impl SeededRng {
    /// A generator whose whole output follows from `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut mix = seed;
        let mut next = || {
            mix = mix.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = mix;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        Self {
            state: [next(), next(), next(), next()],
        }
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        let [a, b, c, d] = &mut self.state;
        let result = b.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = *b << 17;
        *c ^= *a;
        *d ^= *b;
        *b ^= *c;
        *a ^= *d;
        *c ^= t;
        *d = d.rotate_left(45);
        result
    }
}

impl TryRng for SeededRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        // The high half, which is the better half of xoshiro's output.
        Ok(u32::try_from(self.next_u64() >> 32).unwrap_or_default())
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        Ok(self.next_u64())
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), Infallible> {
        for chunk in destination.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            for (out, byte) in chunk.iter_mut().zip(bytes) {
                *out = byte;
            }
        }
        Ok(())
    }
}

impl TryCryptoRng for SeededRng {}

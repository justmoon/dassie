//! Shared plumbing for secret and public byte strings.

use core::fmt;

/// Defines a fixed-size secret newtype.
///
/// Every secret in the crate gets the same treatment, so it lives in one
/// place: the bytes are wiped on drop, `Debug` never prints them, the type is
/// not `Copy` (copies would escape the wipe), and equality is only offered in
/// constant time through [`subtle::ConstantTimeEq`] so that no caller compares
/// secrets with an early-exit `==`.
macro_rules! secret_bytes {
    ($(#[$meta:meta])* $name:ident, $length:expr) => {
        $(#[$meta])*
        pub struct $name([u8; $length]);

        impl $name {
            /// Length in bytes.
            pub const LENGTH: usize = $length;

            /// Wraps raw secret bytes.
            #[must_use]
            pub const fn from_bytes(bytes: [u8; $length]) -> Self {
                Self(bytes)
            }

            /// The raw secret bytes.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $length] {
                &self.0
            }
        }

        impl Drop for $name {
            fn drop(&mut self) {
                zeroize::Zeroize::zeroize(&mut self.0);
            }
        }

        impl zeroize::ZeroizeOnDrop for $name {}

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }

        impl subtle::ConstantTimeEq for $name {
            fn ct_eq(&self, other: &Self) -> subtle::Choice {
                subtle::ConstantTimeEq::ct_eq(self.0.as_slice(), other.0.as_slice())
            }
        }
    };
}

pub(crate) use secret_bytes;

/// Formats public bytes as lowercase hex in `Debug` output, which is far
/// easier to compare against vectors and logs than a list of decimals.
pub(crate) struct Hex<'a>(pub(crate) &'a [u8]);

impl fmt::Debug for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

//! Base58Check in the XRP Ledger's alphabet: addresses and family seeds.

use alloc::string::String;
use alloc::vec::Vec;

use sha2::{Digest, Sha256};

/// The ledger's alphabet: `r` is zero, so addresses start with it.
const ALPHABET: &[u8; 58] = b"rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz";

/// The digit zero.
const ZERO: u8 = b'r';

/// The most bytes this encodes or decodes: a version, a payload of at most
/// 32 bytes, and the checksum.
const MAX_BYTES: usize = 40;

/// The first four bytes of SHA-256 of SHA-256.
fn checksum(bytes: &[u8]) -> [u8; 4] {
    let twice = Sha256::digest(Sha256::digest(bytes));
    let mut out = [0; 4];
    out.copy_from_slice(twice.get(..4).unwrap_or(&[0; 4]));
    out
}

/// `version ‖ payload ‖ checksum`, in base 58. At most [`MAX_BYTES`] in
/// all; beyond that it is a bug.
#[must_use]
pub fn encode_check(version: &[u8], payload: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(version.len() + payload.len() + 4);
    bytes.extend_from_slice(version);
    bytes.extend_from_slice(payload);
    let sum = checksum(&bytes);
    bytes.extend_from_slice(&sum);
    assert!(bytes.len() <= MAX_BYTES, "base58 input too long");
    encode(&bytes)
}

/// The payload of `text`, if it decodes, has a valid checksum, and starts
/// with `version`.
#[must_use]
pub fn decode_check(text: &str, version: &[u8]) -> Option<Vec<u8>> {
    let bytes = decode(text)?;
    let split = bytes.len().checked_sub(4)?;
    let (body, sum) = bytes.split_at(split);
    if checksum(body) != sum {
        return None;
    }
    body.strip_prefix(version).map(<[u8]>::to_vec)
}

fn encode(bytes: &[u8]) -> String {
    // Each leading zero byte is one leading zero digit.
    let zeros = bytes.iter().take_while(|byte| **byte == 0).count();
    // Base 58 digits, least significant first: log(256) / log(58) < 1.37.
    let mut digits: Vec<u8> = Vec::with_capacity(bytes.len() * 137 / 100 + 1);
    for byte in bytes.get(zeros..).unwrap_or(&[]) {
        let mut carry = u32::from(*byte);
        for digit in &mut digits {
            carry += u32::from(*digit) << 8;
            *digit = u8::try_from(carry % 58).unwrap_or(0);
            carry /= 58;
        }
        while carry > 0 {
            digits.push(u8::try_from(carry % 58).unwrap_or(0));
            carry /= 58;
        }
    }
    let mut text = String::with_capacity(zeros + digits.len());
    for _ in 0..zeros {
        text.push(char::from(ZERO));
    }
    for digit in digits.iter().rev() {
        // Every digit is below 58.
        text.push(char::from(
            ALPHABET.get(usize::from(*digit)).copied().unwrap_or(ZERO),
        ));
    }
    text
}

fn decode(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.len() > MAX_BYTES * 2 {
        return None;
    }
    let zeros = text.bytes().take_while(|byte| *byte == ZERO).count();
    let mut bytes: Vec<u8> = Vec::with_capacity(text.len());
    for character in text.bytes().skip(zeros) {
        let value = ALPHABET.iter().position(|a| *a == character)?;
        let mut carry = u32::try_from(value).ok()?;
        for byte in &mut bytes {
            carry += u32::from(*byte) * 58;
            *byte = u8::try_from(carry & 0xff).ok()?;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.push(u8::try_from(carry & 0xff).ok()?);
            carry >>= 8;
        }
    }
    let mut out = alloc::vec![0; zeros];
    out.extend(bytes.iter().rev());
    (out.len() <= MAX_BYTES).then_some(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn known_addresses_round_trip() {
        // The genesis account, and an all-zero account ID.
        let genesis = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
        let payload = decode_check(genesis, &[0]).unwrap();
        assert_eq!(payload.len(), 20);
        assert_eq!(encode_check(&[0], &payload), genesis);
        assert_eq!(encode_check(&[0], &[0; 20]), "rrrrrrrrrrrrrrrrrrrrrhoLvTp");
    }

    #[test]
    fn a_bad_checksum_or_version_is_refused() {
        let genesis = "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh";
        assert!(decode_check("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTi", &[0]).is_none());
        assert!(decode_check(genesis, &[1]).is_none());
        assert!(decode_check("0OIl", &[0]).is_none());
        assert!(decode_check("", &[0]).is_none());
    }
}

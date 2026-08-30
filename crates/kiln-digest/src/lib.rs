//! Kiln's stable, dependency-free SHA-256 value.
//!
//! Strong digests are used only where digest equality is the whole reuse/coherence decision.
//! Keeping the concrete implementation in this small crate lets foundational protocol code and
//! leaf artifact-cache code share one pinned algorithm without pulling generic hashing frameworks
//! or build scripts onto the compiler's rebuild path.

use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Width of a SHA-256 digest in bytes.
pub const DIGEST_BYTES: usize = 32;

/// A stable SHA-256 digest used for source coherence and content-addressed artifacts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Digest([u8; DIGEST_BYTES]);

impl Digest {
    /// Hash arbitrary bytes with the stable SHA-256 implementation.
    pub fn hash(bytes: &[u8]) -> Self {
        Self(sha256(bytes))
    }

    /// Construct a digest from its raw bytes.
    pub const fn from_bytes(bytes: [u8; DIGEST_BYTES]) -> Self {
        Self(bytes)
    }

    /// Return the raw digest bytes.
    pub const fn as_bytes(&self) -> &[u8; DIGEST_BYTES] {
        &self.0
    }

    /// Render the digest as lowercase hexadecimal.
    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(DIGEST_BYTES * 2);
        for byte in self.0 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }

    /// Parse a lowercase or uppercase hexadecimal digest.
    pub fn from_hex(value: &str) -> Result<Self, DigestParseError> {
        if value.len() != DIGEST_BYTES * 2 {
            return Err(DigestParseError {
                message: format!("expected {} hexadecimal characters", DIGEST_BYTES * 2),
            });
        }
        let bytes = value.as_bytes();
        let mut digest = [0; DIGEST_BYTES];
        for (index, output) in digest.iter_mut().enumerate() {
            let high = hex_value(bytes[index * 2]).ok_or_else(|| DigestParseError {
                message: format!("invalid hexadecimal character at position {}", index * 2),
            })?;
            let low = hex_value(bytes[index * 2 + 1]).ok_or_else(|| DigestParseError {
                message: format!(
                    "invalid hexadecimal character at position {}",
                    index * 2 + 1
                ),
            })?;
            *output = (high << 4) | low;
        }
        Ok(Self(digest))
    }
}

impl Display for Digest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&(*self).to_hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Digest")
            .field(&self.to_hex())
            .finish()
    }
}

/// An invalid textual digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestParseError {
    message: String,
}

impl Display for DigestParseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DigestParseError {}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

// FIPS 180-4's 512-bit block schedule, written directly to keep this foundational primitive
// dependency-free and straightforward to audit.
fn sha256(input: &[u8]) -> [u8; DIGEST_BYTES] {
    const INITIAL: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let padded_len = input.len() + 1 + ((64 - ((input.len() + 1 + 8) % 64)) % 64) + 8;
    let mut padded = Vec::with_capacity(padded_len);
    padded.extend_from_slice(input);
    padded.push(0x80);
    padded.resize(padded_len - 8, 0);
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut state = INITIAL;
    for block in padded.chunks_exact(64) {
        let mut schedule = [0u32; 64];
        for (index, word) in schedule[..16].iter_mut().enumerate() {
            let start = index * 4;
            *word = u32::from_be_bytes([
                block[start],
                block[start + 1],
                block[start + 2],
                block[start + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = schedule[index - 15].rotate_right(7)
                ^ schedule[index - 15].rotate_right(18)
                ^ (schedule[index - 15] >> 3);
            let s1 = schedule[index - 2].rotate_right(17)
                ^ schedule[index - 2].rotate_right(19)
                ^ (schedule[index - 2] >> 10);
            schedule[index] = schedule[index - 16]
                .wrapping_add(s0)
                .wrapping_add(schedule[index - 7])
                .wrapping_add(s1);
        }

        let mut working = state;
        for index in 0..64 {
            let [a, b, c, d, e, f, g, h] = working;
            let big_sigma1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(big_sigma1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(schedule[index]);
            let big_sigma0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big_sigma0.wrapping_add(majority);
            working = [
                temp1.wrapping_add(temp2),
                a,
                b,
                c,
                d.wrapping_add(temp1),
                e,
                f,
                g,
            ];
        }
        for (slot, value) in state.iter_mut().zip(working) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut output = [0; DIGEST_BYTES];
    for (index, word) in state.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_reference_vectors() {
        assert_eq!(
            Digest::hash(b"").to_hex(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            Digest::hash(b"abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let digest = Digest::hash(b"Kiln");
        assert_eq!(
            Digest::from_hex(&digest.to_hex()).expect("round trip"),
            digest
        );
        assert!(Digest::from_hex("nope").is_err());
    }
}

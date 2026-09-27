//! Kiln's stable SHA-256 value, backed by the workspace-pinned RustCrypto implementation.
//!
//! Strong digests are used only where digest equality is the whole reuse, coherence, or identity
//! decision.
//! Keeping the concrete digest type here lets protocol and artifact-cache code share one pinned
//! algorithm and output format. `sha2` replaces the original scalar implementation after release
//! measurements showed 6.3–6.6x higher throughput on inputs from 4 KiB through 16 MiB. The
//! dependency adds several small RustCrypto crates; `generic-array` runs a small version-check
//! build script. That rebuild cost is lower than spending a measurable part of every warm native
//! build hashing large generated C and cached artifacts.

use sha2::{Digest as _, Sha256};

use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Width of a SHA-256 digest in bytes.
pub const DIGEST_BYTES: usize = 32;

/// A stable SHA-256 digest used for source coherence, identity, and content-addressed artifacts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Digest([u8; DIGEST_BYTES]);

impl Digest {
    /// Hash arbitrary bytes with the stable SHA-256 implementation.
    pub fn hash(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
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

    /// Render every digest bit as unpadded URL-safe Base64.
    ///
    /// This is the compact filesystem-safe spelling: a SHA-256 value is always 43 characters,
    /// versus 64 in hexadecimal. It remains a representation of the full digest rather than a
    /// truncation, so it is suitable where path length is constrained and equality is the whole
    /// identity decision.
    pub fn to_base64_url(self) -> String {
        let mut output = String::with_capacity(43);
        let (chunks, remainder) = self.0.as_chunks::<3>();
        for chunk in chunks {
            output.push(BASE64_URL[(chunk[0] >> 2) as usize] as char);
            output.push(BASE64_URL[(((chunk[0] & 0x03) << 4) | (chunk[1] >> 4)) as usize] as char);
            output.push(BASE64_URL[(((chunk[1] & 0x0f) << 2) | (chunk[2] >> 6)) as usize] as char);
            output.push(BASE64_URL[(chunk[2] & 0x3f) as usize] as char);
        }
        if let [first, second] = remainder {
            output.push(BASE64_URL[(first >> 2) as usize] as char);
            output.push(BASE64_URL[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
            output.push(BASE64_URL[((second & 0x0f) << 2) as usize] as char);
        }
        debug_assert_eq!(output.len(), 43);
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
const BASE64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

// The previous in-crate implementation remains test-only as an independent compatibility oracle.
// Production hashing has one maintained implementation above; the digest bytes and wire/cache
// identity remain the same full-width FIPS 180-4 SHA-256 value.
#[cfg(test)]
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
    for block in padded.as_chunks::<64>().0 {
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

    #[test]
    fn compact_spelling_is_full_width_url_safe_base64() {
        let digest = Digest::hash(b"abc");
        assert_eq!(
            digest.to_base64_url(),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
        assert_eq!(digest.to_base64_url().len(), 43);
        assert!(
            digest
                .to_base64_url()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
    }

    /// The two-block NIST vectors, which are the first tests a correct compression loop can pass
    /// but a single-block-only one cannot.
    ///
    /// The short vectors above never leave the first block, so they exercise the round function and
    /// nothing else: the block loop runs once, the chaining variables are never fed forward, and the
    /// message schedule is only ever built from a block that already contains the padding. A
    /// mistake in carrying `state` between blocks — or in extending the schedule from the previous
    /// block's words — is invisible until the input spans two blocks. The 56-byte vector spans two
    /// blocks because its padding does not fit; the 112-byte one spans three for the same reason.
    #[test]
    fn sha256_matches_multi_block_reference_vectors() {
        assert_eq!(
            Digest::hash(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").to_hex(),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            Digest::hash(
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmno\
                  ijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
            )
            .to_hex(),
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1"
        );
    }

    /// Lengths that straddle every point where the padding block does or does not fit.
    ///
    /// FIPS 180-4 appends a single `0x80` byte, then zeroes, then a 64-bit big-endian bit count, and
    /// the result must land on a 64-byte boundary. That leaves only 55 usable bytes in a block that
    /// still has room for its own length field. At 56 the length field no longer fits and an entire
    /// extra block appears; at 64 the message fills a block exactly and the padding block is
    /// all-padding; at 65 the overflow block begins again. Each of these is a distinct arm of the
    /// `padded_len` arithmetic, and an off-by-one there is the single most common defect in a
    /// hand-written SHA-256 — it produces a plausible-looking digest for most inputs and a wrong one
    /// for a narrow band of lengths, which is precisely the shape of failure that would let the
    /// artifact cache silently hand back the wrong build.
    ///
    /// Every expected value here was computed with `shasum -a 256` rather than transcribed.
    #[test]
    fn sha256_covers_the_message_length_padding_boundary() {
        let repeated = |count: usize| Digest::hash(&vec![b'a'; count]).to_hex();

        // 55 bytes: the largest message whose padding still fits in its own block.
        assert_eq!(
            repeated(55),
            "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318"
        );
        // 56 bytes: one byte past the boundary, so the length field is pushed into a second block.
        assert_eq!(
            repeated(56),
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a"
        );
        // 63 bytes: one byte short of a full block, so only the `0x80` terminator fits.
        assert_eq!(
            repeated(63),
            "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34"
        );
        // 64 bytes: an exact block, followed by a block that is nothing but padding.
        assert_eq!(
            repeated(64),
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );
        // 65 bytes: the cycle restarts, with one message byte in the second block.
        assert_eq!(
            repeated(65),
            "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0"
        );
    }

    #[test]
    fn maintained_sha256_matches_the_previous_implementation_across_lengths() {
        let lengths =
            (0..=257).chain([4095, 4096, 4097, 55_999, 65_535, 65_536, 65_537, 1_000_000]);
        for length in lengths {
            let bytes = (0_usize..length)
                .map(|index| (index.wrapping_mul(31) ^ (index >> 3)) as u8)
                .collect::<Vec<_>>();
            assert_eq!(
                Digest::hash(&bytes).as_bytes(),
                &sha256(&bytes),
                "digest differs at input length {length}"
            );
        }
    }

    /// The one-million-'a' vector, which is the only cheap check on the 64-bit length field.
    ///
    /// Eight million bits does not fit in 32, so an implementation that narrowed the bit count
    /// anywhere — or that accumulated it in a type that wraps sooner than `u64` — passes every
    /// vector above and fails this one. It runs unconditionally rather than behind `#[ignore]`:
    /// 15,625 blocks costs well under a tenth of a second even in a debug build, which is far too
    /// cheap to trade for the one check that the length field is genuinely 64 bits wide.
    #[test]
    fn sha256_matches_the_million_character_vector() {
        assert_eq!(
            Digest::hash(&vec![b'a'; 1_000_000]).to_hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// The textual spellings must survive a multi-block digest, not just a short one.
    ///
    /// The existing round trips all run on digests of single-block inputs, which is an accident of
    /// convenience rather than coverage: `to_hex`, `from_hex`, and `to_base64_url` only ever see the
    /// 32-byte output, so what matters is that a digest whose bytes came from a longer message still
    /// survives the trip unchanged. Pinning the Base64 spelling against a value derived from
    /// `shasum` output keeps the compact form honest — it is a re-encoding of all 256 bits, not a
    /// truncation that would quietly weaken content addressing.
    #[test]
    fn textual_spellings_round_trip_multi_block_digests() {
        let digest = Digest::hash(
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmno\
              ijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
        );
        assert_eq!(
            Digest::from_hex(&digest.to_hex()).expect("round trip"),
            digest
        );
        assert_eq!(
            Digest::from_hex(&digest.to_hex().to_uppercase()).expect("round trip"),
            digest
        );
        assert_eq!(
            digest.to_base64_url(),
            "z1sWp3ivg4ADbOWeewSSNwskmxHo8HpRr6xFA3r-6dE"
        );
        assert_eq!(digest.to_base64_url().len(), 43);

        let long = Digest::hash(&vec![b'a'; 1_000]);
        assert_eq!(Digest::from_hex(&long.to_hex()).expect("round trip"), long);
        assert_eq!(long.to_base64_url().len(), 43);
        assert_ne!(long.to_base64_url(), digest.to_base64_url());
    }
}

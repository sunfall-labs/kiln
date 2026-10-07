//! Shared cryptographic operations used by both kiln execution engines.
//!
//! The JavaScript facade owns WebCrypto's validation, promises and `CryptoKey` objects. This
//! crate receives only byte slices for one operation, so neither engine stores algorithm state in
//! a second implementation or exports a key handle across its boundary.

use aes::cipher::consts::U12;
use aes_gcm::{
    Aes128Gcm, Aes256Gcm, AesGcm, Nonce,
    aead::{AeadInPlace, KeyInit},
};
use ed25519_dalek::{
    Signature, Signer, SigningKey, Verifier, VerifyingKey,
    pkcs8::{DecodePrivateKey, EncodePrivateKey},
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type Aes192Gcm = AesGcm<aes::Aes192, U12>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidLength,
    InvalidKey,
    AuthenticationFailed,
    RandomUnavailable,
    UnsupportedOperation,
}

/// `__kilnWebCryptoPrimitive` operation numbers shared by the TypeScript facade and private ABI.
pub mod operation {
    pub const SHA256: u32 = 1;
    pub const HKDF_SHA256: u32 = 2;
    pub const AES_GCM_ENCRYPT: u32 = 3;
    pub const AES_GCM_DECRYPT: u32 = 4;
    pub const ED25519_GENERATE: u32 = 5;
    pub const ED25519_PUBLIC_FROM_PKCS8: u32 = 6;
    pub const ED25519_SIGN: u32 = 7;
    pub const ED25519_VERIFY: u32 = 8;
    pub const ED25519_VALIDATE_PUBLIC: u32 = 9;
    pub const HMAC_SHA256: u32 = 10;
    pub const HMAC_SHA256_VERIFY: u32 = 11;
    pub const RANDOM_BYTES: u32 = 12;
}

/// Execute one WebCrypto primitive. `key`, `data`, `iv`, and `additional_data` are copied or
/// consumed during this call and are never retained. `length` is in bytes for HKDF and unused by
/// other operations.
pub fn execute(
    op: u32,
    key: &[u8],
    data: &[u8],
    iv: &[u8],
    additional_data: &[u8],
    length: usize,
) -> Result<Vec<u8>, Error> {
    match op {
        operation::SHA256 => Ok(sha256(data).to_vec()),
        operation::HKDF_SHA256 => hkdf_sha256(key, data, iv, length),
        operation::AES_GCM_ENCRYPT => aes_gcm_encrypt(key, iv, additional_data, data),
        operation::AES_GCM_DECRYPT => aes_gcm_decrypt(key, iv, additional_data, data),
        operation::ED25519_GENERATE => {
            let (private, public) = ed25519_generate()?;
            let private_length = u16::try_from(private.len()).map_err(|_| Error::InvalidKey)?;
            let mut result = Vec::with_capacity(2 + private.len() + public.len());
            result.extend_from_slice(&private_length.to_be_bytes());
            result.extend_from_slice(&private);
            result.extend_from_slice(&public);
            Ok(result)
        }
        operation::ED25519_PUBLIC_FROM_PKCS8 => Ok(ed25519_public_from_pkcs8(key)?.to_vec()),
        operation::ED25519_SIGN => Ok(ed25519_sign(key, data)?.to_vec()),
        operation::ED25519_VERIFY => {
            if iv.len() != 64 {
                Ok(vec![0])
            } else {
                Ok(vec![u8::from(ed25519_verify(key, data, iv)?)])
            }
        }
        operation::ED25519_VALIDATE_PUBLIC => {
            let public: &[u8; 32] = key.try_into().map_err(|_| Error::InvalidKey)?;
            VerifyingKey::from_bytes(public).map_err(|_| Error::InvalidKey)?;
            Ok(Vec::new())
        }
        operation::HMAC_SHA256 => {
            let mut mac =
                <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| Error::InvalidKey)?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }
        operation::HMAC_SHA256_VERIFY => {
            let mut mac =
                <Hmac<Sha256> as Mac>::new_from_slice(key).map_err(|_| Error::InvalidKey)?;
            mac.update(data);
            Ok(vec![u8::from(mac.verify_slice(iv).is_ok())])
        }
        operation::RANDOM_BYTES => {
            if length > 65_536 {
                return Err(Error::InvalidLength);
            }
            let mut bytes = vec![0; length];
            getrandom::fill(&mut bytes).map_err(|_| Error::RandomUnavailable)?;
            Ok(bytes)
        }
        _ => Err(Error::UnsupportedOperation),
    }
}

/// SHA-256 digest of a byte sequence.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// HKDF with SHA-256, as specified by RFC 5869.
pub fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8], length: usize) -> Result<Vec<u8>, Error> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut output = vec![0; length];
    hk.expand(info, &mut output)
        .map_err(|_| Error::InvalidLength)?;
    Ok(output)
}

/// AES-GCM with a 96-bit IV and 128-bit authentication tag.
///
/// The WebCrypto facade validates algorithm dictionaries and supported key lengths before it
/// calls this function. Ciphertext is returned with the authentication tag appended, as WebCrypto
/// requires.
pub fn aes_gcm_encrypt(
    key: &[u8],
    iv: &[u8],
    additional_data: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, Error> {
    let nonce = nonce(iv)?;
    let mut output = plaintext.to_vec();
    let tag = match key.len() {
        16 => Aes128Gcm::new_from_slice(key)
            .map_err(|_| Error::InvalidKey)?
            .encrypt_in_place_detached(nonce, additional_data, &mut output)
            .map_err(|_| Error::InvalidLength)?,
        24 => Aes192Gcm::new_from_slice(key)
            .map_err(|_| Error::InvalidKey)?
            .encrypt_in_place_detached(nonce, additional_data, &mut output)
            .map_err(|_| Error::InvalidLength)?,
        32 => Aes256Gcm::new_from_slice(key)
            .map_err(|_| Error::InvalidKey)?
            .encrypt_in_place_detached(nonce, additional_data, &mut output)
            .map_err(|_| Error::InvalidLength)?,
        _ => return Err(Error::InvalidKey),
    };
    output.extend_from_slice(tag.as_slice());
    Ok(output)
}

/// Authenticate and decrypt AES-GCM ciphertext whose last 16 bytes are the tag.
pub fn aes_gcm_decrypt(
    key: &[u8],
    iv: &[u8],
    additional_data: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, Error> {
    if ciphertext.len() < 16 {
        return Err(Error::AuthenticationFailed);
    }
    let nonce = nonce(iv)?;
    let split = ciphertext.len() - 16;
    let mut output = ciphertext[..split].to_vec();
    let tag = aes_gcm::Tag::from_slice(&ciphertext[split..]);
    let result = match key.len() {
        16 => Aes128Gcm::new_from_slice(key)
            .map_err(|_| Error::InvalidKey)?
            .decrypt_in_place_detached(nonce, additional_data, &mut output, tag),
        24 => Aes192Gcm::new_from_slice(key)
            .map_err(|_| Error::InvalidKey)?
            .decrypt_in_place_detached(nonce, additional_data, &mut output, tag),
        32 => Aes256Gcm::new_from_slice(key)
            .map_err(|_| Error::InvalidKey)?
            .decrypt_in_place_detached(nonce, additional_data, &mut output, tag),
        _ => return Err(Error::InvalidKey),
    };
    result.map_err(|_| Error::AuthenticationFailed)?;
    Ok(output)
}

fn nonce(iv: &[u8]) -> Result<&Nonce<U12>, Error> {
    if iv.len() != 12 {
        return Err(Error::InvalidLength);
    }
    Ok(Nonce::from_slice(iv))
}

/// Make an Ed25519 key pair from the supplied 32-byte seed and return its canonical PKCS#8
/// private key and 32-byte raw public key.
pub fn ed25519_from_seed(seed: &[u8]) -> Result<(Vec<u8>, [u8; 32]), Error> {
    let seed: &[u8; 32] = seed.try_into().map_err(|_| Error::InvalidKey)?;
    let signing = SigningKey::from_bytes(seed);
    let private = signing
        .to_pkcs8_der()
        .map_err(|_| Error::InvalidKey)?
        .as_bytes()
        .to_vec();
    Ok((private, signing.verifying_key().to_bytes()))
}

/// Generate an Ed25519 key pair from the operating system's CSPRNG.
pub fn ed25519_generate() -> Result<(Vec<u8>, [u8; 32]), Error> {
    let mut seed = [0_u8; 32];
    getrandom::fill(&mut seed).map_err(|_| Error::RandomUnavailable)?;
    ed25519_from_seed(&seed)
}

/// Validate PKCS#8 Ed25519 private key material and derive its raw public key.
pub fn ed25519_public_from_pkcs8(pkcs8: &[u8]) -> Result<[u8; 32], Error> {
    let signing = SigningKey::from_pkcs8_der(pkcs8).map_err(|_| Error::InvalidKey)?;
    Ok(signing.verifying_key().to_bytes())
}

/// Sign a message with a PKCS#8 Ed25519 private key.
pub fn ed25519_sign(pkcs8: &[u8], message: &[u8]) -> Result<[u8; 64], Error> {
    let signing = SigningKey::from_pkcs8_der(pkcs8).map_err(|_| Error::InvalidKey)?;
    Ok(signing.sign(message).to_bytes())
}

/// Verify an Ed25519 signature against a raw 32-byte public key.
pub fn ed25519_verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<bool, Error> {
    let public_key: &[u8; 32] = public_key.try_into().map_err(|_| Error::InvalidKey)?;
    let public_key = VerifyingKey::from_bytes(public_key).map_err(|_| Error::InvalidKey)?;
    let signature = Signature::from_slice(signature).map_err(|_| Error::InvalidLength)?;
    Ok(public_key.verify(message, &signature).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkdf_sha256_matches_rfc5869_case_one() {
        let ikm = [0x0b; 22];
        let salt = (0x00..=0x0c).collect::<Vec<u8>>();
        let info = (0xf0..=0xf9).collect::<Vec<u8>>();
        let output = hkdf_sha256(&ikm, &salt, &info, 42).unwrap();
        assert_eq!(
            output,
            [
                0x3c, 0xb2, 0x5f, 0x25, 0xfa, 0xac, 0xd5, 0x7a, 0x90, 0x43, 0x4f, 0x64, 0xd0, 0x36,
                0x2f, 0x2a, 0x2d, 0x2d, 0x0a, 0x90, 0xcf, 0x1a, 0x5a, 0x4c, 0x5d, 0xb0, 0x2d, 0x56,
                0xec, 0xc4, 0xc5, 0xbf, 0x34, 0x00, 0x72, 0x08, 0xd5, 0xb8, 0x87, 0x18, 0x58, 0x65,
            ]
        );
    }

    #[test]
    fn aes_gcm_authenticates_data_and_additional_data() {
        let key = [0x42; 32];
        let iv = [0x24; 12];
        let ciphertext = aes_gcm_encrypt(&key, &iv, b"header", b"secret message").unwrap();
        assert_eq!(
            aes_gcm_decrypt(&key, &iv, b"header", &ciphertext).unwrap(),
            b"secret message"
        );
        assert_eq!(
            aes_gcm_decrypt(&key, &iv, b"wrong", &ciphertext),
            Err(Error::AuthenticationFailed)
        );
    }

    #[test]
    fn ed25519_pkcs8_round_trip_signs_and_verifies() {
        // RFC 8032 test vector 1 seed.
        let seed = decode_hex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60");
        let (private, public) = ed25519_from_seed(&seed).unwrap();
        assert_eq!(
            public,
            decode_hex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
                .as_slice()
        );
        assert_eq!(ed25519_public_from_pkcs8(&private).unwrap(), public);
        let signature = ed25519_sign(&private, b"").unwrap();
        assert!(ed25519_verify(&public, b"", &signature).unwrap());
        assert!(!ed25519_verify(&public, b"other", &signature).unwrap());
    }

    fn decode_hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let digit = |byte: u8| match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    _ => unreachable!(),
                };
                digit(pair[0]) * 16 + digit(pair[1])
            })
            .collect()
    }
}

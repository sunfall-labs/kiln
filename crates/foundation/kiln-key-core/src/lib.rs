//! Validated asymmetric key material shared by the interpreter and native provider.
//!
//! The crate deliberately keeps parser and big integer types behind this boundary. Callers hold
//! an `ImportedKey`, never bytes that are merely labelled as a key. RSA constructors validate
//! the encoded components before a handle can be created.

use rsa::pkcs1::{
    DecodeRsaPrivateKey, DecodeRsaPublicKey, EncodeRsaPrivateKey, EncodeRsaPublicKey,
};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{RsaPrivateKey, RsaPublicKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyType {
    Public,
    Private,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyFormat {
    Pkcs1,
    Pkcs8,
    Spki,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyEncoding {
    Der,
    Pem,
}

#[derive(Debug)]
pub enum KeyError {
    InvalidKey,
    UnsupportedKeyType,
    UnsupportedEncoding,
}

/// Parsed, validated key material. The private key never leaves this value except through an
/// explicitly requested PKCS#1/PKCS#8 export.
#[derive(Clone)]
pub enum ImportedKey {
    RsaPublic(RsaPublicKey),
    RsaPrivate(RsaPrivateKey),
}

impl core::fmt::Debug for ImportedKey {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ImportedKey")
            .field("key_type", &self.key_type())
            .field("asymmetric_key_type", &self.asymmetric_key_type())
            .field("modulus_bits", &self.modulus_bits())
            .finish()
    }
}

impl ImportedKey {
    pub fn key_type(&self) -> KeyType {
        match self {
            Self::RsaPublic(_) => KeyType::Public,
            Self::RsaPrivate(_) => KeyType::Private,
        }
    }

    pub fn asymmetric_key_type(&self) -> &'static str {
        "rsa"
    }

    pub fn modulus_bits(&self) -> usize {
        match self {
            Self::RsaPublic(key) => usize::try_from(key.n().bits()).unwrap_or(usize::MAX),
            Self::RsaPrivate(key) => usize::try_from(key.n().bits()).unwrap_or(usize::MAX),
        }
    }

    pub fn public_key(&self) -> Self {
        match self {
            Self::RsaPublic(key) => Self::RsaPublic(key.clone()),
            Self::RsaPrivate(key) => Self::RsaPublic(RsaPublicKey::from(key)),
        }
    }

    pub fn export(&self, format: KeyFormat, encoding: KeyEncoding) -> Result<Vec<u8>, KeyError> {
        match (self, format, encoding) {
            (Self::RsaPublic(key), KeyFormat::Pkcs1, KeyEncoding::Der) => key
                .to_pkcs1_der()
                .map(|document| document.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            (Self::RsaPublic(key), KeyFormat::Spki, KeyEncoding::Der) => key
                .to_public_key_der()
                .map(|document| document.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            (Self::RsaPrivate(key), KeyFormat::Pkcs1, KeyEncoding::Der) => key
                .to_pkcs1_der()
                .map(|document| document.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            (Self::RsaPrivate(key), KeyFormat::Pkcs8, KeyEncoding::Der) => key
                .to_pkcs8_der()
                .map(|document| document.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            (Self::RsaPublic(key), KeyFormat::Pkcs1, KeyEncoding::Pem) => key
                .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                .map(|pem| pem.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            (Self::RsaPublic(key), KeyFormat::Spki, KeyEncoding::Pem) => key
                .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
                .map(|pem| pem.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            (Self::RsaPrivate(key), KeyFormat::Pkcs1, KeyEncoding::Pem) => key
                .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                .map(|pem| pem.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            (Self::RsaPrivate(key), KeyFormat::Pkcs8, KeyEncoding::Pem) => key
                .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
                .map(|pem| pem.as_bytes().to_vec())
                .map_err(|_| KeyError::InvalidKey),
            _ => Err(KeyError::UnsupportedEncoding),
        }
    }
}

/// Import RSA key material from DER or PEM. PEM is accepted only for the standard RSA/SPKI and
/// PKCS#8 labels; encrypted PEM and unrelated algorithms fail closed.
pub fn import_rsa(
    bytes: &[u8],
    encoding: KeyEncoding,
    expected: KeyType,
) -> Result<ImportedKey, KeyError> {
    match (encoding, expected) {
        (KeyEncoding::Pem, KeyType::Private) => {
            let text = core::str::from_utf8(bytes).map_err(|_| KeyError::InvalidKey)?;
            if text.contains("-----BEGIN RSA PRIVATE KEY-----") {
                RsaPrivateKey::from_pkcs1_pem(text)
                    .map(ImportedKey::RsaPrivate)
                    .map_err(|_| KeyError::InvalidKey)
            } else if text.contains("-----BEGIN PRIVATE KEY-----") {
                RsaPrivateKey::from_pkcs8_pem(text)
                    .map(ImportedKey::RsaPrivate)
                    .map_err(|_| KeyError::InvalidKey)
            } else if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
                Err(KeyError::UnsupportedEncoding)
            } else {
                Err(KeyError::UnsupportedKeyType)
            }
        }
        (KeyEncoding::Pem, KeyType::Public) => {
            let text = core::str::from_utf8(bytes).map_err(|_| KeyError::InvalidKey)?;
            if text.contains("-----BEGIN RSA PUBLIC KEY-----") {
                RsaPublicKey::from_pkcs1_pem(text)
                    .map(ImportedKey::RsaPublic)
                    .map_err(|_| KeyError::InvalidKey)
            } else if text.contains("-----BEGIN PUBLIC KEY-----") {
                RsaPublicKey::from_public_key_pem(text)
                    .map(ImportedKey::RsaPublic)
                    .map_err(|_| KeyError::InvalidKey)
            } else {
                Err(KeyError::UnsupportedKeyType)
            }
        }
        (KeyEncoding::Der, KeyType::Private) => RsaPrivateKey::from_pkcs8_der(bytes)
            .or_else(|_| RsaPrivateKey::from_pkcs1_der(bytes))
            .map(ImportedKey::RsaPrivate)
            .map_err(|_| KeyError::InvalidKey),
        (KeyEncoding::Der, KeyType::Public) => RsaPublicKey::from_public_key_der(bytes)
            .or_else(|_| RsaPublicKey::from_pkcs1_der(bytes))
            .map(ImportedKey::RsaPublic)
            .map_err(|_| KeyError::InvalidKey),
    }
}

/// Import either public or private RSA material, preserving the kind that the encoding contains.
pub fn import_rsa_any(bytes: &[u8], encoding: KeyEncoding) -> Result<ImportedKey, KeyError> {
    match import_rsa(bytes, encoding, KeyType::Private) {
        Ok(key) => Ok(key),
        Err(_) => import_rsa(bytes, encoding, KeyType::Public),
    }
}

/// Import public RSA parameters using the JWK `n` and `e` fields.
pub fn import_rsa_jwk(n: &[u8], e: &[u8]) -> Result<ImportedKey, KeyError> {
    if n.is_empty() || e.is_empty() || n[0] == 0 || e[0] == 0 {
        return Err(KeyError::InvalidKey);
    }
    let modulus = rsa::BigUint::from_bytes_be(n);
    let exponent = rsa::BigUint::from_bytes_be(e);
    RsaPublicKey::new(modulus, exponent)
        .map(ImportedKey::RsaPublic)
        .map_err(|_| KeyError::InvalidKey)
}

/// Parse the RSA subset of a JSON Web Key. Private JWKs require all CRT components and are
/// passed through `RsaPrivateKey::from_components` plus its consistency validator.
pub fn import_rsa_jwk_json(json: &[u8]) -> Result<ImportedKey, KeyError> {
    use base64::Engine;
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|_| KeyError::InvalidKey)?;
    let object = value.as_object().ok_or(KeyError::InvalidKey)?;
    if object.get("kty").and_then(serde_json::Value::as_str) != Some("RSA") {
        return Err(KeyError::UnsupportedKeyType);
    }
    let decode = |name: &str| -> Result<Vec<u8>, KeyError> {
        let encoded = object
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or(KeyError::InvalidKey)?;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| KeyError::InvalidKey)
    };
    let n = decode("n")?;
    let e = decode("e")?;
    let Some(d) = object.get("d") else {
        return import_rsa_jwk(&n, &e);
    };
    let d = d.as_str().ok_or(KeyError::InvalidKey)?;
    let d = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(d)
        .map_err(|_| KeyError::InvalidKey)?;
    let p = decode("p")?;
    let q = decode("q")?;
    let dp = rsa::BigUint::from_bytes_be(&decode("dp")?);
    let dq = rsa::BigUint::from_bytes_be(&decode("dq")?);
    let qi = rsa::BigUint::from_bytes_be(&decode("qi")?);
    if object.contains_key("oth") {
        return Err(KeyError::UnsupportedKeyType);
    }
    let modulus = rsa::BigUint::from_bytes_be(&n);
    let exponent = rsa::BigUint::from_bytes_be(&e);
    let private_exponent = rsa::BigUint::from_bytes_be(&d);
    let primes = vec![
        rsa::BigUint::from_bytes_be(&p),
        rsa::BigUint::from_bytes_be(&q),
    ];
    let key = RsaPrivateKey::from_components(modulus, exponent, private_exponent, primes)
        .map_err(|_| KeyError::InvalidKey)?;
    key.validate().map_err(|_| KeyError::InvalidKey)?;
    let factors = key.primes();
    if factors.len() != 2 {
        return Err(KeyError::UnsupportedKeyType);
    }
    let one = rsa::BigUint::from(1u8);
    let p_minus_one = &factors[0] - &one;
    let q_minus_one = &factors[1] - &one;
    if key.d() % &p_minus_one != dp
        || key.d() % &q_minus_one != dq
        || (&qi * &factors[1]) % &factors[0] != one
    {
        return Err(KeyError::InvalidKey);
    }
    Ok(ImportedKey::RsaPrivate(key))
}

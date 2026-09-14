//! Lossless JavaScript string units shared by the frontend, both runtimes, and host boundaries.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct StringUnits(Repr);

#[derive(Clone, Debug)]
enum Repr {
    Latin1(Arc<[u8]>),
    Utf16(Arc<[u16]>),
}

impl StringUnits {
    pub fn from_utf8(bytes: &[u8]) -> Result<Self, &'static str> {
        let text = std::str::from_utf8(bytes).map_err(|_| "invalid UTF-8")?;
        Ok(Self::from(text))
    }

    pub fn from_units(units: &[u16]) -> Self {
        if units.iter().all(|&unit| unit <= 0xff) {
            Self(Repr::Latin1(units.iter().map(|&unit| unit as u8).collect()))
        } else {
            Self(Repr::Utf16(Arc::from(units)))
        }
    }

    pub fn unit_len(&self) -> usize {
        match &self.0 {
            Repr::Latin1(bytes) => bytes.len(),
            Repr::Utf16(units) => units.len(),
        }
    }

    /// Alias used by language algorithms where the unit basis is already explicit.
    pub fn len(&self) -> usize {
        self.unit_len()
    }
    pub fn is_empty(&self) -> bool {
        self.unit_len() == 0
    }
    pub fn encode_utf16(&self) -> UnitIter<'_> {
        self.units()
    }

    pub fn unit_at(&self, index: usize) -> Option<u16> {
        match &self.0 {
            Repr::Latin1(bytes) => bytes.get(index).copied().map(u16::from),
            Repr::Utf16(units) => units.get(index).copied(),
        }
    }

    pub fn units(&self) -> UnitIter<'_> {
        match &self.0 {
            Repr::Latin1(bytes) => UnitIter::Latin1(bytes.iter()),
            Repr::Utf16(units) => UnitIter::Utf16(units.iter()),
        }
    }

    pub fn to_units(&self) -> Box<[u16]> {
        self.units().collect()
    }

    /// Return scalar UTF-8 text without allocating for the common ASCII representation.
    ///
    /// JavaScript source parsers consume UTF-8 `&str`; callers use this checked view at source
    /// boundaries so malformed UTF-16 is refused instead of being silently replaced.
    pub fn to_scalar_cow(&self) -> Result<Cow<'_, str>, &'static str> {
        match &self.0 {
            Repr::Latin1(bytes) if bytes.is_ascii() => Ok(Cow::Borrowed(
                std::str::from_utf8(bytes).expect("ASCII bytes are valid UTF-8"),
            )),
            Repr::Latin1(bytes) => {
                let mut text = String::with_capacity(bytes.len());
                for &byte in bytes.iter() {
                    text.push(char::from(byte));
                }
                Ok(Cow::Owned(text))
            }
            Repr::Utf16(units) => String::from_utf16(units)
                .map(Cow::Owned)
                .map_err(|_| "string contains a lone surrogate"),
        }
    }

    pub fn to_scalar_string(&self) -> Result<String, &'static str> {
        self.to_scalar_cow().map(Cow::into_owned)
    }

    pub fn to_lossy_string(&self) -> String {
        String::from_utf16_lossy(&self.to_units())
    }
}

pub enum UnitIter<'a> {
    Latin1(std::slice::Iter<'a, u8>),
    Utf16(std::slice::Iter<'a, u16>),
}

impl Iterator for UnitIter<'_> {
    type Item = u16;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Latin1(iter) => iter.next().copied().map(u16::from),
            Self::Utf16(iter) => iter.next().copied(),
        }
    }
}

impl From<&str> for StringUnits {
    fn from(text: &str) -> Self {
        if text.is_ascii() {
            return Self(Repr::Latin1(Arc::from(text.as_bytes())));
        }
        if text.chars().all(|scalar| scalar as u32 <= 0xff) {
            return Self(Repr::Latin1(Arc::from(
                text.chars().map(|scalar| scalar as u8).collect::<Vec<_>>(),
            )));
        }
        let mut units = Vec::with_capacity(text.len());
        for scalar in text.chars() {
            let mut encoded = [0; 2];
            units.extend_from_slice(scalar.encode_utf16(&mut encoded));
        }
        Self::from_units(&units)
    }
}
impl From<String> for StringUnits {
    fn from(text: String) -> Self {
        Self::from(text.as_str())
    }
}
impl Default for StringUnits {
    fn default() -> Self {
        Self::from_units(&[])
    }
}
impl PartialEq for StringUnits {
    fn eq(&self, other: &Self) -> bool {
        self.units().eq(other.units())
    }
}
impl Eq for StringUnits {}
impl PartialEq<str> for StringUnits {
    fn eq(&self, other: &str) -> bool {
        self.units().eq(other.encode_utf16())
    }
}
impl PartialEq<&str> for StringUnits {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}
impl PartialOrd for StringUnits {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for StringUnits {
    fn cmp(&self, other: &Self) -> Ordering {
        self.units().cmp(other.units())
    }
}
impl Hash for StringUnits {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.unit_len().hash(state);
        self.units().for_each(|u| u.hash(state));
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::StringUnits;
    #[test]
    fn latin1_index() {
        let s = StringUnits::from("caf\u{e9}");
        assert_eq!(s.unit_at(3), Some(0xe9));
    }
    #[test]
    fn lone_surrogate_is_lossless() {
        let s = StringUnits::from_units(&[0xd800]);
        assert!(s.to_scalar_string().is_err());
        assert_eq!(s.to_units().as_ref(), &[0xd800]);
    }
    #[test]
    fn scalar_cow_borrows_ascii_and_rejects_lone_surrogates() {
        assert!(matches!(
            StringUnits::from("source").to_scalar_cow(),
            Ok(Cow::Borrowed("source"))
        ));
        assert!(matches!(
            StringUnits::from("hé").to_scalar_cow(),
            Ok(Cow::Owned(text)) if text == "hé"
        ));
        assert!(StringUnits::from_units(&[0xd800]).to_scalar_cow().is_err());
    }

    #[test]
    fn utf8_decodes_pairs() {
        let s = StringUnits::from_utf8("😀".as_bytes()).unwrap();
        assert_eq!(s.to_units().as_ref(), &[0xd83d, 0xde00]);
    }
}

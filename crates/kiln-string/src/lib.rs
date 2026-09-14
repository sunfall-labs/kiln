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

    pub fn from_view(view: StringView<'_>) -> Self {
        match view {
            StringView::Latin1(bytes) => Self(Repr::Latin1(Arc::from(bytes))),
            StringView::Utf16(units) => Self(Repr::Utf16(Arc::from(units))),
        }
    }

    pub fn unit_len(&self) -> usize {
        match &self.0 {
            Repr::Latin1(bytes) => bytes.len(),
            Repr::Utf16(units) => units.len(),
        }
    }

    /// Borrow the unit storage without allocating or widening Latin1 strings.
    pub fn as_view(&self) -> StringView<'_> {
        match &self.0 {
            Repr::Latin1(bytes) => StringView::Latin1(bytes),
            Repr::Utf16(units) => StringView::Utf16(units),
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

    /// Return scalar text while borrowing an ASCII Latin1 representation when possible.
    /// UTF-16 remains checked for lone surrogates at this boundary.
    pub fn to_scalar_cow(&self) -> Result<Cow<'_, str>, &'static str> {
        match self.as_view() {
            StringView::Latin1(bytes) => Ok(encoding_rs::mem::decode_latin1(bytes)),
            StringView::Utf16(units) => String::from_utf16(units)
                .map(Cow::Owned)
                .map_err(|_| "string contains a lone surrogate"),
        }
    }

    pub fn to_scalar_string(&self) -> Result<String, &'static str> {
        self.to_scalar_cow().map(Cow::into_owned)
    }

    pub fn to_lossy_string(&self) -> String {
        match self.as_view() {
            StringView::Latin1(bytes) => encoding_rs::mem::decode_latin1(bytes).into_owned(),
            StringView::Utf16(units) => String::from_utf16_lossy(units),
        }
    }
}

/// A borrowed JavaScript string view. The representation remains visible to consumers that can
/// specialize Latin1 hot loops, while UTF-16 keeps every code unit including lone surrogates.
#[derive(Clone, Copy, Debug)]
pub enum StringView<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
}

impl PartialEq for StringView<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.units().eq(other.units())
    }
}
impl Eq for StringView<'_> {}

impl<'a> StringView<'a> {
    pub fn unit_len(self) -> usize {
        match self {
            Self::Latin1(bytes) => bytes.len(),
            Self::Utf16(units) => units.len(),
        }
    }

    pub fn len(self) -> usize {
        self.unit_len()
    }

    pub fn is_empty(self) -> bool {
        self.unit_len() == 0
    }

    pub fn unit_at(self, index: usize) -> Option<u16> {
        match self {
            Self::Latin1(bytes) => bytes.get(index).copied().map(u16::from),
            Self::Utf16(units) => units.get(index).copied(),
        }
    }

    pub fn latin1_slice(self, start: usize, end: usize) -> Option<&'a [u8]> {
        match self {
            Self::Latin1(bytes) => bytes.get(start..end),
            Self::Utf16(_) => None,
        }
    }

    pub fn units(self) -> UnitIter<'a> {
        match self {
            Self::Latin1(bytes) => UnitIter::Latin1(bytes.iter()),
            Self::Utf16(units) => UnitIter::Utf16(units.iter()),
        }
    }

    pub fn to_units(self) -> Box<[u16]> {
        self.units().collect()
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
    use super::{StringUnits, StringView};
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
    fn borrowed_view_keeps_representation_and_units() {
        let latin = StringUnits::from("abc");
        assert_eq!(latin.as_view(), StringView::Latin1(b"abc"));
        let wide = StringUnits::from_view(StringView::Utf16(&[0x61, 0xd800]));
        assert_eq!(wide.as_view().unit_at(1), Some(0xd800));
        assert!(wide.as_view().latin1_slice(0, 2).is_none());
        assert_ne!(StringView::Utf16(&[0xd800]), StringView::Utf16(&[0xfffd]));
    }

    #[test]
    fn scalar_conversion_uses_existing_storage() {
        let ascii = StringUnits::from("abc");
        assert!(matches!(
            ascii.to_scalar_cow(),
            Ok(std::borrow::Cow::Borrowed("abc"))
        ));
        let latin = StringUnits::from("caf\u{e9}");
        assert_eq!(latin.to_scalar_string().unwrap(), "caf\u{e9}");
        assert_eq!(latin.to_lossy_string(), "caf\u{e9}");
        let wide = StringUnits::from_view(StringView::Utf16(&[0x61, 0xd800]));
        assert!(wide.to_scalar_cow().is_err());
        assert_eq!(wide.to_lossy_string(), "a\u{fffd}");
    }
    #[test]
    fn utf8_decodes_pairs() {
        let s = StringUnits::from_utf8("😀".as_bytes()).unwrap();
        assert_eq!(s.to_units().as_ref(), &[0xd83d, 0xde00]);
    }
}

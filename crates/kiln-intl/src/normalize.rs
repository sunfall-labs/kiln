//! `String.prototype.normalize`.
//!
//! Unicode normalization is a table, not an algorithm anyone should write twice: which sequences
//! compose, which characters decompose canonically and which only for compatibility (`ﬁ` to `fi`,
//! `²` to `2`) is UAX #15 data that ICU4X already links for collation. It crosses the Seam as
//! UTF-16 code units for the reason ADR 0022 gives every string boundary: a JavaScript string may
//! hold an unpaired surrogate, which has no `&str` spelling. ICU4X passes one through unchanged,
//! which is what the language does too.

use icu_normalizer::{ComposingNormalizer, DecomposingNormalizer};

/// The accepted values of the `form` argument.
pub const FORMS: &[&str] = &["NFC", "NFD", "NFKC", "NFKD"];

/// Normalize `units` into `form`, or `None` for a form outside [`FORMS`].
pub fn normalize(units: &[u16], form: &str) -> Option<Vec<u16>> {
    let normalized = match form {
        "NFC" => ComposingNormalizer::new_nfc().normalize_utf16(units),
        "NFD" => DecomposingNormalizer::new_nfd().normalize_utf16(units),
        "NFKC" => ComposingNormalizer::new_nfkc().normalize_utf16(units),
        "NFKD" => DecomposingNormalizer::new_nfkd().normalize_utf16(units),
        _ => return None,
    };
    Some(normalized.into_owned())
}

/// Whether `units` is already in `form`, which is the cheap question a caller asks before
/// paying for the copy.
pub fn is_normalized(units: &[u16], form: &str) -> Option<bool> {
    Some(match form {
        "NFC" => ComposingNormalizer::new_nfc().is_normalized_utf16(units),
        "NFD" => DecomposingNormalizer::new_nfd().is_normalized_utf16(units),
        "NFKC" => ComposingNormalizer::new_nfkc().is_normalized_utf16(units),
        "NFKD" => DecomposingNormalizer::new_nfkd().is_normalized_utf16(units),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_four_forms() {
        let e_acute = units("\u{e9}");
        let decomposed = units("e\u{301}");
        assert_eq!(normalize(&decomposed, "NFC").unwrap(), e_acute);
        assert_eq!(normalize(&e_acute, "NFD").unwrap(), decomposed);
        assert_eq!(normalize(&units("\u{fb01}"), "NFKC").unwrap(), units("fi"));
        assert_eq!(normalize(&units("\u{fb01}"), "NFKD").unwrap(), units("fi"));
        assert_eq!(
            normalize(&units("\u{fb01}"), "NFC").unwrap(),
            units("\u{fb01}")
        );
        assert!(normalize(&e_acute, "nfc").is_none());
        assert_eq!(is_normalized(&e_acute, "NFC"), Some(true));
        assert_eq!(is_normalized(&decomposed, "NFC"), Some(false));
    }

    #[test]
    fn a_lone_surrogate_passes_through() {
        let lone = vec![0x61, 0xD800, 0x62];
        assert_eq!(normalize(&lone, "NFC").unwrap(), lone);
    }
}

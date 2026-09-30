//! Locale sensitive case mappings shared by the interpreter and native provider.

use icu_casemap::CaseMapper;
use icu_locale_core::Locale;

use crate::{Error, locale};

/// Map one scalar string with ECMA-402's validated locale and ICU4X's contextual case data.
///
/// The JavaScript caller owns `CanonicalizeLocaleList` and passes its first canonical tag here;
/// this layer canonicalizes again at the semantic boundary so direct callers cannot skip the
/// grammar or alias table. An omitted locale is represented by the realm's deterministic default.
pub fn map_case(text: &str, tag: Option<&str>, uppercase: bool) -> Result<String, Error> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mapped = map_case_utf16(&units, tag, uppercase)?;
    Ok(String::from_utf16(&mapped).expect("case mapping preserves valid UTF-16"))
}

/// Map JavaScript UTF-16 code units, preserving any unpaired surrogate unchanged.
///
/// ICU4X accepts Unicode scalar strings. Splitting at unpaired surrogates lets its contextual
/// rules operate on each valid run while retaining JavaScript's otherwise-unrepresentable units.
pub fn map_case_utf16(
    units: &[u16],
    tag: Option<&str>,
    uppercase: bool,
) -> Result<Vec<u16>, Error> {
    let canonical = locale::canonicalize(tag.unwrap_or(locale::DEFAULT))?;
    let parsed = Locale::try_from_str(&canonical).map_err(|_| {
        Error::range(format!(
            "Incorrect locale information provided: {canonical}"
        ))
    })?;
    let mapper = CaseMapper::new();
    let mut answer = Vec::with_capacity(units.len());
    let mut run = String::new();
    let flush = |run: &mut String, answer: &mut Vec<u16>| {
        if run.is_empty() {
            return;
        }
        let mapped = if uppercase {
            mapper.uppercase_to_string(run.as_str(), &parsed.id)
        } else {
            mapper.lowercase_to_string(run.as_str(), &parsed.id)
        };
        answer.extend(mapped.encode_utf16());
        run.clear();
    };
    for decoded in char::decode_utf16(units.iter().copied()) {
        match decoded {
            Ok(ch) => run.push(ch),
            Err(error) => {
                flush(&mut run, &mut answer);
                answer.push(error.unpaired_surrogate());
            }
        }
    }
    flush(&mut run, &mut answer);
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::{map_case, map_case_utf16};

    #[test]
    fn applies_language_sensitive_and_contextual_mappings() {
        assert_eq!(map_case("Iİ", Some("tr"), false).unwrap(), "ıi");
        assert_eq!(map_case("istanbul", Some("tr"), true).unwrap(), "İSTANBUL");
        assert_eq!(map_case("ΟΣ", Some("el"), false).unwrap(), "ος");
        assert_eq!(map_case("I", Some("lt"), false).unwrap(), "i");
    }

    #[test]
    fn validates_and_canonicalizes_tags_before_mapping() {
        assert_eq!(map_case("ABC", Some("EN-us"), false).unwrap(), "abc");
        assert!(map_case("abc", Some("not a tag"), false).is_err());
    }

    #[test]
    fn preserves_unpaired_utf16_units_around_contextual_mapping() {
        assert_eq!(
            map_case_utf16(&[0xd800, b'I' as u16, 0xdc00], Some("tr"), false).unwrap(),
            [0xd800, 0x0131, 0xdc00]
        );
    }
}

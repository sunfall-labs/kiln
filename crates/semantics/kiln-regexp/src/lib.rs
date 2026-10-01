//! Shared normalization for the regular-expression engine's Unicode-sets parser.

/// Work around regress 0.12 treating every pair drawn from its punctuation alphabet as reserved.
/// ECMAScript reserves only the explicit doubled punctuators; other adjacent punctuation is a
/// valid literal sequence in a Unicode-sets class. Escaping the first code point preserves that
/// literal meaning while allowing the shared parser to accept the pattern.
pub fn normalize_unicode_sets_pattern(pattern: &[u32]) -> Vec<u32> {
    let mut output = Vec::with_capacity(pattern.len());
    let mut characters = pattern.iter().copied().peekable();
    let mut class_depth = 0usize;
    let mut escaped = false;

    while let Some(character) = characters.next() {
        if escaped {
            output.push(character);
            escaped = false;
            continue;
        }
        if character == u32::from('\\') {
            output.push(character);
            escaped = true;
            continue;
        }
        if character == u32::from('[') {
            class_depth = class_depth.saturating_add(1);
            output.push(character);
            continue;
        }
        if character == u32::from(']') {
            class_depth = class_depth.saturating_sub(1);
            output.push(character);
            continue;
        }
        if class_depth > 0
            && is_regress_double_punctuation(character)
            && characters.peek().is_some_and(|next| {
                is_regress_double_punctuation(*next)
                    && !is_ecmascript_reserved_double_punctuation(character, *next)
            })
        {
            output.push(u32::from('\\'));
        }
        output.push(character);
    }

    output
}

/// Validate Unicode-mode identity escapes and apply Unicode-sets parser normalization.
/// Callers pass code points after combining well-formed UTF-16 surrogate pairs. A raw lone
/// surrogate is valid pattern text, but a preceding unescaped backslash makes it an invalid
/// identity escape under `u` and `v`; the regexp parser accepts this extension, so both engines
/// must reject it before parsing. Keeping this beside the `v` normalization gives construction
/// one shared grammar gate.
pub fn normalize_pattern_for_flags(pattern: &[u32], flags: &str) -> Option<Vec<u32>> {
    let unicode = flags.contains('u') || flags.contains('v');
    if unicode {
        let mut consecutive_backslashes = 0usize;
        for &character in pattern {
            if character == u32::from('\\') {
                consecutive_backslashes = consecutive_backslashes.saturating_add(1);
                continue;
            }
            if (0xD800..=0xDFFF).contains(&character) && consecutive_backslashes % 2 == 1 {
                return None;
            }
            consecutive_backslashes = 0;
        }
    }
    Some(if flags.contains('v') {
        normalize_unicode_sets_pattern(pattern)
    } else {
        pattern.to_vec()
    })
}

fn is_regress_double_punctuation(character: u32) -> bool {
    matches!(
        character,
        0x0021
            | 0x0023
            | 0x0024
            | 0x0025
            | 0x0026
            | 0x002A
            | 0x002B
            | 0x002C
            | 0x002E
            | 0x003A
            | 0x003B
            | 0x003C
            | 0x003D
            | 0x003E
            | 0x003F
            | 0x0040
            | 0x005E
            | 0x0060
            | 0x007E
    )
}

fn is_ecmascript_reserved_double_punctuation(first: u32, second: u32) -> bool {
    matches!(
        (first, second),
        (0x0026, 0x0026)
            | (0x0021, 0x0021)
            | (0x0023, 0x0023)
            | (0x0024, 0x0024)
            | (0x0025, 0x0025)
            | (0x002A, 0x002A)
            | (0x002B, 0x002B)
            | (0x002C, 0x002C)
            | (0x002E, 0x002E)
            | (0x003A, 0x003A)
            | (0x003B, 0x003B)
            | (0x003C, 0x003C)
            | (0x003D, 0x003D)
            | (0x003E, 0x003E)
            | (0x003F, 0x003F)
            | (0x0040, 0x0040)
            | (0x005E, 0x005E)
            | (0x0060, 0x0060)
            | (0x007E, 0x007E)
    )
}

#[cfg(test)]
mod tests {
    use super::{normalize_pattern_for_flags, normalize_unicode_sets_pattern};

    #[test]
    fn escapes_only_non_reserved_adjacent_punctuation_inside_classes() {
        assert_eq!(
            normalize_unicode_sets_pattern(&"[:;]".chars().map(u32::from).collect::<Vec<_>>()),
            "[\\:;]".chars().map(u32::from).collect::<Vec<_>>()
        );
        assert_eq!(
            normalize_unicode_sets_pattern(&"[!!]".chars().map(u32::from).collect::<Vec<_>>()),
            "[!!]".chars().map(u32::from).collect::<Vec<_>>()
        );
        assert_eq!(
            normalize_unicode_sets_pattern(&"[\\! !]".chars().map(u32::from).collect::<Vec<_>>()),
            "[\\! !]".chars().map(u32::from).collect::<Vec<_>>()
        );
        let valid = "[:;]".chars().map(u32::from).collect::<Vec<_>>();
        let normalized = normalize_unicode_sets_pattern(&valid);
        assert!(regress::Regex::from_unicode(normalized.into_iter(), "v").is_ok());
        let reserved = "[!!]".chars().map(u32::from).collect::<Vec<_>>();
        let normalized = normalize_unicode_sets_pattern(&reserved);
        assert_eq!(normalized, reserved);
        assert!(regress::Regex::from_unicode(normalized.into_iter(), "v").is_err());
    }

    #[test]
    fn unicode_identity_escape_rejects_only_odd_backslash_lone_surrogates() {
        let lone = u32::from(0xD800u16);
        assert!(normalize_pattern_for_flags(&[u32::from('\\'), lone], "u").is_none());
        assert!(normalize_pattern_for_flags(&[u32::from('\\'), lone], "v").is_none());
        assert_eq!(
            normalize_pattern_for_flags(&[u32::from('\\'), u32::from('\\'), lone], "v"),
            Some(vec![u32::from('\\'), u32::from('\\'), lone]),
        );
        // A well-formed UTF-16 pair is one scalar by the time it reaches this shared gate.
        assert_eq!(
            normalize_pattern_for_flags(&[0x1F600], "v"),
            Some(vec![0x1F600]),
        );
    }
}

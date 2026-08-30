//! Language tags: validity, canonical form, and the negotiation every `Intl` constructor starts
//! with.
//!
//! ECMA-402 puts three separable jobs under "locale". `IsStructurallyValidLanguageTag` is a
//! grammar question, `CanonicalizeUnicodeLocaleId` is a data question, and `ResolveLocale` is a
//! search over what the implementation actually has data for. ICU4X answers the first two; the
//! third is spec text over an availability predicate, and the predicate is the only place this
//! module makes a judgement call -- see [`is_available`].

use icu_locale::{LocaleCanonicalizer, LocaleExpander, TransformResult};
use icu_locale_core::extensions::unicode::{Key, Value};
use icu_locale_core::{LanguageIdentifier, Locale};

use crate::{Error, Setting};

/// The realm's locale.
///
/// A constant, for the reason ADR 0015 made the clock virtual and the zone UTC: a program that
/// asks the host what language it is in produces a different answer on a different machine, which
/// makes a build unreproducible and a differential probe unrunnable. `en-US` is chosen because it
/// is what the corpus assumes when it does not say, not because it is anyone's actual locale --
/// the honest form of "a locale the host chose" is an explicit argument, exactly as ADR 0015
/// concluded for the timezone.
pub const DEFAULT: &str = "en-US";

/// The value `localeMatcher` may take. Both do the same thing here.
///
/// The specification distinguishes `"lookup"` (defined exactly, in the spec text) from
/// `"best fit"` (deliberately implementation-defined). Kiln implements one algorithm and uses it
/// for both, which is conforming: "best fit" is allowed to be any matcher at least as good as
/// lookup, and claiming a second, better one that does not exist would be the plausible-looking
/// wrong answer. `resolvedOptions()` never reports `localeMatcher`, so nothing observes the
/// difference except which tag comes back, and lookup's answer is a correct answer for both.
pub const LOCALE_MATCHERS: &[&str] = &["lookup", "best fit"];

fn canonicalizer() -> LocaleCanonicalizer<LocaleExpander> {
    // `new_extended` rather than `new_common`: canonicalization has to recognise the deprecated
    // and legacy tags (`sgn-GR`, `de-DD`, `art-lojban`) that are by definition not in the common
    // set, and the extra data is alias tables rather than locale data.
    LocaleCanonicalizer::new_extended()
}

/// `IsStructurallyValidLanguageTag`, as ECMA-402 defines it: a `unicode_locale_id` from UTS 35.
///
/// Note what this excludes, because it is a common source of surprise: the irregular grandfathered
/// tags (`i-klingon`, `en-GB-oed`, `sgn-BE-FR`) and BCP 47 extlangs (`zh-cmn-Hans`) are *not*
/// `unicode_locale_id`s, so they are `RangeError`s rather than tags to be canonicalized. The
/// regular grandfathered tags that happen to parse as language-plus-variant (`art-lojban`,
/// `zh-guoyu`) are valid and canonicalize through the alias table like anything else.
pub fn is_structurally_valid(tag: &str) -> bool {
    Locale::try_from_str(tag).is_ok()
}

/// `CanonicalizeUnicodeLocaleId`, rejecting a structurally invalid tag with a `RangeError`.
pub fn canonicalize(tag: &str) -> Result<String, Error> {
    let mut locale = Locale::try_from_str(tag)
        .map_err(|_| Error::range(format!("Incorrect locale information provided: {tag}")))?;
    canonicalizer().canonicalize(&mut locale);
    canonicalize_keyword_types(&mut locale);
    Ok(locale.to_string())
}

/// The `-u-` keyword type aliases UTS 35 §3.2.1 requires and `icu_locale` does not apply.
///
/// `LocaleCanonicalizer` covers the language, script, region, variant and subdivision alias
/// tables; it does not replace a keyword *type* by its preferred form, so `und-u-ca-islamicc`
/// comes back unchanged. The tables below are the deprecated entries of CLDR's `bcp47` keyword
/// data for the keys ECMA-402 exercises, transcribed rather than derived -- which is a real cost
/// (they move when CLDR moves) and is why they are kept as small and as visible as possible rather
/// than spread through the callers.
///
/// `tz` is deliberately absent. Its alias table is roughly fifty entries and moves with the IANA
/// database; a partial one would canonicalize some zone names and silently leave others alone,
/// which is worse than leaving all of them alone and saying so.
fn canonicalize_keyword_types(locale: &mut Locale) {
    const ALIASES: &[(&str, &[(&str, &str)])] = &[
        (
            "ca",
            &[
                ("ethiopic-amete-alem", "ethioaa"),
                ("islamicc", "islamic-civil"),
            ],
        ),
        (
            "ks",
            &[
                ("primary", "level1"),
                ("secondary", "level2"),
                ("tertiary", "level3"),
                ("quaternary", "level4"),
                ("quarternary", "level4"),
                ("identical", "identic"),
            ],
        ),
        ("ms", &[("imperial", "uksystem")]),
        (
            "colalternate",
            &[("yes", "shifted"), ("no", "non-ignorable")],
        ),
    ];
    // `yes` is the CLDR alias of `true` on the boolean collation keys, and UTS 35 then removes a
    // type of `true` entirely. `icu_locale` does the removal but not the aliasing, so `-u-kb-yes`
    // survives as itself; doing both here keeps the two halves of one rule together.
    const BOOLEAN_KEYS: &[&str] = &["kb", "kc", "kh", "kk", "kn"];

    let keywords = &mut locale.extensions.unicode.keywords;
    let present: Vec<(Key, Value)> = keywords.iter().map(|(k, v)| (*k, v.clone())).collect();
    for (key, value) in present {
        let name = key.as_str();
        let spelled = value.to_string();
        let replacement = if BOOLEAN_KEYS.contains(&name) && spelled == "yes" {
            Some(String::new())
        } else {
            ALIASES
                .iter()
                .find(|(k, _)| *k == name)
                .and_then(|(_, table)| table.iter().find(|(from, _)| *from == spelled))
                .map(|(_, to)| (*to).to_string())
        };
        let Some(replacement) = replacement else {
            continue;
        };
        if replacement.is_empty() {
            // `true` is not written out, so the canonical form of `-u-kb-yes` is `-u-kb`.
            keywords.set(key, Value::default());
        } else if let Ok(value) = Value::try_from_str(&replacement) {
            keywords.set(key, value);
        }
    }
}

/// Whether Kiln claims to have locale data for a tag.
///
/// ECMA-402 leaves `availableLocales` implementation-defined, and there is no honest way to read
/// it out of a `compiled_data` bundle -- the baked provider exposes a lookup, not an enumeration.
/// What is available is CLDR's *likely subtags* table restricted to the common data set, which is
/// generated from the same locales the formatter data is generated from, so "the common expander
/// recognises this language" is a genuine data-driven answer rather than a guess.
///
/// It is not identical to the set another engine would report. A language ICU4X ships in its
/// extended-but-not-common set (`haw`) is claimed unavailable here and available by a full-ICU
/// engine, and a region subtag is accepted without checking that CLDR has data for that exact
/// region. Both differences move which *tag* `resolvedOptions().locale` reports; neither makes a
/// formatter produce different characters, because ICU4X falls back the same way in either case.
fn is_available(id: &LanguageIdentifier) -> bool {
    if id.language.is_unknown() {
        return false;
    }
    // Ask about the language alone: a fully specified tag such as `zh-Hans-CN` is already maximal,
    // so maximizing it reports Unmodified and would look unknown.
    let mut probe = LanguageIdentifier::from(id.language);
    LocaleExpander::new_common().maximize(&mut probe) == TransformResult::Modified
}

/// `LookupMatcher`: the longest prefix of a requested tag that Kiln has data for.
///
/// Returns the available tag together with the requested locale it came from, because
/// `supportedLocalesOf` reports the tag the *caller* asked for while `resolvedOptions().locale`
/// reports the one that was found.
fn best_available(requested: &Locale) -> Option<LanguageIdentifier> {
    let mut candidate = requested.id.clone();
    loop {
        if is_available(&candidate) {
            return Some(candidate);
        }
        if !candidate.variants.is_empty() {
            candidate.variants.clear();
        } else if candidate.region.is_some() {
            candidate.region = None;
        } else if candidate.script.is_some() {
            candidate.script = None;
        } else {
            return None;
        }
    }
}

/// One Unicode extension key a service cares about, and what the caller's options said about it.
#[derive(Clone, Debug)]
pub struct RelevantKey {
    /// The BCP 47 key, e.g. `"nu"`.
    pub key: &'static str,
    /// The value the constructor's own option supplied, which outranks the `-u-` extension.
    pub from_options: Option<String>,
    /// The value to settle on when neither the options nor the tag chose one.
    pub default: Option<String>,
}

impl RelevantKey {
    /// A key with no option-level override and no default.
    pub fn new(key: &'static str) -> Self {
        Self {
            key,
            from_options: None,
            default: None,
        }
    }

    /// A key whose value the constructor's options may override.
    pub fn with_option(mut self, value: Option<String>) -> Self {
        self.from_options = value;
        self
    }

    /// A key that settles on `value` when nothing else chose one.
    pub fn defaulting_to(mut self, value: &str) -> Self {
        self.default = Some(value.to_string());
        self
    }
}

/// The outcome of `ResolveLocale`.
#[derive(Clone, Debug)]
pub struct Resolution {
    /// What `resolvedOptions().locale` reports: the matched locale plus the keywords that survived.
    pub locale: String,
    /// The settled value of each relevant key, in the order they were asked for.
    pub keys: Vec<(&'static str, Option<String>)>,
    /// The locale the ICU formatter is built from. Opaque on purpose -- see the crate docs.
    pub(crate) icu: Locale,
}

impl Resolution {
    /// The settled value of one key, if it has one.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.as_deref())
    }
}

/// `ResolveLocale`: pick the locale and settle every relevant `-u-` keyword.
///
/// `supported` decides whether a key's requested value is one this build can honour; a value it
/// rejects is dropped rather than reported, so `resolvedOptions()` never claims a numbering system
/// the formatter is not going to use.
pub fn resolve(
    requested: &[String],
    keys: &[RelevantKey],
    supported: impl Fn(&str, &str) -> bool,
) -> Resolution {
    let mut matched: Option<(Locale, LanguageIdentifier)> = None;
    for tag in requested {
        let Ok(locale) = Locale::try_from_str(tag) else {
            continue;
        };
        if let Some(found) = best_available(&locale) {
            matched = Some((locale, found));
            break;
        }
    }
    let (requested_locale, mut id) = matched.unwrap_or_else(|| {
        let fallback: Locale = DEFAULT.parse().expect("the realm's default locale parses");
        let id = fallback.id.clone();
        (fallback, id)
    });

    let mut resolved = Locale::from(LanguageIdentifier::UNKNOWN);
    let mut settled = Vec::with_capacity(keys.len());
    for relevant in keys {
        let key = Key::try_from_str(relevant.key).expect("a relevant key is a valid BCP 47 key");
        // The options object outranks the `-u-` extension, which outranks the default (ECMA-402
        // ResolveLocale steps 9.g-9.h). A value none of the three supplies stays absent.
        let from_tag = requested_locale
            .extensions
            .unicode
            .keywords
            .get(&key)
            .map(|value| {
                let spelled = value.to_string();
                // An empty value is how `-u-kn` (with no type) is spelled; it means `true`.
                if spelled.is_empty() {
                    "true".to_string()
                } else {
                    spelled
                }
            });
        let chosen = relevant
            .from_options
            .clone()
            .or(from_tag)
            .filter(|value| supported(relevant.key, value))
            .or_else(|| relevant.default.clone());
        if let Some(value) = &chosen {
            let written = if value == "true" {
                Value::default()
            } else {
                Value::try_from_str(value).unwrap_or_default()
            };
            resolved.extensions.unicode.keywords.set(key, written);
        }
        settled.push((relevant.key, chosen));
    }

    // The reported tag carries only the keys that settled on a non-default value the caller can
    // observe; ECMA-402 builds it from the resolved keywords, not from the requested ones, which
    // is why `en-u-ca-gregory` comes back as `en` from a constructor that does not read `ca`.
    id.variants.clear();
    resolved.id = id;
    let icu = resolved.clone();
    let mut reported = resolved;
    for (key, value) in keys.iter().zip(settled.iter()) {
        // A key whose settled value came only from the implementation default is not written into
        // the reported tag: `new Intl.Collator("de").resolvedOptions().locale` is `de`, not
        // `de-u-co-default`.
        if value.1.is_some() && key.default.as_deref() == value.1.as_deref() {
            let k = Key::try_from_str(key.key).expect("a relevant key is a valid BCP 47 key");
            reported.extensions.unicode.keywords.remove(k);
        }
    }
    Resolution {
        locale: reported.to_string(),
        keys: settled,
        icu,
    }
}

impl Resolution {
    /// The locale an ICU formatter should be built from.
    pub(crate) fn icu(&self) -> &Locale {
        &self.icu
    }
}

/// `SupportedLocales`: the requested tags Kiln has data for, in the order they were asked for.
///
/// The tag returned is the one the caller supplied, not the one the match was found at -- that is
/// what makes `supportedLocalesOf(["en-XA"])` answer `["en-XA"]` even though the data behind it is
/// `en`.
pub fn supported_locales(requested: &[String]) -> Vec<String> {
    requested
        .iter()
        .filter(|tag| {
            Locale::try_from_str(tag)
                .ok()
                .and_then(|l| best_available(&l))
                .is_some()
        })
        .cloned()
        .collect()
}

/// The parts of a tag `Intl.Locale` exposes as accessors, in the order it declares them.
pub fn parts(tag: &str) -> Result<Vec<(&'static str, Setting)>, Error> {
    let locale = Locale::try_from_str(tag)
        .map_err(|_| Error::range(format!("Incorrect locale information provided: {tag}")))?;
    let keyword = |name: &str| {
        Key::try_from_str(name)
            .ok()
            .and_then(|k| locale.extensions.unicode.keywords.get(&k).cloned())
            .map(|v| {
                let spelled = v.to_string();
                if spelled.is_empty() {
                    "true".to_string()
                } else {
                    spelled
                }
            })
    };
    let optional = |value: Option<String>| match value {
        Some(value) => Setting::String(value),
        None => Setting::Undefined,
    };
    Ok(vec![
        ("baseName", Setting::string(locale.id.to_string())),
        ("calendar", optional(keyword("ca"))),
        ("caseFirst", optional(keyword("kf"))),
        ("collation", optional(keyword("co"))),
        ("hourCycle", optional(keyword("hc"))),
        ("numberingSystem", optional(keyword("nu"))),
        (
            "numeric",
            Setting::Bool(keyword("kn").is_some_and(|v| v == "true")),
        ),
        ("language", Setting::string(locale.id.language.as_str())),
        (
            "script",
            optional(locale.id.script.map(|s| s.as_str().to_string())),
        ),
        (
            "region",
            optional(locale.id.region.map(|r| r.as_str().to_string())),
        ),
    ])
}

/// `Intl.Locale.prototype.maximize` and `minimize`.
pub fn maximize(tag: &str) -> Result<String, Error> {
    transform(tag, true)
}

/// The `minimize` half of the pair.
pub fn minimize(tag: &str) -> Result<String, Error> {
    transform(tag, false)
}

fn transform(tag: &str, grow: bool) -> Result<String, Error> {
    let mut locale = Locale::try_from_str(tag)
        .map_err(|_| Error::range(format!("Incorrect locale information provided: {tag}")))?;
    let expander = LocaleExpander::new_extended();
    if grow {
        expander.maximize(&mut locale.id);
    } else {
        expander.minimize(&mut locale.id);
    }
    Ok(locale.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(tag: &str) -> String {
        canonicalize(tag).unwrap_or_else(|e| panic!("{tag}: {e}"))
    }

    #[test]
    fn canonicalization_follows_the_cldr_alias_tables() {
        // Every pair here is one the conformance corpus asserts
        // (intl402/Intl/getCanonicalLocales/canonicalized-tags.js and its neighbours).
        for (input, want) in [
            ("de", "de"),
            ("DE-de", "de-DE"),
            ("cmn", "zh"),
            ("CMN-hANS", "zh-Hans"),
            ("cmn-hans-cn", "zh-Hans-CN"),
            ("es-419", "es-419"),
            ("cmn-hans-cn-u-ca-t-ca-x-t-u", "zh-Hans-CN-t-ca-u-ca-x-t-u"),
            ("sgn-GR", "gss"),
            ("ji", "yi"),
            ("de-DD", "de-DE"),
            ("in", "id"),
            ("sl-rozaj-biske-1994", "sl-1994-biske-rozaj"),
            ("da-u-attr-co-search", "da-u-attr-co-search"),
            ("art-lojban", "jbo"),
            ("zh-guoyu", "zh"),
            ("zh-hakka", "hak"),
            ("mo", "ro"),
            ("aar-x-private", "aa-x-private"),
            ("und-Armn-SU", "und-Armn-AM"),
        ] {
            assert_eq!(canon(input), want, "canonicalizing {input}");
        }
    }

    #[test]
    fn keyword_types_canonicalize_to_their_preferred_form() {
        // The half `icu_locale` does not do. `true` is dropped by ICU4X; `yes` has to be aliased
        // to `true` first, which is why both halves live in one function.
        for (input, want) in [
            ("und-u-ca-islamicc", "und-u-ca-islamic-civil"),
            ("und-u-ca-ethiopic-amete-alem", "und-u-ca-ethioaa"),
            ("und-u-ks-primary", "und-u-ks-level1"),
            ("und-u-ks-tertiary", "und-u-ks-level3"),
            ("und-u-ms-imperial", "und-u-ms-uksystem"),
            ("en-u-ca-true", "en-u-ca"),
            ("und-u-kb-yes", "und-u-kb"),
            ("und-u-kn-yes", "und-u-kn"),
            ("und-u-rg-no23", "und-u-rg-no50"),
            ("und-NO-u-sd-no23", "und-NO-u-sd-no50"),
        ] {
            assert_eq!(canon(input), want, "canonicalizing {input}");
        }
    }

    #[test]
    fn a_tag_that_is_not_a_unicode_locale_id_is_a_range_error() {
        // Irregular grandfathered tags and extlangs are structurally invalid, not legacy spellings
        // to be canonicalized -- the distinction the corpus's `invalid-tags.js` turns on.
        for tag in [
            "en_US",
            "en-",
            "",
            "de-DE-",
            "x-private",
            "en-u",
            "1234",
            "abcdefghi",
            "i-klingon",
            "zh-cmn-Hans",
            "en-GB-oed",
            "sgn-BE-FR",
            "de-DE-1996-1996",
        ] {
            let error = canonicalize(tag).expect_err(&format!("{tag} should be rejected"));
            assert_eq!(error.kind, crate::ErrorKind::Range, "rejecting {tag}");
        }
    }

    #[test]
    fn lookup_falls_back_to_the_realms_locale_when_nothing_matches() {
        let none = resolve(&["xx-YY".into()], &[], |_, _| true);
        assert_eq!(none.locale, DEFAULT);
        let empty = resolve(&[], &[], |_, _| true);
        assert_eq!(empty.locale, DEFAULT);
    }

    #[test]
    fn lookup_keeps_the_longest_prefix_it_has_data_for() {
        assert_eq!(resolve(&["de-AT".into()], &[], |_, _| true).locale, "de-AT");
        assert_eq!(
            resolve(&["zh-Hans-CN".into()], &[], |_, _| true).locale,
            "zh-Hans-CN"
        );
        // No data for `xx`, so the whole tag is discarded rather than truncated to `und`.
        assert_eq!(
            resolve(&["xx-Latn-DE".into()], &[], |_, _| true).locale,
            DEFAULT
        );
    }

    #[test]
    fn an_irrelevant_extension_key_does_not_survive_into_the_resolved_locale() {
        // `ca` means nothing to a number formatter, so `en-u-ca-gregory` resolves to plain `en`.
        let resolved = resolve(
            &["en-u-ca-gregory".into()],
            &[RelevantKey::new("nu")],
            |_, _| true,
        );
        assert_eq!(resolved.locale, "en");
    }

    #[test]
    fn a_relevant_extension_key_survives_and_is_reported() {
        let keys = [RelevantKey::new("nu")];
        let resolved = resolve(&["th-u-nu-thai".into()], &keys, |_, _| true);
        assert_eq!(resolved.locale, "th-u-nu-thai");
        assert_eq!(resolved.value("nu"), Some("thai"));
    }

    #[test]
    fn an_unsupported_keyword_value_is_dropped_rather_than_reported() {
        // The failure this guards against is `resolvedOptions()` naming a numbering system the
        // formatter then does not use -- a claim that is wrong rather than missing.
        let keys = [RelevantKey::new("nu")];
        let resolved = resolve(&["th-u-nu-latn".into()], &keys, |_, value| value == "thai");
        assert_eq!(resolved.locale, "th");
        assert_eq!(resolved.value("nu"), None);
    }

    #[test]
    fn the_options_object_outranks_the_extension_it_conflicts_with() {
        let keys = [RelevantKey::new("nu").with_option(Some("arab".into()))];
        let resolved = resolve(&["th-u-nu-thai".into()], &keys, |_, _| true);
        assert_eq!(resolved.value("nu"), Some("arab"));
        assert_eq!(resolved.locale, "th-u-nu-arab");
    }

    #[test]
    fn supported_locales_echoes_the_requested_tag_not_the_matched_one() {
        let asked = ["de-AT".into(), "cop".into(), "en-XA".into(), "xx".into()];
        assert_eq!(
            supported_locales(&asked),
            vec!["de-AT".to_string(), "en-XA".to_string()]
        );
    }

    #[test]
    fn locale_parts_expose_the_subtags_and_keywords_separately() {
        let parts = parts("ja-Jpan-JP-u-ca-japanese-kn").unwrap();
        let get = |name: &str| parts.iter().find(|(k, _)| *k == name).unwrap().1.clone();
        assert_eq!(get("baseName"), Setting::string("ja-Jpan-JP"));
        assert_eq!(get("language"), Setting::string("ja"));
        assert_eq!(get("script"), Setting::string("Jpan"));
        assert_eq!(get("region"), Setting::string("JP"));
        assert_eq!(get("calendar"), Setting::string("japanese"));
        assert_eq!(get("numeric"), Setting::Bool(true));
        assert_eq!(get("collation"), Setting::Undefined);
    }

    #[test]
    fn maximize_and_minimize_round_trip_through_likely_subtags() {
        assert_eq!(maximize("zh-CN").unwrap(), "zh-Hans-CN");
        assert_eq!(minimize("zh-Hans-CN").unwrap(), "zh");
        assert_eq!(maximize("und-US").unwrap(), "en-Latn-US");
        // A known divergence, recorded rather than papered over: CLDR's likely-subtags table maps
        // bare `und` to `en-Latn-US` and `icu_locale` deliberately leaves it alone, reading `und`
        // as "no information to expand" rather than as a locale. Special-casing the one input the
        // corpus asks about would fix the assertion and leave `und-Zzzz-ZZ` next to it wrong.
        assert_eq!(maximize("und").unwrap(), "und");
    }
}

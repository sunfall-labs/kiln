//! `Intl.Collator` and, through it, `String.prototype.localeCompare`.
//!
//! Collation is the clearest case in the crate for linking rather than reimplementing. The order
//! is a table of primary, secondary and tertiary weights for every assigned code point, plus
//! per-language tailorings, plus a normalization pass -- and the failure mode of getting it
//! slightly wrong is a `sort()` that puts two words in the wrong order without ever raising an
//! error. Kiln's current `localeCompare` compares UTF-16 code units, which is a *defensible* wrong
//! answer only as long as nothing claims otherwise.

use core::cmp::Ordering;

use icu_collator::options::{AlternateHandling, CaseLevel, CollatorOptions, Strength};
use icu_collator::preferences::{CollationCaseFirst, CollationNumericOrdering};
use icu_collator::{Collator as IcuCollator, CollatorBorrowed, CollatorPreferences};

use crate::locale::{DEFAULT, RelevantKey, Resolution, resolve_with_default};
use crate::{Error, Setting};

/// The accepted values of `usage`.
pub const USAGES: &[&str] = &["sort", "search"];
/// The accepted values of `sensitivity`.
pub const SENSITIVITIES: &[&str] = &["base", "accent", "case", "variant"];
/// The accepted values of `caseFirst`.
pub const CASE_FIRSTS: &[&str] = &["upper", "lower", "false"];

/// The options a caller has already read off the JavaScript options object.
#[derive(Clone, Debug)]
pub struct Options {
    /// `usage`.
    pub usage: String,
    /// `collation`, from the options object rather than the `-u-co-` extension.
    pub collation: Option<String>,
    /// `numeric`, `None` when the options object did not say.
    pub numeric: Option<bool>,
    /// `caseFirst`, `None` when the options object did not say.
    pub case_first: Option<String>,
    /// `sensitivity`, `None` when the options object did not say.
    pub sensitivity: Option<String>,
    /// `ignorePunctuation`, `None` when the options object did not say.
    pub ignore_punctuation: Option<bool>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            usage: "sort".into(),
            collation: None,
            numeric: None,
            case_first: None,
            sensitivity: None,
            ignore_punctuation: None,
        }
    }
}

/// The collation types `Intl.supportedValuesOf("collation")` reports.
///
/// Empty, and deliberately so. ICU4X's baked collator data is a lookup rather than an enumeration,
/// and there is no probe that distinguishes "this build has tailoring data for `phonebk`" from
/// "this build fell back to the root collation and produced the same order anyway" -- a collator
/// built for an unknown collation succeeds and sorts, it just sorts the default way. Reporting a
/// guessed list would be claiming data this crate cannot confirm it has, which is the wrong-answer
/// failure mode; an empty list understates and says nothing false. `Intl.Collator` itself still
/// honours `-u-co-` for the collations the bundle does carry.
pub fn available_collations() -> Vec<String> {
    Vec::new()
}

/// A built collator.
pub struct Collator {
    collator: CollatorBorrowed<'static>,
    resolution: Resolution,
    usage: String,
    sensitivity: String,
    ignore_punctuation: bool,
}

impl core::fmt::Debug for Collator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Collator")
            .field("locale", &self.resolution.locale)
            .field("sensitivity", &self.sensitivity)
            .finish_non_exhaustive()
    }
}

impl Collator {
    /// Build a collator.
    pub fn new(requested: &[String], options: Options) -> Result<Self, Error> {
        Self::new_with_default(requested, DEFAULT, options)
    }

    /// Build with the default locale selected by the containing realm.
    pub fn new_with_default(
        requested: &[String],
        default: &str,
        options: Options,
    ) -> Result<Self, Error> {
        let keys = [
            RelevantKey::new("co").with_option(options.collation.clone()),
            RelevantKey::new("kn").with_option(options.numeric.map(|n| n.to_string())),
            RelevantKey::new("kf").with_option(options.case_first.clone()),
        ];
        // `standard` and `search` are the two collation types ECMA-402 refuses in the `-u-co-`
        // extension: `search` is selected by `usage` instead, and `standard` is the absence of a
        // tailoring rather than a tailoring you can ask for.
        let resolution = resolve_with_default(requested, default, &keys, |key, value| match key {
            "co" => !matches!(value, "standard" | "search"),
            "kn" => matches!(value, "true" | "false"),
            "kf" => CASE_FIRSTS.contains(&value),
            _ => false,
        });

        let mut preferences = CollatorPreferences::from(resolution.icu());
        if let Some(numeric) = resolution.value("kn") {
            preferences.numeric_ordering = Some(if numeric == "true" {
                CollationNumericOrdering::True
            } else {
                CollationNumericOrdering::False
            });
        }
        if let Some(case_first) = resolution.value("kf") {
            preferences.case_first = Some(match case_first {
                "upper" => CollationCaseFirst::Upper,
                "lower" => CollationCaseFirst::Lower,
                _ => CollationCaseFirst::False,
            });
        }

        // The four sensitivities are two independent switches wearing one name: how deep the
        // weight comparison goes, and whether case is compared separately from accent.
        let sensitivity = options
            .sensitivity
            .clone()
            .unwrap_or_else(|| "variant".into());
        let mut collator_options = CollatorOptions::default();
        collator_options.strength = Some(match sensitivity.as_str() {
            "base" | "case" => Strength::Primary,
            "accent" => Strength::Secondary,
            _ => Strength::Tertiary,
        });
        collator_options.case_level = Some(if sensitivity == "case" {
            CaseLevel::On
        } else {
            CaseLevel::Off
        });

        // ECMA-402 states this default in spec text rather than leaving it to the data: true for
        // Thai, false everywhere else.
        let ignore_punctuation = options
            .ignore_punctuation
            .unwrap_or_else(|| resolution.locale.starts_with("th"));
        collator_options.alternate_handling = Some(if ignore_punctuation {
            AlternateHandling::Shifted
        } else {
            AlternateHandling::NonIgnorable
        });

        let collator = IcuCollator::try_new(preferences, collator_options)
            .map_err(|_| Error::range("no collation data for the resolved locale"))?;
        Ok(Self {
            collator,
            resolution,
            usage: options.usage,
            sensitivity,
            ignore_punctuation,
        })
    }

    /// `compare(x, y)`, over UTF-16 code units.
    ///
    /// The code units rather than a `&str` for the reason ADR 0025 gave the regexp provider the
    /// same shape: the language counts in code units and a JavaScript string may contain an
    /// unpaired surrogate, which has no `&str` spelling. ICU4X maps an unpaired surrogate to
    /// U+FFFD, which is a defined answer rather than a panic or a truncation.
    pub fn compare(&self, left: &[u16], right: &[u16]) -> Ordering {
        self.collator.compare_utf16(left, right)
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        vec![
            ("locale", Setting::string(&self.resolution.locale)),
            ("usage", Setting::string(&self.usage)),
            ("sensitivity", Setting::string(&self.sensitivity)),
            ("ignorePunctuation", Setting::Bool(self.ignore_punctuation)),
            (
                "collation",
                Setting::string(self.resolution.value("co").unwrap_or("default")),
            ),
            (
                "numeric",
                Setting::Bool(self.resolution.value("kn") == Some("true")),
            ),
            (
                "caseFirst",
                Setting::string(self.resolution.value("kf").unwrap_or("false")),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn compare(locale: &str, options: Options, left: &str, right: &str) -> Ordering {
        Collator::new(&[locale.to_string()], options)
            .expect("the collator builds")
            .compare(&units(left), &units(right))
    }

    #[test]
    fn accents_sort_after_the_base_letter_rather_than_after_z() {
        // Code-unit order puts every accented letter after every unaccented one, which is the
        // answer this module exists to replace.
        assert_eq!(
            compare("en-US", Options::default(), "resume", "r\u{e9}sum\u{e9}"),
            Ordering::Less
        );
        assert_eq!(
            compare("en-US", Options::default(), "r\u{e9}sum\u{e9}", "z"),
            Ordering::Less
        );
    }

    #[test]
    fn a_locale_tailoring_changes_the_order() {
        // Swedish sorts a-ring after z; German sorts a-umlaut with a. One table, two answers.
        assert_eq!(
            compare("sv", Options::default(), "\u{e5}", "z"),
            Ordering::Greater
        );
        assert_eq!(
            compare("de", Options::default(), "\u{e4}", "z"),
            Ordering::Less
        );
    }

    #[test]
    fn sensitivity_decides_which_differences_count() {
        let with = |sensitivity: &str, left: &str, right: &str| {
            compare(
                "en-US",
                Options {
                    sensitivity: Some(sensitivity.into()),
                    ..Options::default()
                },
                left,
                right,
            )
        };
        assert_eq!(with("base", "a", "\u{e1}"), Ordering::Equal);
        assert_eq!(with("base", "a", "A"), Ordering::Equal);
        assert_eq!(with("accent", "a", "\u{e1}"), Ordering::Less);
        assert_eq!(with("accent", "a", "A"), Ordering::Equal);
        assert_eq!(with("case", "a", "\u{e1}"), Ordering::Equal);
        assert_eq!(with("case", "a", "A"), Ordering::Less);
        assert_eq!(with("variant", "a", "\u{e1}"), Ordering::Less);
        assert_eq!(with("variant", "a", "A"), Ordering::Less);
    }

    #[test]
    fn numeric_collation_compares_runs_of_digits_as_numbers() {
        let numeric = Options {
            numeric: Some(true),
            ..Options::default()
        };
        assert_eq!(compare("en-US", numeric, "item2", "item10"), Ordering::Less);
        assert_eq!(
            compare("en-US", Options::default(), "item2", "item10"),
            Ordering::Greater
        );
    }

    #[test]
    fn the_numeric_extension_reaches_the_collator_and_is_reported() {
        let collator = Collator::new(&["en-US-u-kn-true".into()], Options::default()).unwrap();
        assert_eq!(
            collator.compare(&units("item2"), &units("item10")),
            Ordering::Less
        );
        let resolved = collator.resolved_options();
        assert!(resolved.contains(&("numeric", Setting::Bool(true))));
        assert_eq!(
            collator.resolved_options()[0].1,
            Setting::string("en-US-u-kn")
        );
    }

    #[test]
    fn a_collation_the_extension_may_not_name_is_dropped() {
        // `-u-co-search` and `-u-co-standard` are refused by the specification, so they must not
        // reach `resolvedOptions().collation`.
        for refused in ["search", "standard"] {
            let collator =
                Collator::new(&[format!("de-u-co-{refused}")], Options::default()).unwrap();
            let resolved = collator.resolved_options();
            assert!(resolved.contains(&("collation", Setting::string("default"))));
        }
        let phonebook = Collator::new(&["de-u-co-phonebk".into()], Options::default()).unwrap();
        assert!(
            phonebook
                .resolved_options()
                .contains(&("collation", Setting::string("phonebk")))
        );
    }

    #[test]
    fn ignore_punctuation_defaults_the_way_the_specification_states_it() {
        let english = Collator::new(&["en-US".into()], Options::default()).unwrap();
        assert!(
            english
                .resolved_options()
                .contains(&("ignorePunctuation", Setting::Bool(false)))
        );
        let thai = Collator::new(&["th".into()], Options::default()).unwrap();
        assert!(
            thai.resolved_options()
                .contains(&("ignorePunctuation", Setting::Bool(true)))
        );
    }

    #[test]
    fn an_unpaired_surrogate_compares_rather_than_panics() {
        let lone = [0xd800u16];
        let a = units("a");
        // The answer is whatever U+FFFD compares as; the point is that there is one.
        let _ = Collator::new(&["en-US".into()], Options::default())
            .unwrap()
            .compare(&lone, &a);
    }
}

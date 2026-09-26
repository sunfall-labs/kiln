//! `Intl.DisplayNames`.
//!
//! "What is `de` called, in French?" is a table lookup and nothing else, which is why it is here
//! rather than implemented: the table is CLDR's, it has an entry per language per display locale,
//! and its interesting cases (a dialect name such as "American English" versus the assembled
//! "English (United States)") are a documented algorithm over that table rather than a formatting
//! rule anyone can restate.
//!
//! # What this build has names for
//!
//! ECMA-402 names six `type`s. ICU4X carries language, region and script; it does not carry
//! currency, calendar or date-time-field names. Those three are accepted as types -- refusing them
//! outright would be wrong, since the specification defines them -- and answer through the
//! specification's own `fallback` option: a code with no name resolves to the code itself under
//! `fallback: "code"` and to `undefined` under `fallback: "none"`, which is exactly what a
//! conforming implementation without a name for that particular code does. The difference from a
//! full-data engine is discoverable (`of("gregory")` is `"gregory"` rather than `"Gregorian
//! Calendar"`) and is never a *wrong* name.

use icu_experimental::displaynames::multi::{
    LocaleDisplayNamesFormatter, RegionDisplayNames, ScriptDisplayNames,
};
use icu_experimental::displaynames::{DisplayNamesOptions, Fallback, LanguageDisplay, Style};
use icu_locale_core::Locale;
use icu_locale_core::subtags::{Region, Script};

use crate::locale::{Resolution, resolve};
use crate::{Error, Setting};

/// The accepted values of `type`.
pub const TYPES: &[&str] = &[
    "language",
    "region",
    "script",
    "currency",
    "calendar",
    "dateTimeField",
];
/// The accepted values of `style`.
pub const STYLES: &[&str] = &["narrow", "short", "long"];
/// The accepted values of `fallback`.
pub const FALLBACKS: &[&str] = &["code", "none"];
/// The accepted values of `languageDisplay`.
pub const LANGUAGE_DISPLAYS: &[&str] = &["dialect", "standard"];

/// The `dateTimeField` codes the language names.
const DATE_TIME_FIELDS: &[&str] = &[
    "era",
    "year",
    "quarter",
    "month",
    "weekOfYear",
    "weekday",
    "day",
    "dayPeriod",
    "hour",
    "minute",
    "second",
    "timeZoneName",
];

/// The options, already read off the options object.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `style`.
    pub style: Option<String>,
    /// `type`, which is required and has no default.
    pub kind: String,
    /// `fallback`.
    pub fallback: Option<String>,
    /// `languageDisplay`.
    pub language_display: Option<String>,
}

/// A built display-name lookup.
pub struct DisplayNames {
    locales: Option<LocaleDisplayNamesFormatter>,
    regions: Option<RegionDisplayNames>,
    scripts: Option<ScriptDisplayNames>,
    resolution: Resolution,
    kind: String,
    style: String,
    fallback: String,
    language_display: String,
}

impl core::fmt::Debug for DisplayNames {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DisplayNames")
            .field("locale", &self.resolution.locale)
            .field("type", &self.kind)
            .field("style", &self.style)
            .finish_non_exhaustive()
    }
}

impl DisplayNames {
    /// Build a lookup for one `type`.
    pub fn new(requested: &[String], options: Options) -> Result<Self, Error> {
        let resolution = resolve(requested, &[], |_, _| false);
        let style = options.style.clone().unwrap_or_else(|| "long".to_owned());
        let fallback = options
            .fallback
            .clone()
            .unwrap_or_else(|| "code".to_owned());
        let language_display = options
            .language_display
            .clone()
            .unwrap_or_else(|| "dialect".to_owned());
        let mut bag = DisplayNamesOptions::default();
        bag.style = match style.as_str() {
            "narrow" => Some(Style::Narrow),
            "short" => Some(Style::Short),
            _ => Some(Style::Long),
        };
        // The language's own `fallback` is applied in `of` rather than here: the adapter has to
        // know whether a name was actually found in order to echo the code back, and ICU4X's own
        // fallback would hide the difference.
        bag.fallback = Fallback::None;
        bag.language_display = if language_display == "standard" {
            LanguageDisplay::Standard
        } else {
            LanguageDisplay::Dialect
        };
        let preferences = resolution.icu().clone().into();
        let (mut locales, mut regions, mut scripts) = (None, None, None);
        match options.kind.as_str() {
            "language" => {
                locales = Some(
                    LocaleDisplayNamesFormatter::try_new(preferences, bag)
                        .map_err(|_| Error::range("no display name data for the locale"))?,
                );
            }
            "region" => {
                regions = Some(
                    RegionDisplayNames::try_new(preferences, bag)
                        .map_err(|_| Error::range("no display name data for the locale"))?,
                );
            }
            "script" => {
                scripts = Some(
                    ScriptDisplayNames::try_new(preferences, bag)
                        .map_err(|_| Error::range("no display name data for the locale"))?,
                );
            }
            // `currency`, `calendar` and `dateTimeField` have no table in this build; the code
            // check still runs and the `fallback` option still decides the answer.
            _ => {}
        }
        Ok(Self {
            locales,
            regions,
            scripts,
            resolution,
            kind: options.kind,
            style,
            fallback,
            language_display,
        })
    }

    /// `of(code)`.
    ///
    /// A code that is not well formed for the type is a `RangeError` -- that check is the
    /// specification's, and it happens whether or not there is a name behind the code. `Ok(None)`
    /// means "well formed, no name", which the caller turns into `undefined` or into the code
    /// itself depending on `fallback`.
    pub fn of(&self, code: &str) -> Result<Option<String>, Error> {
        let canonical = self.canonical_code(code)?;
        let found = match self.kind.as_str() {
            "language" => Locale::try_from_str(&canonical)
                .ok()
                .and_then(|locale| {
                    self.locales
                        .as_ref()
                        .map(|names| names.of(&locale).into_owned())
                })
                .filter(|name| *name != canonical),
            "region" => Region::try_from_str(&canonical).ok().and_then(|region| {
                self.regions
                    .as_ref()
                    .and_then(|names| names.of(region))
                    .map(str::to_owned)
            }),
            "script" => Script::try_from_str(&canonical).ok().and_then(|script| {
                self.scripts
                    .as_ref()
                    .and_then(|names| names.of(script))
                    .map(str::to_owned)
            }),
            _ => None,
        };
        Ok(match found {
            Some(name) => Some(name),
            None if self.fallback == "code" => Some(canonical),
            None => None,
        })
    }

    /// `IsValidCodeForDisplayNames`, plus the canonical spelling the answer is keyed by.
    fn canonical_code(&self, code: &str) -> Result<String, Error> {
        let invalid = || Error::range(format!("invalid value {code} for option type"));
        match self.kind.as_str() {
            "language" => {
                // A `unicode_language_id`, not a full locale: `IsValidCodeForDisplayNames` names
                // the language-script-region-variant grammar, so `en-a-bcd` -- perfectly good as a
                // *tag* -- is not a language, and a singleton subtag is a RangeError here.
                if !crate::locale::is_language_identifier(code) {
                    return Err(invalid());
                }
                crate::locale::canonicalize(code)
            }
            "region" => {
                // Two letters or three digits, and the letters are reported upper case.
                let alphabetic = code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic());
                let numeric = code.len() == 3 && code.chars().all(|c| c.is_ascii_digit());
                if !alphabetic && !numeric {
                    return Err(invalid());
                }
                Ok(code.to_ascii_uppercase())
            }
            "script" => {
                if code.len() != 4 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
                    return Err(invalid());
                }
                let mut spelled = code.to_ascii_lowercase();
                spelled[..1].make_ascii_uppercase();
                Ok(spelled)
            }
            "currency" => {
                if code.len() != 3 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
                    return Err(invalid());
                }
                Ok(code.to_ascii_uppercase())
            }
            "calendar" => {
                // A well-formed Unicode `type`, with the `-u-ca-` alias table applied so that
                // `islamicc` and `islamic-civil` are one question rather than two.
                let well_formed = !code.is_empty()
                    && code.split('-').all(|part| {
                        (3..=8).contains(&part.len())
                            && part.chars().all(|c| c.is_ascii_alphanumeric())
                    });
                if !well_formed {
                    return Err(invalid());
                }
                Ok(crate::locale::canonicalize_keyword_type(
                    "ca",
                    &code.to_ascii_lowercase(),
                ))
            }
            "dateTimeField" => {
                if !DATE_TIME_FIELDS.contains(&code) {
                    return Err(invalid());
                }
                Ok(code.to_owned())
            }
            _ => Err(invalid()),
        }
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        let mut settings = vec![
            ("locale", Setting::string(&self.resolution.locale)),
            ("style", Setting::string(&self.style)),
            ("type", Setting::string(&self.kind)),
            ("fallback", Setting::string(&self.fallback)),
        ];
        // `languageDisplay` is reported only for the type it means anything for.
        if self.kind == "language" {
            settings.push(("languageDisplay", Setting::string(&self.language_display)));
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(kind: &str) -> DisplayNames {
        DisplayNames::new(
            &["en-US".to_owned()],
            Options {
                kind: kind.to_owned(),
                ..Options::default()
            },
        )
        .expect("the lookup builds")
    }

    #[test]
    fn a_display_name_is_the_locales_word_for_the_code() {
        assert_eq!(
            names("region").of("US").unwrap().as_deref(),
            Some("United States")
        );
        assert_eq!(
            names("script").of("Latn").unwrap().as_deref(),
            Some("Latin")
        );
        let french = DisplayNames::new(
            &["fr".to_owned()],
            Options {
                kind: "region".to_owned(),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(french.of("US").unwrap().as_deref(), Some("\u{c9}tats-Unis"));
    }

    #[test]
    fn a_language_name_is_the_dialect_name_rather_than_an_assembled_one() {
        // The reason this is a documented algorithm over a table rather than concatenation:
        // `en-US` has its own name, and `en-Latn-US` does not.
        assert_eq!(
            names("language").of("en-US").unwrap().as_deref(),
            Some("American English")
        );
        let standard = DisplayNames::new(
            &["en-US".to_owned()],
            Options {
                kind: "language".to_owned(),
                language_display: Some("standard".to_owned()),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(
            standard.of("en-US").unwrap().as_deref(),
            Some("English (United States)")
        );
    }

    #[test]
    fn a_code_the_type_cannot_take_is_a_range_error() {
        for (kind, code) in [
            ("region", "USA"),
            ("region", "1"),
            ("script", "Lat"),
            ("currency", "US"),
            ("dateTimeField", "fortnight"),
            ("language", "en_US"),
            // A tag with an extension is a tag and not a language identifier.
            ("language", "en-a-bcd"),
            ("language", "en-u-ca-gregory"),
        ] {
            let error = names(kind)
                .of(code)
                .expect_err(&format!("{kind}/{code} should be refused"));
            assert_eq!(error.kind, crate::ErrorKind::Range);
        }
    }

    #[test]
    fn a_missing_name_follows_the_fallback_option_rather_than_inventing_one() {
        let coded = names("currency");
        // No currency table in this build, so `code` gives the code back, upper-cased.
        assert_eq!(coded.of("usd").unwrap().as_deref(), Some("USD"));
        let none = DisplayNames::new(
            &["en-US".to_owned()],
            Options {
                kind: "currency".to_owned(),
                fallback: Some("none".to_owned()),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(none.of("USD").unwrap(), None);
    }

    #[test]
    fn resolved_options_report_language_display_only_where_it_means_something() {
        let language = names("language").resolved_options();
        assert!(language.iter().any(|(key, _)| *key == "languageDisplay"));
        let region = names("region").resolved_options();
        assert!(!region.iter().any(|(key, _)| *key == "languageDisplay"));
    }
}

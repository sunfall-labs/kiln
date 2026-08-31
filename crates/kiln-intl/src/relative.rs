//! `Intl.RelativeTimeFormat`.
//!
//! "3 days ago" is not "3" plus a word. Polish needs one phrase for 1, another for 2-4 and a third
//! for 5 and up; Arabic has a dual; English has "yesterday" for -1 day and nothing for -1 week.
//! That is the plural-selected pattern table CLDR carries per unit per width, and it is what this
//! module gets from ICU4X. What it adds is ECMA-402's own vocabulary: the seven units the language
//! names (each in singular and plural spelling), the `numeric: "auto"` switch, and the split of a
//! finished phrase into the parts `formatToParts` reports.

use fixed_decimal::Decimal;
use icu_experimental::relativetime::RelativeTimeFormatter;
use icu_experimental::relativetime::options::{Numeric, RelativeTimeFormatterOptions};

use crate::locale::{RelevantKey, Resolution, resolve};
use crate::{Error, Setting};

/// The accepted values of `style`.
pub const STYLES: &[&str] = &["long", "short", "narrow"];
/// The accepted values of `numeric`.
pub const NUMERICS: &[&str] = &["always", "auto"];

/// The units `format` accepts, singular first.
///
/// The language accepts both spellings of each -- `"day"` and `"days"` -- and reports neither in
/// `resolvedOptions()`, so the pair is only ever a vocabulary check and a normalisation.
const UNITS: &[(&str, &str)] = &[
    ("year", "years"),
    ("quarter", "quarters"),
    ("month", "months"),
    ("week", "weeks"),
    ("day", "days"),
    ("hour", "hours"),
    ("minute", "minutes"),
    ("second", "seconds"),
];

/// The singular form of a unit the language named, or `None` if it named no unit at all.
pub fn canonical_unit(unit: &str) -> Option<&'static str> {
    UNITS
        .iter()
        .find(|(singular, plural)| *singular == unit || *plural == unit)
        .map(|(singular, _)| *singular)
}

/// The options `Intl.RelativeTimeFormat` takes, already read off the options object.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `numberingSystem`.
    pub numbering_system: Option<String>,
    /// `style`.
    pub style: Option<String>,
    /// `numeric`.
    pub numeric: Option<String>,
}

/// A built relative-time formatter.
///
/// ICU4X's constructors are one per unit per width -- `try_new_long_day`, `try_new_short_year` and
/// twenty-two others -- because each loads a different data key. The language chooses the unit at
/// *format* time rather than at construction, so all eight for the chosen width are built up front
/// and the one the caller asks for is picked out of them. That is eight data loads rather than one
/// and it is the price of the language's shape; building lazily would mean a formatter that can
/// throw on its fourth call and not its third.
pub struct RelativeTimeFormat {
    formatters: Vec<(&'static str, RelativeTimeFormatter)>,
    resolution: Resolution,
    numbering_system: String,
    style: String,
    numeric: String,
}

impl core::fmt::Debug for RelativeTimeFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RelativeTimeFormat")
            .field("locale", &self.resolution.locale)
            .field("style", &self.style)
            .field("numeric", &self.numeric)
            .finish_non_exhaustive()
    }
}

impl RelativeTimeFormat {
    /// Build a formatter.
    pub fn new(requested: &[String], options: Options) -> Result<Self, Error> {
        let keys = [RelevantKey::new("nu").with_option(options.numbering_system.clone())];
        let resolution = resolve(requested, &keys, |key, value| {
            key == "nu" && crate::number::numbering_system_is_supported(value)
        });
        let style = options.style.clone().unwrap_or_else(|| "long".to_owned());
        let numeric = options
            .numeric
            .clone()
            .unwrap_or_else(|| "always".to_owned());
        let mut bag = RelativeTimeFormatterOptions::default();
        bag.numeric = if numeric == "auto" {
            Numeric::Auto
        } else {
            Numeric::Always
        };
        let preferences = resolution.icu().clone().into();
        // A macro rather than a table of function pointers: ICU4X's constructors are twenty-four
        // separate functions (one per unit per width) whose error type this crate would otherwise
        // have to name, and naming it would put `icu_provider` in the dependency list for the sake
        // of one `Result`.
        macro_rules! formatters {
            ($($unit:literal => ($long:ident, $short:ident, $narrow:ident)),* $(,)?) => {{
                let mut built: Vec<(&'static str, RelativeTimeFormatter)> = Vec::new();
                $(
                    let one = match style.as_str() {
                        "short" => RelativeTimeFormatter::$short(preferences, bag),
                        "narrow" => RelativeTimeFormatter::$narrow(preferences, bag),
                        _ => RelativeTimeFormatter::$long(preferences, bag),
                    }
                    .map_err(|_| Error::range("no relative time data for the resolved locale"))?;
                    built.push(($unit, one));
                )*
                built
            }};
        }
        let formatters = formatters![
            "year" => (try_new_long_year, try_new_short_year, try_new_narrow_year),
            "quarter" => (try_new_long_quarter, try_new_short_quarter, try_new_narrow_quarter),
            "month" => (try_new_long_month, try_new_short_month, try_new_narrow_month),
            "week" => (try_new_long_week, try_new_short_week, try_new_narrow_week),
            "day" => (try_new_long_day, try_new_short_day, try_new_narrow_day),
            "hour" => (try_new_long_hour, try_new_short_hour, try_new_narrow_hour),
            "minute" => (try_new_long_minute, try_new_short_minute, try_new_narrow_minute),
            "second" => (try_new_long_second, try_new_short_second, try_new_narrow_second),
        ];
        let numbering_system = resolution.value("nu").unwrap_or("latn").to_owned();
        Ok(Self {
            formatters,
            resolution,
            numbering_system,
            style,
            numeric,
        })
    }

    fn formatter(&self, unit: &str) -> Result<&RelativeTimeFormatter, Error> {
        self.formatters
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, formatter)| formatter)
            .ok_or_else(|| Error::range(format!("invalid unit: {unit}")))
    }

    /// `format(value, unit)`. `unit` is already the singular form.
    pub fn format(&self, value: f64, unit: &str) -> Result<String, Error> {
        let formatter = self.formatter(unit)?;
        Ok(formatter.format(decimal(value)).to_string())
    }

    /// `formatToParts(value, unit)`.
    ///
    /// The parts are `(type, value, unit)`, where `unit` is set only on the runs that make up the
    /// number -- that is ECMA-402's shape, and the `unit` field is what distinguishes a relative
    /// time's parts from a plain number's.
    ///
    /// ICU4X annotates the whole phrase as one run when it comes from the literal table
    /// (`yesterday`) and otherwise interpolates a formatted number into a pattern without
    /// annotating the seam. The number is therefore located rather than guessed at: the same
    /// decimal formatter is asked for the numeral on its own, and the phrase is split at the one
    /// place that numeral occurs. When it occurs zero times -- which is exactly the literal case --
    /// the whole phrase is one literal, which is also the right answer.
    pub fn format_to_parts(
        &self,
        value: f64,
        unit: &str,
    ) -> Result<Vec<(&'static str, String, bool)>, Error> {
        let formatter = self.formatter(unit)?;
        let phrase = formatter.format(decimal(value)).to_string();
        let numeral = crate::number::NumberFormat::new(
            std::slice::from_ref(&self.resolution.locale),
            crate::number::Options {
                numbering_system: Some(self.numbering_system.clone()),
                ..crate::number::Options::default()
            },
        )
        .and_then(|formatter| {
            formatter.format_to_parts(&crate::number::Numeric::Double(value.abs()))
        })
        .unwrap_or_default();
        let spelled: String = numeral.iter().map(|(_, text)| text.as_str()).collect();
        let Some(at) = phrase.find(&spelled).filter(|_| !spelled.is_empty()) else {
            return Ok(vec![("literal", phrase, false)]);
        };
        let mut parts: Vec<(&'static str, String, bool)> = Vec::new();
        if at > 0 {
            parts.push(("literal", phrase[..at].to_owned(), false));
        }
        for (kind, text) in numeral {
            parts.push((kind, text, true));
        }
        let rest = &phrase[at + spelled.len()..];
        if !rest.is_empty() {
            parts.push(("literal", rest.to_owned(), false));
        }
        Ok(parts)
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        vec![
            ("locale", Setting::string(&self.resolution.locale)),
            ("style", Setting::string(&self.style)),
            ("numeric", Setting::string(&self.numeric)),
            ("numberingSystem", Setting::string(&self.numbering_system)),
        ]
    }
}

/// The value as a decimal, keeping the digits a double actually has -- and its sign.
///
/// `-0` is not a rounding artefact here: the language distinguishes `format(0, "second")` from
/// `format(-0, "second")` because one is "in 0 seconds" and the other is "0 seconds ago", so the
/// negative zero has to survive into the decimal rather than being normalised away.
fn decimal(value: f64) -> Decimal {
    // `Decimal` has no exact `f64` constructor, and the specification's own conversion is
    // `ToString` on the Number, so the shortest round-tripping spelling is the right input: it is
    // the same digits `String(value)` would show.
    let text = if value == 0.0 && value.is_sign_negative() {
        "-0".to_owned()
    } else {
        format!("{value}")
    };
    text.parse().unwrap_or_else(|_| Decimal::from(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatter(locale: &str, style: &str, numeric: &str) -> RelativeTimeFormat {
        RelativeTimeFormat::new(
            &[locale.to_owned()],
            Options {
                style: Some(style.to_owned()),
                numeric: Some(numeric.to_owned()),
                ..Options::default()
            },
        )
        .expect("the formatter builds")
    }

    #[test]
    fn a_relative_time_is_a_phrase_rather_than_a_number_and_a_word() {
        let long = formatter("en-US", "long", "always");
        assert_eq!(long.format(-1.0, "day").unwrap(), "1 day ago");
        assert_eq!(long.format(3.0, "day").unwrap(), "in 3 days");
        let auto = formatter("en-US", "long", "auto");
        // The whole point of `numeric: "auto"`: -1 day is a word, not a count.
        assert_eq!(auto.format(-1.0, "day").unwrap(), "yesterday");
        assert_eq!(auto.format(0.0, "day").unwrap(), "today");
        // A negative zero is a direction, not a rounding artefact: the language distinguishes the
        // two and so must the decimal handed to ICU4X.
        assert_eq!(long.format(0.0, "second").unwrap(), "in 0 seconds");
        assert_eq!(long.format(-0.0, "second").unwrap(), "0 seconds ago");
        assert_eq!(auto.format(-1.0, "week").unwrap(), "last week");
    }

    #[test]
    fn the_plural_form_is_the_locales_rather_than_a_final_s() {
        // Polish splits 2-4 from 5-and-up, which no amount of appending an `s` discovers.
        let polish = formatter("pl", "long", "always");
        assert_eq!(polish.format(1.0, "day").unwrap(), "za 1 dzie\u{144}");
        assert_eq!(polish.format(3.0, "day").unwrap(), "za 3 dni");
        assert_eq!(polish.format(5.0, "day").unwrap(), "za 5 dni");
    }

    #[test]
    fn format_to_parts_marks_the_number_and_leaves_the_rest_literal() {
        let long = formatter("en-US", "long", "always");
        assert_eq!(
            long.format_to_parts(3.0, "day").unwrap(),
            vec![
                ("literal", "in ".to_owned(), false),
                ("integer", "3".to_owned(), true),
                ("literal", " days".to_owned(), false),
            ]
        );
        // A phrase with no number in it is one literal run rather than an invented split.
        let auto = formatter("en-US", "long", "auto");
        assert_eq!(
            auto.format_to_parts(-1.0, "day").unwrap(),
            vec![("literal", "yesterday".to_owned(), false)]
        );
    }

    #[test]
    fn a_unit_the_language_does_not_name_is_refused() {
        assert_eq!(canonical_unit("days"), Some("day"));
        assert_eq!(canonical_unit("day"), Some("day"));
        assert_eq!(canonical_unit("fortnight"), None);
        assert_eq!(canonical_unit("Day"), None);
    }

    #[test]
    fn the_numbering_system_reaches_the_digits() {
        let arabic =
            RelativeTimeFormat::new(&["ar-EG-u-nu-arab".to_owned()], Options::default()).unwrap();
        let phrase = arabic.format(3.0, "day").unwrap();
        assert!(
            phrase.contains('\u{0663}'),
            "expected Arabic-Indic three in {phrase}"
        );
        assert!(
            arabic
                .resolved_options()
                .contains(&("numberingSystem", Setting::string("arab")))
        );
    }
}

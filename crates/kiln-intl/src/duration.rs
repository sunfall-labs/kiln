//! `Intl.DurationFormat`.
//!
//! A duration is not a number and a noun. "1 hr, 2 min" in English is "1 Std., 2 Min." in German
//! and "1:02:00" once the style turns digital; the separator between the numeric fields is
//! per-locale data, the plural form of every unit name is per-locale data, and the way the fields
//! are joined into one phrase is `Intl.ListFormat`'s pattern table. All of that is CLDR-shaped and
//! comes from ICU4X's `DurationFormatter`.
//!
//! What does *not* come from ICU4X is `GetDurationUnitOptions`. Which style a unit falls back to,
//! which combinations are a `RangeError`, and whether a zero field is shown are exact ECMA-402
//! algorithm, they are observable through the options object's getters, and ICU4X's own
//! implementation of the same table has a defaulting bug (it tests `unit != Hour || unit !=
//! Minute`, which is always true, so a digital `hoursDisplay` comes out `"auto"` where the
//! specification says `"always"`). The resolution therefore happens on the Kiln side and arrives
//! here already settled, which is also what keeps the option *reads* on the machine where a proxy
//! can see them.

use icu_experimental::duration::options::{
    BaseStyle, DayStyle, DurationFormatterOptions, FieldDisplay, FractionalDigits, HourStyle,
    MicroSecondStyle, MilliSecondStyle, MinuteStyle, MonthStyle, NanoSecondStyle, SecondStyle,
    WeekStyle, YearStyle,
};
use icu_experimental::duration::{
    Duration as IcuDuration, DurationFormatter, DurationSign, ValidatedDurationFormatterOptions,
};

use crate::locale::{RelevantKey, Resolution, resolve};
use crate::{Error, Setting};

/// The ten units, in the order the specification's Table 1 lists them, which is also the order
/// they are read, resolved and reported in.
pub const UNITS: &[&str] = &[
    "years",
    "months",
    "weeks",
    "days",
    "hours",
    "minutes",
    "seconds",
    "milliseconds",
    "microseconds",
    "nanoseconds",
];

/// The accepted values of `style`.
pub const STYLES: &[&str] = &["long", "short", "narrow", "digital"];
/// The accepted values of every `<unit>Display` option.
pub const DISPLAYS: &[&str] = &["auto", "always"];

/// A flattened duration formatting part: its kind, text, and optional unit.
pub type DurationPart = (&'static str, String, Option<&'static str>);

const TEXT_STYLES: &[&str] = &["long", "short", "narrow"];
const NUMERIC_STYLES: &[&str] = &["long", "short", "narrow", "numeric", "2-digit"];
const SUBSECOND_STYLES: &[&str] = &["long", "short", "narrow", "numeric"];

/// The `style` vocabulary of one unit -- the "Values" column of Table 1.
///
/// The three date units take only the three text widths; hours, minutes and seconds add the two
/// digital spellings; the sub-second units add `"numeric"` but not `"2-digit"`, because a
/// two-digit millisecond is not a thing the digital format has a slot for.
pub fn styles_for(unit: &str) -> &'static [&'static str] {
    match unit {
        "hours" | "minutes" | "seconds" => NUMERIC_STYLES,
        "milliseconds" | "microseconds" | "nanoseconds" => SUBSECOND_STYLES,
        _ => TEXT_STYLES,
    }
}

/// The style a `style: "digital"` base falls back to for one unit -- Table 1's "Digital Default".
pub fn digital_default(unit: &str) -> &'static str {
    match unit {
        "years" | "months" | "weeks" | "days" => "short",
        _ => "numeric",
    }
}

/// The singular spelling `formatToParts` reports in a part's `unit` field.
///
/// The specification calls it the "NumberFormat Unit" of the row: the plural option name minus its
/// `s`, which is the sanctioned unit identifier rather than a coincidence of English.
pub fn singular(unit: &str) -> &'static str {
    match unit {
        "years" => "year",
        "months" => "month",
        "weeks" => "week",
        "days" => "day",
        "hours" => "hour",
        "minutes" => "minute",
        "seconds" => "second",
        "milliseconds" => "millisecond",
        "microseconds" => "microsecond",
        _ => "nanosecond",
    }
}

/// The options `Intl.DurationFormat` takes, with every unit's style and display already settled.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `numberingSystem`.
    pub numbering_system: Option<String>,
    /// `style`, one of [`STYLES`].
    pub style: String,
    /// The settled `(style, display)` of each of [`UNITS`], in that order.
    ///
    /// The style is the one `resolvedOptions()` reports, so a sub-second unit that the
    /// specification internally calls `"fractional"` arrives here spelled `"numeric"`.
    pub units: Vec<(String, String)>,
    /// `fractionalDigits`, absent when the option was not given.
    pub fractional_digits: Option<u8>,
}

/// A duration to format: ten unsigned components and one sign for all of them.
///
/// One sign rather than ten is the language's own rule -- `IsValidDuration` rejects a duration
/// whose components disagree about direction -- and it is also ICU4X's shape.
///
/// The magnitudes are 128-bit because a *valid* duration's sub-second components are: the
/// specification's bound is on the duration's total length in seconds, so `9007199254740991e6`
/// microseconds is a legal duration and does not fit in 64 bits. Whether the formatter can render
/// one is a separate question, answered when the components are folded -- see [`fold`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Value {
    /// Whether the whole duration runs backwards.
    pub negative: bool,
    /// The magnitude of each of [`UNITS`], in that order.
    pub units: [u128; 10],
}

/// A built duration formatter.
pub struct DurationFormat {
    formatter: DurationFormatter,
    /// The same formatter with the absorbing unit's display forced on -- see [`Self::pick`].
    shown: Option<DurationFormatter>,
    /// Which unit the fractional sub-second components fold into, if any.
    absorber: Option<usize>,
    resolution: Resolution,
    numbering_system: String,
    style: String,
    units: Vec<(String, String)>,
    fractional_digits: Option<u8>,
}

impl core::fmt::Debug for DurationFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DurationFormat")
            .field("locale", &self.resolution.locale)
            .field("style", &self.style)
            .finish_non_exhaustive()
    }
}

impl DurationFormat {
    /// Build a formatter from an already-resolved option set.
    pub fn new(requested: &[String], options: Options) -> Result<Self, Error> {
        let keys = [RelevantKey::new("nu").with_option(options.numbering_system.clone())];
        let resolution = resolve(requested, &keys, |key, value| {
            key == "nu" && crate::number::numbering_system_is_supported(value)
        });
        let numbering_system = resolution.value("nu").unwrap_or("latn").to_owned();
        let bag = icu_options(&options)?;
        let validated = ValidatedDurationFormatterOptions::validate(bag)
            .map_err(|error| Error::range(format!("invalid duration options: {error}")))?;
        let build = |validated| {
            DurationFormatter::try_new(resolution.icu().clone().into(), validated)
                .map_err(|_| Error::range("no duration formatting data for the resolved locale"))
        };
        let formatter = build(validated)?;
        // The absorbing unit only needs a second formatter when the options left its display on
        // `"auto"`; when it is already `"always"` the two would be the same formatter.
        let absorber = absorber(&options.units);
        let shown = match absorber {
            Some(index) if options.units[index].1 == "auto" => {
                let mut forced = options.clone();
                forced.units[index].1 = "always".to_owned();
                let bag = icu_options(&forced)?;
                let validated = ValidatedDurationFormatterOptions::validate(bag)
                    .map_err(|error| Error::range(format!("invalid duration options: {error}")))?;
                Some(build(validated)?)
            }
            _ => None,
        };
        Ok(Self {
            formatter,
            shown,
            absorber,
            resolution,
            numbering_system,
            style: options.style,
            units: options.units,
            fractional_digits: options.fractional_digits,
        })
    }

    /// `format(duration)`.
    pub fn format(&self, value: &Value) -> Result<String, Error> {
        Ok(self
            .format_to_parts(value)?
            .into_iter()
            .map(|(_, text, _)| text)
            .collect())
    }

    /// Which of the two built formatters this duration needs.
    ///
    /// ICU4X decides whether to show the absorbing field by testing the field's *own* component
    /// against zero, where the specification tests the field's value *after* the sub-second
    /// components have folded into it. `format({hours: 1, milliseconds: 1})` is `"1:00:00.001"`
    /// and not `"1"` for exactly that reason. The two disagree only when the field is zero and
    /// something below it is not, and forcing the field's display on in that case is the same
    /// answer the specification's own condition gives.
    fn pick(&self, folded: &[u128; 10]) -> &DurationFormatter {
        let Some(index) = self.absorber else {
            return &self.formatter;
        };
        let Some(shown) = &self.shown else {
            return &self.formatter;
        };
        if folded[index..].iter().any(|amount| *amount != 0) {
            shown
        } else {
            &self.formatter
        }
    }

    /// `formatToParts(duration)`, as `(type, value, unit)` triples.
    ///
    /// Built from ICU4X's own run annotations rather than by re-splitting the finished string:
    /// the formatter marks each unit's span and, inside it, the decimal formatter marks the
    /// integer digits, the group separators, the decimal point and the fraction, so every boundary
    /// the language reports is one ICU4X drew rather than one this crate guessed.
    ///
    /// Two things happen on top of the raw stream, and both are the specification's:
    ///
    /// * A group separator inside a `numeric` or `2-digit` field is dropped. `FormatNumericHours`
    ///   and its two siblings set `useGrouping` to `false` -- `1234:1234567:12345678` is a
    ///   duration, not a thousands-grouped number -- and ICU4X's digital formatter groups anyway.
    ///   Dropping the runs ICU4X *named* as group separators is exact; deleting commas from the
    ///   finished string would not be.
    /// * The text a unit pattern contributes around the number is split into its whitespace and
    ///   its unit name, so that `" months"` is reported as a `literal` and a `unit` the way the
    ///   specification's `PartitionNumberPattern` reports a `style: "unit"` number. This is the
    ///   one boundary here that ICU4X does not annotate: the pattern is `{0}`-plus-literal-text
    ///   and the unit name is the non-space run of it.
    pub fn format_to_parts(&self, value: &Value) -> Result<Vec<DurationPart>, Error> {
        use writeable::Writeable;
        let folded = fold(value, &self.units);
        let duration = icu_duration(value.negative, &folded)?;
        let ungrouped: Vec<&'static str> = UNITS
            .iter()
            .zip(&self.units)
            .filter(|(_, (style, _))| style == "numeric" || style == "2-digit")
            .map(|(unit, _)| singular(unit))
            .collect();
        let mut sink = Collector::new(ungrouped);
        self.pick(&folded)
            .format(&duration)
            .write_to_parts(&mut sink)
            .map_err(|_| Error::range("the duration could not be formatted"))?;
        Ok(sink.finish())
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        let mut settings = vec![
            ("locale", Setting::string(&self.resolution.locale)),
            ("numberingSystem", Setting::string(&self.numbering_system)),
            ("style", Setting::string(&self.style)),
        ];
        for (unit, (style, display)) in UNITS.iter().zip(&self.units) {
            settings.push((unit, Setting::string(style)));
            settings.push((display_key(unit), Setting::string(display)));
        }
        // `fractionalDigits` is created only when it has a value: the specification's
        // `CreateDataPropertyOrThrow` is guarded, and `Object.keys` sees the difference.
        if let Some(digits) = self.fractional_digits {
            settings.push(("fractionalDigits", Setting::Number(f64::from(digits))));
        }
        settings
    }
}

/// The `<unit>Display` property name, as a `&'static str` so the record can carry it.
fn display_key(unit: &str) -> &'static str {
    match unit {
        "years" => "yearsDisplay",
        "months" => "monthsDisplay",
        "weeks" => "weeksDisplay",
        "days" => "daysDisplay",
        "hours" => "hoursDisplay",
        "minutes" => "minutesDisplay",
        "seconds" => "secondsDisplay",
        "milliseconds" => "millisecondsDisplay",
        "microseconds" => "microsecondsDisplay",
        _ => "nanosecondsDisplay",
    }
}

/// Translate the settled options into ICU4X's bag.
///
/// The sub-second units are handed over spelled `"numeric"` rather than `"fractional"`: ICU4X has
/// no public spelling of the fractional style, and its own validation performs the specification's
/// step 4 -- a `numeric` millisecond *is* a fractional one -- so the conversion happens there.
fn icu_options(options: &Options) -> Result<DurationFormatterOptions, Error> {
    let mut bag = DurationFormatterOptions::default();
    bag.base = match options.style.as_str() {
        "short" => BaseStyle::Short,
        "narrow" => BaseStyle::Narrow,
        "digital" => BaseStyle::Digital,
        _ => BaseStyle::Long,
    };
    let mut settled = options.units.iter();
    let mut next = |unit: &str| -> Result<(String, FieldDisplay), Error> {
        let (style, display) = settled
            .next()
            .ok_or_else(|| Error::range(format!("no settled style for {unit}")))?;
        Ok((
            style.clone(),
            if display == "always" {
                FieldDisplay::Always
            } else {
                FieldDisplay::Auto
            },
        ))
    };
    let (style, display) = next("years")?;
    bag.year = Some(match style.as_str() {
        "narrow" => YearStyle::Narrow,
        "long" => YearStyle::Long,
        _ => YearStyle::Short,
    });
    bag.year_visibility = Some(display);
    let (style, display) = next("months")?;
    bag.month = Some(match style.as_str() {
        "narrow" => MonthStyle::Narrow,
        "long" => MonthStyle::Long,
        _ => MonthStyle::Short,
    });
    bag.month_visibility = Some(display);
    let (style, display) = next("weeks")?;
    bag.week = Some(match style.as_str() {
        "narrow" => WeekStyle::Narrow,
        "long" => WeekStyle::Long,
        _ => WeekStyle::Short,
    });
    bag.week_visibility = Some(display);
    let (style, display) = next("days")?;
    bag.day = Some(match style.as_str() {
        "narrow" => DayStyle::Narrow,
        "long" => DayStyle::Long,
        _ => DayStyle::Short,
    });
    bag.day_visibility = Some(display);
    let (style, display) = next("hours")?;
    bag.hour = Some(match style.as_str() {
        "narrow" => HourStyle::Narrow,
        "long" => HourStyle::Long,
        "numeric" => HourStyle::Numeric,
        "2-digit" => HourStyle::TwoDigit,
        _ => HourStyle::Short,
    });
    bag.hour_visibility = Some(display);
    let (style, display) = next("minutes")?;
    bag.minute = Some(match style.as_str() {
        "narrow" => MinuteStyle::Narrow,
        "long" => MinuteStyle::Long,
        "numeric" => MinuteStyle::Numeric,
        "2-digit" => MinuteStyle::TwoDigit,
        _ => MinuteStyle::Short,
    });
    bag.minute_visibility = Some(display);
    let (style, display) = next("seconds")?;
    bag.second = Some(match style.as_str() {
        "narrow" => SecondStyle::Narrow,
        "long" => SecondStyle::Long,
        "numeric" => SecondStyle::Numeric,
        "2-digit" => SecondStyle::TwoDigit,
        _ => SecondStyle::Short,
    });
    bag.second_visibility = Some(display);
    let (style, display) = next("milliseconds")?;
    bag.millisecond = Some(match style.as_str() {
        "narrow" => MilliSecondStyle::Narrow,
        "long" => MilliSecondStyle::Long,
        "numeric" => MilliSecondStyle::Numeric,
        _ => MilliSecondStyle::Short,
    });
    bag.millisecond_visibility = Some(display);
    let (style, display) = next("microseconds")?;
    bag.microsecond = Some(match style.as_str() {
        "narrow" => MicroSecondStyle::Narrow,
        "long" => MicroSecondStyle::Long,
        "numeric" => MicroSecondStyle::Numeric,
        _ => MicroSecondStyle::Short,
    });
    bag.microsecond_visibility = Some(display);
    let (style, display) = next("nanoseconds")?;
    bag.nanosecond = Some(match style.as_str() {
        "narrow" => NanoSecondStyle::Narrow,
        "long" => NanoSecondStyle::Long,
        "numeric" => NanoSecondStyle::Numeric,
        _ => NanoSecondStyle::Short,
    });
    bag.nanosecond_visibility = Some(display);
    bag.fractional_digits = match options.fractional_digits {
        Some(digits) => FractionalDigits::Fixed(digits),
        None => FractionalDigits::ShowAll,
    };
    Ok(bag)
}

/// Which unit the fractional sub-second components fold into, by index into [`UNITS`].
///
/// A sub-second unit whose settled style is `"numeric"` has no field of its own -- it is the
/// specification's `"fractional"` style -- and its value becomes digits after the decimal point of
/// the first unit above it that does have one. The fractional units are always a suffix of the
/// three, because a unit below a fractional one is required to be fractional too, so there is at
/// most one absorbing unit.
fn absorber(units: &[(String, String)]) -> Option<usize> {
    let fractional = |index: usize| {
        units
            .get(index)
            .is_some_and(|(style, _)| style == "numeric")
    };
    // 7, 8 and 9 are milliseconds, microseconds and nanoseconds.
    if fractional(7) {
        Some(6)
    } else if fractional(8) {
        Some(7)
    } else if fractional(9) {
        Some(8)
    } else {
        None
    }
}

/// Carry each folding sub-second component into the unit above it.
///
/// `AddFractionalDigits` adds `nanoseconds / 10^9` to the seconds, so a duration of 1234567
/// milliseconds inside a `12:34:56` is `12:34:1290.567` and not `12:34:56.567`. ICU4X's own
/// implementation keeps the fraction but drops the carry into the absorbing unit, so the carry is
/// applied to the input here instead: after this, every folding component is below 1000 and
/// ICU4X's arithmetic and this crate's agree.
fn fold(value: &Value, units: &[(String, String)]) -> [u128; 10] {
    let mut amounts = value.units;
    let fractional = |index: usize| {
        units
            .get(index)
            .is_some_and(|(style, _)| style == "numeric")
    };
    for (from, into) in [(9usize, 8usize), (8, 7), (7, 6)] {
        if fractional(from) {
            amounts[into] += amounts[from] / 1_000;
            amounts[from] %= 1_000;
        }
    }
    amounts
}

/// The folded components as ICU4X's duration, or a refusal if one of them will not fit.
///
/// ICU4X counts each component in 64 bits and the language's own validity rule is looser than
/// that: it bounds the duration's total length in seconds, which leaves `9007199254740991e6`
/// microseconds legal and unrepresentable. Folding removes the common case -- a sub-second
/// component that is only large because it has not been carried yet -- and what is left is
/// refused rather than truncated.
fn icu_duration(negative: bool, amounts: &[u128; 10]) -> Result<IcuDuration, Error> {
    let mut fitted = [0u64; 10];
    for (slot, amount) in fitted.iter_mut().zip(amounts) {
        *slot = u64::try_from(*amount)
            .map_err(|_| Error::range("the duration is larger than the formatter can represent"))?;
    }
    Ok(IcuDuration {
        sign: if negative {
            DurationSign::Negative
        } else {
            DurationSign::Positive
        },
        years: fitted[0],
        months: fitted[1],
        weeks: fitted[2],
        days: fitted[3],
        hours: fitted[4],
        minutes: fitted[5],
        seconds: fitted[6],
        milliseconds: fitted[7],
        microseconds: fitted[8],
        nanoseconds: fitted[9],
    })
}

/// The sink that turns ICU4X's nested run annotations into the language's flat part list.
///
/// Nested rather than the single-level [`crate::parts::Collector`] the other services use, because
/// a duration's parts carry *two* facts at once: which unit a run belongs to (the outer
/// annotation) and what the run is (the inner one). A stack is the only way to have both.
struct Collector {
    stack: Vec<writeable::Part>,
    parts: Vec<DurationPart>,
    /// The singular names of the units whose field is `numeric` or `2-digit`, and whose group
    /// separators therefore have to go.
    ungrouped: Vec<&'static str>,
}

impl Collector {
    fn new(ungrouped: Vec<&'static str>) -> Self {
        Self {
            stack: Vec::new(),
            parts: Vec::new(),
            ungrouped,
        }
    }

    fn finish(self) -> Vec<DurationPart> {
        self.parts
    }

    /// The unit the innermost enclosing unit annotation names, if any.
    fn unit(&self) -> Option<&'static str> {
        self.stack
            .iter()
            .rev()
            .find(|part| part.category == "unit")
            .and_then(|part| {
                UNITS
                    .iter()
                    .map(|unit| singular(unit))
                    .find(|name| *name == part.value)
            })
    }

    /// The innermost decimal annotation, translated into the language's vocabulary.
    fn numeric_kind(&self) -> Option<&'static str> {
        let part = self
            .stack
            .iter()
            .rev()
            .find(|part| part.category == "decimal")?;
        Some(match part.value {
            "integer" => "integer",
            "group" => "group",
            "decimal" => "decimal",
            "fraction" => "fraction",
            "plusSign" => "plusSign",
            "minusSign" => "minusSign",
            // A decimal annotation this build does not know the name of is reported as a literal
            // rather than invented: a wrong `type` is worse than a coarse one.
            _ => "literal",
        })
    }

    fn push(&mut self, kind: &'static str, text: &str, unit: Option<&'static str>) {
        if text.is_empty() {
            return;
        }
        match self.parts.last_mut() {
            Some((last, buffer, last_unit)) if *last == kind && *last_unit == unit => {
                buffer.push_str(text);
            }
            _ => self.parts.push((kind, text.to_owned(), unit)),
        }
    }
}

impl core::fmt::Write for Collector {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        if text.is_empty() {
            return Ok(());
        }
        let unit = self.unit();
        if let Some(kind) = self.numeric_kind() {
            // The group separator of a field the specification formats with `useGrouping: false`.
            if kind == "group" && unit.is_some_and(|name| self.ungrouped.contains(&name)) {
                return Ok(());
            }
            self.push(kind, text, unit);
            return Ok(());
        }
        let Some(unit) = unit else {
            // Text outside any unit: the list's own separators and the digital format's `:`.
            self.push("literal", text, None);
            return Ok(());
        };
        // Inside a unit but outside the number: the unit pattern's own text. Its whitespace is the
        // specification's `literal` and the rest is its `unit`.
        let head = text.len() - text.trim_start().len();
        let tail = text.len() - text.trim_end().len();
        self.push("literal", &text[..head], Some(unit));
        self.push("unit", text[head..text.len() - tail].trim(), Some(unit));
        self.push("literal", &text[text.len() - tail..], Some(unit));
        Ok(())
    }
}

impl writeable::PartsWrite for Collector {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: writeable::Part,
        mut f: impl FnMut(&mut Self::SubPartsWrite) -> core::fmt::Result,
    ) -> core::fmt::Result {
        self.stack.push(part);
        let result = f(self);
        self.stack.pop();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The option set `new Intl.DurationFormat(locale, { style })` settles on, written out so the
    /// tests exercise the adapter rather than a helper that could drift from the interpreter's
    /// own resolution.
    fn settled(style: &str) -> Options {
        let mut units = Vec::new();
        let mut previous: Option<String> = None;
        for unit in UNITS {
            let resolved = if style == "digital" {
                digital_default(unit).to_owned()
            } else if matches!(previous.as_deref(), Some("numeric" | "2-digit")) {
                if matches!(*unit, "minutes" | "seconds") {
                    "2-digit".to_owned()
                } else {
                    "numeric".to_owned()
                }
            } else {
                style.to_owned()
            };
            let display = if style == "digital" && matches!(*unit, "hours" | "minutes" | "seconds")
            {
                "always"
            } else {
                "auto"
            };
            previous = Some(resolved.clone());
            units.push((resolved, display.to_owned()));
        }
        Options {
            style: style.to_owned(),
            units,
            ..Options::default()
        }
    }

    fn formatter(locale: &str, style: &str) -> DurationFormat {
        DurationFormat::new(&[locale.to_owned()], settled(style)).expect("the formatter builds")
    }

    fn value(pairs: &[(&str, u128)], negative: bool) -> Value {
        let mut units = [0u128; 10];
        for (name, amount) in pairs {
            let index = UNITS.iter().position(|unit| unit == name).expect("a unit");
            units[index] = *amount;
        }
        Value { negative, units }
    }

    #[test]
    fn a_duration_is_a_phrase_the_locale_joins_rather_than_a_join() {
        let long = formatter("en", "long");
        assert_eq!(
            long.format(&value(
                &[("hours", 1), ("minutes", 46), ("seconds", 40)],
                false
            ))
            .unwrap(),
            "1 hour, 46 minutes, 40 seconds"
        );
        let german = formatter("de", "long");
        assert_eq!(
            german
                .format(&value(&[("hours", 1), ("minutes", 46)], false))
                .unwrap(),
            "1 Stunde, 46 Minuten"
        );
    }

    #[test]
    fn the_digital_style_is_a_clock_rather_than_a_list() {
        let digital = formatter("en", "digital");
        assert_eq!(
            digital
                .format(&value(
                    &[("hours", 1), ("minutes", 46), ("seconds", 40)],
                    false
                ))
                .unwrap(),
            "1:46:40"
        );
        // `hoursDisplay` is `"always"` in the digital style, so a zero hour is still a field.
        assert_eq!(
            digital.format(&value(&[("minutes", 2)], false)).unwrap(),
            "0:02:00"
        );
    }

    #[test]
    fn a_numeric_field_is_not_thousands_grouped() {
        // `FormatNumericHours` sets `useGrouping: false`; ICU4X's digital formatter groups anyway,
        // and a grouped `1,234:1,234,567` would be a wrong answer rather than a missing one.
        let digital = formatter("en", "digital");
        assert_eq!(
            digital
                .format(&value(
                    &[("hours", 1234), ("minutes", 1234567), ("seconds", 12345678)],
                    false
                ))
                .unwrap(),
            "1234:1234567:12345678"
        );
        // A unit-styled field *is* grouped, because there the specification leaves `useGrouping`
        // alone.
        assert_eq!(
            formatter("en", "short")
                .format(&value(&[("years", 1234)], false))
                .unwrap(),
            "1,234 yrs"
        );
    }

    #[test]
    fn only_the_first_field_carries_the_sign() {
        let long = formatter("en", "long");
        assert_eq!(
            long.format(&value(&[("years", 1), ("months", 2)], true))
                .unwrap(),
            "-1 year, 2 months"
        );
        let digital = formatter("en", "digital");
        assert_eq!(
            digital.format(&value(&[("minutes", 2)], true)).unwrap(),
            "-0:02:00"
        );
    }

    #[test]
    fn the_parts_name_the_unit_and_the_run_within_it() {
        let digital = formatter("en", "digital");
        assert_eq!(
            digital
                .format_to_parts(&value(
                    &[
                        ("hours", 7),
                        ("minutes", 8),
                        ("seconds", 9),
                        ("milliseconds", 123),
                        ("microseconds", 456),
                        ("nanoseconds", 789)
                    ],
                    false
                ))
                .unwrap(),
            vec![
                ("integer", "7".to_owned(), Some("hour")),
                ("literal", ":".to_owned(), None),
                ("integer", "08".to_owned(), Some("minute")),
                ("literal", ":".to_owned(), None),
                ("integer", "09".to_owned(), Some("second")),
                ("decimal", ".".to_owned(), Some("second")),
                ("fraction", "123456789".to_owned(), Some("second")),
            ]
        );
    }

    #[test]
    fn a_unit_pattern_splits_into_its_spacing_and_its_name() {
        let narrow = formatter("en", "narrow");
        assert_eq!(
            narrow
                .format_to_parts(&value(&[("years", 1), ("months", 2)], false))
                .unwrap(),
            vec![
                ("integer", "1".to_owned(), Some("year")),
                ("unit", "y".to_owned(), Some("year")),
                ("literal", " ".to_owned(), None),
                ("integer", "2".to_owned(), Some("month")),
                ("unit", "m".to_owned(), Some("month")),
            ]
        );
        let long = formatter("en", "long");
        assert_eq!(
            long.format_to_parts(&value(&[("months", 2)], false))
                .unwrap(),
            vec![
                ("integer", "2".to_owned(), Some("month")),
                ("literal", " ".to_owned(), Some("month")),
                ("unit", "months".to_owned(), Some("month")),
            ]
        );
    }

    #[test]
    fn a_sub_second_component_carries_into_the_unit_that_absorbs_it() {
        // 1234567 milliseconds is 1234 seconds and 567 milliseconds, so the seconds field is
        // 56 + 1234 rather than 56. ICU4X keeps the fraction and drops the carry, which would be a
        // wrong number rather than a missing one.
        let digital = formatter("en", "digital");
        assert_eq!(
            digital
                .format(&value(
                    &[
                        ("hours", 12),
                        ("minutes", 34),
                        ("seconds", 56),
                        ("milliseconds", 1234567)
                    ],
                    false
                ))
                .unwrap(),
            "12:34:1290.567"
        );
        assert_eq!(
            digital
                .format(&value(
                    &[("seconds", 1), ("nanoseconds", 9007199254740991)],
                    false
                ))
                .unwrap(),
            "0:00:9007200.254740991"
        );
    }

    #[test]
    fn a_field_that_is_zero_only_above_its_fraction_is_still_shown() {
        // `secondsDisplay` is "auto" and the seconds component is 0, but the *value* the
        // specification tests is 0.001, so the field is displayed and the minutes separator with
        // it.
        let mut options = settled("short");
        options.units[4] = ("numeric".to_owned(), "always".to_owned());
        options.units[5] = ("2-digit".to_owned(), "auto".to_owned());
        options.units[6] = ("2-digit".to_owned(), "auto".to_owned());
        for index in 7..10 {
            options.units[index] = ("numeric".to_owned(), "auto".to_owned());
        }
        let formatter =
            DurationFormat::new(&["en".to_owned()], options).expect("the formatter builds");
        assert_eq!(
            formatter
                .format(&value(&[("hours", 1), ("milliseconds", 1)], false))
                .unwrap(),
            "1:00:00.001"
        );
        // With nothing below the seconds either, the zero field stays hidden.
        assert_eq!(
            formatter.format(&value(&[("hours", 1)], false)).unwrap(),
            "1"
        );
    }

    #[test]
    fn resolved_options_reports_the_settled_table_in_order() {
        let digital = formatter("en", "digital");
        let names: Vec<&str> = digital
            .resolved_options()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            &names[..5],
            &[
                "locale",
                "numberingSystem",
                "style",
                "years",
                "yearsDisplay"
            ]
        );
        assert_eq!(names.len(), 23);
        assert!(
            digital
                .resolved_options()
                .contains(&("hoursDisplay", Setting::string("always")))
        );
    }
}

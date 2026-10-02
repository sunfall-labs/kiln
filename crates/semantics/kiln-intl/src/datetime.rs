//! `Intl.DateTimeFormat`.
//!
//! This is the formatter whose data is a *pattern* rather than a table. Where a number formatter
//! needs separators and digits, a date formatter needs to know that `en-US` writes the month
//! before the day and `de-DE` after it, that Japanese ends the year with a character rather than
//! a comma, that "full" in Thai reaches for the Buddhist era, and that a range whose two ends fall
//! in the same month is written `Jan 1 - 5, 2020` rather than twice over. All of that is CLDR
//! skeleton and interval data, so all of it comes from ICU4X.
//!
//! # What is decided here rather than in ICU4X
//!
//! ICU4X's interface is a *semantic field set* -- "year, month and day, medium" -- while
//! ECMA-402's is a bag of independently-styled components. Translating one into the other is this
//! module's job and it is a lossy translation in one direction: ICU4X's field sets enumerate the
//! combinations CLDR actually has patterns for, and the language lets a program ask for
//! combinations CLDR does not have (a year and a day with no month). Where the two disagree, this
//! module resolves the request onto the nearest field set ICU4X *does* have and then reports what
//! it actually produced -- see [`DateTimeFormat::resolved_options`], which reads the components
//! back out of the resolved pattern instead of echoing the request. A caller that asks for
//! something CLDR cannot spell learns so from `resolvedOptions()` rather than being told it got
//! what it asked for.
//!
//! # The realm has one timezone, and it is not the host's
//!
//! ADR 0015 fixes the realm's implicit zone at UTC and its clock at the epoch. An explicit
//! `timeZone` uses the compiled IANA TZif database, shared with Temporal, to resolve the offset at
//! each instant. This keeps the answer independent of the machine's local zone while correctly
//! handling daylight-saving transitions, aliases, and historical offsets.

use std::cell::RefCell;

use icu_calendar::AnyCalendarKind;
use icu_datetime::DateTimeFormatter;
use icu_datetime::fieldsets::builder::{DateFields, FieldSetBuilder, ZoneStyle};
use icu_datetime::fieldsets::enums::CompositeFieldSet;
use icu_datetime::options::{Alignment, Length, SubsecondDigits, TimePrecision, YearStyle};
use icu_datetime::provider::fields::components;
use icu_datetime::range::DateRangeFormatter;
use icu_decimal::DecimalFormatter;
use icu_decimal::options::DecimalFormatterOptions;
use icu_locale_core::preferences::extensions::unicode::keywords::{
    CalendarAlgorithm, HourCycle, NumberingSystem,
};
use icu_time::zone::UtcOffset;
use icu_time::zone::iana::IanaParserExtended;
use icu_time::{DateTime, TimeZone, ZonedDateTime};
#[cfg(feature = "timezone_database")]
use std::sync::OnceLock;
#[cfg(feature = "timezone_database")]
use timezone_provider::provider::TimeZoneProvider;
#[cfg(feature = "timezone_database")]
use timezone_provider::tzif::CompiledTzdbProvider;

use crate::locale::{RelevantKey, Resolution};
use crate::{Error, Setting};

/// The kinds of value `format` accepts besides a time value.
///
/// ECMA-402 formats a `Temporal` object against the *fields that object has*, not against the
/// whole option bag: a `PlainYearMonth` asked for `dateStyle: "short"` is written without its day,
/// and a `PlainDate` asked for the hour is a `TypeError` rather than a midnight. The shape is what
/// says which fields exist, and it is a plain enum rather than a `Temporal` type so that
/// `kiln-temporal` and `kiln-intl` stay unaware of each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A `Temporal.Instant`: a point on the timeline, so every field including the zone.
    Instant,
    /// A `Temporal.PlainDate`.
    PlainDate,
    /// A `Temporal.PlainDateTime`.
    PlainDateTime,
    /// A `Temporal.PlainYearMonth`.
    PlainYearMonth,
    /// A `Temporal.PlainMonthDay`.
    PlainMonthDay,
    /// A `Temporal.PlainTime`.
    PlainTime,
}

/// Which fields a shape carries, and therefore which of the requested components survive.
struct Reach {
    weekday: bool,
    era: bool,
    year: bool,
    month: bool,
    day: bool,
    time: bool,
    zone: bool,
}

impl Shape {
    fn reach(self) -> Reach {
        let (weekday, era, year, month, day, time, zone) = match self {
            Self::Instant => (true, true, true, true, true, true, true),
            Self::PlainDateTime => (true, true, true, true, true, true, false),
            Self::PlainDate => (true, true, true, true, true, false, false),
            Self::PlainYearMonth => (false, true, true, true, false, false, false),
            Self::PlainMonthDay => (false, false, false, true, true, false, false),
            Self::PlainTime => (false, false, false, false, false, true, false),
        };
        Reach {
            weekday,
            era,
            year,
            month,
            day,
            time,
            zone,
        }
    }

    /// Whether the value is a wall clock rather than a point on the timeline.
    ///
    /// A wall clock is placed at UTC and formatted there, which is why the formatter's own zone
    /// makes no difference to it -- `format` of a `PlainDateTime` in `Pacific/Apia` shows the
    /// fields the object has, not the fields a zone conversion would produce.
    const fn is_plain(self) -> bool {
        !matches!(self, Self::Instant)
    }
}

/// The accepted values of `dateStyle` and `timeStyle`.
pub const STYLES: &[&str] = &["full", "long", "medium", "short"];
/// The accepted values of `weekday`, `era` and `dayPeriod`.
pub const TEXT_WIDTHS: &[&str] = &["narrow", "short", "long"];
/// The accepted values of `year`, `day`, `hour`, `minute` and `second`.
pub const NUMERIC_WIDTHS: &[&str] = &["2-digit", "numeric"];
/// The accepted values of `month`, which is the one component with all five widths.
pub const MONTH_WIDTHS: &[&str] = &["2-digit", "numeric", "narrow", "short", "long"];
/// The accepted values of `timeZoneName`.
pub const ZONE_NAMES: &[&str] = &[
    "short",
    "long",
    "shortOffset",
    "longOffset",
    "shortGeneric",
    "longGeneric",
];
/// The accepted values of `hourCycle`.
pub const HOUR_CYCLES: &[&str] = &["h11", "h12", "h23", "h24"];
/// The accepted values of `formatMatcher`.
///
/// Like `localeMatcher`, both name the same algorithm here: the specification defines `"basic"`
/// exactly and leaves `"best fit"` to the implementation, and claiming a second, better matcher
/// that does not exist would be the plausible-looking wrong answer.
pub const FORMAT_MATCHERS: &[&str] = &["basic", "best fit"];

/// The component options, as the language spells them.
///
/// Every field is the string the option took, already validated against the vocabulary above by
/// the caller -- the machine does the `GetOption` reads, because their order is observable.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `calendar`, a Unicode `-u-ca` type.
    pub calendar: Option<String>,
    /// `numberingSystem`, a Unicode `-u-nu` type.
    pub numbering_system: Option<String>,
    /// `hour12`.
    pub hour12: Option<bool>,
    /// `hourCycle`, a `-u-hc` type.
    pub hour_cycle: Option<String>,
    /// `timeZone`, already canonicalized by [`canonical_time_zone`].
    pub time_zone: Option<TimeZoneChoice>,
    /// `weekday`.
    pub weekday: Option<String>,
    /// `era`.
    pub era: Option<String>,
    /// `year`.
    pub year: Option<String>,
    /// `month`.
    pub month: Option<String>,
    /// `day`.
    pub day: Option<String>,
    /// `dayPeriod`.
    pub day_period: Option<String>,
    /// `hour`.
    pub hour: Option<String>,
    /// `minute`.
    pub minute: Option<String>,
    /// `second`.
    pub second: Option<String>,
    /// `fractionalSecondDigits`, 1 to 3.
    pub fractional_second_digits: Option<u8>,
    /// `timeZoneName`.
    pub time_zone_name: Option<String>,
    /// `dateStyle`.
    pub date_style: Option<String>,
    /// `timeStyle`.
    pub time_style: Option<String>,
}

impl Options {
    /// Whether any of the component options -- the ones `dateStyle` and `timeStyle` exclude -- was
    /// supplied. The specification makes the combination a `TypeError`, and the check has to see
    /// all of them at once, so it lives with the options rather than at the call site.
    pub fn has_components(&self) -> bool {
        self.weekday.is_some()
            || self.era.is_some()
            || self.year.is_some()
            || self.month.is_some()
            || self.day.is_some()
            || self.day_period.is_some()
            || self.hour.is_some()
            || self.minute.is_some()
            || self.second.is_some()
            || self.fractional_second_digits.is_some()
            || self.time_zone_name.is_some()
    }
}

/// A `timeZone` that has already been recognised.
///
/// Two shapes, because the specification recognises two: an offset string such as `+05:30`, whose
/// meaning is entirely in its spelling, and a named IANA identifier, whose meaning is in a
/// database. Keeping them apart is what lets the named one be reported by the name the caller used
/// while the offset one is reported in its canonical `±HH:MM` form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeZoneChoice {
    /// A fixed offset in seconds, from a `+HH:MM`-style identifier.
    Offset(i32),
    /// A named zone: the identifier as it should be reported, and ICU4X's BCP-47 zone.
    Named(String, TimeZone),
}

/// `GetAvailableNamedTimeZoneIdentifier`: recognise a named zone, case-insensitively.
///
/// Returns the identifier in its correct casing. The specification deliberately does *not*
/// canonicalize a link to its target any more -- `Asia/Calcutta` stays `Asia/Calcutta` -- so what
/// comes back is ICU4X's normalized spelling of the identifier that was asked for, not the
/// canonical zone behind it.
pub fn canonical_time_zone(identifier: &str) -> Option<TimeZoneChoice> {
    let parser = IanaParserExtended::new();
    let parsed = parser.parse(identifier);
    if parsed.time_zone == TimeZone::UNKNOWN {
        return None;
    }
    Some(TimeZoneChoice::Named(
        parsed.normalized.to_owned(),
        parsed.time_zone,
    ))
}

/// Every named zone identifier this build recognises, canonical spellings only, sorted.
///
/// `Intl.supportedValuesOf("timeZone")` wants the *primary* identifiers rather than every link, so
/// this reports the canonical name of each zone once.
pub fn available_time_zones() -> Vec<String> {
    let parser = IanaParserExtended::new();
    let mut names: Vec<String> = parser
        .iter()
        .map(|entry| entry.canonical.to_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every calendar `Intl.supportedValuesOf("calendar")` reports, sorted.
///
/// `islamic` and `islamic-rgsa` are accepted as *requests* and are not in this list: the
/// specification requires each of them to resolve to a concrete Hijri calendar rather than to
/// itself, so reporting them as available would be advertising a calendar nothing ever resolves to.
pub fn available_calendars() -> Vec<String> {
    let mut names: Vec<String> = CALENDARS
        .iter()
        .filter(|name| !matches!(**name, "islamic" | "islamic-rgsa"))
        .map(|name| (*name).to_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The `-u-ca` types this build accepts.
///
/// Transcribed from `CalendarAlgorithm`'s own vocabulary rather than derived, because ICU4X keeps
/// the mapping in a macro and does not enumerate it. Every entry is checked against
/// `CalendarAlgorithm::try_from` by a test below, so the list cannot silently drift from what the
/// formatter will actually accept.
const CALENDARS: &[&str] = &[
    "buddhist",
    "chinese",
    "coptic",
    "dangi",
    "ethioaa",
    "ethiopic",
    "gregory",
    "hebrew",
    "indian",
    "islamic",
    "islamic-civil",
    "islamic-rgsa",
    "islamic-tbla",
    "islamic-umalqura",
    "iso8601",
    "japanese",
    "persian",
    "roc",
];

/// The concrete calendar `islamic` and `islamic-rgsa` resolve to.
///
/// ECMA-402 step 9 of `CreateDateTimeFormat` requires the substitution and leaves the target
/// implementation-defined; `islamic-civil` is the tabular calendar ICU4X has data for and is one of
/// the values `AvailableCalendars` reports, which is the specification's only constraint.
const HIJRI_FALLBACK: &str = "islamic-civil";

/// The BCP-47 name of a resolved calendar, for a formatter whose locale chose one.
fn calendar_name(kind: AnyCalendarKind) -> &'static str {
    match kind {
        AnyCalendarKind::Buddhist => "buddhist",
        AnyCalendarKind::Chinese => "chinese",
        AnyCalendarKind::Coptic => "coptic",
        AnyCalendarKind::Dangi => "dangi",
        AnyCalendarKind::Ethiopian => "ethiopic",
        AnyCalendarKind::EthiopianAmeteAlem => "ethioaa",
        AnyCalendarKind::Hebrew => "hebrew",
        AnyCalendarKind::Indian => "indian",
        AnyCalendarKind::HijriTabularTypeIIFriday => "islamic-civil",
        AnyCalendarKind::HijriTabularTypeIIThursday => "islamic-tbla",
        AnyCalendarKind::HijriUmmAlQura => "islamic-umalqura",
        AnyCalendarKind::Iso => "iso8601",
        AnyCalendarKind::Japanese => "japanese",
        AnyCalendarKind::Persian => "persian",
        AnyCalendarKind::Roc => "roc",
        _ => "gregory",
    }
}

/// A built date-time formatter.
pub struct DateTimeFormat {
    formatter: DateTimeFormatter<CompositeFieldSet>,
    ranges: DateRangeFormatter<CompositeFieldSet>,
    /// The request as it arrived, kept because a `Temporal` value is formatted from a *restricted*
    /// copy of it rather than from the field set this formatter resolved to.
    options: Options,
    /// What the shaped formatters are built from, which is the locale plus the four `-u-`
    /// preferences the constructor settled.
    preferences: icu_datetime::DateTimeFormatterPreferences,
    /// The per-shape formatters, built the first time a value of that shape is formatted.
    shaped: RefCell<Vec<(Shape, std::rc::Rc<Shaped>)>>,
    resolution: Resolution,
    calendar: String,
    numbering_system: String,
    /// The zero glyph used by the resolved numbering system. ICU's column alignment is global
    /// across date and time fields; keeping this lets the language-level widths be restored per
    /// field when that global option padded a field requested as `numeric`.
    zero_digit: String,
    zone: TimeZoneChoice,
    date_style: Option<String>,
    time_style: Option<String>,
    /// The components read back out of the resolved pattern, in `resolvedOptions()` order.
    components: Vec<(&'static str, Setting)>,
    hour_cycle: Option<&'static str>,
}

impl core::fmt::Debug for DateTimeFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DateTimeFormat")
            .field("locale", &self.resolution.locale)
            .field("calendar", &self.calendar)
            .field("timeZone", &self.zone)
            .finish_non_exhaustive()
    }
}

/// The numbering systems `Intl.DateTimeFormat` will honour on the `-u-nu` key.
fn numbering_system_is_known(value: &str) -> bool {
    // Well-formedness is not enough: `ja-JP-u-nu-native` is a perfectly good subtag with no digit
    // data behind it, and reporting it would claim digits the formatter is not going to use.
    parsed_numbering_system(value).is_some() && crate::number::numbering_system_is_supported(value)
}

fn parsed_numbering_system(value: &str) -> Option<NumberingSystem> {
    let value = icu_locale_core::extensions::unicode::Value::try_from_str(value).ok()?;
    NumberingSystem::try_from(&value).ok()
}

fn parsed_calendar_algorithm(value: &str) -> Option<CalendarAlgorithm> {
    let value = icu_locale_core::extensions::unicode::Value::try_from_str(value).ok()?;
    CalendarAlgorithm::try_from(&value).ok()
}

impl DateTimeFormat {
    /// Build a formatter. `options` has already been read off the options object, in order.
    pub fn new(requested: &[String], options: Options) -> Result<Self, Error> {
        Self::new_with_default(requested, crate::locale::DEFAULT, options)
    }

    /// Build with the default locale selected by the containing realm.
    pub fn new_with_default(
        requested: &[String],
        default: &str,
        options: Options,
    ) -> Result<Self, Error> {
        // A `-u-` keyword type is case-insensitive and canonically lower case, so
        // `{ calendar: "GREGORY" }` and `-u-ca-gregory` are one request rather than two.
        let calendar_request = options
            .calendar
            .as_deref()
            .map(|value| crate::locale::canonicalize_keyword_type("ca", &value.to_lowercase()));
        let numbering_request = options.numbering_system.as_deref().map(str::to_lowercase);
        let keys = [
            RelevantKey::new("ca").with_option(calendar_request),
            RelevantKey::new("nu").with_option(numbering_request),
            RelevantKey::new("hc").with_option(options.hour_cycle.clone()),
        ];
        // `hour12` does not merely outrank `hourCycle`: it removes the `hc` keyword from the
        // resolved locale altogether, so `en-US-u-hc-h11` with `hour12: false` reports `en-US`.
        let ignores_hour_cycle = options.hour12.is_some();
        let resolution = crate::locale::resolve_with_default(
            requested,
            default,
            &keys,
            |key, value| match key {
                "ca" => CALENDARS.contains(&value),
                "nu" => numbering_system_is_known(value),
                "hc" => !ignores_hour_cycle && HOUR_CYCLES.contains(&value),
                _ => false,
            },
        );

        let mut preferences: icu_datetime::DateTimeFormatterPreferences =
            resolution.icu().clone().into();
        if let Some(system) = resolution.value("nu").and_then(parsed_numbering_system) {
            preferences.numbering_system = Some(system);
        }
        // `islamic` and `islamic-rgsa` are requests for "a Hijri calendar" rather than for a
        // calendar; the substitution happens here so the preference, the reported calendar and the
        // formatted output are all the same one.
        let requested_calendar = resolution.value("ca").map(|value| {
            match value {
                "islamic" | "islamic-rgsa" => HIJRI_FALLBACK,
                other => other,
            }
            .to_owned()
        });
        if let Some(algorithm) = requested_calendar
            .as_deref()
            .and_then(parsed_calendar_algorithm)
        {
            preferences.calendar_algorithm = Some(algorithm);
        }

        // `hour12` outranks `hourCycle`, which outranks the `-u-hc` extension, which is already in
        // `resolution`. The specification spells `hour12: false` as h23 outright rather than
        // letting the locale choose between h23 and h24.
        let hour_cycle = match options.hour12 {
            Some(true) => Some(HourCycle::Clock12),
            Some(false) => Some(HourCycle::H23),
            None => resolution.value("hc").and_then(spelled_hour_cycle),
        };
        preferences.hour_cycle = hour_cycle;
        let zero_digit = DecimalFormatter::try_new(
            preferences.clone().into(),
            DecimalFormatterOptions::default(),
        )
        .map(|formatter| {
            formatter
                .format(&fixed_decimal::Decimal::from(0))
                .to_string()
        })
        .unwrap_or_else(|_| "0".to_owned());

        let zone = options
            .time_zone
            .clone()
            .unwrap_or(TimeZoneChoice::Offset(0));

        let field_set = build_field_set(&options)?;
        let formatter = DateTimeFormatter::try_new(preferences, field_set)
            .map_err(|_| Error::range("no date formatting data for the resolved locale"))?;
        let ranges = DateRangeFormatter::try_new(preferences, field_set)
            .map_err(|_| Error::range("no date range formatting data for the resolved locale"))?;

        let calendar = requested_calendar
            .unwrap_or_else(|| calendar_name(formatter.calendar().kind()).to_owned());
        let numbering_system = resolution
            .value("nu")
            .map(str::to_owned)
            .unwrap_or_else(|| default_numbering_system(&resolution));

        let mut built = Self {
            formatter,
            ranges,
            options: options.clone(),
            preferences,
            shaped: RefCell::new(Vec::new()),
            resolution,
            calendar,
            numbering_system,
            zero_digit,
            zone,
            date_style: options.date_style.clone(),
            time_style: options.time_style.clone(),
            components: Vec::new(),
            hour_cycle: None,
        };
        // The pattern is what the formatter actually resolved to, so the components come out of
        // it rather than out of the request: a build that could not honour `month: "narrow"` says
        // so here instead of claiming it did.
        let (mut components, cycle) = built.resolved_components();
        built.restore_requested_numeric_widths(&mut components);
        built.components = components;
        built.hour_cycle = cycle;
        Ok(built)
    }

    /// The instant, as ICU4X's zoned input using the exact offset at that instant.
    ///
    /// `naming` is the instant the zone's *name* is resolved at, which is the instant itself for a
    /// single date and the start of the range for both ends of a range: ICU4X treats two inputs
    /// whose zone-name timestamps differ as being in incomparable zones and falls straight back to
    /// `{start} - {end}`, and the two ends of a range are by construction in one zone.
    fn at(&self, epoch_milliseconds: f64, naming: f64) -> Result<ZonedInstant, Error> {
        let offset = resolve_offset(&self.zone, epoch_milliseconds).ok_or_else(|| {
            Error::range(format!(
                "time zone {} has no transition data",
                self.time_zone()
            ))
        })?;
        let naming_offset = resolve_offset(&self.zone, naming).ok_or_else(|| {
            Error::range(format!(
                "time zone {} has no transition data",
                self.time_zone()
            ))
        })?;
        let milliseconds = epoch_milliseconds as i64;
        let zoned = ZonedDateTime::from_epoch_milliseconds_and_utc_offset(milliseconds, offset);
        let id = match &self.zone {
            TimeZoneChoice::Named(_, id) => *id,
            TimeZoneChoice::Offset(_) => TimeZone::UNKNOWN,
        };
        let naming =
            ZonedDateTime::from_epoch_milliseconds_and_utc_offset(naming as i64, naming_offset);
        let info = id.with_offset(Some(naming_offset)).at_date_time(DateTime {
            date: naming.date,
            time: naming.time,
        });
        Ok(ZonedInstant(ZonedDateTime {
            date: zoned.date,
            time: zoned.time,
            zone: info,
        }))
    }

    /// The instant as ICU4X's zoned input at UTC, for a wall-clock value.
    ///
    /// No offset lookup and no `TimeClip`: a plain value is placed on the timeline at UTC by the
    /// specification itself, its zone is never rendered, and its range is the `Temporal` one --
    /// which is wider than a `Date`'s at the extremes and must not be clipped to it.
    fn at_utc(&self, epoch_milliseconds: f64, naming: f64) -> ZonedInstant {
        let offset = UtcOffset::zero();
        let zoned = ZonedDateTime::from_epoch_milliseconds_and_utc_offset(
            epoch_milliseconds as i64,
            offset,
        );
        // The zone-name timestamp is the start of a range for both of its ends: ICU4X treats two
        // inputs whose timestamps differ as being in incomparable zones and stops collapsing the
        // fields the two ends share, which is the same reason `at` takes it.
        let named = ZonedDateTime::from_epoch_milliseconds_and_utc_offset(naming as i64, offset);
        let info = TimeZone::UNKNOWN
            .with_offset(Some(offset))
            .at_date_time(DateTime {
                date: named.date,
                time: named.time,
            });
        ZonedInstant(ZonedDateTime {
            date: zoned.date,
            time: zoned.time,
            zone: info,
        })
    }

    /// The formatter a value of this shape is written with, built once and kept.
    fn shaped(&self, shape: Shape) -> Result<std::rc::Rc<Shaped>, Error> {
        if let Some((_, found)) = self
            .shaped
            .borrow()
            .iter()
            .find(|(known, _)| *known == shape)
        {
            return Ok(found.clone());
        }
        let restricted = restrict(&self.options, shape)?;
        let shows_time = restricted.time_style.is_some()
            || restricted.hour.is_some()
            || restricted.minute.is_some()
            || restricted.second.is_some()
            || restricted.fractional_second_digits.is_some();
        let field_set = shaped_field_set(&restricted, shape)?;
        let formatter = DateTimeFormatter::try_new(self.preferences, field_set)
            .map_err(|_| Error::range("no date formatting data for the resolved locale"))?;
        let ranges = DateRangeFormatter::try_new(self.preferences, field_set)
            .map_err(|_| Error::range("no date range formatting data for the resolved locale"))?;
        let built = std::rc::Rc::new(Shaped {
            formatter,
            ranges,
            options: restricted,
            shows_time,
        });
        self.shaped.borrow_mut().push((shape, built.clone()));
        Ok(built)
    }

    fn shaped_at(
        &self,
        shape: Shape,
        epoch_milliseconds: f64,
        naming: f64,
    ) -> Result<ZonedInstant, Error> {
        if shape.is_plain() {
            Ok(self.at_utc(epoch_milliseconds, naming))
        } else {
            self.at(epoch_milliseconds, naming)
        }
    }

    /// `format(temporalValue)`, with the value already placed on the timeline.
    pub fn format_shape(&self, shape: Shape, epoch_milliseconds: f64) -> Result<String, Error> {
        let shaped = self.shaped(shape)?;
        let at = self.shaped_at(shape, epoch_milliseconds, epoch_milliseconds)?;
        if !needs_numeric_width_adjustment(&shaped.options) {
            return Ok(shaped.formatter.format(&at.0).to_string());
        }
        use writeable::Writeable;
        let mut sink = PartCollector::default();
        let _ = shaped.formatter.format(&at.0).write_to_parts(&mut sink);
        Ok(join_parts(
            adjust_numeric_widths(sink.finish(), &shaped.options, &self.zero_digit)
                .into_iter()
                .map(|(_, text)| text),
        ))
    }

    /// `formatToParts(temporalValue)`.
    pub fn format_shape_to_parts(
        &self,
        shape: Shape,
        epoch_milliseconds: f64,
    ) -> Result<Vec<(&'static str, String)>, Error> {
        use writeable::Writeable;
        let shaped = self.shaped(shape)?;
        let at = self.shaped_at(shape, epoch_milliseconds, epoch_milliseconds)?;
        let mut sink = PartCollector::default();
        let _ = shaped.formatter.format(&at.0).write_to_parts(&mut sink);
        Ok(adjust_numeric_widths(
            sink.finish(),
            &shaped.options,
            &self.zero_digit,
        ))
    }

    /// Both ends of a shaped range, with the time of day dropped when the pattern has no time in
    /// it -- the same reduction the numeric path makes, for the same reason.
    fn shaped_range_ends(
        &self,
        shape: Shape,
        shaped: &Shaped,
        start: f64,
        end: f64,
    ) -> Result<(ZonedInstant, ZonedInstant), Error> {
        let (from, to) = if shaped.shows_time {
            (start, end)
        } else {
            let day = 86_400_000.0;
            ((start / day).floor() * day, (end / day).floor() * day)
        };
        Ok((
            self.shaped_at(shape, from, from)?,
            self.shaped_at(shape, to, from)?,
        ))
    }

    /// `formatRange(temporalValue, temporalValue)`.
    pub fn format_shape_range(&self, shape: Shape, start: f64, end: f64) -> Result<String, Error> {
        let shaped = self.shaped(shape)?;
        let (first, second) = self.shaped_range_ends(shape, &shaped, start, end)?;
        if !needs_numeric_width_adjustment(&shaped.options) {
            return Ok(shaped.ranges.format(&first.0, &second.0).to_string());
        }
        use writeable::Writeable;
        let mut sink = PartCollector::default();
        let _ = shaped
            .ranges
            .format(&first.0, &second.0)
            .write_to_parts(&mut sink);
        Ok(join_parts(
            adjust_numeric_widths(sink.finish(), &shaped.options, &self.zero_digit)
                .into_iter()
                .map(|(_, text)| text),
        ))
    }

    /// `formatRangeToParts(temporalValue, temporalValue)`.
    pub fn format_shape_range_to_parts(
        &self,
        shape: Shape,
        start: f64,
        end: f64,
    ) -> Result<Vec<(&'static str, &'static str, String)>, Error> {
        use writeable::Writeable;
        let shaped = self.shaped(shape)?;
        let (first, second) = self.shaped_range_ends(shape, &shaped, start, end)?;
        let mut sink = PartCollector::default();
        let _ = shaped
            .ranges
            .format(&first.0, &second.0)
            .write_to_parts(&mut sink);
        let parts = adjust_numeric_widths(sink.finish(), &shaped.options, &self.zero_digit);
        Ok(attribute_sources(parts))
    }

    /// `format(date)`.
    pub fn format(&self, epoch_milliseconds: f64) -> Result<String, Error> {
        if !needs_numeric_width_adjustment(&self.options) {
            let at = self.at(epoch_milliseconds, epoch_milliseconds)?;
            return Ok(self.formatter.format(&at.0).to_string());
        }
        self.format_to_parts(epoch_milliseconds)
            .map(|parts| join_parts(parts.into_iter().map(|(_, text)| text)))
    }

    /// `formatToParts(date)`.
    pub fn format_to_parts(
        &self,
        epoch_milliseconds: f64,
    ) -> Result<Vec<(&'static str, String)>, Error> {
        use writeable::Writeable;
        let at = self.at(epoch_milliseconds, epoch_milliseconds)?;
        let mut sink = PartCollector::default();
        let _ = self.formatter.format(&at.0).write_to_parts(&mut sink);
        Ok(adjust_numeric_widths(
            sink.finish(),
            &self.options,
            &self.zero_digit,
        ))
    }

    /// Whether the resolved pattern shows a time at all.
    ///
    /// A date-only formatter must treat two instants on the same day as one date -- ECMA-402
    /// compares only the fields the pattern *has* -- so the hours are dropped before the range
    /// formatter sees them, or `formatRange(morning, evening)` comes back as `X - X`.
    fn shows_time(&self) -> bool {
        self.time_style.is_some()
            || self
                .components
                .iter()
                .any(|(name, _)| matches!(*name, "hour" | "minute" | "second"))
    }

    fn range_ends(&self, start: f64, end: f64) -> Result<(ZonedInstant, ZonedInstant), Error> {
        let (from, to) = if self.shows_time() {
            (start, end)
        } else {
            let day = 86_400_000.0;
            ((start / day).floor() * day, (end / day).floor() * day)
        };
        Ok((self.at(from, from)?, self.at(to, from)?))
    }

    /// `formatRange(start, end)`.
    pub fn format_range(&self, start: f64, end: f64) -> Result<String, Error> {
        if !needs_numeric_width_adjustment(&self.options) {
            let (from, to) = self.range_ends(start, end)?;
            return Ok(self.ranges.format(&from.0, &to.0).to_string());
        }
        self.format_range_to_parts(start, end)
            .map(|parts| join_parts(parts.into_iter().map(|(_, _, text)| text)))
    }

    /// `formatRangeToParts(start, end)`: each run, with the side of the range it came from.
    ///
    /// ICU4X writes the interval pattern's two halves back to back and annotates the *fields*, not
    /// the halves, so the `source` of each run is reconstructed here from the shape of the result.
    /// The rule is the specification's own: a field that the interval pattern mentions once is
    /// shared between the two ends, and a field it mentions twice is the thing the two ends differ
    /// in, so its first occurrence belongs to the start and its second to the end. Everything
    /// between the first and last repeated field on a side belongs to that side; everything
    /// outside both spans -- the separator, and the fields the two ends agree on -- is shared.
    pub fn format_range_to_parts(
        &self,
        start: f64,
        end: f64,
    ) -> Result<Vec<(&'static str, &'static str, String)>, Error> {
        use writeable::Writeable;
        let (start_at, end_at) = self.range_ends(start, end)?;
        let mut sink = PartCollector::default();
        let _ = self
            .ranges
            .format(&start_at.0, &end_at.0)
            .write_to_parts(&mut sink);
        let parts = adjust_numeric_widths(sink.finish(), &self.options, &self.zero_digit);
        Ok(attribute_sources(parts))
    }

    /// The calendar `resolvedOptions().calendar` reports.
    ///
    /// A `Temporal` value carries its own calendar and may only be formatted by a formatter that
    /// agrees with it, so the comparison has to happen where the value is -- on the machine.
    pub fn calendar(&self) -> &str {
        &self.calendar
    }

    /// The identifier `resolvedOptions().timeZone` reports.
    pub fn time_zone(&self) -> String {
        match &self.zone {
            TimeZoneChoice::Named(name, _) => name.clone(),
            TimeZoneChoice::Offset(seconds) => offset_identifier(*seconds),
        }
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        let mut settings = vec![
            ("locale", Setting::string(&self.resolution.locale)),
            ("calendar", Setting::string(&self.calendar)),
            ("numberingSystem", Setting::string(&self.numbering_system)),
            ("timeZone", Setting::String(self.time_zone())),
        ];
        if let Some(cycle) = self.hour_cycle {
            settings.push(("hourCycle", Setting::string(cycle)));
            settings.push(("hour12", Setting::Bool(cycle == "h11" || cycle == "h12")));
        }
        // `dateStyle`/`timeStyle` and the component options are mutually exclusive, so only one of
        // the two groups is ever reported.
        if self.date_style.is_some() || self.time_style.is_some() {
            if let Some(style) = &self.date_style {
                settings.push(("dateStyle", Setting::string(style)));
            }
            if let Some(style) = &self.time_style {
                settings.push(("timeStyle", Setting::string(style)));
            }
        } else {
            settings.extend(self.components.iter().cloned());
        }
        settings
    }

    /// Read the components back out of the pattern the formatter resolved to.
    ///
    /// The pattern can in principle depend on the instant being formatted -- a year style of
    /// "auto" shows an era for a date before the common era and not for one after -- so it is
    /// resolved once, at the epoch, which is also the instant ADR 0015 starts the realm's clock
    /// at. A formatter whose reported components changed with its argument would be worse.
    fn resolved_components(&self) -> (Vec<(&'static str, Setting)>, Option<&'static str>) {
        let Ok(at) = self.at(0.0, 0.0) else {
            return (Vec::new(), None);
        };
        let pattern = self.formatter.format(&at.0).pattern();
        let bag = components::Bag::from(&pattern);
        let mut out: Vec<(&'static str, Setting)> = Vec::new();
        macro_rules! push {
            ($name:expr, $value:expr $(,)?) => {
                out.push(($name, Setting::string($value)))
            };
        }
        if let Some(weekday) = bag.weekday {
            push!("weekday", text_width(weekday));
        }
        if let Some(era) = bag.era {
            push!("era", text_width(era));
        }
        if let Some(year) = bag.year {
            push!(
                "year",
                match year {
                    components::Year::TwoDigit | components::Year::TwoDigitWeekOf => "2-digit",
                    _ => "numeric",
                },
            );
        }
        if let Some(month) = bag.month {
            push!(
                "month",
                match month {
                    components::Month::Numeric => "numeric",
                    components::Month::TwoDigit => "2-digit",
                    components::Month::Long => "long",
                    components::Month::Short => "short",
                    components::Month::Narrow => "narrow",
                    _ => "numeric",
                },
            );
        }
        if let Some(day) = bag.day {
            push!(
                "day",
                match day {
                    components::Day::TwoDigitDayOfMonth => "2-digit",
                    _ => "numeric",
                },
            );
        }
        if let Some(hour) = bag.hour {
            push!("hour", numeric_width(hour));
        }
        if let Some(minute) = bag.minute {
            push!("minute", numeric_width(minute));
        }
        if let Some(second) = bag.second {
            push!("second", numeric_width(second));
        }
        if let Some(digits) = bag.subsecond {
            out.push((
                "fractionalSecondDigits",
                Setting::Number(digits as u8 as f64),
            ));
        }
        if let Some(zone) = bag.time_zone_name {
            push!(
                "timeZoneName",
                match zone {
                    components::TimeZoneName::ShortSpecific => "short",
                    components::TimeZoneName::LongSpecific => "long",
                    components::TimeZoneName::ShortOffset => "shortOffset",
                    components::TimeZoneName::LongOffset => "longOffset",
                    components::TimeZoneName::ShortGeneric => "shortGeneric",
                    components::TimeZoneName::LongGeneric => "longGeneric",
                    _ => "short",
                },
            );
        }
        let cycle = bag.hour.and(bag.hour_cycle).map(|cycle| match cycle {
            HourCycle::H11 => "h11",
            HourCycle::H23 | HourCycle::Clock24 => "h23",
            _ => "h12",
        });
        (out, cycle)
    }

    /// ICU's `Alignment::Column` changes every one-digit month, day and hour in a composite
    /// pattern, even when only one of them was requested as `2-digit`. ECMA-402 resolves those
    /// widths independently, so restore the explicit request for fields present in the pattern.
    fn restore_requested_numeric_widths(&self, components: &mut [(&'static str, Setting)]) {
        if !needs_numeric_width_adjustment(&self.options) {
            return;
        }
        for (name, setting) in components {
            let requested = match *name {
                "month" => self.options.month.as_deref(),
                "day" => self.options.day.as_deref(),
                "hour" => self.options.hour.as_deref(),
                _ => None,
            };
            if let Some(width @ ("numeric" | "2-digit")) = requested {
                *setting = Setting::string(width);
            }
        }
    }
}

fn text_width(width: components::Text) -> &'static str {
    match width {
        components::Text::Long => "long",
        components::Text::Narrow => "narrow",
        _ => "short",
    }
}

fn numeric_width(width: components::Numeric) -> &'static str {
    match width {
        components::Numeric::TwoDigit => "2-digit",
        _ => "numeric",
    }
}

fn spelled_hour_cycle(value: &str) -> Option<HourCycle> {
    match value {
        "h11" => Some(HourCycle::H11),
        "h12" => Some(HourCycle::H12),
        "h23" => Some(HourCycle::H23),
        // ICU4X has no h24: the cycle whose midnight is written 24 is not in CLDR's pattern data
        // for any locale ICU4X ships, so the request resolves to the 24-hour cycle that is, and
        // `resolvedOptions()` -- which reads the pattern rather than the request -- reports h23.
        "h24" => Some(HourCycle::Clock24),
        _ => None,
    }
}

/// The numbering system a locale falls back to when nothing chose one.
///
/// `latn` for everything the common data set spells with ASCII digits, which is all of it except a
/// handful of locales whose CLDR default is their own script's digits.
fn default_numbering_system(resolution: &Resolution) -> String {
    use icu_decimal::DecimalFormatter;
    use icu_decimal::options::DecimalFormatterOptions;
    let preferences = resolution.icu().clone().into();
    match DecimalFormatter::try_new(preferences, DecimalFormatterOptions::default()) {
        Ok(formatter) => {
            // The formatter knows which digits it will use; asking it is the only way to read the
            // resolved system back out of a baked provider.
            let zero = formatter
                .format(&fixed_decimal::Decimal::from(0))
                .to_string();
            match zero.chars().next() {
                Some('0') | None => "latn".to_owned(),
                Some(digit) => numbering_system_for_zero(digit).to_owned(),
            }
        }
        Err(_) => "latn".to_owned(),
    }
}

/// The numbering systems whose zero digit identifies them, for locales whose CLDR default is not
/// `latn`. Transcribed from UTS 35's numbering system table, restricted to the decimal systems
/// that appear as a CLDR locale default.
fn numbering_system_for_zero(zero: char) -> &'static str {
    match zero {
        '\u{0660}' => "arab",
        '\u{06f0}' => "arabext",
        '\u{09e6}' => "beng",
        '\u{0966}' => "deva",
        '\u{0ae6}' => "gujr",
        '\u{0a66}' => "guru",
        '\u{0be6}' => "tamldec",
        '\u{0c66}' => "telu",
        '\u{0ce6}' => "knda",
        '\u{0d66}' => "mlym",
        '\u{0b66}' => "orya",
        '\u{0e50}' => "thai",
        '\u{0ed0}' => "laoo",
        '\u{1040}' => "mymr",
        '\u{17e0}' => "khmr",
        '\u{0f20}' => "tibt",
        '\u{1946}' => "limb",
        '\u{aa50}' => "cham",
        _ => "latn",
    }
}

/// Resolve the exact offset at an instant, or `None` when timezone data was not linked.
fn resolve_offset(zone: &TimeZoneChoice, epoch_milliseconds: f64) -> Option<UtcOffset> {
    match zone {
        TimeZoneChoice::Offset(seconds) => UtcOffset::try_from_seconds(*seconds).ok(),
        TimeZoneChoice::Named(name, _) => {
            #[cfg(feature = "timezone_database")]
            {
                static TZDB: OnceLock<CompiledTzdbProvider> = OnceLock::new();
                let provider = TZDB.get_or_init(CompiledTzdbProvider::default);
                let zone = provider.get(name.as_bytes()).ok()?;
                let timestamp_ns = (epoch_milliseconds as i128).checked_mul(1_000_000)?;
                let seconds = provider
                    .transition_nanoseconds_for_utc_epoch_nanoseconds(zone, timestamp_ns)
                    .ok()?
                    .0;
                UtcOffset::try_from_seconds(i32::try_from(seconds).ok()?).ok()
            }
            #[cfg(not(feature = "timezone_database"))]
            {
                let _ = (name, epoch_milliseconds);
                None
            }
        }
    }
}

/// The `±HH:MM` spelling of a fixed offset, which is what `resolvedOptions()` reports for one.
fn offset_identifier(seconds: i32) -> String {
    let sign = if seconds < 0 { '-' } else { '+' };
    let magnitude = seconds.abs();
    let (hours, minutes, remainder) = (magnitude / 3600, (magnitude % 3600) / 60, magnitude % 60);
    if remainder == 0 {
        format!("{sign}{hours:02}:{minutes:02}")
    } else {
        format!("{sign}{hours:02}:{minutes:02}:{remainder:02}")
    }
}

/// ICU4X's zoned input, kept opaque so `icu_time` does not appear in a signature.
struct ZonedInstant(
    ZonedDateTime<icu_calendar::Iso, icu_time::TimeZoneInfo<icu_time::zone::models::AtTime>>,
);

/// `ToDateTimeOptions`'s defaulting step: a request with no date and no time component at all
/// asks for the numeric year, month and day.
///
/// `timeZoneName` deliberately does not count as a component here, which is why
/// `{ timeZoneName: "short" }` is a date *and* a zone rather than a bare zone -- the
/// specification's `needDefaults` list names the eleven date and time fields and not the zone.
fn with_defaults(options: &Options) -> Options {
    let has_field = options.weekday.is_some()
        || options.year.is_some()
        || options.month.is_some()
        || options.day.is_some()
        || options.day_period.is_some()
        || options.hour.is_some()
        || options.minute.is_some()
        || options.second.is_some()
        || options.fractional_second_digits.is_some();
    if has_field {
        return options.clone();
    }
    Options {
        year: Some("numeric".to_owned()),
        month: Some("numeric".to_owned()),
        day: Some("numeric".to_owned()),
        ..options.clone()
    }
}

/// One shape's formatters, built from the option bag restricted to the fields that shape has.
struct Shaped {
    formatter: DateTimeFormatter<CompositeFieldSet>,
    ranges: DateRangeFormatter<CompositeFieldSet>,
    options: Options,
    /// Whether the resolved pattern shows a time, which decides whether a range compares two
    /// instants or two days.
    shows_time: bool,
}

/// `GetDateTimeFormat`'s restriction step: the request, narrowed to the fields the value has.
///
/// Three things happen, in the specification's order.
///
/// * Every component the shape does not carry is removed. A `PlainYearMonth` has no day and a
///   `PlainTime` has no month, so a request for one is dropped rather than answered with a
///   reference value -- a `PlainMonthDay` printed with a year would be printing 1972.
/// * `dateStyle` survives only for a shape with date fields and `timeStyle` only for one with time
///   fields, and the zone is dropped for every wall-clock shape.
/// * If nothing at all survives, the answer depends on whether anything was *asked* for: a request
///   that named components which the shape does not have is a `TypeError` ("no overlap"), while a
///   request that named none gets that shape's own defaults. This is why
///   `new Intl.DateTimeFormat().format(plainTime)` is a time and not a date.
fn restrict(options: &Options, shape: Shape) -> Result<Options, Error> {
    let reach = shape.reach();
    let mut kept = options.clone();
    if !reach.weekday {
        kept.weekday = None;
    }
    if !reach.era {
        kept.era = None;
    }
    if !reach.year {
        kept.year = None;
    }
    if !reach.month {
        kept.month = None;
    }
    if !reach.day {
        kept.day = None;
    }
    if !reach.time {
        kept.day_period = None;
        kept.hour = None;
        kept.minute = None;
        kept.second = None;
        kept.fractional_second_digits = None;
        kept.time_style = None;
    }
    if !reach.zone {
        kept.time_zone_name = None;
    }
    let has_date = reach.weekday || reach.era || reach.year || reach.month || reach.day;
    if !has_date {
        kept.date_style = None;
    }
    if names_a_field(&kept) || kept.date_style.is_some() || kept.time_style.is_some() {
        return Ok(kept);
    }
    if names_a_field(options) || options.date_style.is_some() || options.time_style.is_some() {
        return Err(Error::type_error(
            "the formatter's options and the value have no field in common",
        ));
    }
    let numeric = || Some("numeric".to_owned());
    if has_date {
        kept.year = reach.year.then(numeric).flatten();
        kept.month = numeric();
        kept.day = reach.day.then(numeric).flatten();
    }
    // A `Temporal` value that carries a time defaults to one, which is where these shapes differ
    // from a `Date`: `Intl.DateTimeFormat`'s own default is a date and nothing else, while a
    // `PlainDateTime` or an `Instant` with no options at all is written the way
    // `Date.prototype.toLocaleString` writes one -- date *and* time.
    if reach.time {
        kept.hour = numeric();
        kept.minute = numeric();
        kept.second = numeric();
    }
    Ok(kept)
}

/// Whether the bag names any of the components `needDefaults` counts.
///
/// `era` and `timeZoneName` are deliberately not in the list: the specification's own
/// `needDefaults` step names the nine fields below and nothing else, which is why
/// `{ era: "narrow" }` is a request for a defaulted date *with* an era rather than for an era on
/// its own.
fn names_a_field(options: &Options) -> bool {
    options.weekday.is_some()
        || options.year.is_some()
        || options.month.is_some()
        || options.day.is_some()
        || options.day_period.is_some()
        || options.hour.is_some()
        || options.minute.is_some()
        || options.second.is_some()
        || options.fractional_second_digits.is_some()
}

/// The field set a shape's already-restricted options resolve to.
///
/// Two things differ from the numeric path. A style keeps its length but narrows its date fields
/// to what the shape has, so a `PlainYearMonth` written with `dateStyle: "short"` is a short
/// year-month rather than a short date with an invented day. And every wall-clock shape loses its
/// zone: `timeStyle: "long"` names the zone in its pattern, and a value with no zone must not be
/// given the formatter's.
fn shaped_field_set(options: &Options, shape: Shape) -> Result<CompositeFieldSet, Error> {
    let mut builder = FieldSetBuilder::new();
    if options.date_style.is_some() || options.time_style.is_some() {
        apply_styles(&mut builder, options);
        match shape {
            Shape::PlainYearMonth => builder.date_fields = Some(DateFields::YM),
            Shape::PlainMonthDay => builder.date_fields = Some(DateFields::MD),
            _ => {}
        }
    } else {
        // Already defaulted by `restrict`, so `with_defaults` must not run again: it would give a
        // `PlainTime` a year, a month and a day.
        apply_components(&mut builder, options);
    }
    if shape.is_plain() {
        builder.zone_style = None;
    }
    builder
        .clone()
        .build_composite()
        .or_else(|_| {
            let mut relaxed = builder.clone();
            relaxed.year_style = None;
            relaxed.alignment = None;
            relaxed.build_composite()
        })
        .map_err(|_| Error::range("no date format matches the requested components"))
}

/// Translate the option bag into the ICU4X field set that comes closest to it.
fn build_field_set(options: &Options) -> Result<CompositeFieldSet, Error> {
    let mut builder = FieldSetBuilder::new();
    if options.date_style.is_some() || options.time_style.is_some() {
        apply_styles(&mut builder, options);
    } else {
        apply_components(&mut builder, &with_defaults(options));
    }
    builder
        .clone()
        .build_composite()
        .or_else(|_| {
            // The builder rejects an option the chosen field set cannot honour -- a year style
            // with no year in it, an alignment with nothing to align. Dropping those and
            // rebuilding is not a guess: the fields are what the caller asked for and the
            // dropped option was only ever a refinement of them.
            let mut relaxed = builder.clone();
            relaxed.year_style = None;
            relaxed.alignment = None;
            relaxed.build_composite()
        })
        .map_err(|_| Error::range("no date format matches the requested components"))
}

/// `dateStyle`/`timeStyle`: the four standard patterns each locale carries, by name.
fn apply_styles(builder: &mut FieldSetBuilder, options: &Options) {
    if let Some(style) = options.date_style.as_deref() {
        builder.date_fields = Some(match style {
            "full" => DateFields::YMDE,
            _ => DateFields::YMD,
        });
        builder.length = Some(match style {
            "full" | "long" => Length::Long,
            "medium" => Length::Medium,
            _ => Length::Short,
        });
    }
    if let Some(style) = options.time_style.as_deref() {
        builder.time_precision = Some(match style {
            "short" => TimePrecision::Minute,
            _ => TimePrecision::Second,
        });
        match style {
            "full" => builder.zone_style = Some(ZoneStyle::SpecificLong),
            "long" => builder.zone_style = Some(ZoneStyle::SpecificShort),
            _ => {}
        }
        if builder.length.is_none() {
            builder.length = Some(match style {
                "full" | "long" => Length::Long,
                "medium" => Length::Medium,
                _ => Length::Short,
            });
        }
    }
}

/// The component bag: which fields, how wide, and how the two map onto a field set plus a length.
fn apply_components(builder: &mut FieldSetBuilder, options: &Options) {
    let (weekday, year, month, day) = (
        options.weekday.is_some(),
        options.year.is_some(),
        options.month.is_some(),
        options.day.is_some(),
    );
    let has_time = options.hour.is_some()
        || options.minute.is_some()
        || options.second.is_some()
        || options.fractional_second_digits.is_some()
        || options.day_period.is_some();
    let has_date = weekday || year || month || day;

    if has_date {
        // The combinations CLDR has patterns for. A request outside them -- a year with a day and
        // no month, say -- resolves to the smallest field set that contains everything asked for,
        // which is why `resolvedOptions()` reads the components back out of the pattern: the
        // caller is told about the field that had to be added.
        builder.date_fields = Some(match (year, month, day, weekday) {
            (true, _, true, true) => DateFields::YMDE,
            (true, _, true, false) => DateFields::YMD,
            (false, true, true, true) => DateFields::MDE,
            (false, true, true, false) => DateFields::MD,
            (false, false, true, true) => DateFields::DE,
            (false, false, true, false) => DateFields::D,
            (true, true, false, true) => DateFields::YMDE,
            (true, true, false, false) => DateFields::YM,
            (true, false, false, true) => DateFields::YMDE,
            (true, false, false, false) => DateFields::Y,
            (false, true, false, true) => DateFields::MDE,
            (false, true, false, false) => DateFields::M,
            (false, false, false, true) => DateFields::E,
            (false, false, false, false) => DateFields::D,
        });
    }

    // The width of the *text* fields is what ICU4X spells as a length: a long month is a wide
    // pattern, an abbreviated one is medium, and a numeric one is short. The month decides when it
    // is present because it is the field whose width varies most; otherwise the weekday does.
    let width = options
        .month
        .as_deref()
        .map(|value| match value {
            "long" => Length::Long,
            "short" | "narrow" => Length::Medium,
            _ => Length::Short,
        })
        .or_else(|| {
            options.weekday.as_deref().map(|value| match value {
                "long" => Length::Long,
                "short" => Length::Medium,
                _ => Length::Short,
            })
        })
        .or_else(|| {
            options.era.as_deref().map(|value| {
                if value == "long" {
                    Length::Long
                } else {
                    Length::Medium
                }
            })
        });
    if let Some(width) = width {
        builder.length = Some(width);
    }

    // `2-digit` is ICU4X's column alignment: the numeric fields pad to two digits so a column of
    // dates lines up, which is the same rendering the language asks for.
    let two_digit = [&options.month, &options.day, &options.hour]
        .into_iter()
        .any(|value| value.as_deref() == Some("2-digit"));
    if two_digit {
        builder.alignment = Some(Alignment::Column);
    }

    if year {
        // `year: "numeric"` means the whole year, always: ICU4X's default would abbreviate 2025 to
        // 25 in a short pattern, which is the `2-digit` request rather than this one.
        builder.year_style = Some(if options.era.is_some() {
            YearStyle::WithEra
        } else if options.year.as_deref() == Some("2-digit") {
            YearStyle::Auto
        } else {
            YearStyle::Full
        });
    }

    if has_time {
        builder.time_precision = Some(match options.fractional_second_digits {
            Some(1) => TimePrecision::Subsecond(SubsecondDigits::S1),
            Some(2) => TimePrecision::Subsecond(SubsecondDigits::S2),
            Some(_) => TimePrecision::Subsecond(SubsecondDigits::S3),
            None if options.second.is_some() => TimePrecision::Second,
            None if options.minute.is_some() => TimePrecision::Minute,
            None => TimePrecision::Hour,
        });
    }

    if let Some(name) = options.time_zone_name.as_deref() {
        builder.zone_style = Some(match name {
            "long" => ZoneStyle::SpecificLong,
            "shortOffset" => ZoneStyle::LocalizedOffsetShort,
            "longOffset" => ZoneStyle::LocalizedOffsetLong,
            "shortGeneric" => ZoneStyle::GenericShort,
            "longGeneric" => ZoneStyle::GenericLong,
            _ => ZoneStyle::SpecificShort,
        });
    }
}

/// A `writeable` sink that names ICU4X's date-time runs with the language's vocabulary.
///
/// Unlike the number formatter's collector this one needs the whole stack rather than the
/// innermost annotation: the fractional second is a `decimal` fraction *inside* a `datetime`
/// second, and the two together are what tell it apart from the second's own digits.
#[derive(Default)]
struct PartCollector {
    parts: Vec<(&'static str, String)>,
    stack: Vec<writeable::Part>,
}

impl PartCollector {
    fn finish(self) -> Vec<(&'static str, String)> {
        self.parts
    }

    fn name(&self) -> &'static str {
        let outer = self
            .stack
            .iter()
            .find(|part| part.category == "datetime")
            .copied();
        let inner = self
            .stack
            .iter()
            .rev()
            .find(|part| part.category == "decimal")
            .copied();
        let Some(outer) = outer else {
            return "literal";
        };
        let name = match outer.value {
            "era" => "era",
            "year" => "year",
            "relatedYear" => "relatedYear",
            "yearName" => "yearName",
            "extendedYear" => "year",
            "month" => "month",
            "day" => "day",
            "weekday" => "weekday",
            "dayPeriod" => "dayPeriod",
            "hour" => "hour",
            "minute" => "minute",
            "second" => "second",
            "timeZoneName" => "timeZoneName",
            _ => "literal",
        };
        if name == "second"
            && let Some(inner) = inner
        {
            // Inside a second, the decimal separator is a literal and the fraction is its own
            // part; only the integer digits are the second itself.
            return match inner.value {
                "fraction" => "fractionalSecond",
                "decimal" => "literal",
                _ => "second",
            };
        }
        name
    }

    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let name = self.name();
        match self.parts.last_mut() {
            Some((last, buffer)) if *last == name => buffer.push_str(text),
            _ => self.parts.push((name, text.to_owned())),
        }
    }
}

impl core::fmt::Write for PartCollector {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        self.push(text);
        Ok(())
    }
}

impl writeable::PartsWrite for PartCollector {
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

/// Undo padding that ICU's whole-pattern column alignment applied to a field whose
/// `Intl.DateTimeFormat` width is independently `numeric`.
fn adjust_numeric_widths(
    mut parts: Vec<(&'static str, String)>,
    options: &Options,
    zero_digit: &str,
) -> Vec<(&'static str, String)> {
    for (kind, text) in &mut parts {
        let requested = match *kind {
            "month" => options.month.as_deref(),
            "day" => options.day.as_deref(),
            "hour" => options.hour.as_deref(),
            _ => None,
        };
        if requested != Some("numeric") {
            continue;
        }
        if let Some(unpadded) = text.strip_prefix(zero_digit)
            && unpadded.chars().count() == 1
        {
            *text = unpadded.to_owned();
        }
    }
    parts
}

/// Whether ICU's global column alignment can pad an explicitly numeric month, day or hour.
fn needs_numeric_width_adjustment(options: &Options) -> bool {
    let widths = [
        options.month.as_deref(),
        options.day.as_deref(),
        options.hour.as_deref(),
    ];
    widths.contains(&Some("2-digit")) && widths.contains(&Some("numeric"))
}

fn join_parts(parts: impl IntoIterator<Item = String>) -> String {
    let mut joined = String::new();
    for part in parts {
        joined.push_str(&part);
    }
    joined
}

/// Give each run of a formatted range the side of the range it came from.
///
/// See [`DateTimeFormat::format_range_to_parts`] for the rule and why it is reconstructed here.
fn attribute_sources(
    parts: Vec<(&'static str, String)>,
) -> Vec<(&'static str, &'static str, String)> {
    // A field mentioned an even number of times is one the two ends differ in; the first half of
    // its occurrences belongs to the start and the second half to the end. A field mentioned once
    // is agreed on by both ends and is shared.
    let mut first_span: Option<(usize, usize)> = None;
    let mut second_span: Option<(usize, usize)> = None;
    for (index, (kind, _)) in parts.iter().enumerate() {
        if *kind == "literal" {
            continue;
        }
        let occurrences: Vec<usize> = parts
            .iter()
            .enumerate()
            .filter(|(_, (other, _))| other == kind)
            .map(|(position, _)| position)
            .collect();
        if !occurrences.len().is_multiple_of(2) || occurrences.is_empty() {
            continue;
        }
        let half = occurrences.len() / 2;
        let side = if occurrences.iter().position(|p| *p == index).unwrap_or(0) < half {
            &mut first_span
        } else {
            &mut second_span
        };
        *side = Some(match *side {
            None => (index, index),
            Some((low, high)) => (low.min(index), high.max(index)),
        });
    }
    parts
        .into_iter()
        .enumerate()
        .map(|(index, (kind, text))| {
            let source = match (first_span, second_span) {
                (Some((low, high)), _) if index >= low && index <= high => "startRange",
                (_, Some((low, high))) if index >= low && index <= high => "endRange",
                _ => "shared",
            };
            (kind, source, text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> Option<TimeZoneChoice> {
        canonical_time_zone("UTC")
    }

    fn format(locale: &str, options: Options, at: f64) -> String {
        DateTimeFormat::new(&[locale.to_owned()], options)
            .expect("the formatter builds")
            .format(at)
            .expect("the instant formats")
    }

    fn styled(date: Option<&str>, time: Option<&str>) -> Options {
        Options {
            date_style: date.map(str::to_owned),
            time_style: time.map(str::to_owned),
            time_zone: utc(),
            ..Options::default()
        }
    }

    #[test]
    fn the_date_styles_are_the_locales_four_patterns() {
        // 1886-05-01T14:12:47Z, the instant `intl402/DateTimeFormat/prototype/format/
        // timedatestyle-en.js` uses, so a divergence here is a divergence there.
        let at = -2_640_332_833_000.0;
        assert_eq!(
            format("en-US", styled(Some("full"), None), at),
            "Saturday, May 1, 1886"
        );
        assert_eq!(
            format("en-US", styled(Some("long"), None), at),
            "May 1, 1886"
        );
        assert_eq!(
            format("en-US", styled(Some("medium"), None), at),
            "May 1, 1886"
        );
        // A recorded divergence rather than a match: CLDR's `short` date pattern writes a
        // two-digit year, and ICU4X widens it to four whenever two digits would be ambiguous --
        // its window is roughly 1969 to 2069, so 1970 is `1/1/70` and 1886 is not `5/1/86`. That
        // is ICU4X's policy, not a guess made here, and it is the more informative of the two.
        assert_eq!(format("en-US", styled(Some("short"), None), at), "5/1/1886");
        assert_eq!(format("en-US", styled(Some("short"), None), 0.0), "1/1/70");
    }

    #[test]
    fn a_component_bag_produces_the_locales_field_order_rather_than_a_fixed_one() {
        let bag = |locale: &str| {
            format(
                locale,
                Options {
                    year: Some("numeric".to_owned()),
                    month: Some("long".to_owned()),
                    day: Some("numeric".to_owned()),
                    time_zone: utc(),
                    ..Options::default()
                },
                0.0,
            )
        };
        assert_eq!(bag("en-US"), "January 1, 1970");
        assert_eq!(bag("de-DE"), "1. Januar 1970");
    }

    #[test]
    fn resolved_options_report_the_pattern_rather_than_the_request() {
        let formatter = DateTimeFormat::new(
            &["en-US".to_owned()],
            Options {
                year: Some("numeric".to_owned()),
                month: Some("numeric".to_owned()),
                day: Some("numeric".to_owned()),
                time_zone: utc(),
                ..Options::default()
            },
        )
        .unwrap();
        let options = formatter.resolved_options();
        let get = |name: &str| {
            options
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(get("locale"), Some(Setting::string("en-US")));
        assert_eq!(get("calendar"), Some(Setting::string("gregory")));
        assert_eq!(get("numberingSystem"), Some(Setting::string("latn")));
        assert_eq!(get("timeZone"), Some(Setting::string("UTC")));
        assert_eq!(get("year"), Some(Setting::string("numeric")));
        assert_eq!(get("month"), Some(Setting::string("numeric")));
        assert_eq!(get("day"), Some(Setting::string("numeric")));
        // Nothing asked for a time, so nothing reports one.
        assert_eq!(get("hour"), None);
        assert_eq!(get("hourCycle"), None);
    }

    #[test]
    fn the_hour_cycle_follows_hour12_and_then_the_locale() {
        let hours = |locale: &str, hour12: Option<bool>| {
            format(
                locale,
                Options {
                    hour: Some("numeric".to_owned()),
                    minute: Some("numeric".to_owned()),
                    hour12,
                    time_zone: utc(),
                    ..Options::default()
                },
                14.0 * 3_600_000.0,
            )
        };
        assert!(hours("en-US", None).contains("PM"));
        assert!(hours("en-US", Some(false)).starts_with("14"));
        assert!(!hours("de-DE", None).contains("PM"));
    }

    #[test]
    fn a_named_zone_is_reported_by_the_name_the_caller_used() {
        // The specification stopped canonicalizing links: `Asia/Calcutta` stays itself even though
        // the zone behind it is `Asia/Kolkata`.
        let calcutta = canonical_time_zone("Asia/CALCUTTA").expect("a known zone");
        assert!(matches!(&calcutta, TimeZoneChoice::Named(name, _) if name == "Asia/Calcutta"));
        assert!(canonical_time_zone("America/San_Francisco").is_none());
        assert!(canonical_time_zone("PST").is_none());
    }

    #[test]
    #[cfg(feature = "timezone_database")]
    fn named_daylight_zone_uses_the_transition_at_each_instant() {
        let india = DateTimeFormat::new(
            &["en-US".to_owned()],
            Options {
                hour: Some("numeric".to_owned()),
                minute: Some("numeric".to_owned()),
                hour12: Some(false),
                time_zone: canonical_time_zone("Asia/Kolkata"),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(india.format(0.0).unwrap(), "05:30");

        let vancouver = DateTimeFormat::new(
            &["en-US".to_owned()],
            Options {
                hour: Some("2-digit".to_owned()),
                minute: Some("2-digit".to_owned()),
                hour_cycle: Some("h23".to_owned()),
                time_zone: canonical_time_zone("America/Vancouver"),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(vancouver.format(1_705_320_000_000.0).unwrap(), "04:00");
        assert_eq!(vancouver.format(1_719_835_200_000.0).unwrap(), "05:00");
        assert_eq!(vancouver.time_zone(), "America/Vancouver");
    }

    #[test]
    fn format_to_parts_names_each_run_rather_than_splitting_the_string() {
        let formatter = DateTimeFormat::new(
            &["en-US".to_owned()],
            Options {
                year: Some("numeric".to_owned()),
                month: Some("short".to_owned()),
                day: Some("numeric".to_owned()),
                time_zone: utc(),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(
            formatter.format_to_parts(0.0).unwrap(),
            vec![
                ("month", "Jan".to_owned()),
                ("literal", " ".to_owned()),
                ("day", "1".to_owned()),
                ("literal", ", ".to_owned()),
                ("year", "1970".to_owned()),
            ]
        );
    }

    #[test]
    fn a_range_whose_ends_share_a_month_is_written_once() {
        let formatter = DateTimeFormat::new(
            &["en-US".to_owned()],
            Options {
                year: Some("numeric".to_owned()),
                month: Some("short".to_owned()),
                day: Some("numeric".to_owned()),
                time_zone: utc(),
                ..Options::default()
            },
        )
        .unwrap();
        let day = 86_400_000.0;
        // The separator is CLDR 48's, a thin space either side of an en dash, rather than the
        // ordinary spaces an older bundle used; a differential run against a node built on a
        // different CLDR will see that difference and nothing else.
        assert_eq!(
            formatter.format_range(0.0, 4.0 * day).unwrap(),
            "Jan 1\u{2009}\u{2013}\u{2009}5, 1970"
        );
        // The whole reason `formatRange` is not two calls to `format` joined by a dash.
        assert_ne!(
            formatter.format_range(0.0, 4.0 * day).unwrap(),
            "Jan 1, 1970 – Jan 5, 1970"
        );
    }

    #[test]
    fn a_range_part_says_which_end_it_came_from() {
        let formatter = DateTimeFormat::new(
            &["en-US".to_owned()],
            Options {
                year: Some("numeric".to_owned()),
                month: Some("short".to_owned()),
                day: Some("numeric".to_owned()),
                time_zone: utc(),
                ..Options::default()
            },
        )
        .unwrap();
        let day = 86_400_000.0;
        let parts = formatter.format_range_to_parts(0.0, 4.0 * day).unwrap();
        let sources: Vec<(&str, &str)> = parts
            .iter()
            .map(|(kind, source, _)| (*kind, *source))
            .collect();
        assert_eq!(
            sources,
            vec![
                ("month", "shared"),
                ("literal", "shared"),
                ("day", "startRange"),
                ("literal", "shared"),
                ("day", "endRange"),
                ("literal", "shared"),
                ("year", "shared"),
            ]
        );
        // Two equal instants are one date, and every part of it belongs to both ends.
        let same = formatter.format_range_to_parts(0.0, 0.0).unwrap();
        assert!(same.iter().all(|(_, source, _)| *source == "shared"));
    }

    #[test]
    fn a_calendar_option_changes_the_year_rather_than_the_language() {
        let buddhist = format(
            "en-US",
            Options {
                calendar: Some("buddhist".to_owned()),
                year: Some("numeric".to_owned()),
                time_zone: utc(),
                ..Options::default()
            },
            0.0,
        );
        assert!(buddhist.contains("2513"), "buddhist year was {buddhist}");
    }

    #[test]
    fn every_calendar_this_build_accepts_is_one_icu_can_build() {
        // The table is transcribed, so this is what stops it drifting from what the formatter will
        // take: a name accepted here that ICU4X does not know would resolve silently to Gregorian.
        for name in CALENDARS {
            assert!(
                parsed_calendar_algorithm(name).is_some(),
                "{name} is not a calendar ICU4X names"
            );
        }
        for name in available_calendars() {
            let formatter = DateTimeFormat::new(
                &["en-US".to_owned()],
                Options {
                    calendar: Some(name.clone()),
                    time_zone: utc(),
                    ..Options::default()
                },
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(
                formatter
                    .resolved_options()
                    .contains(&("calendar", Setting::string(&name))),
                "{name} is not reported back"
            );
        }
        // The two Hijri requests that are not calendars resolve to one that is.
        let hijri = DateTimeFormat::new(
            &["en-US".to_owned()],
            Options {
                calendar: Some("islamic".to_owned()),
                time_zone: utc(),
                ..Options::default()
            },
        )
        .unwrap();
        assert!(
            hijri
                .resolved_options()
                .contains(&("calendar", Setting::string(HIJRI_FALLBACK)))
        );
    }

    #[test]
    fn the_available_zone_and_calendar_lists_are_sorted_and_unique() {
        let zones = available_time_zones();
        assert!(zones.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(zones.iter().any(|zone| zone == "America/New_York"));
        let calendars = available_calendars();
        assert!(calendars.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(calendars.iter().any(|calendar| calendar == "gregory"));
    }
}

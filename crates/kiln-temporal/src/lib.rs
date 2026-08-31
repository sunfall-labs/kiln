//! `Temporal`'s calendar and time semantics, behind a Kiln-shaped Seam.
//!
//! A calendar is not a page of code. `Temporal` carries eight interlocking types, six calendar
//! systems, the IANA zone database and a rounding model that has to agree with itself across all
//! of them; a second implementation of that would agree with the first on `2024-01-01` and
//! disagree somewhere in the Hebrew leap-month rules, which is the *wrong answer* failure mode the
//! three-corpus arrangement exists to prevent. So the engine is linked, not rewritten: ADR 0025
//! made that call for regular expressions and ADR 0029 §4 names `temporal_rs` as the engine for
//! this one.
//!
//! What this crate adds on top of `temporal_rs` is the Seam. Nothing in the public interface below
//! mentions a `temporal_rs` type: a [`TemporalValue`] is opaque, every field comes back as a
//! scalar or a string in [`Field`], and every option arrives as the string the language spells it
//! with. That is what keeps the engine out of HIR, IR, the protocol records and the embedding ABI
//! (ADR 0029's adoption rule, condition 2), and it is what makes the deletion test meaningful:
//! removing `temporal_rs` would leave this interface standing and its implementation missing.
//!
//! The other thing the Seam buys is the clock. `temporal_rs` reads the host's clock and the host's
//! zone through its `sys` features, and this realm has neither: the clock is virtual and starts at
//! the epoch, and the zone is UTC (ADR 0015). Those features are off, and [`now`] takes the
//! realm's instant as an argument, so there is no path from here to the host's clock to forget to
//! close.

use std::fmt;
use std::str::FromStr;

use temporal_rs::fields::{CalendarFields, DateTimeFields};
use temporal_rs::options::{
    DifferenceSettings, Disambiguation, DisplayCalendar, DisplayOffset, DisplayTimeZone,
    OffsetDisambiguation, Overflow, RelativeTo, RoundingIncrement, RoundingOptions,
    ToStringRoundingOptions, Unit,
};
use temporal_rs::parsers::Precision;
use temporal_rs::partial::{PartialDate, PartialDuration, PartialTime, PartialYearMonth};
use temporal_rs::{
    Calendar, Duration, Instant, MonthCode, PlainDate, PlainDateTime, PlainMonthDay, PlainTime,
    PlainYearMonth, TimeZone, ZonedDateTime,
};

/// Which of the eight `Temporal` types a value is.
///
/// The language keeps them apart -- `Temporal.PlainDate.compare` refuses a `PlainTime`, and every
/// constructor's `from` has a different set of fields it will accept -- so the caller has to be
/// able to ask, and asking must not require pattern-matching on the engine's types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    PlainDate,
    PlainTime,
    PlainDateTime,
    PlainYearMonth,
    PlainMonthDay,
    Duration,
    Instant,
    ZonedDateTime,
}

impl Kind {
    /// The name the language uses, which is also what `Symbol.toStringTag` answers.
    pub fn tag(self) -> &'static str {
        match self {
            Self::PlainDate => "Temporal.PlainDate",
            Self::PlainTime => "Temporal.PlainTime",
            Self::PlainDateTime => "Temporal.PlainDateTime",
            Self::PlainYearMonth => "Temporal.PlainYearMonth",
            Self::PlainMonthDay => "Temporal.PlainMonthDay",
            Self::Duration => "Temporal.Duration",
            Self::Instant => "Temporal.Instant",
            Self::ZonedDateTime => "Temporal.ZonedDateTime",
        }
    }
}

/// Which JavaScript error a failed operation should become.
///
/// `Temporal` is unusually specific about this -- a field out of range is a `RangeError`, a
/// malformed ISO string is a `RangeError` too, and only a genuinely wrong *type* of argument is a
/// `TypeError` -- and Test262 checks the distinction on nearly every path, so the engine's own
/// classification is carried through rather than flattened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Type,
    Range,
    Syntax,
}

/// A failure, already classified into the error the language throws for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    fn range(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Range,
            message: message.into(),
        }
    }

    pub fn type_error(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Type,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<temporal_rs::TemporalError> for Error {
    fn from(error: temporal_rs::TemporalError) -> Self {
        // `Generic` and `Assert` have no JavaScript counterpart that `Temporal` ever throws, and
        // the specification's fallback for "this cannot be represented" is a RangeError, so they
        // map there rather than to a bare `Error` a test would not be expecting.
        let kind = match error.kind() {
            temporal_rs::error::ErrorKind::Type => ErrorKind::Type,
            temporal_rs::error::ErrorKind::Syntax => ErrorKind::Syntax,
            _ => ErrorKind::Range,
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// One value read out of a `Temporal` object, in the terms the language reports it in.
///
/// Deliberately not a JavaScript value: this crate has no idea what one is. [`Field::Integer128`]
/// is the `epochNanoseconds` case, which the language represents as a `BigInt` and which no `f64`
/// can hold exactly; the caller turns it into one.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Number(f64),
    Integer128(i128),
    Text(String),
    Flag(bool),
    /// The property exists and its value is `undefined` -- `weekOfYear` outside the ISO calendar,
    /// which has no weeks to number. Distinct from "no such property", which is `None`.
    Undefined,
}

/// The calendar and wall-clock fields a `Temporal` object can be built from or updated with.
///
/// One flat record rather than a per-type one because the language's own `with` accepts whichever
/// subset the receiver understands and rejects the rest, and a caller that has already read a
/// property bag should not have to know which type it is about to hand it to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fields {
    pub era: Option<String>,
    pub era_year: Option<f64>,
    pub year: Option<f64>,
    pub month: Option<f64>,
    pub month_code: Option<String>,
    pub day: Option<f64>,
    pub hour: Option<f64>,
    pub minute: Option<f64>,
    pub second: Option<f64>,
    pub millisecond: Option<f64>,
    pub microsecond: Option<f64>,
    pub nanosecond: Option<f64>,
    pub offset: Option<String>,
    pub time_zone: Option<String>,
    pub calendar: Option<String>,
}

impl Fields {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn calendar_fields(&self) -> Result<CalendarFields> {
        let mut fields = CalendarFields::new();
        fields.year = self.year.map(integral_i32).transpose()?;
        fields.month = self.month.map(positive_u8).transpose()?;
        fields.day = self.day.map(positive_u8).transpose()?;
        fields.era_year = self.era_year.map(integral_i32).transpose()?;
        fields.month_code = match &self.month_code {
            Some(code) => Some(MonthCode::try_from_utf8(code.as_bytes())?),
            None => None,
        };
        fields.era = match &self.era {
            Some(era) => Some(
                temporal_rs::TinyAsciiStr::<19>::try_from_str(era)
                    .map_err(|_| Error::range("era is not a known era code"))?,
            ),
            None => None,
        };
        Ok(fields)
    }

    fn partial_time(&self) -> Result<PartialTime> {
        Ok(PartialTime {
            hour: self.hour.map(bounded_u8).transpose()?,
            minute: self.minute.map(bounded_u8).transpose()?,
            second: self.second.map(bounded_u8).transpose()?,
            millisecond: self.millisecond.map(bounded_u16).transpose()?,
            microsecond: self.microsecond.map(bounded_u16).transpose()?,
            nanosecond: self.nanosecond.map(bounded_u16).transpose()?,
        })
    }
}

/// The ten `Duration` components, each as the language delivers it: a number, not an integer.
///
/// Kept apart from [`Fields`] because they are apart in the language too -- a duration has weeks
/// and no month *code*, a date has a month code and no weeks -- and merging them would produce a
/// record where most of the fields are wrong for any given call.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DurationFields {
    pub years: Option<f64>,
    pub months: Option<f64>,
    pub weeks: Option<f64>,
    pub days: Option<f64>,
    pub hours: Option<f64>,
    pub minutes: Option<f64>,
    pub seconds: Option<f64>,
    pub milliseconds: Option<f64>,
    pub microseconds: Option<f64>,
    pub nanoseconds: Option<f64>,
}

impl DurationFields {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn to_partial(&self) -> Result<PartialDuration> {
        Ok(PartialDuration {
            years: self.years.map(integral_i64).transpose()?,
            months: self.months.map(integral_i64).transpose()?,
            weeks: self.weeks.map(integral_i64).transpose()?,
            days: self.days.map(integral_i64).transpose()?,
            hours: self.hours.map(integral_i64).transpose()?,
            minutes: self.minutes.map(integral_i64).transpose()?,
            seconds: self.seconds.map(integral_i64).transpose()?,
            milliseconds: self.milliseconds.map(integral_i64).transpose()?,
            microseconds: self.microseconds.map(integral_i128).transpose()?,
            nanoseconds: self.nanoseconds.map(integral_i128).transpose()?,
        })
    }
}

/// The options bag, already read out of the JavaScript object by the caller.
///
/// The caller must do the reading, not this crate: property reads are observable -- a getter runs,
/// and the *order* the properties are read in is specified and tested -- so a design that handed
/// an object over here would have moved observable behaviour behind a Seam that cannot express it.
/// What arrives is the strings the language spells the choices with; validating them is this
/// crate's job, because the set of legal values is the engine's business.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub overflow: Option<String>,
    pub largest_unit: Option<String>,
    pub smallest_unit: Option<String>,
    pub rounding_mode: Option<String>,
    pub rounding_increment: Option<f64>,
    /// `fractionalSecondDigits`: `None` for absent, `Some(None)` for `"auto"`, `Some(Some(n))`
    /// for a digit count.
    pub fractional_second_digits: Option<Option<u8>>,
    pub calendar_name: Option<String>,
    pub time_zone_name: Option<String>,
    pub offset: Option<String>,
    pub disambiguation: Option<String>,
    pub offset_conflict: Option<String>,
    pub unit: Option<String>,
    pub relative_to: Option<TemporalValue>,
}

impl Options {
    fn overflow(&self) -> Result<Option<Overflow>> {
        parse_option(self.overflow.as_deref(), "overflow")
    }

    fn difference_settings(&self) -> Result<DifferenceSettings> {
        let mut settings = DifferenceSettings::default();
        settings.largest_unit = parse_option(self.largest_unit.as_deref(), "largestUnit")?;
        settings.smallest_unit = parse_option(self.smallest_unit.as_deref(), "smallestUnit")?;
        settings.rounding_mode = parse_option(self.rounding_mode.as_deref(), "roundingMode")?;
        settings.increment = self.increment()?;
        Ok(settings)
    }

    fn rounding_options(&self) -> Result<RoundingOptions> {
        // The engine's default is `largestUnit: auto`, not all-absent, and the difference is
        // observable: `round({})` with no smallest unit has to reach the engine's own "you must
        // name a unit" error rather than a locally invented one.
        let mut rounding = RoundingOptions::default();
        if self.largest_unit.is_some() {
            rounding.largest_unit = parse_option(self.largest_unit.as_deref(), "largestUnit")?;
        }
        rounding.smallest_unit = parse_option(self.smallest_unit.as_deref(), "smallestUnit")?;
        rounding.rounding_mode = parse_option(self.rounding_mode.as_deref(), "roundingMode")?;
        rounding.increment = self.increment()?;
        Ok(rounding)
    }

    fn increment(&self) -> Result<Option<RoundingIncrement>> {
        let Some(raw) = self.rounding_increment else {
            return Ok(None);
        };
        if !raw.is_finite() {
            return Err(Error::range("roundingIncrement must be finite"));
        }
        let truncated = raw.trunc();
        if !(1.0..=1_000_000_000.0).contains(&truncated) {
            return Err(Error::range("roundingIncrement is out of range"));
        }
        Ok(Some(RoundingIncrement::try_new(truncated as u32)?))
    }

    fn to_string_rounding(&self) -> Result<ToStringRoundingOptions> {
        let precision = match self.fractional_second_digits {
            None | Some(None) => Precision::Auto,
            Some(Some(digits)) => Precision::Digit(digits),
        };
        Ok(ToStringRoundingOptions {
            precision,
            smallest_unit: parse_option(self.smallest_unit.as_deref(), "smallestUnit")?,
            rounding_mode: parse_option(self.rounding_mode.as_deref(), "roundingMode")?,
        })
    }

    fn display_calendar(&self) -> Result<DisplayCalendar> {
        Ok(parse_option(self.calendar_name.as_deref(), "calendarName")?
            .unwrap_or(DisplayCalendar::Auto))
    }

    fn display_offset(&self) -> Result<DisplayOffset> {
        Ok(parse_option(self.offset.as_deref(), "offset")?.unwrap_or(DisplayOffset::Auto))
    }

    fn display_time_zone(&self) -> Result<DisplayTimeZone> {
        Ok(
            parse_option(self.time_zone_name.as_deref(), "timeZoneName")?
                .unwrap_or(DisplayTimeZone::Auto),
        )
    }

    fn relative_to(&self) -> Result<Option<RelativeTo>> {
        match self.relative_to.as_ref().map(|value| &value.0) {
            None => Ok(None),
            Some(Repr::PlainDate(date)) => Ok(Some(RelativeTo::PlainDate(date.clone()))),
            Some(Repr::ZonedDateTime(zoned)) => Ok(Some(RelativeTo::ZonedDateTime(zoned.clone()))),
            Some(_) => Err(Error::type_error(
                "relativeTo must be a PlainDate or a ZonedDateTime",
            )),
        }
    }
}

fn parse_option<T: FromStr>(text: Option<&str>, name: &str) -> Result<Option<T>> {
    match text {
        None => Ok(None),
        Some(text) => T::from_str(text)
            .map(Some)
            .map_err(|_| Error::range(format!("{text:?} is not a valid value for {name}"))),
    }
}

// ==== Numeric narrowing ====
//
// Every `Temporal` entry point takes numbers and wants integers, and the specification is precise
// about the difference between the two ways it does that. `ToIntegerWithTruncation` throws only on
// a non-finite value and truncates the rest; `ToIntegerIfIntegral` throws on a fraction. Getting
// them the wrong way round is a silently wrong answer -- `new Temporal.PlainDate(2000, 1, 1.5)` is
// a date and `new Temporal.Duration(1.5)` is a RangeError -- so they are separate functions here.

fn truncated(value: f64, what: &str) -> Result<f64> {
    if !value.is_finite() {
        return Err(Error::range(format!("{what} must be a finite number")));
    }
    Ok(value.trunc())
}

fn integral(value: f64, what: &str) -> Result<f64> {
    if !value.is_finite() || value.fract() != 0.0 {
        return Err(Error::range(format!("{what} must be an integer")));
    }
    Ok(value)
}

fn integral_i32(value: f64) -> Result<i32> {
    let value = truncated(value, "a calendar field")?;
    if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&value) {
        return Err(Error::range("a calendar field is out of range"));
    }
    Ok(value as i32)
}

fn integral_i64(value: f64) -> Result<i64> {
    let value = integral(value, "a duration field")?;
    // 2^63 is beyond the double's exact-integer range, so the comparison is done against the
    // largest double that is still a `i64`, not against `i64::MAX` rounded up.
    if !(-9.223_372_036_854_775e18..=9.223_372_036_854_775e18).contains(&value) {
        return Err(Error::range("a duration field is out of range"));
    }
    Ok(value as i64)
}

fn integral_i128(value: f64) -> Result<i128> {
    let value = integral(value, "a duration field")?;
    if value.abs() > 1e30 {
        return Err(Error::range("a duration field is out of range"));
    }
    Ok(value as i128)
}

fn positive_u8(value: f64) -> Result<u8> {
    let value = truncated(value, "a calendar field")?;
    if !(1.0..=255.0).contains(&value) {
        return Err(Error::range("a calendar field is out of range"));
    }
    Ok(value as u8)
}

fn bounded_u8(value: f64) -> Result<u8> {
    let value = truncated(value, "a time field")?;
    if !(0.0..=255.0).contains(&value) {
        return Err(Error::range("a time field is out of range"));
    }
    Ok(value as u8)
}

fn bounded_u16(value: f64) -> Result<u16> {
    let value = truncated(value, "a time field")?;
    if !(0.0..=65535.0).contains(&value) {
        return Err(Error::range("a time field is out of range"));
    }
    Ok(value as u16)
}

fn calendar_from(id: Option<&str>) -> Result<Calendar> {
    match id {
        None => Ok(Calendar::ISO),
        Some(id) => Ok(Calendar::try_from_utf8(id.as_bytes())?),
    }
}

fn time_zone_from(id: &str) -> Result<TimeZone> {
    Ok(TimeZone::try_from_identifier_str(id)?)
}

/// The engine's representation, private so that no `temporal_rs` type is reachable from outside.
#[derive(Clone, Debug)]
enum Repr {
    PlainDate(PlainDate),
    PlainTime(PlainTime),
    PlainDateTime(PlainDateTime),
    PlainYearMonth(PlainYearMonth),
    PlainMonthDay(PlainMonthDay),
    Duration(Duration),
    Instant(Instant),
    ZonedDateTime(ZonedDateTime),
}

/// One `Temporal` value: opaque, immutable, and cheap to clone.
///
/// Immutable is not an implementation convenience, it is the language's rule -- every `Temporal`
/// type is a value type and every operation returns a new one -- which is why this needs no
/// interior mutability and no identity of its own. The caller supplies identity by wrapping it.
#[derive(Clone, Debug)]
pub struct TemporalValue(Repr);

impl PartialEq for TemporalValue {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Repr::PlainDate(a), Repr::PlainDate(b)) => a == b,
            (Repr::PlainTime(a), Repr::PlainTime(b)) => a == b,
            (Repr::PlainDateTime(a), Repr::PlainDateTime(b)) => a == b,
            (Repr::PlainYearMonth(a), Repr::PlainYearMonth(b)) => a == b,
            (Repr::PlainMonthDay(a), Repr::PlainMonthDay(b)) => a == b,
            (Repr::Duration(a), Repr::Duration(b)) => a == b,
            (Repr::Instant(a), Repr::Instant(b)) => a == b,
            // `temporal_rs` exposes the specification's equality operation rather than a Rust
            // `PartialEq` implementation because time-zone equality can consult provider data.
            // Both values here were created against the compiled provider, so an error means the
            // values cannot be established equal and is conservatively false.
            (Repr::ZonedDateTime(a), Repr::ZonedDateTime(b)) => a.equals(b).unwrap_or(false),
            _ => false,
        }
    }
}

impl TemporalValue {
    pub fn kind(&self) -> Kind {
        match &self.0 {
            Repr::PlainDate(_) => Kind::PlainDate,
            Repr::PlainTime(_) => Kind::PlainTime,
            Repr::PlainDateTime(_) => Kind::PlainDateTime,
            Repr::PlainYearMonth(_) => Kind::PlainYearMonth,
            Repr::PlainMonthDay(_) => Kind::PlainMonthDay,
            Repr::Duration(_) => Kind::Duration,
            Repr::Instant(_) => Kind::Instant,
            Repr::ZonedDateTime(_) => Kind::ZonedDateTime,
        }
    }

    /// The calendar identifier, for the five types that carry one.
    pub fn calendar_id(&self) -> Option<&'static str> {
        Some(match &self.0 {
            Repr::PlainDate(date) => date.calendar().identifier(),
            Repr::PlainDateTime(datetime) => datetime.calendar().identifier(),
            Repr::PlainYearMonth(year_month) => year_month.calendar().identifier(),
            Repr::PlainMonthDay(month_day) => month_day.calendar().identifier(),
            Repr::ZonedDateTime(zoned) => zoned.calendar().identifier(),
            _ => return None,
        })
    }

    /// The time zone identifier, for the one type that carries one.
    pub fn time_zone_id(&self) -> Option<String> {
        match &self.0 {
            Repr::ZonedDateTime(zoned) => zoned.time_zone().identifier().ok(),
            _ => None,
        }
    }
}

// ==== Construction ====

pub fn plain_date(
    year: f64,
    month: f64,
    day: f64,
    calendar: Option<&str>,
) -> Result<TemporalValue> {
    let calendar = calendar_from(calendar)?;
    let (year, month, day) = (
        integral_i32(year)?,
        truncated(month, "isoMonth")?,
        truncated(day, "isoDay")?,
    );
    // The narrowing is deliberately *not* a clamp: `new Temporal.PlainDate(2000, 300, 1)` is a
    // RangeError, so a month that does not fit a `u8` is the same error as a month of 300 and has
    // to be reported rather than wrapped.
    let month = in_u8(month)?;
    let day = in_u8(day)?;
    Ok(TemporalValue(Repr::PlainDate(PlainDate::try_new(
        year, month, day, calendar,
    )?)))
}

fn in_u8(value: f64) -> Result<u8> {
    if !(0.0..=255.0).contains(&value) {
        return Err(Error::range("a date component is out of range"));
    }
    Ok(value as u8)
}

#[allow(clippy::too_many_arguments)]
pub fn plain_time(
    hour: f64,
    minute: f64,
    second: f64,
    millisecond: f64,
    microsecond: f64,
    nanosecond: f64,
) -> Result<TemporalValue> {
    let time = PlainTime::try_new(
        in_u8(truncated(hour, "hour")?)?,
        in_u8(truncated(minute, "minute")?)?,
        in_u8(truncated(second, "second")?)?,
        in_u16(truncated(millisecond, "millisecond")?)?,
        in_u16(truncated(microsecond, "microsecond")?)?,
        in_u16(truncated(nanosecond, "nanosecond")?)?,
    )?;
    Ok(TemporalValue(Repr::PlainTime(time)))
}

fn in_u16(value: f64) -> Result<u16> {
    if !(0.0..=65535.0).contains(&value) {
        return Err(Error::range("a time component is out of range"));
    }
    Ok(value as u16)
}

#[allow(clippy::too_many_arguments)]
pub fn plain_date_time(
    year: f64,
    month: f64,
    day: f64,
    hour: f64,
    minute: f64,
    second: f64,
    millisecond: f64,
    microsecond: f64,
    nanosecond: f64,
    calendar: Option<&str>,
) -> Result<TemporalValue> {
    let calendar = calendar_from(calendar)?;
    let datetime = PlainDateTime::try_new(
        integral_i32(year)?,
        in_u8(truncated(month, "isoMonth")?)?,
        in_u8(truncated(day, "isoDay")?)?,
        in_u8(truncated(hour, "hour")?)?,
        in_u8(truncated(minute, "minute")?)?,
        in_u8(truncated(second, "second")?)?,
        in_u16(truncated(millisecond, "millisecond")?)?,
        in_u16(truncated(microsecond, "microsecond")?)?,
        in_u16(truncated(nanosecond, "nanosecond")?)?,
        calendar,
    )?;
    Ok(TemporalValue(Repr::PlainDateTime(datetime)))
}

pub fn plain_year_month(
    year: f64,
    month: f64,
    reference_day: Option<f64>,
    calendar: Option<&str>,
) -> Result<TemporalValue> {
    let calendar = calendar_from(calendar)?;
    let reference_day = match reference_day {
        Some(day) => Some(in_u8(truncated(day, "referenceISODay")?)?),
        None => None,
    };
    let year_month = PlainYearMonth::try_new(
        integral_i32(year)?,
        in_u8(truncated(month, "isoMonth")?)?,
        reference_day,
        calendar,
    )?;
    Ok(TemporalValue(Repr::PlainYearMonth(year_month)))
}

pub fn plain_month_day(
    month: f64,
    day: f64,
    reference_year: Option<f64>,
    calendar: Option<&str>,
) -> Result<TemporalValue> {
    let calendar = calendar_from(calendar)?;
    // `PlainMonthDay`'s reference year is 1972 -- the first ISO leap year after the epoch, so that
    // February 29th is representable, which is the whole reason the slot exists.
    let year = match reference_year {
        Some(year) => integral_i32(year)?,
        None => 1972,
    };
    let month_day = PlainMonthDay::new_with_overflow(
        in_u8(truncated(month, "isoMonth")?)?,
        in_u8(truncated(day, "isoDay")?)?,
        calendar,
        Overflow::Reject,
        Some(year),
    )?;
    Ok(TemporalValue(Repr::PlainMonthDay(month_day)))
}

pub fn duration(fields: &DurationFields) -> Result<TemporalValue> {
    let partial = fields.to_partial()?;
    let duration = Duration::new(
        partial.years.unwrap_or_default(),
        partial.months.unwrap_or_default(),
        partial.weeks.unwrap_or_default(),
        partial.days.unwrap_or_default(),
        partial.hours.unwrap_or_default(),
        partial.minutes.unwrap_or_default(),
        partial.seconds.unwrap_or_default(),
        partial.milliseconds.unwrap_or_default(),
        partial.microseconds.unwrap_or_default(),
        partial.nanoseconds.unwrap_or_default(),
    )?;
    Ok(TemporalValue(Repr::Duration(duration)))
}

/// `new Temporal.Instant(epochNanoseconds)`, where the argument is a `BigInt` and so arrives as an
/// `i128` rather than a number: the representable range is ±10^17 milliseconds, which is well past
/// what a double holds exactly once it is counted in nanoseconds.
pub fn instant(epoch_nanoseconds: i128) -> Result<TemporalValue> {
    Ok(TemporalValue(Repr::Instant(Instant::try_new(
        epoch_nanoseconds,
    )?)))
}

pub fn instant_from_epoch_milliseconds(epoch_milliseconds: f64) -> Result<TemporalValue> {
    let milliseconds = integral(epoch_milliseconds, "epochMilliseconds")?;
    if milliseconds.abs() > 9e18 {
        return Err(Error::range("epochMilliseconds is out of range"));
    }
    Ok(TemporalValue(Repr::Instant(
        Instant::from_epoch_milliseconds(milliseconds as i64)?,
    )))
}

pub fn zoned_date_time(
    epoch_nanoseconds: i128,
    time_zone: &str,
    calendar: Option<&str>,
) -> Result<TemporalValue> {
    let zoned = ZonedDateTime::try_new(
        epoch_nanoseconds,
        time_zone_from(time_zone)?,
        calendar_from(calendar)?,
    )?;
    Ok(TemporalValue(Repr::ZonedDateTime(zoned)))
}

// ==== Parsing ====

/// `Temporal.X.from(aString)`: the ISO 8601 / RFC 9557 grammar, and nothing else.
pub fn parse(kind: Kind, text: &str) -> Result<TemporalValue> {
    let bytes = text.as_bytes();
    Ok(TemporalValue(match kind {
        Kind::PlainDate => Repr::PlainDate(PlainDate::from_utf8(bytes)?),
        Kind::PlainTime => Repr::PlainTime(PlainTime::from_utf8(bytes)?),
        Kind::PlainDateTime => Repr::PlainDateTime(PlainDateTime::from_utf8(bytes)?),
        Kind::PlainYearMonth => Repr::PlainYearMonth(PlainYearMonth::from_utf8(bytes)?),
        Kind::PlainMonthDay => Repr::PlainMonthDay(PlainMonthDay::from_utf8(bytes)?),
        Kind::Duration => Repr::Duration(Duration::from_utf8(bytes)?),
        Kind::Instant => Repr::Instant(Instant::from_utf8(bytes)?),
        Kind::ZonedDateTime => {
            return Err(Error::range(
                "a ZonedDateTime string needs a disambiguation policy; use parse_zoned_date_time",
            ));
        }
    }))
}

/// `Temporal.ZonedDateTime.from(aString, options)`, which unlike every other `from` has to decide
/// what a wall-clock time that does not exist, or exists twice, means.
pub fn parse_zoned_date_time(text: &str, options: &Options) -> Result<TemporalValue> {
    let disambiguation =
        parse_option::<Disambiguation>(options.disambiguation.as_deref(), "disambiguation")?
            .unwrap_or(Disambiguation::Compatible);
    let offset_conflict =
        parse_option::<OffsetDisambiguation>(options.offset_conflict.as_deref(), "offset")?
            .unwrap_or(OffsetDisambiguation::Reject);
    Ok(TemporalValue(Repr::ZonedDateTime(
        ZonedDateTime::from_utf8(text.as_bytes(), disambiguation, offset_conflict)?,
    )))
}

/// `Temporal.X.from(aPropertyBag, options)`.
pub fn from_fields(kind: Kind, fields: &Fields, options: &Options) -> Result<TemporalValue> {
    if fields.is_empty() {
        return Err(Error::type_error(
            "no recognised Temporal fields were present",
        ));
    }
    let overflow = options.overflow()?;
    let calendar = calendar_from(fields.calendar.as_deref())?;
    Ok(TemporalValue(match kind {
        Kind::PlainDate => {
            let partial = PartialDate {
                calendar_fields: fields.calendar_fields()?,
                calendar,
            };
            Repr::PlainDate(PlainDate::from_partial(partial, overflow)?)
        }
        Kind::PlainTime => {
            Repr::PlainTime(PlainTime::from_partial(fields.partial_time()?, overflow)?)
        }
        Kind::PlainDateTime => {
            let partial = temporal_rs::partial::PartialDateTime {
                fields: DateTimeFields {
                    calendar_fields: fields.calendar_fields()?,
                    time: fields.partial_time()?,
                },
                calendar,
            };
            Repr::PlainDateTime(PlainDateTime::from_partial(partial, overflow)?)
        }
        Kind::PlainYearMonth => {
            let partial = PartialYearMonth {
                calendar_fields: year_month_fields(fields)?,
                calendar,
            };
            Repr::PlainYearMonth(PlainYearMonth::from_partial(partial, overflow)?)
        }
        Kind::PlainMonthDay => {
            let partial = PartialDate {
                calendar_fields: fields.calendar_fields()?,
                calendar,
            };
            Repr::PlainMonthDay(PlainMonthDay::from_partial(partial, overflow)?)
        }
        Kind::ZonedDateTime => {
            let Some(time_zone) = fields.time_zone.as_deref() else {
                return Err(Error::type_error(
                    "a ZonedDateTime property bag needs a timeZone",
                ));
            };
            let partial = temporal_rs::partial::PartialZonedDateTime {
                fields: temporal_rs::fields::ZonedDateTimeFields {
                    calendar_fields: fields.calendar_fields()?,
                    time: fields.partial_time()?,
                    offset: match fields.offset.as_deref() {
                        Some(offset) => Some(temporal_rs::UtcOffset::from_utf8(offset.as_bytes())?),
                        None => None,
                    },
                },
                timezone: Some(time_zone_from(time_zone)?),
                calendar,
            };
            Repr::ZonedDateTime(ZonedDateTime::from_partial(
                partial,
                overflow,
                parse_option::<Disambiguation>(
                    options.disambiguation.as_deref(),
                    "disambiguation",
                )?,
                parse_option::<OffsetDisambiguation>(options.offset_conflict.as_deref(), "offset")?,
            )?)
        }
        Kind::Duration | Kind::Instant => {
            return Err(Error::type_error(
                "this Temporal type is not built from calendar fields",
            ));
        }
    }))
}

/// `Temporal.Duration.from(aPropertyBag)`, which takes no options and rejects a fractional field.
pub fn duration_from_fields(fields: &DurationFields) -> Result<TemporalValue> {
    if fields.is_empty() {
        return Err(Error::type_error(
            "no recognised Duration fields were present",
        ));
    }
    Ok(TemporalValue(Repr::Duration(
        Duration::from_partial_duration(fields.to_partial()?)?,
    )))
}

// ==== Field access ====

impl TemporalValue {
    /// One property, by the name the language spells it with.
    ///
    /// `None` means the receiver has no such property at all, which is the caller's cue to fall
    /// through to the prototype chain. `Some(Field::Undefined)` means it *has* the property and
    /// the answer is `undefined` -- `weekOfYear` in a calendar with no week numbering -- and the
    /// two are not interchangeable.
    pub fn get(&self, name: &str) -> Option<Field> {
        match &self.0 {
            Repr::PlainDate(date) => date_field(date, name),
            Repr::PlainTime(time) => time_field(time, name),
            Repr::PlainDateTime(datetime) => datetime_field(datetime, name),
            Repr::PlainYearMonth(year_month) => year_month_field(year_month, name),
            Repr::PlainMonthDay(month_day) => month_day_field(month_day, name),
            Repr::Duration(duration) => duration_field(duration, name),
            Repr::Instant(instant) => instant_field(instant, name),
            Repr::ZonedDateTime(zoned) => zoned_field(zoned, name),
        }
    }
}

fn optional_text(value: Option<impl AsRef<str>>) -> Field {
    match value {
        Some(text) => Field::Text(text.as_ref().to_owned()),
        None => Field::Undefined,
    }
}

fn optional_number(value: Option<impl Into<f64>>) -> Field {
    match value {
        Some(number) => Field::Number(number.into()),
        None => Field::Undefined,
    }
}

fn date_field(date: &PlainDate, name: &str) -> Option<Field> {
    Some(match name {
        "calendarId" => Field::Text(date.calendar().identifier().to_owned()),
        "era" => optional_text(date.era().map(|era| era.as_str().to_owned())),
        "eraYear" => optional_number(date.era_year()),
        "year" => Field::Number(f64::from(date.year())),
        "month" => Field::Number(f64::from(date.month())),
        "monthCode" => Field::Text(date.month_code().as_str().to_owned()),
        "day" => Field::Number(f64::from(date.day())),
        "dayOfWeek" => Field::Number(f64::from(date.day_of_week())),
        "dayOfYear" => Field::Number(f64::from(date.day_of_year())),
        "weekOfYear" => optional_number(date.week_of_year()),
        "yearOfWeek" => optional_number(date.year_of_week()),
        "daysInWeek" => Field::Number(f64::from(date.days_in_week())),
        "daysInMonth" => Field::Number(f64::from(date.days_in_month())),
        "daysInYear" => Field::Number(f64::from(date.days_in_year())),
        "monthsInYear" => Field::Number(f64::from(date.months_in_year())),
        "inLeapYear" => Field::Flag(date.in_leap_year()),
        _ => return None,
    })
}

fn time_field(time: &PlainTime, name: &str) -> Option<Field> {
    Some(match name {
        "hour" => Field::Number(f64::from(time.hour())),
        "minute" => Field::Number(f64::from(time.minute())),
        "second" => Field::Number(f64::from(time.second())),
        "millisecond" => Field::Number(f64::from(time.millisecond())),
        "microsecond" => Field::Number(f64::from(time.microsecond())),
        "nanosecond" => Field::Number(f64::from(time.nanosecond())),
        _ => return None,
    })
}

fn datetime_field(datetime: &PlainDateTime, name: &str) -> Option<Field> {
    Some(match name {
        "calendarId" => Field::Text(datetime.calendar().identifier().to_owned()),
        "era" => optional_text(datetime.era().map(|era| era.as_str().to_owned())),
        "eraYear" => optional_number(datetime.era_year()),
        "year" => Field::Number(f64::from(datetime.year())),
        "month" => Field::Number(f64::from(datetime.month())),
        "monthCode" => Field::Text(datetime.month_code().as_str().to_owned()),
        "day" => Field::Number(f64::from(datetime.day())),
        "hour" => Field::Number(f64::from(datetime.hour())),
        "minute" => Field::Number(f64::from(datetime.minute())),
        "second" => Field::Number(f64::from(datetime.second())),
        "millisecond" => Field::Number(f64::from(datetime.millisecond())),
        "microsecond" => Field::Number(f64::from(datetime.microsecond())),
        "nanosecond" => Field::Number(f64::from(datetime.nanosecond())),
        "dayOfWeek" => Field::Number(f64::from(datetime.day_of_week())),
        "dayOfYear" => Field::Number(f64::from(datetime.day_of_year())),
        "weekOfYear" => optional_number(datetime.week_of_year()),
        "yearOfWeek" => optional_number(datetime.year_of_week()),
        "daysInWeek" => Field::Number(f64::from(datetime.days_in_week())),
        "daysInMonth" => Field::Number(f64::from(datetime.days_in_month())),
        "daysInYear" => Field::Number(f64::from(datetime.days_in_year())),
        "monthsInYear" => Field::Number(f64::from(datetime.months_in_year())),
        "inLeapYear" => Field::Flag(datetime.in_leap_year()),
        _ => return None,
    })
}

fn year_month_field(year_month: &PlainYearMonth, name: &str) -> Option<Field> {
    Some(match name {
        "calendarId" => Field::Text(year_month.calendar().identifier().to_owned()),
        "era" => optional_text(year_month.era().map(|era| era.as_str().to_owned())),
        "eraYear" => optional_number(year_month.era_year()),
        "year" => Field::Number(f64::from(year_month.year())),
        "month" => Field::Number(f64::from(year_month.month())),
        "monthCode" => Field::Text(year_month.month_code().as_str().to_owned()),
        "daysInMonth" => Field::Number(f64::from(year_month.days_in_month())),
        "daysInYear" => Field::Number(f64::from(year_month.days_in_year())),
        "monthsInYear" => Field::Number(f64::from(year_month.months_in_year())),
        "inLeapYear" => Field::Flag(year_month.in_leap_year()),
        _ => return None,
    })
}

fn month_day_field(month_day: &PlainMonthDay, name: &str) -> Option<Field> {
    Some(match name {
        "calendarId" => Field::Text(month_day.calendar().identifier().to_owned()),
        "monthCode" => Field::Text(month_day.month_code().as_str().to_owned()),
        "day" => Field::Number(f64::from(month_day.day())),
        _ => return None,
    })
}

fn duration_field(duration: &Duration, name: &str) -> Option<Field> {
    // Every component comes back as a *number*, even the two the engine keeps in 128 bits: the
    // language's `microseconds` and `nanoseconds` getters are numbers, and a duration that would
    // lose precision here could not have been constructed in the first place.
    Some(match name {
        "years" => Field::Number(duration.years() as f64),
        "months" => Field::Number(duration.months() as f64),
        "weeks" => Field::Number(duration.weeks() as f64),
        "days" => Field::Number(duration.days() as f64),
        "hours" => Field::Number(duration.hours() as f64),
        "minutes" => Field::Number(duration.minutes() as f64),
        "seconds" => Field::Number(duration.seconds() as f64),
        "milliseconds" => Field::Number(duration.milliseconds() as f64),
        "microseconds" => Field::Number(duration.microseconds() as f64),
        "nanoseconds" => Field::Number(duration.nanoseconds() as f64),
        "sign" => Field::Number(f64::from(duration.sign() as i8)),
        "blank" => Field::Flag(duration.is_zero()),
        _ => return None,
    })
}

/// The instant a value is formatted at, in epoch milliseconds, for `Intl.DateTimeFormat`.
///
/// ECMA-402 formats a wall-clock value by placing it on the timeline at UTC -- a date at noon, a
/// time on 1970-01-01, a month-day in its reference year -- and then formatting that instant with
/// the time zone suppressed, which is what makes `format` ignore the formatter's own zone for a
/// plain value. The placement is exact specification text (`HandleDateTimeTemporalDate` and its
/// siblings) and the engine already implements it, so it is read from there rather than
/// reconstructed from the calendar fields, which would go wrong for a non-ISO calendar.
///
/// `None` for a `ZonedDateTime`, which ECMA-402 refuses outright.
pub fn format_instant(value: &TemporalValue) -> Option<f64> {
    let nanoseconds: i128 = match &value.0 {
        Repr::PlainDate(date) => date.epoch_ns_for_utc().as_i128(),
        Repr::PlainDateTime(date_time) => date_time.epoch_ns_for_utc().as_i128(),
        Repr::PlainYearMonth(year_month) => year_month.epoch_ns_for_utc().as_i128(),
        Repr::PlainMonthDay(month_day) => month_day.epoch_ns_for_utc().as_i128(),
        Repr::PlainTime(time) => time.epoch_ns_for_utc().as_i128(),
        Repr::Instant(instant) => instant.as_i128(),
        Repr::Duration(_) | Repr::ZonedDateTime(_) => return None,
    };
    // Floor rather than truncate: a millisecond before the epoch is still the millisecond the
    // nanosecond falls inside, and rounding towards zero would move it forwards.
    Some(nanoseconds.div_euclid(1_000_000) as f64)
}

/// The ten components of a `Temporal.Duration`, in the order ECMA-402's duration table lists them.
///
/// Separate from [`TemporalValue::get`] because that answers in `f64`, which is what the
/// language's own getters return, and two of the components are held in 128 bits. A formatter that
/// has to render `microseconds` exactly cannot go through a double to get them.
pub fn duration_components(value: &TemporalValue) -> Option<[i128; 10]> {
    let Repr::Duration(duration) = &value.0 else {
        return None;
    };
    Some([
        i128::from(duration.years()),
        i128::from(duration.months()),
        i128::from(duration.weeks()),
        i128::from(duration.days()),
        i128::from(duration.hours()),
        i128::from(duration.minutes()),
        i128::from(duration.seconds()),
        i128::from(duration.milliseconds()),
        duration.microseconds(),
        duration.nanoseconds(),
    ])
}

fn instant_field(instant: &Instant, name: &str) -> Option<Field> {
    Some(match name {
        "epochMilliseconds" => Field::Number(instant.epoch_milliseconds() as f64),
        "epochNanoseconds" => Field::Integer128(instant.as_i128()),
        _ => return None,
    })
}

fn zoned_field(zoned: &ZonedDateTime, name: &str) -> Option<Field> {
    Some(match name {
        "calendarId" => Field::Text(zoned.calendar().identifier().to_owned()),
        "timeZoneId" => match zoned.time_zone().identifier() {
            Ok(identifier) => Field::Text(identifier),
            Err(_) => Field::Undefined,
        },
        "era" => optional_text(zoned.era().map(|era| era.as_str().to_owned())),
        "eraYear" => optional_number(zoned.era_year()),
        "year" => Field::Number(f64::from(zoned.year())),
        "month" => Field::Number(f64::from(zoned.month())),
        "monthCode" => Field::Text(zoned.month_code().as_str().to_owned()),
        "day" => Field::Number(f64::from(zoned.day())),
        "hour" => Field::Number(f64::from(zoned.hour())),
        "minute" => Field::Number(f64::from(zoned.minute())),
        "second" => Field::Number(f64::from(zoned.second())),
        "millisecond" => Field::Number(f64::from(zoned.millisecond())),
        "microsecond" => Field::Number(f64::from(zoned.microsecond())),
        "nanosecond" => Field::Number(f64::from(zoned.nanosecond())),
        "epochMilliseconds" => Field::Number(zoned.epoch_milliseconds() as f64),
        "epochNanoseconds" => Field::Integer128(zoned.epoch_nanoseconds().as_i128()),
        "offset" => Field::Text(zoned.offset()),
        "offsetNanoseconds" => Field::Number(zoned.offset_nanoseconds() as f64),
        "hoursInDay" => match zoned.hours_in_day() {
            Ok(hours) => Field::Number(hours),
            Err(_) => Field::Undefined,
        },
        "dayOfWeek" => Field::Number(f64::from(zoned.day_of_week())),
        "dayOfYear" => Field::Number(f64::from(zoned.day_of_year())),
        "weekOfYear" => optional_number(zoned.week_of_year()),
        "yearOfWeek" => optional_number(zoned.year_of_week()),
        "daysInWeek" => Field::Number(f64::from(zoned.days_in_week())),
        "daysInMonth" => Field::Number(f64::from(zoned.days_in_month())),
        "daysInYear" => Field::Number(f64::from(zoned.days_in_year())),
        "monthsInYear" => Field::Number(f64::from(zoned.months_in_year())),
        "inLeapYear" => Field::Flag(zoned.in_leap_year()),
        _ => return None,
    })
}

// ==== Operations ====

impl TemporalValue {
    /// `receiver.with(fields, options)`: the receiver's fields, overridden by the ones supplied.
    pub fn with(&self, fields: &Fields, options: &Options) -> Result<Self> {
        if fields.is_empty() {
            return Err(Error::type_error(
                "no recognised Temporal fields were present",
            ));
        }
        let overflow = options.overflow()?;
        Ok(TemporalValue(match &self.0 {
            Repr::PlainDate(date) => {
                Repr::PlainDate(date.with(fields.calendar_fields()?, overflow)?)
            }
            Repr::PlainTime(time) => Repr::PlainTime(time.with(fields.partial_time()?, overflow)?),
            Repr::PlainDateTime(datetime) => Repr::PlainDateTime(datetime.with(
                DateTimeFields {
                    calendar_fields: fields.calendar_fields()?,
                    time: fields.partial_time()?,
                },
                overflow,
            )?),
            Repr::PlainYearMonth(year_month) => {
                Repr::PlainYearMonth(year_month.with(year_month_fields(fields)?, overflow)?)
            }
            Repr::PlainMonthDay(month_day) => {
                Repr::PlainMonthDay(month_day.with(fields.calendar_fields()?, overflow)?)
            }
            Repr::ZonedDateTime(zoned) => {
                let disambiguation =
                    parse_option(options.disambiguation.as_deref(), "disambiguation")?;
                let offset_conflict = parse_option(options.offset_conflict.as_deref(), "offset")?;
                let zoned_fields = temporal_rs::fields::ZonedDateTimeFields {
                    calendar_fields: fields.calendar_fields()?,
                    time: fields.partial_time()?,
                    offset: match fields.offset.as_deref() {
                        Some(offset) => Some(temporal_rs::UtcOffset::from_utf8(offset.as_bytes())?),
                        None => None,
                    },
                };
                Repr::ZonedDateTime(zoned.with(
                    zoned_fields,
                    disambiguation,
                    offset_conflict,
                    overflow,
                )?)
            }
            Repr::Duration(_) | Repr::Instant(_) => {
                return Err(Error::type_error("this Temporal type has no `with`"));
            }
        }))
    }

    /// `duration.with(fields)`, which is the odd one out: no options, and a fractional field is a
    /// RangeError rather than something to truncate.
    pub fn duration_with(&self, fields: &DurationFields) -> Result<Self> {
        let Repr::Duration(duration) = &self.0 else {
            return Err(Error::type_error("this is not a Temporal.Duration"));
        };
        if fields.is_empty() {
            return Err(Error::type_error(
                "no recognised Duration fields were present",
            ));
        }
        let supplied = fields.to_partial()?;
        let merged = PartialDuration {
            years: Some(supplied.years.unwrap_or_else(|| duration.years())),
            months: Some(supplied.months.unwrap_or_else(|| duration.months())),
            weeks: Some(supplied.weeks.unwrap_or_else(|| duration.weeks())),
            days: Some(supplied.days.unwrap_or_else(|| duration.days())),
            hours: Some(supplied.hours.unwrap_or_else(|| duration.hours())),
            minutes: Some(supplied.minutes.unwrap_or_else(|| duration.minutes())),
            seconds: Some(supplied.seconds.unwrap_or_else(|| duration.seconds())),
            milliseconds: Some(
                supplied
                    .milliseconds
                    .unwrap_or_else(|| duration.milliseconds()),
            ),
            microseconds: Some(
                supplied
                    .microseconds
                    .unwrap_or_else(|| duration.microseconds()),
            ),
            nanoseconds: Some(
                supplied
                    .nanoseconds
                    .unwrap_or_else(|| duration.nanoseconds()),
            ),
        };
        Ok(TemporalValue(Repr::Duration(
            Duration::from_partial_duration(merged)?,
        )))
    }

    pub fn add(&self, other: &Self, options: &Options) -> Result<Self> {
        self.shift(other, options, false)
    }

    pub fn subtract(&self, other: &Self, options: &Options) -> Result<Self> {
        self.shift(other, options, true)
    }

    fn shift(&self, other: &Self, options: &Options, negate: bool) -> Result<Self> {
        // Everything except `Duration` adds a duration to itself; `Duration` adds another
        // duration. Reaching the wrong one is a TypeError, not a silently different answer.
        if let Repr::Duration(receiver) = &self.0 {
            let Repr::Duration(operand) = &other.0 else {
                return Err(Error::type_error("expected a Temporal.Duration"));
            };
            let sum = if negate {
                receiver.subtract(operand)?
            } else {
                receiver.add(operand)?
            };
            return Ok(TemporalValue(Repr::Duration(sum)));
        }
        let Repr::Duration(operand) = &other.0 else {
            return Err(Error::type_error("expected a Temporal.Duration"));
        };
        let operand = if negate { operand.negated() } else { *operand };
        let overflow = options.overflow()?;
        Ok(TemporalValue(match &self.0 {
            Repr::PlainDate(date) => Repr::PlainDate(date.add(&operand, overflow)?),
            Repr::PlainTime(time) => Repr::PlainTime(time.add(&operand)?),
            Repr::PlainDateTime(datetime) => Repr::PlainDateTime(datetime.add(&operand, overflow)?),
            Repr::PlainYearMonth(year_month) => Repr::PlainYearMonth(
                year_month.add(&operand, overflow.unwrap_or(Overflow::Constrain))?,
            ),
            Repr::Instant(instant) => Repr::Instant(instant.add(&operand)?),
            Repr::ZonedDateTime(zoned) => Repr::ZonedDateTime(zoned.add(&operand, overflow)?),
            Repr::PlainMonthDay(_) => {
                return Err(Error::type_error("a PlainMonthDay has no arithmetic"));
            }
            Repr::Duration(_) => unreachable!("handled above"),
        }))
    }

    pub fn until(&self, other: &Self, options: &Options) -> Result<Self> {
        self.difference(other, options, false)
    }

    pub fn since(&self, other: &Self, options: &Options) -> Result<Self> {
        self.difference(other, options, true)
    }

    fn difference(&self, other: &Self, options: &Options, since: bool) -> Result<Self> {
        let settings = options.difference_settings()?;
        let duration = match (&self.0, &other.0) {
            (Repr::PlainDate(a), Repr::PlainDate(b)) => {
                if since {
                    a.since(b, settings)?
                } else {
                    a.until(b, settings)?
                }
            }
            (Repr::PlainTime(a), Repr::PlainTime(b)) => {
                if since {
                    a.since(b, settings)?
                } else {
                    a.until(b, settings)?
                }
            }
            (Repr::PlainDateTime(a), Repr::PlainDateTime(b)) => {
                if since {
                    a.since(b, settings)?
                } else {
                    a.until(b, settings)?
                }
            }
            (Repr::PlainYearMonth(a), Repr::PlainYearMonth(b)) => {
                if since {
                    a.since(b, settings)?
                } else {
                    a.until(b, settings)?
                }
            }
            (Repr::Instant(a), Repr::Instant(b)) => {
                if since {
                    a.since(b, settings)?
                } else {
                    a.until(b, settings)?
                }
            }
            (Repr::ZonedDateTime(a), Repr::ZonedDateTime(b)) => {
                if since {
                    a.since(b, settings)?
                } else {
                    a.until(b, settings)?
                }
            }
            _ => {
                return Err(Error::type_error(
                    "until and since need two values of the same Temporal type",
                ));
            }
        };
        Ok(TemporalValue(Repr::Duration(duration)))
    }

    pub fn round(&self, options: &Options) -> Result<Self> {
        let rounding = options.rounding_options()?;
        Ok(TemporalValue(match &self.0 {
            Repr::PlainTime(time) => Repr::PlainTime(time.round(rounding)?),
            Repr::PlainDateTime(datetime) => Repr::PlainDateTime(datetime.round(rounding)?),
            Repr::Instant(instant) => Repr::Instant(instant.round(rounding)?),
            Repr::ZonedDateTime(zoned) => Repr::ZonedDateTime(zoned.round(rounding)?),
            Repr::Duration(duration) => {
                Repr::Duration(duration.round(rounding, options.relative_to()?)?)
            }
            _ => return Err(Error::type_error("this Temporal type has no `round`")),
        }))
    }

    /// `duration.total(options)`, the one operation that answers with a bare number.
    pub fn total(&self, options: &Options) -> Result<f64> {
        let Repr::Duration(duration) = &self.0 else {
            return Err(Error::type_error("this is not a Temporal.Duration"));
        };
        let Some(unit) = parse_option::<Unit>(options.unit.as_deref(), "unit")? else {
            return Err(Error::range("total needs a unit"));
        };
        Ok(duration.total(unit, options.relative_to()?)?.as_inner())
    }

    pub fn negated(&self) -> Result<Self> {
        let Repr::Duration(duration) = &self.0 else {
            return Err(Error::type_error("this is not a Temporal.Duration"));
        };
        Ok(TemporalValue(Repr::Duration(duration.negated())))
    }

    pub fn abs(&self) -> Result<Self> {
        let Repr::Duration(duration) = &self.0 else {
            return Err(Error::type_error("this is not a Temporal.Duration"));
        };
        Ok(TemporalValue(Repr::Duration(duration.abs())))
    }

    /// `a.equals(b)`, which is *not* `compare(a, b) === 0`: two dates on the same day in different
    /// calendars compare equal and are not `equals`.
    pub fn equals(&self, other: &Self) -> Result<bool> {
        Ok(match (&self.0, &other.0) {
            (Repr::PlainDate(a), Repr::PlainDate(b)) => a == b,
            (Repr::PlainTime(a), Repr::PlainTime(b)) => a == b,
            (Repr::PlainDateTime(a), Repr::PlainDateTime(b)) => a == b,
            (Repr::PlainYearMonth(a), Repr::PlainYearMonth(b)) => a == b,
            (Repr::PlainMonthDay(a), Repr::PlainMonthDay(b)) => a == b,
            (Repr::Instant(a), Repr::Instant(b)) => a == b,
            (Repr::ZonedDateTime(a), Repr::ZonedDateTime(b)) => a.equals(b)?,
            (Repr::Duration(_), Repr::Duration(_)) => {
                return Err(Error::type_error("a Temporal.Duration has no `equals`"));
            }
            _ => {
                return Err(Error::type_error(
                    "equals needs two values of the same Temporal type",
                ));
            }
        })
    }

    pub fn to_temporal_string(&self, options: &Options) -> Result<String> {
        Ok(match &self.0 {
            Repr::PlainDate(date) => date.to_ixdtf_string(options.display_calendar()?),
            Repr::PlainTime(time) => time.to_ixdtf_string(options.to_string_rounding()?)?,
            Repr::PlainDateTime(datetime) => datetime
                .to_ixdtf_string(options.to_string_rounding()?, options.display_calendar()?)?,
            Repr::PlainYearMonth(year_month) => {
                year_month.to_ixdtf_string(options.display_calendar()?)
            }
            Repr::PlainMonthDay(month_day) => {
                month_day.to_ixdtf_string(options.display_calendar()?)
            }
            Repr::Duration(duration) => {
                duration.as_temporal_string(options.to_string_rounding()?)?
            }
            Repr::Instant(instant) => {
                instant.to_ixdtf_string(None, options.to_string_rounding()?)?
            }
            Repr::ZonedDateTime(zoned) => zoned.to_ixdtf_string(
                options.display_offset()?,
                options.display_time_zone()?,
                options.display_calendar()?,
                options.to_string_rounding()?,
            )?,
        })
    }
}

fn year_month_fields(fields: &Fields) -> Result<temporal_rs::fields::YearMonthCalendarFields> {
    let calendar_fields = fields.calendar_fields()?;
    Ok(temporal_rs::fields::YearMonthCalendarFields {
        year: calendar_fields.year,
        month: calendar_fields.month,
        month_code: calendar_fields.month_code,
        era: calendar_fields.era,
        era_year: calendar_fields.era_year,
    })
}

/// `Temporal.X.compare(a, b)`: -1, 0 or 1.
///
/// Ordering, not equality: it ignores the calendar, because two dates naming the same day are the
/// same point on the timeline whatever calendar each was written in.
pub fn compare(a: &TemporalValue, b: &TemporalValue) -> Result<i32> {
    let ordering = match (&a.0, &b.0) {
        (Repr::PlainDate(a), Repr::PlainDate(b)) => a.compare_iso(b),
        (Repr::PlainTime(a), Repr::PlainTime(b)) => a.cmp(b),
        (Repr::PlainDateTime(a), Repr::PlainDateTime(b)) => a.compare_iso(b),
        (Repr::PlainYearMonth(a), Repr::PlainYearMonth(b)) => a.compare_iso(b),
        (Repr::Instant(a), Repr::Instant(b)) => a.cmp(b),
        (Repr::ZonedDateTime(a), Repr::ZonedDateTime(b)) => a.compare_instant(b),
        _ => {
            return Err(Error::type_error(
                "compare needs two values of the same Temporal type",
            ));
        }
    };
    Ok(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}

/// `Temporal.Duration.compare(a, b, options)`, which needs a `relativeTo` as soon as either
/// duration carries a calendar unit: how long a month is depends on which month.
pub fn compare_durations(a: &TemporalValue, b: &TemporalValue, options: &Options) -> Result<i32> {
    let (Repr::Duration(a), Repr::Duration(b)) = (&a.0, &b.0) else {
        return Err(Error::type_error(
            "compare needs two Temporal.Duration values",
        ));
    };
    let ordering = Duration::compare(a, b, options.relative_to()?)?;
    Ok(match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}

// ==== Conversions ====

impl TemporalValue {
    /// `receiver.toPlainDate(...)` and its siblings, for the pairs the language defines.
    pub fn to_kind(&self, target: Kind, supplement: Option<&Fields>) -> Result<Self> {
        Ok(TemporalValue(match (&self.0, target) {
            (Repr::PlainDate(date), Kind::PlainDateTime) => {
                let time = match supplement.map(|fields| fields.partial_time()).transpose()? {
                    Some(partial) if partial != PartialTime::default() => {
                        Some(PlainTime::from_partial(partial, None)?)
                    }
                    _ => None,
                };
                Repr::PlainDateTime(date.to_plain_date_time(time)?)
            }
            (Repr::PlainDate(date), Kind::PlainYearMonth) => {
                Repr::PlainYearMonth(date.to_plain_year_month()?)
            }
            (Repr::PlainDate(date), Kind::PlainMonthDay) => {
                Repr::PlainMonthDay(date.to_plain_month_day()?)
            }
            (Repr::PlainDateTime(datetime), Kind::PlainDate) => {
                Repr::PlainDate(datetime.to_plain_date())
            }
            (Repr::PlainDateTime(datetime), Kind::PlainTime) => {
                Repr::PlainTime(datetime.to_plain_time())
            }
            (Repr::ZonedDateTime(zoned), Kind::PlainDate) => Repr::PlainDate(zoned.to_plain_date()),
            (Repr::ZonedDateTime(zoned), Kind::PlainTime) => Repr::PlainTime(zoned.to_plain_time()),
            (Repr::ZonedDateTime(zoned), Kind::PlainDateTime) => {
                Repr::PlainDateTime(zoned.to_plain_date_time())
            }
            (Repr::ZonedDateTime(zoned), Kind::Instant) => Repr::Instant(zoned.to_instant()),
            _ => {
                return Err(Error::type_error(
                    "this Temporal type does not convert to that one",
                ));
            }
        }))
    }

    /// `plainDate.toZonedDateTime(...)`, `instant.toZonedDateTimeISO(...)` and
    /// `plainDateTime.toZonedDateTime(...)`: the three ways a wall-clock value acquires a zone.
    pub fn to_zoned_date_time(
        &self,
        time_zone: &str,
        time: Option<&Self>,
        options: &Options,
    ) -> Result<Self> {
        let zone = time_zone_from(time_zone)?;
        let disambiguation =
            parse_option::<Disambiguation>(options.disambiguation.as_deref(), "disambiguation")?
                .unwrap_or(Disambiguation::Compatible);
        Ok(TemporalValue(Repr::ZonedDateTime(match &self.0 {
            Repr::PlainDate(date) => {
                let time = match time.map(|value| &value.0) {
                    None => None,
                    Some(Repr::PlainTime(time)) => Some(*time),
                    Some(_) => return Err(Error::type_error("expected a Temporal.PlainTime")),
                };
                date.to_zoned_date_time(zone, time)?
            }
            Repr::PlainDateTime(datetime) => datetime.to_zoned_date_time(zone, disambiguation)?,
            Repr::Instant(instant) => instant.to_zoned_date_time_iso(zone)?,
            _ => {
                return Err(Error::type_error(
                    "this Temporal type does not convert to a ZonedDateTime",
                ));
            }
        })))
    }

    /// `plainYearMonth.toPlainDate({ day })` and `plainMonthDay.toPlainDate({ year })`: the two
    /// conversions that need the caller to supply the missing component.
    pub fn to_plain_date_with(&self, fields: &Fields) -> Result<Self> {
        let supplied = fields.calendar_fields()?;
        Ok(TemporalValue(Repr::PlainDate(match &self.0 {
            Repr::PlainYearMonth(year_month) => year_month.to_plain_date(Some(supplied))?,
            Repr::PlainMonthDay(month_day) => month_day.to_plain_date(Some(supplied))?,
            _ => {
                return Err(Error::type_error(
                    "this Temporal type does not convert to a PlainDate that way",
                ));
            }
        })))
    }

    /// `zonedDateTime.withTimeZone`, `zonedDateTime.withCalendar` and friends.
    pub fn with_time_zone(&self, time_zone: &str) -> Result<Self> {
        let Repr::ZonedDateTime(zoned) = &self.0 else {
            return Err(Error::type_error("this is not a Temporal.ZonedDateTime"));
        };
        Ok(TemporalValue(Repr::ZonedDateTime(
            zoned.with_timezone(time_zone_from(time_zone)?)?,
        )))
    }

    pub fn with_calendar(&self, calendar: &str) -> Result<Self> {
        let calendar = calendar_from(Some(calendar))?;
        Ok(TemporalValue(match &self.0 {
            Repr::PlainDate(date) => Repr::PlainDate(date.with_calendar(calendar)),
            Repr::PlainDateTime(datetime) => Repr::PlainDateTime(datetime.with_calendar(calendar)),
            Repr::ZonedDateTime(zoned) => Repr::ZonedDateTime(zoned.with_calendar(calendar)),
            _ => {
                return Err(Error::type_error(
                    "this Temporal type has no `withCalendar`",
                ));
            }
        }))
    }

    /// `plainDateTime.withPlainTime(...)` and `zonedDateTime.withPlainTime(...)`, where an absent
    /// argument means midnight rather than "leave it alone".
    pub fn with_plain_time(&self, time: Option<&Self>) -> Result<Self> {
        let time = match time.map(|value| &value.0) {
            None => None,
            Some(Repr::PlainTime(time)) => Some(*time),
            Some(_) => return Err(Error::type_error("expected a Temporal.PlainTime")),
        };
        Ok(TemporalValue(match &self.0 {
            Repr::PlainDateTime(datetime) => Repr::PlainDateTime(datetime.with_time(time)?),
            Repr::ZonedDateTime(zoned) => Repr::ZonedDateTime(zoned.with_plain_time(time)?),
            _ => {
                return Err(Error::type_error(
                    "this Temporal type has no `withPlainTime`",
                ));
            }
        }))
    }

    /// `zonedDateTime.startOfDay()`, which is not always midnight: a zone that springs forward at
    /// midnight has no such wall-clock time, and the day starts an hour later.
    pub fn start_of_day(&self) -> Result<Self> {
        let Repr::ZonedDateTime(zoned) = &self.0 else {
            return Err(Error::type_error("this is not a Temporal.ZonedDateTime"));
        };
        Ok(TemporalValue(Repr::ZonedDateTime(zoned.start_of_day()?)))
    }
}

// ==== Temporal.Now ====

/// The realm's clock, read the way `Date.now()` reads it.
///
/// `temporal_rs` will read the host's clock and the host's zone if asked -- that is what its `sys`
/// features are for -- and this realm has neither. The clock is virtual and starts at the epoch so
/// that a build is reproducible and the differential harness has something to agree with, and the
/// zone is UTC (ADR 0015). So the instant arrives here as an argument and the default zone is
/// fixed, and there is no path from this crate to `SystemTime::now` to forget to close.
pub mod now {
    use super::{Error, Kind, Repr, Result, TemporalValue, TimeZone, time_zone_from};
    use temporal_rs::TemporalResult;
    use temporal_rs::host::{HostClock, HostTimeZone};
    use temporal_rs::provider::TimeZoneProvider;
    use temporal_rs::unix_time::EpochNanoseconds;

    /// The realm standing in for the host: it answers with the instant it was given and with UTC.
    struct RealmClock {
        epoch_nanoseconds: i128,
    }

    impl HostClock for RealmClock {
        fn get_host_epoch_nanoseconds(&self) -> TemporalResult<EpochNanoseconds> {
            Ok(EpochNanoseconds::from(self.epoch_nanoseconds))
        }
    }

    impl HostTimeZone for RealmClock {
        /// The realm's zone is UTC by *name*, not by offset.
        ///
        /// The two are not interchangeable in a `Temporal` value: a zone built from a `+00:00`
        /// offset prints as `2026-08-31T00:00:00+00:00[+00:00]`, and the named one prints
        /// `[UTC]` -- which is the identifier [`time_zone_id`] already answers, and the one ADR
        /// 0015 fixes the realm at. An offset zone would have made
        /// `Temporal.Now.zonedDateTimeISO()` disagree with `Temporal.Now.timeZoneId()` about the
        /// zone the program is running in.
        fn get_host_time_zone(
            &self,
            _: &(impl TimeZoneProvider + ?Sized),
        ) -> TemporalResult<TimeZone> {
            TimeZone::try_from_identifier_str(time_zone_id())
        }
    }

    impl temporal_rs::host::HostHooks for RealmClock {}

    /// `Temporal.Now.timeZoneId()`, which is UTC and says so rather than leaving it to be
    /// discovered.
    pub fn time_zone_id() -> &'static str {
        "UTC"
    }

    /// `Temporal.Now.instant()`, `plainDateISO()`, `plainTimeISO()`, `plainDateTimeISO()` and
    /// `zonedDateTimeISO()`, all off the one instant the realm supplies.
    pub fn read(
        kind: Kind,
        epoch_nanoseconds: i128,
        time_zone: Option<&str>,
    ) -> Result<TemporalValue> {
        let zone = match time_zone {
            Some(identifier) => Some(time_zone_from(identifier)?),
            None => None,
        };
        let clock = || temporal_rs::now::Now::new(RealmClock { epoch_nanoseconds });
        Ok(TemporalValue(match kind {
            Kind::Instant => Repr::Instant(clock().instant()?),
            Kind::PlainDate => Repr::PlainDate(clock().plain_date_iso(zone)?),
            Kind::PlainTime => Repr::PlainTime(clock().plain_time_iso(zone)?),
            Kind::PlainDateTime => Repr::PlainDateTime(clock().plain_date_time_iso(zone)?),
            Kind::ZonedDateTime => Repr::ZonedDateTime(clock().zoned_date_time_iso(zone)?),
            _ => {
                return Err(Error::type_error(
                    "Temporal.Now does not produce that Temporal type",
                ));
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: &TemporalValue, name: &str) -> f64 {
        match value.get(name) {
            Some(Field::Number(number)) => number,
            other => panic!("{name} was {other:?}, not a number"),
        }
    }

    fn text(value: &TemporalValue, name: &str) -> String {
        match value.get(name) {
            Some(Field::Text(text)) => text,
            other => panic!("{name} was {other:?}, not a string"),
        }
    }

    fn iso(value: &TemporalValue) -> String {
        value
            .to_temporal_string(&Options::default())
            .expect("string")
    }

    fn days(count: f64) -> TemporalValue {
        duration(&DurationFields {
            days: Some(count),
            ..Default::default()
        })
        .expect("duration")
    }

    #[test]
    fn a_plain_date_reports_its_calendar_fields() {
        let date = plain_date(2024.0, 2.0, 29.0, None).expect("leap day");
        assert_eq!(number(&date, "year"), 2024.0);
        assert_eq!(number(&date, "month"), 2.0);
        assert_eq!(number(&date, "day"), 29.0);
        assert_eq!(text(&date, "monthCode"), "M02");
        assert_eq!(number(&date, "dayOfYear"), 60.0);
        assert_eq!(number(&date, "daysInYear"), 366.0);
        assert!(value_flag(&date, "inLeapYear"));
        assert_eq!(iso(&date), "2024-02-29");
    }

    fn value_flag(value: &TemporalValue, name: &str) -> bool {
        match value.get(name) {
            Some(Field::Flag(flag)) => flag,
            other => panic!("{name} was {other:?}, not a boolean"),
        }
    }

    #[test]
    fn a_property_that_does_not_exist_is_distinguishable_from_one_that_is_undefined() {
        let date = plain_date(2024.0, 1.0, 1.0, None).expect("date");
        assert_eq!(date.get("hour"), None, "a PlainDate has no hour at all");
        let hebrew = plain_date(2024.0, 1.0, 1.0, Some("hebrew")).expect("hebrew date");
        assert_eq!(
            hebrew.get("weekOfYear"),
            Some(Field::Undefined),
            "a calendar without week numbering answers undefined, not nothing"
        );
    }

    #[test]
    fn the_february_29_rollover_follows_the_overflow_option() {
        let leap_day = plain_date(2024.0, 2.0, 29.0, None).expect("leap day");
        let year_later = leap_day
            .add(&year_duration(1.0), &Options::default())
            .expect("add");
        assert_eq!(iso(&year_later), "2025-02-28", "constrain is the default");

        let rejecting = Options {
            overflow: Some("reject".to_owned()),
            ..Default::default()
        };
        let error = leap_day
            .add(&year_duration(1.0), &rejecting)
            .expect_err("reject");
        assert_eq!(error.kind, ErrorKind::Range);
    }

    fn year_duration(count: f64) -> TemporalValue {
        duration(&DurationFields {
            years: Some(count),
            ..Default::default()
        })
        .expect("duration")
    }

    #[test]
    fn subtract_is_add_of_the_negated_duration() {
        let date = plain_date(2024.0, 3.0, 10.0, None).expect("date");
        let back = date
            .subtract(&days(20.0), &Options::default())
            .expect("subtract");
        assert_eq!(iso(&back), "2024-02-19");
    }

    #[test]
    fn until_and_since_are_the_same_difference_with_opposite_signs() {
        let start = plain_date(2024.0, 1.0, 1.0, None).expect("start");
        let end = plain_date(2024.0, 3.0, 15.0, None).expect("end");
        let until = start.until(&end, &Options::default()).expect("until");
        let since = end.since(&start, &Options::default()).expect("since");
        assert_eq!(number(&until, "days"), 74.0);
        assert_eq!(number(&since, "days"), 74.0);
        let back = end.until(&start, &Options::default()).expect("until");
        assert_eq!(number(&back, "days"), -74.0);
    }

    #[test]
    fn largest_unit_decides_which_components_a_difference_uses() {
        let start = plain_date(2024.0, 1.0, 1.0, None).expect("start");
        let end = plain_date(2025.0, 3.0, 15.0, None).expect("end");
        let options = Options {
            largest_unit: Some("year".to_owned()),
            ..Default::default()
        };
        let until = start.until(&end, &options).expect("until");
        assert_eq!(number(&until, "years"), 1.0);
        assert_eq!(number(&until, "months"), 2.0);
        assert_eq!(number(&until, "days"), 14.0);
    }

    #[test]
    fn compare_ignores_the_calendar_and_equals_does_not() {
        let iso_date = plain_date(2024.0, 1.0, 1.0, None).expect("iso");
        let hebrew = iso_date.with_calendar("hebrew").expect("hebrew");
        assert_eq!(compare(&iso_date, &hebrew).expect("compare"), 0);
        assert!(!iso_date.equals(&hebrew).expect("equals"));
        assert!(iso_date.equals(&iso_date.clone()).expect("equals"));
    }

    #[test]
    fn a_duration_rejects_a_fractional_component_and_a_date_truncates_one() {
        let fractional = DurationFields {
            days: Some(1.5),
            ..Default::default()
        };
        let error = duration(&fractional).expect_err("fractional duration");
        assert_eq!(error.kind, ErrorKind::Range);

        let date = plain_date(2024.0, 1.0, 1.9, None).expect("truncating constructor");
        assert_eq!(number(&date, "day"), 1.0);
    }

    #[test]
    fn a_non_finite_component_is_a_range_error_everywhere() {
        assert_eq!(
            plain_date(f64::NAN, 1.0, 1.0, None).expect_err("nan").kind,
            ErrorKind::Range
        );
        assert_eq!(
            plain_time(f64::INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0)
                .expect_err("infinity")
                .kind,
            ErrorKind::Range
        );
    }

    #[test]
    fn parsing_accepts_the_iso_grammar_and_rejects_the_rest() {
        let date = parse(Kind::PlainDate, "2024-03-15").expect("iso date");
        assert_eq!(iso(&date), "2024-03-15");
        let annotated = parse(Kind::PlainDate, "2024-03-15[u-ca=iso8601]").expect("annotated");
        assert_eq!(iso(&annotated), "2024-03-15");
        assert!(parse(Kind::PlainDate, "March 15, 2024").is_err());
        assert!(parse(Kind::PlainDate, "2024-13-01").is_err());

        let duration = parse(Kind::Duration, "P1Y2M3DT4H5M6S").expect("iso duration");
        assert_eq!(number(&duration, "years"), 1.0);
        assert_eq!(number(&duration, "hours"), 4.0);
        assert_eq!(iso(&duration), "P1Y2M3DT4H5M6S");
    }

    #[test]
    fn a_calendar_the_engine_does_not_know_is_a_range_error() {
        let error = plain_date(2024.0, 1.0, 1.0, Some("gregorian")).expect_err("misspelt");
        assert_eq!(error.kind, ErrorKind::Range);
        assert!(plain_date(2024.0, 1.0, 1.0, Some("gregory")).is_ok());
    }

    #[test]
    fn with_replaces_only_the_fields_it_is_given() {
        let date = plain_date(2024.0, 2.0, 29.0, None).expect("leap day");
        let fields = Fields {
            year: Some(2023.0),
            ..Default::default()
        };
        let moved = date.with(&fields, &Options::default()).expect("with");
        assert_eq!(
            iso(&moved),
            "2023-02-28",
            "constrained onto a non-leap year"
        );
        let rejecting = Options {
            overflow: Some("reject".to_owned()),
            ..Default::default()
        };
        assert!(date.with(&fields, &rejecting).is_err());
    }

    #[test]
    fn an_empty_property_bag_is_a_type_error_not_an_empty_value() {
        // The distinction matters: `date.with({})` is a TypeError in the language, because a
        // caller that meant to change nothing did not mean to call `with`.
        let date = plain_date(2024.0, 1.0, 1.0, None).expect("date");
        let error = date
            .with(&Fields::default(), &Options::default())
            .expect_err("empty");
        assert_eq!(error.kind, ErrorKind::Type);
    }

    #[test]
    fn a_plain_time_rounds_to_the_unit_it_is_asked_for() {
        let time = plain_time(3.0, 34.0, 56.0, 987.0, 654.0, 321.0).expect("time");
        let options = Options {
            smallest_unit: Some("hour".to_owned()),
            ..Default::default()
        };
        let rounded = time.round(&options).expect("round");
        assert_eq!(iso(&rounded), "04:00:00");

        let half_even = Options {
            smallest_unit: Some("minute".to_owned()),
            rounding_mode: Some("floor".to_owned()),
            ..Default::default()
        };
        assert_eq!(iso(&time.round(&half_even).expect("round")), "03:34:00");
    }

    #[test]
    fn rounding_a_duration_of_calendar_units_needs_something_to_be_relative_to() {
        let mixed = duration(&DurationFields {
            months: Some(1.0),
            days: Some(15.0),
            ..Default::default()
        })
        .expect("duration");
        let options = Options {
            smallest_unit: Some("day".to_owned()),
            largest_unit: Some("day".to_owned()),
            ..Default::default()
        };
        assert!(
            mixed.round(&options).is_err(),
            "a month has no fixed length without a date to measure from"
        );

        let anchored = Options {
            relative_to: Some(plain_date(2024.0, 1.0, 1.0, None).expect("anchor")),
            ..options
        };
        let rounded = mixed.round(&anchored).expect("round");
        assert_eq!(number(&rounded, "days"), 46.0, "January is 31 days");
    }

    #[test]
    fn total_answers_with_a_bare_number() {
        let duration = duration(&DurationFields {
            hours: Some(1.0),
            minutes: Some(30.0),
            ..Default::default()
        })
        .expect("duration");
        let options = Options {
            unit: Some("minute".to_owned()),
            ..Default::default()
        };
        assert_eq!(duration.total(&options).expect("total"), 90.0);
    }

    #[test]
    fn an_instant_carries_more_precision_than_a_double_holds() {
        let nanoseconds = 1_234_567_890_123_456_789_i128;
        let instant = instant(nanoseconds).expect("instant");
        assert_eq!(
            instant.get("epochNanoseconds"),
            Some(Field::Integer128(nanoseconds))
        );
        assert_eq!(number(&instant, "epochMilliseconds"), 1_234_567_890_123.0);
        assert_eq!(iso(&instant), "2009-02-13T23:31:30.123456789Z");
    }

    #[test]
    fn a_zoned_date_time_projects_an_instant_into_a_named_zone() {
        // ADR 0015 fixes the realm's *ambient* zone at UTC. A program that names a zone is asking
        // an explicit question, and this is the answer, which is why the zone database is linked.
        let zoned = zoned_date_time(0, "America/Chicago", None).expect("zoned");
        assert_eq!(number(&zoned, "year"), 1969.0);
        assert_eq!(number(&zoned, "hour"), 18.0);
        assert_eq!(text(&zoned, "offset"), "-06:00");
        assert_eq!(text(&zoned, "timeZoneId"), "America/Chicago");
    }

    #[test]
    fn a_zone_that_springs_forward_has_no_midnight() {
        // 12 March 2023 in New York begins at 00:00 and skips 02:00; the day is 23 hours long.
        let zoned = parse_zoned_date_time(
            "2023-03-12T12:00:00-04:00[America/New_York]",
            &Options::default(),
        )
        .expect("zoned");
        assert_eq!(number(&zoned, "hoursInDay"), 23.0);
        let start = zoned.start_of_day().expect("start of day");
        assert_eq!(number(&start, "hour"), 0.0);
    }

    #[test]
    fn now_reads_the_instant_it_is_handed_and_never_the_host_clock() {
        // The realm's clock starts at the epoch and only a timer moves it (ADR 0015), so this is
        // what an unqualified `Temporal.Now.plainDateISO()` has to say.
        let at_epoch = now::read(Kind::PlainDate, 0, None).expect("now");
        assert_eq!(iso(&at_epoch), "1970-01-01");
        assert_eq!(now::time_zone_id(), "UTC");

        let advanced = now::read(Kind::PlainDateTime, 90_061_000_000_000, None).expect("now");
        assert_eq!(iso(&advanced), "1970-01-02T01:01:01");

        let elsewhere = now::read(Kind::PlainDateTime, 0, Some("Asia/Tokyo")).expect("now");
        assert_eq!(iso(&elsewhere), "1970-01-01T09:00:00");
    }

    #[test]
    fn an_unqualified_now_names_its_zone_rather_than_describing_its_offset() {
        // `Temporal.Now.zonedDateTimeISO()` with no argument asks for the zone the program is
        // running in, and ADR 0015 makes that UTC. A zone built from a `+00:00` offset would name
        // the same instant and print `[+00:00]`, which is the offset UTC happens to have rather
        // than the zone `time_zone_id` says the realm is in -- and two readers of one realm
        // disagreeing about its zone is a wrong answer whichever one a program believes.
        let zoned = now::read(Kind::ZonedDateTime, 0, None).expect("now");
        assert_eq!(iso(&zoned), "1970-01-01T00:00:00+00:00[UTC]");
        assert_eq!(text(&zoned, "timeZoneId"), now::time_zone_id());

        // A zone the program names explicitly is still its own answer, not the realm's.
        let named = now::read(Kind::ZonedDateTime, 0, Some("Asia/Tokyo")).expect("now");
        assert_eq!(iso(&named), "1970-01-01T09:00:00+09:00[Asia/Tokyo]");
    }

    #[test]
    fn now_is_repeatable_because_the_instant_is_an_argument() {
        let first = now::read(Kind::Instant, 12_345, None).expect("now");
        let second = now::read(Kind::Instant, 12_345, None).expect("now");
        assert_eq!(
            first.get("epochNanoseconds"),
            second.get("epochNanoseconds")
        );
    }

    #[test]
    fn a_year_month_and_a_month_day_each_need_the_component_they_lack() {
        let year_month = plain_year_month(2024.0, 2.0, None, None).expect("year month");
        assert_eq!(number(&year_month, "daysInMonth"), 29.0);
        assert_eq!(iso(&year_month), "2024-02");
        let day = Fields {
            day: Some(29.0),
            ..Default::default()
        };
        assert_eq!(
            iso(&year_month.to_plain_date_with(&day).expect("to date")),
            "2024-02-29"
        );

        let month_day = plain_month_day(2.0, 29.0, None, None).expect("month day");
        assert_eq!(iso(&month_day), "02-29");
        // 2023 has no 29 February, and `toPlainDate` constrains rather than refusing -- the one
        // place in this API where the missing component is supplied and the overflow is implied.
        let year = Fields {
            year: Some(2023.0),
            ..Default::default()
        };
        assert_eq!(
            iso(&month_day.to_plain_date_with(&year).expect("to date")),
            "2023-02-28"
        );
    }

    #[test]
    fn a_month_day_defaults_to_the_reference_year_that_has_a_29th_of_february() {
        let month_day = plain_month_day(2.0, 29.0, None, None).expect("month day");
        assert_eq!(
            month_day
                .to_temporal_string(&Options {
                    calendar_name: Some("always".to_owned()),
                    ..Default::default()
                })
                .expect("string"),
            "1972-02-29[u-ca=iso8601]"
        );
    }

    #[test]
    fn the_engine_stays_behind_the_seam() {
        // Not a behavioural test: a compile-time one. Everything a caller can reach is a scalar,
        // a string, an opaque `TemporalValue` or an `Error`, which is the property ADR 0029's
        // adoption rule asks for and the property a careless `pub use` would quietly remove.
        let date = plain_date(2024.0, 1.0, 1.0, None).expect("date");
        let field: Option<Field> = date.get("year");
        let kind: Kind = date.kind();
        let calendar: Option<&'static str> = date.calendar_id();
        assert!(matches!(field, Some(Field::Number(_))));
        assert_eq!(kind, Kind::PlainDate);
        assert_eq!(calendar, Some("iso8601"));
    }

    #[test]
    fn a_mismatched_pair_is_a_type_error_rather_than_a_wrong_answer() {
        let date = plain_date(2024.0, 1.0, 1.0, None).expect("date");
        let time = plain_time(1.0, 0.0, 0.0, 0.0, 0.0, 0.0).expect("time");
        assert_eq!(
            compare(&date, &time).expect_err("mismatch").kind,
            ErrorKind::Type
        );
        assert_eq!(
            date.until(&time, &Options::default())
                .expect_err("mismatch")
                .kind,
            ErrorKind::Type
        );
        assert_eq!(
            date.equals(&time).expect_err("mismatch").kind,
            ErrorKind::Type
        );
    }
}

use super::*;

/// The civil date a count of days since the epoch names.
///
/// Hinnant's algorithm, which is exact for the whole range a `f64` instant can reach and needs no
/// tables. Shifting the year to start in March is what makes the leap day the last day of the
/// year and removes every special case from the month arithmetic.
pub(super) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (year + i64::from(month <= 2), month, day)
}

/// The inverse of [`civil_from_days`], for `Date.UTC` and the constructor's field form.
pub(super) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Which part of an instant a reader returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateField {
    Time,
    FullYear,
    Month,
    Day,
    Weekday,
    Hours,
    Minutes,
    Seconds,
    Milliseconds,
    /// Always zero: the realm has no zone but UTC, so there is nothing to offset by.
    TimezoneOffset,
}

/// One function on `Date.prototype`.
///
/// One variant per method rather than one per field read, because the specification gives each
/// its own function object. `getMonth` and `getUTCMonth` answer the same number in a realm whose
/// only zone is UTC (ADR 0015) and are still two functions, with two names and two arities that
/// a program can read; a single variant behind both names could report only one of each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateMethod {
    ValueOf,
    GetTime,
    GetFullYear,
    GetUtcFullYear,
    GetMonth,
    GetUtcMonth,
    GetDate,
    GetUtcDate,
    GetDay,
    GetUtcDay,
    GetHours,
    GetUtcHours,
    GetMinutes,
    GetUtcMinutes,
    GetSeconds,
    GetUtcSeconds,
    GetMilliseconds,
    GetUtcMilliseconds,
    GetTimezoneOffset,
    /// Annex B: the year less 1900, which is the shape the language shipped with.
    GetYear,
    SetTime,
    SetFullYear,
    SetUtcFullYear,
    SetMonth,
    SetUtcMonth,
    SetDate,
    SetUtcDate,
    SetHours,
    SetUtcHours,
    SetMinutes,
    SetUtcMinutes,
    SetSeconds,
    SetUtcSeconds,
    SetMilliseconds,
    SetUtcMilliseconds,
    /// Annex B, and not `setFullYear` under another name: a year in 0..99 means 19xx here, the
    /// same legacy rule the two-argument constructor keeps.
    SetYear,
    ToIsoString,
    ToJson,
    ToString,
    ToDateString,
    ToTimeString,
    ToUtcString,
    ToLocaleString,
    ToLocaleDateString,
    ToLocaleTimeString,
    /// `Date.prototype[Symbol.toPrimitive]`, the one method here published under a symbol.
    ToPrimitive,
}

impl DateMethod {
    /// The name the language spells this method.
    ///
    /// Tabulated rather than derived from the variant, which the general derivation cannot
    /// reach: the language writes `UTC`, `ISO` and `JSON` where Rust spells them `Utc`, `Iso`
    /// and `Json`, and a case distinction the input does not carry cannot be recovered.
    pub(super) const fn js_name(self) -> &'static str {
        match self {
            Self::ValueOf => "valueOf",
            Self::GetTime => "getTime",
            Self::GetFullYear => "getFullYear",
            Self::GetUtcFullYear => "getUTCFullYear",
            Self::GetMonth => "getMonth",
            Self::GetUtcMonth => "getUTCMonth",
            Self::GetDate => "getDate",
            Self::GetUtcDate => "getUTCDate",
            Self::GetDay => "getDay",
            Self::GetUtcDay => "getUTCDay",
            Self::GetHours => "getHours",
            Self::GetUtcHours => "getUTCHours",
            Self::GetMinutes => "getMinutes",
            Self::GetUtcMinutes => "getUTCMinutes",
            Self::GetSeconds => "getSeconds",
            Self::GetUtcSeconds => "getUTCSeconds",
            Self::GetMilliseconds => "getMilliseconds",
            Self::GetUtcMilliseconds => "getUTCMilliseconds",
            Self::GetTimezoneOffset => "getTimezoneOffset",
            Self::GetYear => "getYear",
            Self::SetTime => "setTime",
            Self::SetFullYear => "setFullYear",
            Self::SetUtcFullYear => "setUTCFullYear",
            Self::SetMonth => "setMonth",
            Self::SetUtcMonth => "setUTCMonth",
            Self::SetDate => "setDate",
            Self::SetUtcDate => "setUTCDate",
            Self::SetHours => "setHours",
            Self::SetUtcHours => "setUTCHours",
            Self::SetMinutes => "setMinutes",
            Self::SetUtcMinutes => "setUTCMinutes",
            Self::SetSeconds => "setSeconds",
            Self::SetUtcSeconds => "setUTCSeconds",
            Self::SetMilliseconds => "setMilliseconds",
            Self::SetUtcMilliseconds => "setUTCMilliseconds",
            Self::SetYear => "setYear",
            Self::ToIsoString => "toISOString",
            Self::ToJson => "toJSON",
            Self::ToString => "toString",
            Self::ToDateString => "toDateString",
            Self::ToTimeString => "toTimeString",
            Self::ToUtcString => "toUTCString",
            Self::ToLocaleString => "toLocaleString",
            Self::ToLocaleDateString => "toLocaleDateString",
            Self::ToLocaleTimeString => "toLocaleTimeString",
            // A symbol-keyed method is named for the key it is published under, in brackets --
            // which is what a stack trace through one has to print.
            Self::ToPrimitive => "[Symbol.toPrimitive]",
        }
    }

    /// The declared parameter count.
    ///
    /// A setter declares the field it names and every finer one it will also accept, which is
    /// what makes `setHours(h, m, s, ms)` a single call rather than four -- and the count a
    /// caller reads to decide how many arguments to forward.
    pub(super) const fn length(self) -> f64 {
        match self {
            Self::SetHours | Self::SetUtcHours => 4.0,
            Self::SetFullYear | Self::SetUtcFullYear | Self::SetMinutes | Self::SetUtcMinutes => {
                3.0
            }
            Self::SetMonth | Self::SetUtcMonth | Self::SetSeconds | Self::SetUtcSeconds => 2.0,
            Self::SetTime
            | Self::SetDate
            | Self::SetUtcDate
            | Self::SetMilliseconds
            | Self::SetUtcMilliseconds
            | Self::SetYear
            | Self::ToJson
            | Self::ToPrimitive => 1.0,
            _ => 0.0,
        }
    }

    /// The part of the instant a reader answers with, or `None` for anything that is not one.
    ///
    /// The local readers answer the UTC field because the realm's local zone *is* UTC -- ADR
    /// 0015 -- not because the two are being conflated.
    pub(super) const fn reads(self) -> Option<DateField> {
        Some(match self {
            Self::ValueOf | Self::GetTime => DateField::Time,
            Self::GetFullYear | Self::GetUtcFullYear => DateField::FullYear,
            Self::GetMonth | Self::GetUtcMonth => DateField::Month,
            Self::GetDate | Self::GetUtcDate => DateField::Day,
            Self::GetDay | Self::GetUtcDay => DateField::Weekday,
            Self::GetHours | Self::GetUtcHours => DateField::Hours,
            Self::GetMinutes | Self::GetUtcMinutes => DateField::Minutes,
            Self::GetSeconds | Self::GetUtcSeconds => DateField::Seconds,
            Self::GetMilliseconds | Self::GetUtcMilliseconds => DateField::Milliseconds,
            Self::GetTimezoneOffset => DateField::TimezoneOffset,
            _ => return None,
        })
    }

    /// The coarsest field a writer replaces, or `None` for anything that is not one.
    pub(super) const fn writes(self) -> Option<DateField> {
        Some(match self {
            Self::SetTime => DateField::Time,
            Self::SetFullYear | Self::SetUtcFullYear | Self::SetYear => DateField::FullYear,
            Self::SetMonth | Self::SetUtcMonth => DateField::Month,
            Self::SetDate | Self::SetUtcDate => DateField::Day,
            Self::SetHours | Self::SetUtcHours => DateField::Hours,
            Self::SetMinutes | Self::SetUtcMinutes => DateField::Minutes,
            Self::SetSeconds | Self::SetUtcSeconds => DateField::Seconds,
            Self::SetMilliseconds | Self::SetUtcMilliseconds => DateField::Milliseconds,
            _ => return None,
        })
    }
}

/// `TimeClip`: the instant a date is allowed to hold, or `NaN` for one it is not.
///
/// Two things happen here that look like details and are not. An instant outside +/-8.64e15
/// collapses to the single invalid date, which is why every reader only has to test for NaN;
/// and the truncation's `-0` is folded back to `+0`, which `Object.is` can see and the
/// specification requires -- a date one millisecond before the epoch's *fraction* is the epoch.
pub(super) fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8.64e15 {
        return f64::NAN;
    }
    time.trunc() + 0.0
}

/// The number one reader takes out of an instant.
pub(super) fn date_read(field: DateField, time: f64) -> f64 {
    match field {
        DateField::Time => return time,
        // The realm has one zone and it is UTC, which this reports honestly rather than leaving
        // to be discovered. An invalid date still has no offset to report.
        DateField::TimezoneOffset => return if time.is_nan() { f64::NAN } else { 0.0 },
        _ => {}
    }
    let Some(parts) = date_parts(time) else {
        // Every reader on an invalid date is NaN, which is what makes one propagate.
        return f64::NAN;
    };
    match field {
        // `getMonth` is zero-based and `getDate` is one-based, which is the language's oldest
        // wart and is not this compiler's to fix.
        DateField::FullYear => parts.year as f64,
        DateField::Month => f64::from(parts.month) - 1.0,
        DateField::Day => f64::from(parts.day),
        DateField::Weekday => f64::from(parts.weekday),
        DateField::Hours => f64::from(parts.hour),
        DateField::Minutes => f64::from(parts.minute),
        DateField::Seconds => f64::from(parts.second),
        DateField::Milliseconds => f64::from(parts.millisecond),
        DateField::Time | DateField::TimezoneOffset => unreachable!("handled above"),
    }
}

/// The instant a writer produces, or `None` for a write the specification does not perform.
///
/// The distinction is observable and there is a test for it: a setter that finds an invalid date
/// answers NaN *without storing anything*, so a `valueOf` on one of its arguments that moved this
/// same date keeps its effect. Storing the NaN would quietly undo it.
pub(super) fn date_written(
    method: DateMethod,
    field: DateField,
    time: f64,
    supplied: &[f64],
) -> Option<f64> {
    if matches!(field, DateField::Time) {
        return Some(time_clip(supplied.first().copied().unwrap_or(f64::NAN)));
    }
    // The year setters are the ones that *revive* an invalid date: the specification starts them
    // from the epoch rather than propagating the NaN, which is what makes
    // `new Date(NaN).setFullYear(2000)` a real date. Every other setter leaves it invalid.
    let revives = matches!(
        method,
        DateMethod::SetFullYear | DateMethod::SetUtcFullYear | DateMethod::SetYear
    );
    let parts = date_parts(if time.is_nan() && revives { 0.0 } else { time })?;
    // The order the fields appear in, so "this one and the ones after it" is a slice.
    let mut values = [
        parts.year as f64,
        f64::from(parts.month) - 1.0,
        f64::from(parts.day),
        f64::from(parts.hour),
        f64::from(parts.minute),
        f64::from(parts.second),
        f64::from(parts.millisecond),
    ];
    let first = match field {
        DateField::FullYear => 0,
        DateField::Month => 1,
        DateField::Day => 2,
        DateField::Hours => 3,
        DateField::Minutes => 4,
        DateField::Seconds => 5,
        DateField::Milliseconds => 6,
        DateField::Time | DateField::Weekday | DateField::TimezoneOffset => return Some(time),
    };
    for (offset, value) in supplied.iter().enumerate() {
        let Some(slot) = values.get_mut(first + offset) else {
            break;
        };
        *slot = *value;
    }
    // Annex B's `setYear` keeps the two-digit rule the constructor keeps: a year in 0..99 is
    // 19xx. `setFullYear` deliberately does not, which is the whole reason both exist.
    if matches!(method, DateMethod::SetYear) && (0.0..=99.0).contains(&values[0].trunc()) {
        values[0] = values[0].trunc() + 1900.0;
    }
    Some(time_clip(parts_to_instant(values)))
}

/// A year as a date string writes it: four digits at least, and a sign for a year before 1 CE.
pub(super) fn year_string(year: i64) -> String {
    if year < 0 {
        format!("-{:04}", year.unsigned_abs())
    } else {
        format!("{year:04}")
    }
}

/// `toDateString`: the weekday and calendar date, `Thu Jan 01 1970`.
pub(super) fn date_only_string(time: f64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let Some(parts) = date_parts(time) else {
        return "Invalid Date".to_owned();
    };
    format!(
        "{} {} {:02} {}",
        WEEKDAYS[parts.weekday as usize],
        MONTH_NAMES[(parts.month - 1) as usize],
        parts.day,
        year_string(parts.year)
    )
}

/// `toTimeString`: the clock and the zone, which is UTC here because the realm has no other.
pub(super) fn time_only_string(time: f64) -> String {
    let Some(parts) = date_parts(time) else {
        return "Invalid Date".to_owned();
    };
    format!(
        "{:02}:{:02}:{:02} GMT+0000 (Coordinated Universal Time)",
        parts.hour, parts.minute, parts.second
    )
}

/// `toUTCString`: `Thu, 01 Jan 1970 00:00:00 GMT`, the form an HTTP date is written in.
///
/// A different shape from `toString`, not a different zone: the comma, the day before the month
/// and the bare `GMT` are what a header parser expects, and getting them wrong is the sort of
/// difference that only shows up on the wire.
pub(super) fn utc_string(time: f64) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let Some(parts) = date_parts(time) else {
        return "Invalid Date".to_owned();
    };
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[parts.weekday as usize],
        parts.day,
        MONTH_NAMES[(parts.month - 1) as usize],
        year_string(parts.year),
        parts.hour,
        parts.minute,
        parts.second
    )
}

/// The three-letter month names every date string shares.
pub(super) const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The parts of an instant, all in UTC -- see ADR 0015 for why the realm has no other zone.
pub(super) struct DateParts {
    pub(super) year: i64,
    pub(super) month: u32,
    pub(super) day: u32,
    pub(super) weekday: u32,
    pub(super) hour: u32,
    pub(super) minute: u32,
    pub(super) second: u32,
    pub(super) millisecond: u32,
}

pub(super) fn date_parts(time: f64) -> Option<DateParts> {
    if !time.is_finite() {
        return None;
    }
    let millis = time as i64;
    // Floor division, not truncation: an instant before the epoch is a negative count and must
    // round *down* to the day containing it.
    let days = millis.div_euclid(86_400_000);
    let within = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    Some(DateParts {
        year,
        month,
        day,
        // 1970-01-01 was a Thursday, which is day 4.
        weekday: (days + 4).rem_euclid(7) as u32,
        hour: (within / 3_600_000) as u32,
        minute: (within / 60_000 % 60) as u32,
        second: (within / 1000 % 60) as u32,
        millisecond: (within % 1000) as u32,
    })
}

/// The instant a set of UTC parts names, with every part allowed out of range.
///
/// Out-of-range parts roll over rather than clamping: `setMonth(12)` is January of the next year
/// and `setDate(0)` is the last day of the previous month. That falls out of normalising the
/// month into a year offset and then counting in whole units, and it is what the specification
/// says -- clamping would quietly produce a different date than the program asked for.
pub(super) fn parts_to_instant(parts: [f64; 7]) -> f64 {
    if parts.iter().any(|part| !part.is_finite()) {
        return f64::NAN;
    }
    // `MakeDay` and `MakeTime` truncate every field toward zero before any arithmetic runs,
    // which is what makes `setHours(1.9)` one o'clock exactly rather than something between.
    let [year, months, day, hour, minute, second, millisecond] = parts.map(f64::trunc);
    let year = year + (months / 12.0).floor();
    let month = months.rem_euclid(12.0);
    // Beyond what a civil date can name; the calendar arithmetic below would overflow.
    if year.abs() > 400_000.0 {
        return f64::NAN;
    }
    let days = days_from_civil(year as i64, month as i64 + 1, 1) as f64 + (day - 1.0);
    // Doubles, and summed in the specification's own grouping. That reads like a detail and is
    // not: these are IEEE-754 additions, so the same terms accumulated in another order round
    // differently, and a day count large enough to matter is reachable from `Date.UTC`.
    let clock = ((hour * 3_600_000.0 + minute * 60_000.0) + second * 1000.0) + millisecond;
    days * 86_400_000.0 + clock
}

/// `toISOString`, or `None` for an instant that has no ISO spelling.
pub(super) fn iso_string(time: f64) -> Option<String> {
    let parts = date_parts(time)?;
    // A year outside four digits is written with a sign and six, which is the only shape that
    // stays unambiguous.
    let year = if (0..=9999).contains(&parts.year) {
        format!("{:04}", parts.year)
    } else {
        format!(
            "{}{:06}",
            if parts.year < 0 { '-' } else { '+' },
            parts.year.abs()
        )
    };
    Some(format!(
        "{year}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        parts.month, parts.day, parts.hour, parts.minute, parts.second, parts.millisecond
    ))
}

/// `String(date)`: the human form, which is UTC here because the realm has no other zone.
///
/// Assembled from the two halves rather than formatted again, because the language publishes
/// those halves as `toDateString` and `toTimeString` and the three must agree character for
/// character -- a program that concatenates the pair gets exactly this string.
pub(super) fn date_to_string(time: f64) -> String {
    if date_parts(time).is_none() {
        return "Invalid Date".to_owned();
    }
    format!("{} {}", date_only_string(time), time_only_string(time))
}

/// The instant named by `(year, month, day, hours, minutes, seconds, milliseconds)`.
///
/// Out-of-range fields roll over rather than failing -- month 13 is January of the next year --
/// which is why the arithmetic is done in milliseconds rather than validated field by field.
pub(super) fn date_from_fields(arguments: &[Value]) -> f64 {
    let field = |index: usize, default: f64| -> f64 {
        arguments.get(index).map_or(default, Value::as_number)
    };
    let year = field(0, f64::NAN);
    if !year.is_finite() {
        return f64::NAN;
    }
    // A two-digit year means 19xx, which is the one piece of legacy the constructor keeps -- and
    // the test is on the *truncated* year, so `Date.UTC(-0.999999)` is 1900 and not year zero.
    let year = if (0.0..=99.0).contains(&year.trunc()) {
        year.trunc() + 1900.0
    } else {
        year
    };
    parts_to_instant([
        year,
        field(1, 0.0),
        field(2, 1.0),
        field(3, 0.0),
        field(4, 0.0),
        field(5, 0.0),
        field(6, 0.0),
    ])
}

/// Parse the forms `Date.parse` and the string constructor accept.
///
/// Deliberately narrow: the specification also allows an implementation-defined set of other
/// formats, and guessing at one silently turns a typo into a date. The two non-ISO spellings
/// here are not guesses -- they are what this engine's own `toString` and `toUTCString` emit,
/// and the specification requires `Date.parse` to round-trip both.
pub(super) fn parse_date(text: &str) -> f64 {
    let text = text.trim();
    match iso_date(text).or_else(|| legacy_date(text)) {
        Some(time) => time_clip(time),
        None => f64::NAN,
    }
}

/// The Date Time String Format: `YYYY-MM-DDTHH:mm:ss.sssZ` and every prefix of it the
/// specification allows, with the extended six-digit year and an explicit UTC offset.
pub(super) fn iso_date(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    // A fixed-width run of digits, or nothing. Written out because `parse` would accept a sign
    // or a shorter field, and both would let a malformed string name a date.
    let digits = |at: usize, count: usize| -> Option<i64> {
        let slice = text.get(at..at.checked_add(count)?)?;
        slice
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then_some(())?;
        slice.parse().ok()
    };
    let (year, mut at) = match bytes.first() {
        Some(sign @ (b'+' | b'-')) => {
            let magnitude = digits(1, 6)?;
            // `-000000` is not a spelling of year zero: the specification refuses it outright so
            // that one year cannot be written two ways.
            if *sign == b'-' && magnitude == 0 {
                return None;
            }
            (if *sign == b'-' { -magnitude } else { magnitude }, 7)
        }
        _ => (digits(0, 4)?, 4),
    };
    let (mut month, mut day) = (1, 1);
    if bytes.get(at) == Some(&b'-') {
        month = digits(at + 1, 2)?;
        at += 3;
        if bytes.get(at) == Some(&b'-') {
            day = digits(at + 1, 2)?;
            at += 3;
        }
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let date = days_from_civil(year, month, day) as f64 * 86_400_000.0;
    if at == bytes.len() {
        // A date with no time is UTC midnight, where a date *with* one is local time. The two
        // are the same instant in this realm and are still two rules -- ADR 0015.
        return Some(date);
    }
    if bytes.get(at) != Some(&b'T') {
        return None;
    }
    let hour = digits(at + 1, 2)?;
    if bytes.get(at + 3) != Some(&b':') {
        return None;
    }
    let minute = digits(at + 4, 2)?;
    at += 6;
    let (mut second, mut fraction) = (0, 0);
    if bytes.get(at) == Some(&b':') {
        second = digits(at + 1, 2)?;
        at += 3;
        if bytes.get(at) == Some(&b'.') {
            fraction = digits(at + 1, 3)?;
            at += 4;
        }
    }
    if hour > 24 || minute > 59 || second > 59 {
        return None;
    }
    // `24:00` names the midnight that ends the day, and nothing at all past it.
    if hour == 24 && (minute != 0 || second != 0 || fraction != 0) {
        return None;
    }
    let clock = (hour * 3_600_000 + minute * 60_000 + second * 1000 + fraction) as f64;
    match bytes.get(at) {
        // No offset means local time, which is UTC here.
        None => Some(date + clock),
        Some(b'Z') if at + 1 == bytes.len() => Some(date + clock),
        Some(sign @ (b'+' | b'-')) => {
            let offset_hour = digits(at + 1, 2)?;
            if bytes.get(at + 3) != Some(&b':') {
                return None;
            }
            let offset_minute = digits(at + 4, 2)?;
            if at + 6 != bytes.len() || offset_hour > 23 || offset_minute > 59 {
                return None;
            }
            let offset = (offset_hour * 3_600_000 + offset_minute * 60_000) as f64;
            // An offset says what the clock read *there*, so reaching UTC subtracts it.
            Some(if *sign == b'-' {
                date + clock + offset
            } else {
                date + clock - offset
            })
        }
        _ => None,
    }
}

/// The two human forms this engine emits: `Thu Jan 01 1970 00:00:00 GMT+0000 (...)` from
/// `toString`, and `Thu, 01 Jan 1970 00:00:00 GMT` from `toUTCString`.
///
/// Here because the specification requires `Date.parse(d.toString())` to name the same instant
/// as `d` -- a round-trip programs rely on, and one an ISO-only parser cannot make.
pub(super) fn legacy_date(text: &str) -> Option<f64> {
    let mut fields = text.split_whitespace();
    let first = fields.next()?;
    // The comma after the weekday is what tells the two forms apart, and it is also what says
    // which of the next two fields is the month.
    let (month, day) = if first.ends_with(',') {
        let day = fields.next()?;
        (fields.next()?, day)
    } else {
        let month = fields.next()?;
        (month, fields.next()?)
    };
    let month = 1 + MONTH_NAMES.iter().position(|name| *name == month)? as i64;
    let day = day.parse::<i64>().ok()?;
    let year = fields.next()?.parse::<i64>().ok()?;
    if !(1..=31).contains(&day) {
        return None;
    }
    let date = days_from_civil(year, month, day) as f64 * 86_400_000.0;
    // `toDateString` stops here, and its output is a date the specification does not require to
    // round-trip -- but reading it as midnight is what every engine does and costs nothing.
    let Some(clock) = fields.next() else {
        return Some(date);
    };
    let mut clock = clock.split(':');
    let hour = clock.next()?.parse::<i64>().ok()?;
    let minute = clock.next()?.parse::<i64>().ok()?;
    let second = clock.next()?.parse::<i64>().ok()?;
    if clock.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let clock = (hour * 3_600_000 + minute * 60_000 + second * 1000) as f64;
    let zone = fields.next()?;
    let offset = match zone.strip_prefix("GMT") {
        Some("") => 0.0,
        Some(rest) => {
            let (sign, digits) = rest.split_at(1);
            if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let magnitude = (digits[..2].parse::<i64>().ok()? * 3_600_000
                + digits[2..].parse::<i64>().ok()? * 60_000) as f64;
            match sign {
                "+" => -magnitude,
                "-" => magnitude,
                _ => return None,
            }
        }
        None => return None,
    };
    Some(date + clock + offset)
}

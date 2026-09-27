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

/// One of the two functions on `Date.prototype` this engine still implements.
///
/// Everything else on `Date.prototype` -- every field reader and writer, the formatters,
/// `toJSON` and `@@toPrimitive` -- is the shared bootstrap's (`kiln-builtins/src/date.ts`, ADR
/// 0060), which captures these two at realm start and builds the whole prototype on them. They
/// are the representation operations: the only way TypeScript can read or replace the instant a
/// date holds (ADR 0065).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateMethod {
    GetTime,
    SetTime,
}

impl DateMethod {
    /// The name the language spells this method.
    pub(super) const fn js_name(self) -> &'static str {
        match self {
            Self::GetTime => "getTime",
            Self::SetTime => "setTime",
        }
    }

    /// The declared parameter count.
    pub(super) const fn length(self) -> f64 {
        match self {
            Self::GetTime => 0.0,
            Self::SetTime => 1.0,
        }
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

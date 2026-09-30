//! Shared ECMAScript Date string parsing and time clipping.
//!
//! Both execution engines use this owner so the implementation-defined round-trip forms and
//! ISO grammar cannot drift between them.

pub const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn time_clip(time: f64) -> f64 {
    if !time.is_finite() || time.abs() > 8.64e15 {
        return f64::NAN;
    }
    time.trunc() + 0.0
}

pub fn parse_date(text: &str) -> f64 {
    let text = text.trim();
    match iso_date(text).or_else(|| legacy_date(text)) {
        Some(time) => time_clip(time),
        None => f64::NAN,
    }
}

/// The Date Time String Format: `YYYY-MM-DDTHH:mm:ss.sssZ` and every prefix of it the
/// specification allows, with the extended six-digit year and an explicit UTC offset.
pub fn iso_date(text: &str) -> Option<f64> {
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
pub fn legacy_date(text: &str) -> Option<f64> {
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

use super::INVALID_ARGUMENT;
use kiln_temporal::{Fields, Kind, Options, TemporalValue};
use std::cell::RefCell;

/// A failed call left a message behind. Negative, so a caller testing `< 0` catches every one.
///
/// All three classes are carried rather than flattened. `Temporal` picks between them on nearly
/// every path -- a field out of range is a RangeError, malformed ISO text a SyntaxError, a
/// wrong argument *type* a TypeError -- and the interpreter raises exactly what the engine
/// chose, so collapsing any two here would make the two engines disagree about which error a
/// program catches.
const RANGE_ERROR: i32 = -2;
const TYPE_ERROR: i32 = -3;
const SYNTAX_ERROR: i32 = -4;

/// The numeric field slots [`kiln_temporal_from_fields`] reads, in order: eraYear, year,
/// month, day, hour, minute, second, millisecond, microsecond, nanosecond. NaN means absent,
/// which is what lets one array carry an arbitrary subset without a presence mask beside it.
const FIELD_SLOTS: usize = 10;

/// The ten `Duration` components, in the order [`kiln_temporal::DurationFields`] declares them.
const DURATION_SLOTS: usize = 10;

/// The field string slots, in order: era, monthCode, offset, timeZone, calendar.
///
/// They arrive as one concatenated buffer plus a length per slot, because five pointer/length
/// pairs in a C signature is a shape nobody can call correctly twice. A length of -1 means the
/// program did not supply the field at all, which is not the same as supplying `""`.
///
/// Options are *not* here: they travel in [`OPTION_TEXT_SLOTS`], because `overflow` is not the
/// only one a constructor reads -- a `ZonedDateTime` built from a bag also decides what a
/// wall-clock time that exists twice means -- and one array an option can be read from is
/// better than two.
const TEXT_SLOTS: usize = 5;

thread_local! {
    /// The message and class of the last failure on this thread.
    ///
    /// Carried out-of-band because the fallible calls already spend their return value on the
    /// error *class* -- and `Temporal` is unusually specific about that class, so flattening a
    /// RangeError into a TypeError would be a visible wrong answer rather than a lost detail.
    static LAST_ERROR: RefCell<(i32, String)> = const { RefCell::new((0, String::new())) };
}

fn remember(error: &kiln_temporal::Error) -> i32 {
    let code = match error.kind {
        kiln_temporal::ErrorKind::Type => TYPE_ERROR,
        kiln_temporal::ErrorKind::Syntax => SYNTAX_ERROR,
        kiln_temporal::ErrorKind::Range => RANGE_ERROR,
    };
    LAST_ERROR.with(|slot| *slot.borrow_mut() = (code, error.message.clone()));
    code
}

/// Remember a TypeError this side raised, rather than one the engine did.
///
/// The engine's own constructors are private, and rightly so: the two cases this needs are
/// argument-arity failures that never reach it, and inventing an error *class* here would be
/// exactly the flattening `remember` exists to prevent. The class is the one the language
/// gives a method called with too few arguments.
fn reject_type(message: &str) -> i32 {
    LAST_ERROR.with(|slot| *slot.borrow_mut() = (TYPE_ERROR, message.to_owned()));
    TYPE_ERROR
}

/// An operation that needs a second `Temporal` value and did not get one.
fn missing_operand() -> i32 {
    reject_type("this Temporal operation needs another Temporal value")
}

/// An operation that needs a property bag or a string argument and did not get one.
fn missing_argument() -> i32 {
    reject_type("this Temporal operation needs an argument")
}

fn kind_of(code: u32) -> Option<Kind> {
    Some(match code {
        0 => Kind::PlainDate,
        1 => Kind::PlainTime,
        2 => Kind::PlainDateTime,
        3 => Kind::PlainYearMonth,
        4 => Kind::PlainMonthDay,
        5 => Kind::Duration,
        6 => Kind::Instant,
        7 => Kind::ZonedDateTime,
        _ => return None,
    })
}

fn code_of(kind: Kind) -> i32 {
    match kind {
        Kind::PlainDate => 0,
        Kind::PlainTime => 1,
        Kind::PlainDateTime => 2,
        Kind::PlainYearMonth => 3,
        Kind::PlainMonthDay => 4,
        Kind::Duration => 5,
        Kind::Instant => 6,
        Kind::ZonedDateTime => 7,
    }
}

/// A byte span, or `None` when the pointer contract is violated.
///
/// # Safety
///
/// A non-null `pointer` must be readable for `length` bytes.
unsafe fn span<'a>(pointer: *const u8, length: usize) -> Option<&'a [u8]> {
    if length == 0 {
        return Some(&[]);
    }
    if pointer.is_null() || length > isize::MAX as usize {
        return None;
    }
    // SAFETY: checked above and guaranteed by the caller contract.
    Some(unsafe { core::slice::from_raw_parts(pointer, length) })
}

/// An optional UTF-8 string argument: absent when the pointer is null.
///
/// # Safety
///
/// A non-null `pointer` must be readable for `length` bytes.
unsafe fn text<'a>(pointer: *const u8, length: usize) -> Result<Option<&'a str>, ()> {
    if pointer.is_null() {
        return Ok(None);
    }
    // SAFETY: the caller contract guarantees the span is readable for its length.
    let Some(bytes) = (unsafe { span(pointer, length) }) else {
        return Err(());
    };
    core::str::from_utf8(bytes).map(Some).map_err(|_| ())
}

/// Copy `value` into `buffer`, answering the length written, or the negative length it wanted.
///
/// The retry protocol the BigInt provider uses: one negative answer tells the caller exactly
/// how much to allocate, so a second call always succeeds.
///
/// # Safety
///
/// `buffer` must be writable for `capacity` bytes when `capacity` is non-zero.
unsafe fn emit(value: &str, buffer: *mut u8, capacity: usize) -> i32 {
    let bytes = value.as_bytes();
    let Ok(length) = i32::try_from(bytes.len()) else {
        return INVALID_ARGUMENT;
    };
    if bytes.len() > capacity || buffer.is_null() {
        return length.checked_neg().unwrap_or(INVALID_ARGUMENT);
    }
    // SAFETY: `buffer` is writable for `capacity >= bytes.len()` by the caller contract, and
    // the source is a distinct allocation owned by this side.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    length
}

/// Split the packed text buffer into its slots.
///
/// # Safety
///
/// `text` must be readable for the sum of the non-negative lengths, and `lengths` for `count`
/// values.
unsafe fn unpack<'a, const N: usize>(
    text: *const u8,
    lengths: *const i64,
    count: usize,
) -> Option<[Option<&'a str>; N]> {
    if count != N || lengths.is_null() {
        return None;
    }
    // SAFETY: the caller contract guarantees `lengths` is readable for `count` values.
    let lengths = unsafe { core::slice::from_raw_parts(lengths, count) };
    let mut out: [Option<&str>; N] = [None; N];
    let mut offset = 0usize;
    for (slot, &length) in lengths.iter().enumerate() {
        if length < 0 {
            continue;
        }
        let length = usize::try_from(length).ok()?;
        let start = text.wrapping_add(offset);
        // SAFETY: the caller contract guarantees the concatenation is readable for the sum of
        // the non-negative lengths, and `offset` never passes that sum.
        let bytes = unsafe { span(start, length) }?;
        *out.get_mut(slot)? = Some(core::str::from_utf8(bytes).ok()?);
        offset = offset.checked_add(length)?;
    }
    Some(out)
}

/// The calendar and wall-clock bag, out of the numeric slots and the packed strings.
///
/// Shared by [`kiln_temporal_from_fields`] and by `with`, which merges exactly the same bag
/// into an existing value -- one reader, so the two cannot come to disagree about which slot
/// carries `monthCode`.
fn field_bag(slots: &[f64], strings: &[Option<&str>; TEXT_SLOTS]) -> Fields {
    let present = |index: usize| slots.get(index).copied().filter(|value| !value.is_nan());
    let owned = |index: usize| strings.get(index).copied().flatten().map(str::to_owned);
    Fields {
        era: owned(0),
        era_year: present(0),
        year: present(1),
        month: present(2),
        month_code: owned(1),
        day: present(3),
        hour: present(4),
        minute: present(5),
        second: present(6),
        millisecond: present(7),
        microsecond: present(8),
        nanosecond: present(9),
        offset: owned(2),
        time_zone: owned(3),
        calendar: owned(4),
    }
}

/// The ten duration components out of their numeric slots.
fn duration_bag(slots: &[f64]) -> kiln_temporal::DurationFields {
    let present = |index: usize| slots.get(index).copied().filter(|value| !value.is_nan());
    kiln_temporal::DurationFields {
        years: present(0),
        months: present(1),
        weeks: present(2),
        days: present(3),
        hours: present(4),
        minutes: present(5),
        seconds: present(6),
        milliseconds: present(7),
        microseconds: present(8),
        nanoseconds: present(9),
    }
}

/// Build one of the eight types from a field bag the caller has already read.
///
/// The caller does the reading, and in the order the specification names: property reads are
/// observable, so a design that took the JavaScript object here would have moved observable
/// behaviour behind a boundary that cannot express it. What crosses is the result.
///
/// Returns 0 and writes the handle on success; -1 for a violated argument contract, and
/// [`RANGE_ERROR`], [`TYPE_ERROR`] or [`SYNTAX_ERROR`] for a rejection whose message
/// [`kiln_temporal_last_error`] then carries.
///
/// # Safety
///
/// `fields` must be readable for `field_count` doubles, `text` and `text_lengths` must satisfy
/// [`unpack`], and `out_handle` must be a writable slot. The handle belongs to the caller and
/// is released by [`kiln_temporal_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_from_fields(
    kind: u32,
    fields: *const f64,
    field_count: usize,
    text: *const u8,
    text_lengths: *const i64,
    text_count: usize,
    option_text: *const u8,
    option_text_lengths: *const i64,
    option_text_count: usize,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if out_handle.is_null() || fields.is_null() || field_count != FIELD_SLOTS {
        return INVALID_ARGUMENT;
    }
    let Some(kind) = kind_of(kind) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees `fields` is readable for `field_count` doubles.
    let slots = unsafe { core::slice::from_raw_parts(fields, field_count) };
    // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
    let Some(strings) = (unsafe { unpack::<TEXT_SLOTS>(text, text_lengths, text_count) }) else {
        return INVALID_ARGUMENT;
    };
    let bag = field_bag(slots, &strings);
    // SAFETY: the caller contract guarantees the packed options satisfy `unpack`.
    let Some(options) =
        (unsafe { option_strings(option_text, option_text_lengths, option_text_count) })
    else {
        return INVALID_ARGUMENT;
    };
    match kiln_temporal::from_fields(kind, &bag, &options) {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// `new Temporal.X(...)`: the *positional* constructors, which are not `from` with a bag.
///
/// The difference is not cosmetic and is the reason this is a separate entry point rather
/// than a flag on [`kiln_temporal_from_fields`]. A bag goes through the calendar: `from`
/// resolves `monthCode`, honours an `overflow` option and defaults it to `constrain`. A
/// constructor takes *ISO* components and rejects anything out of range --
/// `new Temporal.PlainDate(2000, 300, 1)` is a RangeError where
/// `Temporal.PlainDate.from({ year: 2000, month: 300, day: 1 })` is 2000-12-01. Routing one
/// through the other would have been a silent wrong answer in both directions, so each
/// reaches the engine function the interpreter already reaches: `kiln_temporal::plain_date`
/// and its seven neighbours, the same linked engine under ADR 0025.
///
/// `fields` carries the ten numeric slots in the [`kiln_temporal_from_fields`] layout, except
/// for a `Duration`, whose ten are the [`kiln_temporal_duration_from_fields`] layout -- one
/// array either way, because the caller already knows which of the eight it is building. NaN
/// is absent, which is what carries the two optional reference components: a
/// `PlainYearMonth`'s `referenceISODay` rides in the `day` slot and a `PlainMonthDay`'s
/// `referenceISOYear` in the `year` slot, and leaving either out is not the same as passing
/// zero. A missing *required* component stays NaN and the engine rejects it, which is the
/// same RangeError the language gives.
///
/// `epoch_high` and `epoch_low` are the signed top and unsigned bottom halves of the epoch
/// nanosecond count the two exact types take, split because C99 has no portable 128-bit
/// integer. They are ignored by the other six.
///
/// # Safety
///
/// `fields` must be readable for `field_count` doubles, `text` and `text_lengths` must
/// satisfy [`unpack`], and `out_handle` must be a writable slot. The handle belongs to the
/// caller and is released by [`kiln_temporal_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_construct(
    kind: u32,
    fields: *const f64,
    field_count: usize,
    text: *const u8,
    text_lengths: *const i64,
    text_count: usize,
    epoch_high: i64,
    epoch_low: u64,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if out_handle.is_null() || fields.is_null() || field_count != FIELD_SLOTS {
        return INVALID_ARGUMENT;
    }
    let Some(kind) = kind_of(kind) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees `fields` is readable for `field_count` doubles.
    let slots = unsafe { core::slice::from_raw_parts(fields, field_count) };
    // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
    let Some(strings) = (unsafe { unpack::<TEXT_SLOTS>(text, text_lengths, text_count) }) else {
        return INVALID_ARGUMENT;
    };
    let calendar = strings[4];
    let optional = |index: usize| {
        let value = slots[index];
        if value.is_nan() { None } else { Some(value) }
    };
    let epoch_nanoseconds = (i128::from(epoch_high) << 64) | i128::from(epoch_low);
    let built = match kind {
        Kind::PlainDate => kiln_temporal::plain_date(slots[1], slots[2], slots[3], calendar),
        Kind::PlainTime => {
            kiln_temporal::plain_time(slots[4], slots[5], slots[6], slots[7], slots[8], slots[9])
        }
        Kind::PlainDateTime => kiln_temporal::plain_date_time(
            slots[1], slots[2], slots[3], slots[4], slots[5], slots[6], slots[7], slots[8],
            slots[9], calendar,
        ),
        Kind::PlainYearMonth => {
            kiln_temporal::plain_year_month(slots[1], slots[2], optional(3), calendar)
        }
        Kind::PlainMonthDay => {
            kiln_temporal::plain_month_day(slots[2], slots[3], optional(1), calendar)
        }
        Kind::Duration => kiln_temporal::duration(&duration_bag(slots)),
        Kind::Instant => kiln_temporal::instant(epoch_nanoseconds),
        Kind::ZonedDateTime => match strings[3] {
            Some(zone) => kiln_temporal::zoned_date_time(epoch_nanoseconds, zone, calendar),
            // The one argument of the eight constructors that is neither numeric nor
            // optional. Rejected here rather than defaulted to UTC, which would build a
            // value the program did not ask for.
            None => {
                return reject_type("new Temporal.ZonedDateTime needs a time zone");
            }
        },
    };
    match built {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// Build a `Temporal.Duration` from its ten components, NaN for a component left out.
///
/// Separate from [`kiln_temporal_from_fields`] because the two bags are separate in the
/// language: a duration has weeks and no month code, a date has a month code and no weeks.
///
/// # Safety
///
/// `fields` must be readable for `field_count` doubles and `out_handle` a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_duration_from_fields(
    fields: *const f64,
    field_count: usize,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if out_handle.is_null() || fields.is_null() || field_count != DURATION_SLOTS {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees `fields` is readable for `field_count` doubles.
    let slots = unsafe { core::slice::from_raw_parts(fields, field_count) };
    let bag = duration_bag(slots);
    match kiln_temporal::duration_from_fields(&bag) {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// Parse one of the eight types out of its ISO text.
///
/// # Safety
///
/// `bytes` must be readable for `len`, and `out_handle` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_parse(
    kind: u32,
    bytes: *const u8,
    len: usize,
    option_text: *const u8,
    option_text_lengths: *const i64,
    option_text_count: usize,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if out_handle.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(kind) = kind_of(kind) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the span is readable for its length.
    let Ok(Some(source)) = (unsafe { text(bytes, len) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the packed options satisfy `unpack`.
    let Some(options) =
        (unsafe { option_strings(option_text, option_text_lengths, option_text_count) })
    else {
        return INVALID_ARGUMENT;
    };
    // A `ZonedDateTime` string is the one `from` that has to decide what a wall-clock time
    // which does not exist, or exists twice, means -- so it reads the options the rest ignore.
    let parsed = if matches!(kind, Kind::ZonedDateTime) {
        kiln_temporal::parse_zoned_date_time(source, &options)
    } else {
        kiln_temporal::parse(kind, source)
    };
    match parsed {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// `Temporal.Now`, off the instant the realm supplies rather than any clock reachable here.
///
/// The instant arrives split in two because C99 has no portable 128-bit integer: `high` is the
/// signed top half and `low` the unsigned bottom half of the epoch nanosecond count. ADR 0015
/// makes that count the realm's virtual clock, which starts at the epoch.
///
/// `zone` is the optional argument `Temporal.Now.zonedDateTimeISO(zone)` takes; null means the
/// realm's own, which ADR 0015 fixes at UTC.
///
/// # Safety
///
/// `out_handle` must be a writable slot, and a non-null `zone` readable for `zone_len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_now(
    kind: u32,
    high: i64,
    low: u64,
    zone: *const u8,
    zone_len: usize,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if out_handle.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(kind) = kind_of(kind) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the span is readable for its length.
    let Ok(zone) = (unsafe { text(zone, zone_len) }) else {
        return INVALID_ARGUMENT;
    };
    let epoch_nanoseconds = (i128::from(high) << 64) | i128::from(low);
    match kiln_temporal::now::read(kind, epoch_nanoseconds, zone) {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// `Temporal.Instant.fromEpochMilliseconds(ms)`.
///
/// A separate entry point from [`kiln_temporal_from_fields`] because it is not a field bag:
/// the argument is one number counted from the epoch, and the engine rejects a fractional or
/// out-of-range one with the RangeError the language specifies rather than truncating it.
///
/// # Safety
///
/// `out_handle` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_instant_from_epoch_milliseconds(
    milliseconds: f64,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if out_handle.is_null() {
        return INVALID_ARGUMENT;
    }
    match kiln_temporal::instant_from_epoch_milliseconds(milliseconds) {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// `Temporal.Instant.fromEpochNanoseconds(ns)`, where the argument is a `BigInt`.
///
/// It arrives split the way [`kiln_temporal_now`]'s instant does, for the same reason: C99 has
/// no portable 128-bit integer. The caller reads the two halves out of the BigInt provider,
/// which is the only other place in this archive that holds a number this wide -- and the two
/// providers stay independent features because nothing but scalars passes between them.
///
/// # Safety
///
/// `out_handle` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_instant_from_epoch_nanoseconds(
    high: i64,
    low: u64,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if out_handle.is_null() {
        return INVALID_ARGUMENT;
    }
    let epoch_nanoseconds = (i128::from(high) << 64) | i128::from(low);
    match kiln_temporal::instant(epoch_nanoseconds) {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// Release a handle from any of the constructors.
///
/// # Safety
///
/// `handle` must have come from this module and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_free(handle: *mut TemporalValue) {
    if handle.is_null() {
        return;
    }
    // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw` above.
    drop(unsafe { Box::from_raw(handle) });
}

/// Which of the eight types `handle` is, as the code [`kiln_temporal_from_fields`] takes.
///
/// # Safety
///
/// `handle` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_kind(handle: *const TemporalValue) -> i32 {
    if handle.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    code_of(unsafe { &*handle }.kind())
}

/// One property, by the name the language spells it with.
///
/// This is the call that makes the whole surface cheap: `kiln-temporal` answers every field of
/// all eight types through one name-keyed reader, so the C side gets `year`, `daysInMonth` and
/// `blank` alike without a line of per-field code.
///
/// The answer says which shape it is: 0 no such property, 1 a number in `out_number`, 2 text
/// written to `buffer` under the [`emit`] retry protocol, 3 a flag in `out_number`, 4 the
/// property exists and is `undefined`, and 5 a value only a BigInt can hold -- `epochNanoseconds`
/// -- whose exact decimal digits are written to `buffer` under the same retry protocol as
/// shape 2, for the caller to hand to `kiln_bigint_parse`.
///
/// Shape 5 travels as digits rather than as a BigInt handle on purpose: the two providers are
/// independent features of one archive, and a nanosecond count is at most forty characters, so
/// the decimal spelling is exact, is the spelling the language would print anyway, and costs
/// this side no knowledge of what a BigInt is. Rounding it through the `f64` slot instead is
/// the one thing that must never happen: a `Temporal` instant carries more precision than a
/// double holds, and a rounded answer is a wrong answer that prints.
///
/// # Safety
///
/// `handle` must be live, `name` readable for `name_len`, `out_number` a writable slot, and
/// `buffer` writable for `capacity` bytes when `capacity` is non-zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_get(
    handle: *const TemporalValue,
    name: *const u8,
    name_len: usize,
    out_number: *mut f64,
    buffer: *mut u8,
    capacity: usize,
    out_length: *mut i32,
) -> i32 {
    if handle.is_null() || out_number.is_null() || out_length.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the span is readable for its length.
    let Ok(Some(name)) = (unsafe { text(name, name_len) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the handle is live.
    let Some(field) = unsafe { &*handle }.get(name) else {
        return 0;
    };
    match field {
        kiln_temporal::Field::Number(number) => {
            // SAFETY: `out_number` is a writable slot by the caller contract.
            unsafe { out_number.write(number) };
            1
        }
        kiln_temporal::Field::Text(value) => {
            // SAFETY: `buffer` is writable for `capacity` by the caller contract.
            let written = unsafe { emit(&value, buffer, capacity) };
            // SAFETY: `out_length` is a writable slot by the caller contract.
            unsafe { out_length.write(written) };
            2
        }
        kiln_temporal::Field::Flag(flag) => {
            // SAFETY: `out_number` is a writable slot by the caller contract.
            unsafe { out_number.write(if flag { 1.0 } else { 0.0 }) };
            3
        }
        kiln_temporal::Field::Undefined => 4,
        kiln_temporal::Field::Integer128(integer) => {
            // SAFETY: `buffer` is writable for `capacity` by the caller contract.
            let written = unsafe { emit(&integer.to_string(), buffer, capacity) };
            // SAFETY: `out_length` is a writable slot by the caller contract.
            unsafe { out_length.write(written) };
            5
        }
    }
}

/// The value's own ISO text, which is what `toString` and `toJSON` both answer.
///
/// # Safety
///
/// `handle` must be live and `buffer` writable for `capacity` bytes when `capacity` is
/// non-zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_to_string(
    handle: *const TemporalValue,
    buffer: *mut u8,
    capacity: usize,
) -> i32 {
    if handle.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    match unsafe { &*handle }.to_temporal_string(&Options::default()) {
        // SAFETY: `buffer` is writable for `capacity` by the caller contract.
        Ok(text) => unsafe { emit(&text, buffer, capacity) },
        Err(error) => remember(&error),
    }
}

/// Order two values of the same type: -1, 0 or 1, as `compare` answers.
///
/// Two durations are the exception, and they take the options: whether one month is longer
/// than thirty days depends on *which* month, so `Temporal.Duration.compare` needs a
/// `relativeTo` and refuses without one rather than picking an average month.
///
/// # Safety
///
/// Both handles must be live, a non-null `relative_to` must be live, the packed options must
/// satisfy [`unpack`], and `out_order` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_compare(
    left: *const TemporalValue,
    right: *const TemporalValue,
    relative_to: *const TemporalValue,
    option_text: *const u8,
    option_text_lengths: *const i64,
    option_text_count: usize,
    out_order: *mut i32,
) -> i32 {
    if left.is_null() || right.is_null() || out_order.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees both handles are live.
    let (left, right) = unsafe { (&*left, &*right) };
    let ordered = if left.kind() == Kind::Duration && right.kind() == Kind::Duration {
        // SAFETY: the caller contract guarantees the packed options satisfy `unpack`.
        let Some(mut options) =
            (unsafe { option_strings(option_text, option_text_lengths, option_text_count) })
        else {
            return INVALID_ARGUMENT;
        };
        if !relative_to.is_null() {
            // SAFETY: the caller contract guarantees a non-null handle is live.
            options.relative_to = Some(unsafe { &*relative_to }.clone());
        }
        kiln_temporal::compare_durations(left, right, &options)
    } else {
        kiln_temporal::compare(left, right)
    };
    match ordered {
        Ok(order) => {
            // SAFETY: `out_order` is a writable slot by the caller contract.
            unsafe { out_order.write(order) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// The option-string slots, in the order the language reads them: calendarName,
/// disambiguation, largestUnit, offset, overflow, roundingMode, smallestUnit, timeZoneName,
/// unit.
///
/// The reads themselves happen on the caller's side, because a getter on an options bag runs
/// and the order it runs in is observable. What crosses is the strings that came back.
const OPTION_TEXT_SLOTS: usize = 9;

/// `[roundingIncrement, fractionalSecondDigits]`. NaN is absent; a negative digit count is the
/// `"auto"` spelling, which is not the same as absent.
const OPTION_NUMBER_SLOTS: usize = 2;

/// Everything one operation needs, in one struct.
///
/// A struct rather than twenty parameters: the arithmetic and conversion surface is
/// name-keyed, so a single entry point serves all of it, and the alternative shape is a C
/// signature nobody can call correctly twice.
#[repr(C)]
pub struct TemporalCall {
    /// The value the method was reached through.
    pub receiver: *const TemporalValue,
    /// The operation, by the name the language spells it with.
    pub name: *const u8,
    pub name_len: usize,
    /// The first argument when it is another `Temporal` value -- `add`'s duration, `until`'s
    /// endpoint, `withPlainTime`'s time. Null when the operation takes none.
    pub operand: *const TemporalValue,
    /// `relativeTo`, which decides whether a day is twenty-four hours.
    pub relative_to: *const TemporalValue,
    /// The first argument when it is text -- `withCalendar`'s calendar, `withTimeZone`'s zone.
    pub argument: *const u8,
    pub argument_len: usize,
    /// A field bag in the [`kiln_temporal_from_fields`] layout, or null for none. `with`
    /// merges it; `toPlainDate` on a year-month completes with it.
    pub fields: *const f64,
    pub field_count: usize,
    pub field_text: *const u8,
    pub field_text_lengths: *const i64,
    pub field_text_count: usize,
    /// The option numbers, [`OPTION_NUMBER_SLOTS`] of them, or null for none.
    pub option_numbers: *const f64,
    pub option_number_count: usize,
    /// The option strings, [`OPTION_TEXT_SLOTS`] of them packed as [`unpack`] takes them, or
    /// null for none.
    pub option_text: *const u8,
    pub option_text_lengths: *const i64,
    pub option_text_count: usize,
    /// Where a new value lands, when the answer is one.
    pub out_handle: *mut *mut TemporalValue,
    /// Where a number or a flag lands.
    pub out_number: *mut f64,
    /// Where text lands, under the [`emit`] retry protocol.
    pub out_buffer: *mut u8,
    pub out_capacity: usize,
    pub out_length: *mut i32,
}

/// The answer is a new value in `out_handle`.
const ANSWER_HANDLE: i32 = 0;
/// The answer is a number in `out_number`.
const ANSWER_NUMBER: i32 = 1;
/// The answer is text, written under the [`emit`] protocol with its length in `out_length`.
const ANSWER_TEXT: i32 = 2;
/// The answer is a flag in `out_number`.
const ANSWER_FLAG: i32 = 3;
/// This type has no such operation, and the caller refuses by name rather than answering.
const ANSWER_UNKNOWN: i32 = 6;

/// The nine option strings, out of the packed buffer the caller supplies.
///
/// # Safety
///
/// `text` and `lengths` must satisfy [`unpack`] for `count` slots.
unsafe fn option_strings(text: *const u8, lengths: *const i64, count: usize) -> Option<Options> {
    let mut options = Options::default();
    if text.is_null() && count == 0 {
        return Some(options);
    }
    // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
    let strings = unsafe { unpack::<OPTION_TEXT_SLOTS>(text, lengths, count) }?;
    let owned = |index: usize| strings[index].map(str::to_owned);
    options.calendar_name = owned(0);
    options.disambiguation = owned(1);
    options.largest_unit = owned(2);
    // `offset` on an options bag is the conflict policy, not the field of the same name.
    options.offset_conflict = owned(3);
    options.overflow = owned(4);
    options.rounding_mode = owned(5);
    options.smallest_unit = owned(6);
    options.time_zone_name = owned(7);
    options.unit = owned(8);
    Some(options)
}

/// The whole option bag a call carries: the strings, the two numbers, and `relativeTo`.
///
/// # Safety
///
/// The call's option pointers must satisfy the contract [`kiln_temporal_invoke`] states.
unsafe fn options_of(call: &TemporalCall) -> Option<Options> {
    // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
    let mut options = unsafe {
        option_strings(
            call.option_text,
            call.option_text_lengths,
            call.option_text_count,
        )
    }?;
    if !call.option_numbers.is_null() {
        if call.option_number_count != OPTION_NUMBER_SLOTS {
            return None;
        }
        // SAFETY: the caller contract guarantees the array is readable for its count.
        let numbers =
            unsafe { core::slice::from_raw_parts(call.option_numbers, OPTION_NUMBER_SLOTS) };
        if !numbers[0].is_nan() {
            options.rounding_increment = Some(numbers[0]);
        }
        if !numbers[1].is_nan() {
            options.fractional_second_digits = Some(if numbers[1] < 0.0 {
                None
            } else {
                Some(numbers[1].trunc() as u8)
            });
        }
    }
    if !call.relative_to.is_null() {
        // SAFETY: the caller contract guarantees a non-null handle is live.
        options.relative_to = Some(unsafe { &*call.relative_to }.clone());
    }
    Some(options)
}

/// The field bag the call carries, or `None` when it carries none.
///
/// # Safety
///
/// The call's field pointers must satisfy the contract [`kiln_temporal_invoke`] states.
unsafe fn call_fields(call: &TemporalCall) -> Option<Option<Fields>> {
    if call.fields.is_null() {
        return Some(None);
    }
    if call.field_count != FIELD_SLOTS {
        return None;
    }
    // SAFETY: the caller contract guarantees the array is readable for its count.
    let slots = unsafe { core::slice::from_raw_parts(call.fields, FIELD_SLOTS) };
    // SAFETY: the caller contract guarantees the packed text satisfies `unpack`.
    let strings = unsafe {
        unpack::<TEXT_SLOTS>(
            call.field_text,
            call.field_text_lengths,
            call.field_text_count,
        )
    }?;
    Some(Some(field_bag(slots, &strings)))
}

/// The wall-clock half of a `PlainTime`, as the bag `toPlainDateTime` supplements with.
///
/// Read through the same name-keyed reader the caller uses, so there is no second copy of the
/// component list to drift from the first.
fn time_supplement(time: &TemporalValue) -> Fields {
    let mut fields = Fields::default();
    let read = |name: &str| match time.get(name) {
        Some(kiln_temporal::Field::Number(number)) => Some(number),
        _ => None,
    };
    fields.hour = read("hour");
    fields.minute = read("minute");
    fields.second = read("second");
    fields.millisecond = read("millisecond");
    fields.microsecond = read("microsecond");
    fields.nanosecond = read("nanosecond");
    fields
}

/// One arithmetic or conversion operation, by the name the language spells it with.
///
/// This is the other half of what [`kiln_temporal_get`] does for the readers: `kiln-temporal`
/// already exposes the whole surface behind names and scalars, so one entry point carries
/// `add`, `until`, `round`, `with`, `equals`, `total` and the eleven conversions without a
/// line of per-method code on either side of the boundary.
///
/// Answers [`ANSWER_HANDLE`], [`ANSWER_NUMBER`], [`ANSWER_TEXT`], [`ANSWER_FLAG`] or
/// [`ANSWER_UNKNOWN`] -- the last of which is not a failure, but the signal that this type
/// does not have this operation and the caller should refuse by name. A negative answer is
/// [`INVALID_ARGUMENT`] or an error class whose message [`kiln_temporal_last_error`] carries.
///
/// # Safety
///
/// `call` must point at a live [`TemporalCall`] whose `receiver` is live, whose `name` is
/// readable for `name_len`, whose non-null handles are live, whose non-null `argument` is
/// readable for `argument_len`, whose non-null `fields`/`option_numbers` are readable for
/// their counts, whose packed option and field text satisfies [`unpack`], and whose `out_*`
/// slots are writable. A handle written to `out_handle` belongs to the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_invoke(call: *const TemporalCall) -> i32 {
    if call.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the struct is live.
    let call = unsafe { &*call };
    if call.receiver.is_null()
        || call.out_handle.is_null()
        || call.out_number.is_null()
        || call.out_length.is_null()
    {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the span is readable for its length.
    let Ok(Some(name)) = (unsafe { text(call.name, call.name_len) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the handle is live.
    let receiver = unsafe { &*call.receiver };
    // SAFETY: the caller contract guarantees a non-null handle is live.
    let operand = if call.operand.is_null() {
        None
    } else {
        Some(unsafe { &*call.operand })
    };
    // SAFETY: the caller contract guarantees the span is readable for its length.
    let Ok(argument) = (unsafe { text(call.argument, call.argument_len) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the option pointers satisfy `options_of`.
    let Some(options) = (unsafe { options_of(call) }) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the field pointers satisfy `call_fields`.
    let Some(fields) = (unsafe { call_fields(call) }) else {
        return INVALID_ARGUMENT;
    };

    let produced: Result<TemporalValue, kiln_temporal::Error> = match name {
        "add" | "subtract" => match operand {
            None => return missing_operand(),
            Some(operand) if name == "add" => receiver.add(operand, &options),
            Some(operand) => receiver.subtract(operand, &options),
        },
        "until" | "since" => match operand {
            None => return missing_operand(),
            Some(operand) if name == "until" => receiver.until(operand, &options),
            Some(operand) => receiver.since(operand, &options),
        },
        "round" => receiver.round(&options),
        "with" => {
            let Some(bag) = fields else {
                return missing_argument();
            };
            if receiver.kind() == Kind::Duration {
                // A duration's bag is the *other* bag -- weeks and no month code -- and the
                // caller packs it into the same numeric slots
                // `kiln_temporal_duration_from_fields` reads.
                // SAFETY: `call_fields` established the array is non-null and readable for
                // `FIELD_SLOTS`, which is `DURATION_SLOTS`.
                let slots = unsafe { core::slice::from_raw_parts(call.fields, FIELD_SLOTS) };
                receiver.duration_with(&duration_bag(slots))
            } else {
                receiver.with(&bag, &options)
            }
        }
        "negated" => receiver.negated(),
        "abs" => receiver.abs(),
        "startOfDay" => receiver.start_of_day(),
        "withCalendar" => match argument {
            None => return missing_argument(),
            Some(calendar) => receiver.with_calendar(calendar),
        },
        "withTimeZone" => match argument {
            None => return missing_argument(),
            Some(zone) => receiver.with_time_zone(zone),
        },
        "withPlainTime" => receiver.with_plain_time(operand),
        "toZonedDateTime" | "toZonedDateTimeISO" => match argument {
            None => return missing_argument(),
            Some(zone) => receiver.to_zoned_date_time(zone, operand, &options),
        },
        // `toPlainDate` means two different things: on a date-and-time it drops the time, and
        // on a year-month or a month-day it supplies the component that was missing.
        "toPlainDate" if matches!(receiver.kind(), Kind::PlainYearMonth | Kind::PlainMonthDay) => {
            match fields {
                None => return missing_argument(),
                Some(fields) => receiver.to_plain_date_with(&fields),
            }
        }
        "toPlainDate" => receiver.to_kind(Kind::PlainDate, None),
        "toPlainTime" => receiver.to_kind(Kind::PlainTime, None),
        "toPlainDateTime" => {
            let supplement = match (operand, fields) {
                (Some(time), _) if time.kind() == Kind::PlainTime => Some(time_supplement(time)),
                (_, Some(fields)) => Some(fields),
                _ => None,
            };
            receiver.to_kind(Kind::PlainDateTime, supplement.as_ref())
        }
        "toPlainYearMonth" => receiver.to_kind(Kind::PlainYearMonth, None),
        "toPlainMonthDay" => receiver.to_kind(Kind::PlainMonthDay, None),
        "toInstant" => receiver.to_kind(Kind::Instant, None),
        "total" => {
            return match receiver.total(&options) {
                Ok(number) => {
                    // SAFETY: `out_number` is a writable slot by the caller contract.
                    unsafe { call.out_number.write(number) };
                    ANSWER_NUMBER
                }
                Err(error) => remember(&error),
            };
        }
        "equals" => {
            let Some(operand) = operand else {
                return missing_operand();
            };
            return match receiver.equals(operand) {
                Ok(flag) => {
                    // SAFETY: `out_number` is a writable slot by the caller contract.
                    unsafe { call.out_number.write(if flag { 1.0 } else { 0.0 }) };
                    ANSWER_FLAG
                }
                Err(error) => remember(&error),
            };
        }
        "toString" => {
            return match receiver.to_temporal_string(&options) {
                Ok(value) => {
                    // SAFETY: `out_buffer` is writable for `out_capacity` by the contract.
                    let written = unsafe { emit(&value, call.out_buffer, call.out_capacity) };
                    // SAFETY: `out_length` is a writable slot by the caller contract.
                    unsafe { call.out_length.write(written) };
                    ANSWER_TEXT
                }
                Err(error) => remember(&error),
            };
        }
        _ => return ANSWER_UNKNOWN,
    };
    match produced {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { call.out_handle.write(Box::into_raw(Box::new(value))) };
            ANSWER_HANDLE
        }
        Err(error) => remember(&error),
    }
}

/// `ToTemporalDate` and its siblings: narrow a value to another of the eight types.
///
/// Not a method -- it is the abstract operation `Temporal.PlainDate.from(zonedDateTime)` runs,
/// which is why it is its own entry point rather than a name through [`kiln_temporal_invoke`].
/// A pair the language does not define is a TypeError, which the engine raises rather than
/// this side guessing at the table.
///
/// # Safety
///
/// `handle` must be live and `out_handle` a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_to_kind(
    handle: *const TemporalValue,
    kind: u32,
    out_handle: *mut *mut TemporalValue,
) -> i32 {
    if handle.is_null() || out_handle.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(kind) = kind_of(kind) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees the handle is live.
    match unsafe { &*handle }.to_kind(kind, None) {
        Ok(value) => {
            // SAFETY: `out_handle` is a writable slot by the caller contract.
            unsafe { out_handle.write(Box::into_raw(Box::new(value))) };
            0
        }
        Err(error) => remember(&error),
    }
}

/// The message and class of the last failure on this thread.
///
/// Writes the class to `out_kind` -- [`RANGE_ERROR`] or [`TYPE_ERROR`] -- and the message under
/// the [`emit`] retry protocol. `Temporal` distinguishes the two classes on nearly every path,
/// so the caller raises the one the engine chose rather than guessing.
///
/// # Safety
///
/// `out_kind` must be a writable slot and `buffer` writable for `capacity` bytes when
/// `capacity` is non-zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_temporal_last_error(
    out_kind: *mut i32,
    buffer: *mut u8,
    capacity: usize,
) -> i32 {
    if out_kind.is_null() {
        return INVALID_ARGUMENT;
    }
    LAST_ERROR.with(|slot| {
        let (code, message) = &*slot.borrow();
        // SAFETY: `out_kind` is a writable slot by the caller contract.
        unsafe { out_kind.write(*code) };
        // SAFETY: `buffer` is writable for `capacity` by the caller contract.
        unsafe { emit(message, buffer, capacity) }
    })
}

macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (temporal, kiln_temporal_from_fields, temporal_from_fields) => {
            const _: unsafe extern "C" fn(u32, *const f64, usize, *const u8, *const i64, usize, *const u8, *const i64, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_from_fields;
        };
        (temporal, kiln_temporal_construct, temporal_construct) => {
            const _: unsafe extern "C" fn(u32, *const f64, usize, *const u8, *const i64, usize, i64, u64, *mut *mut TemporalValue) -> i32 = kiln_temporal_construct;
        };
        (temporal, kiln_temporal_duration_from_fields, temporal_duration_from_fields) => {
            const _: unsafe extern "C" fn(*const f64, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_duration_from_fields;
        };
        (temporal, kiln_temporal_parse, temporal_parse) => {
            const _: unsafe extern "C" fn(u32, *const u8, usize, *const u8, *const i64, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_parse;
        };
        (temporal, kiln_temporal_now, temporal_now) => {
            const _: unsafe extern "C" fn(u32, i64, u64, *const u8, usize, *mut *mut TemporalValue) -> i32 = kiln_temporal_now;
        };
        (temporal, kiln_temporal_instant_from_epoch_milliseconds, temporal_instant_ms) => {
            const _: unsafe extern "C" fn(f64, *mut *mut TemporalValue) -> i32 = kiln_temporal_instant_from_epoch_milliseconds;
        };
        (temporal, kiln_temporal_instant_from_epoch_nanoseconds, temporal_instant_ns) => {
            const _: unsafe extern "C" fn(i64, u64, *mut *mut TemporalValue) -> i32 = kiln_temporal_instant_from_epoch_nanoseconds;
        };
        (temporal, kiln_temporal_free, temporal_free) => {
            const _: unsafe extern "C" fn(*mut TemporalValue) = kiln_temporal_free;
        };
        (temporal, kiln_temporal_kind, temporal_kind) => {
            const _: unsafe extern "C" fn(*const TemporalValue) -> i32 = kiln_temporal_kind;
        };
        (temporal, kiln_temporal_get, temporal_get) => {
            const _: unsafe extern "C" fn(*const TemporalValue, *const u8, usize, *mut f64, *mut u8, usize, *mut i32) -> i32 = kiln_temporal_get;
        };
        (temporal, kiln_temporal_to_string, temporal_to_string) => {
            const _: unsafe extern "C" fn(*const TemporalValue, *mut u8, usize) -> i32 = kiln_temporal_to_string;
        };
        (temporal, kiln_temporal_compare, temporal_compare) => {
            const _: unsafe extern "C" fn(*const TemporalValue, *const TemporalValue, *const TemporalValue, *const u8, *const i64, usize, *mut i32) -> i32 = kiln_temporal_compare;
        };
        (temporal, kiln_temporal_invoke, temporal_invoke) => {
            const _: unsafe extern "C" fn(*const TemporalCall) -> i32 = kiln_temporal_invoke;
        };
        (temporal, kiln_temporal_to_kind, temporal_to_kind) => {
            const _: unsafe extern "C" fn(*const TemporalValue, u32, *mut *mut TemporalValue) -> i32 = kiln_temporal_to_kind;
        };
        (temporal, kiln_temporal_last_error, temporal_last_error) => {
            const _: unsafe extern "C" fn(*mut i32, *mut u8, usize) -> i32 = kiln_temporal_last_error;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

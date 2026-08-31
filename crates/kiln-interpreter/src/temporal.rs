//! `Temporal`, wired to the linked calendar engine.
//!
//! This is glue, not semantics. Every calendar question -- how long a month is, what a duration
//! rounds to, which wall-clock times a zone skips -- is answered by `kiln-temporal`, which answers
//! it with `temporal_rs` (ADR 0025's linked-engine rule, named for this feature by ADR 0029 §4).
//! What lives here is the part that is genuinely the language's: which property reads are
//! observable and in what order, which failures are `TypeError` and which are `RangeError`, and
//! how a JavaScript value becomes a `Temporal` one.
//!
//! It is a separate module from `lib.rs` for the reason `kiln-native` split `number_provider.rs`
//! out: the surface that touches the engine is self-contained, and the interpreter's own file is
//! long enough that adding a ninth built-in family to it inline would bury the machine.

use std::rc::Rc;

use kiln_host_abi::HostOps;
use kiln_temporal as engine;

use crate::{ErrorKind, ExecutionError, Native, Value, Vm};

/// What a `Native::Temporal` stands for.
///
/// One `Native` variant with a payload rather than ninety flat ones: `Temporal` has eight types
/// answering to almost the same set of methods, so the receiver decides what a method *means* and
/// the variant only has to say which method it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// The `Temporal` namespace itself.
    Namespace,
    /// The `Temporal.Now` namespace.
    Now,
    /// `Temporal.Now.instant()` and its four siblings, which differ only in what they produce.
    NowRead(engine::Kind),
    /// `Temporal.Now.timeZoneId()`.
    NowTimeZoneId,
    Constructor(engine::Kind),
    /// `Temporal.PlainDate.prototype` and its seven siblings.
    Prototype(engine::Kind),
    From(engine::Kind),
    Compare(engine::Kind),
    /// `Temporal.PlainDate.from`'s counterpart for a `PlainMonthDay`/`PlainYearMonth` day slot.
    Method(Method),
}

/// A prototype method, dispatched on the receiver rather than on the constructor it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    With,
    Add,
    Subtract,
    Until,
    Since,
    Round,
    Equals,
    ToString,
    ToJson,
    ToLocaleString,
    ValueOf,
    Total,
    Negated,
    Abs,
    WithCalendar,
    WithTimeZone,
    WithPlainTime,
    StartOfDay,
    ToPlainDate,
    ToPlainTime,
    ToPlainDateTime,
    ToPlainYearMonth,
    ToPlainMonthDay,
    ToZonedDateTime,
    ToInstant,
}

/// The name a `Temporal` built-in reports through `.name`.
pub fn js_name(op: Op) -> String {
    match op {
        Op::Namespace => "Temporal".to_owned(),
        Op::Now => "Now".to_owned(),
        Op::NowTimeZoneId => "timeZoneId".to_owned(),
        Op::NowRead(kind) => match kind {
            engine::Kind::Instant => "instant",
            engine::Kind::PlainDate => "plainDateISO",
            engine::Kind::PlainTime => "plainTimeISO",
            engine::Kind::PlainDateTime => "plainDateTimeISO",
            _ => "zonedDateTimeISO",
        }
        .to_owned(),
        Op::Constructor(kind) | Op::Prototype(kind) => short_name(kind).to_owned(),
        Op::From(_) => "from".to_owned(),
        Op::Compare(_) => "compare".to_owned(),
        Op::Method(method) => method_name(method).to_owned(),
    }
}

/// The namespace spelling, for the two `Temporal` values that are namespaces rather than functions.
pub const fn namespace_name(op: Op) -> Option<&'static str> {
    match op {
        Op::Namespace => Some("Temporal"),
        Op::Now => Some("Temporal.Now"),
        _ => None,
    }
}

const fn short_name(kind: engine::Kind) -> &'static str {
    match kind {
        engine::Kind::PlainDate => "PlainDate",
        engine::Kind::PlainTime => "PlainTime",
        engine::Kind::PlainDateTime => "PlainDateTime",
        engine::Kind::PlainYearMonth => "PlainYearMonth",
        engine::Kind::PlainMonthDay => "PlainMonthDay",
        engine::Kind::Duration => "Duration",
        engine::Kind::Instant => "Instant",
        engine::Kind::ZonedDateTime => "ZonedDateTime",
    }
}

const fn method_name(method: Method) -> &'static str {
    match method {
        Method::With => "with",
        Method::Add => "add",
        Method::Subtract => "subtract",
        Method::Until => "until",
        Method::Since => "since",
        Method::Round => "round",
        Method::Equals => "equals",
        Method::ToString => "toString",
        Method::ToJson => "toJSON",
        Method::ToLocaleString => "toLocaleString",
        Method::ValueOf => "valueOf",
        Method::Total => "total",
        Method::Negated => "negated",
        Method::Abs => "abs",
        Method::WithCalendar => "withCalendar",
        Method::WithTimeZone => "withTimeZone",
        Method::WithPlainTime => "withPlainTime",
        Method::StartOfDay => "startOfDay",
        Method::ToPlainDate => "toPlainDate",
        Method::ToPlainTime => "toPlainTime",
        Method::ToPlainDateTime => "toPlainDateTime",
        Method::ToPlainYearMonth => "toPlainYearMonth",
        Method::ToPlainMonthDay => "toPlainMonthDay",
        Method::ToZonedDateTime => "toZonedDateTime",
        Method::ToInstant => "toInstant",
    }
}

/// `Temporal.X` and `Temporal.X.from`: what a *static* read off one of these answers.
pub fn static_property(op: Op, name: &str) -> Option<Value> {
    Some(match (op, name) {
        (Op::Namespace, "Now") => Value::Native(Native::Temporal(Op::Now)),
        (Op::Namespace, _) => Value::Native(Native::Temporal(Op::Constructor(named_kind(name)?))),
        (Op::Now, "instant") => Value::Native(Native::Temporal(Op::NowRead(engine::Kind::Instant))),
        (Op::Now, "plainDateISO") => {
            Value::Native(Native::Temporal(Op::NowRead(engine::Kind::PlainDate)))
        }
        (Op::Now, "plainTimeISO") => {
            Value::Native(Native::Temporal(Op::NowRead(engine::Kind::PlainTime)))
        }
        (Op::Now, "plainDateTimeISO") => {
            Value::Native(Native::Temporal(Op::NowRead(engine::Kind::PlainDateTime)))
        }
        (Op::Now, "zonedDateTimeISO") => {
            Value::Native(Native::Temporal(Op::NowRead(engine::Kind::ZonedDateTime)))
        }
        (Op::Now, "timeZoneId") => Value::Native(Native::Temporal(Op::NowTimeZoneId)),
        (Op::Constructor(kind), "prototype") => {
            Value::Native(Native::Temporal(Op::Prototype(kind)))
        }
        (Op::Constructor(kind), "from") => Value::Native(Native::Temporal(Op::From(kind))),
        // `Temporal.PlainMonthDay` has no ordering: two month-days in different calendars name no
        // point on any timeline, so the language does not offer a `compare` to get wrong.
        (Op::Constructor(kind), "compare") if kind != engine::Kind::PlainMonthDay => {
            Value::Native(Native::Temporal(Op::Compare(kind)))
        }
        (Op::Constructor(engine::Kind::Instant), "fromEpochMilliseconds") => {
            Value::Native(Native::Temporal(Op::Method(Method::ToInstant)))
        }
        (Op::Prototype(kind), "constructor") => {
            Value::Native(Native::Temporal(Op::Constructor(kind)))
        }
        // The tag lives on the prototype, which is where an instance reads it from and where a
        // program that borrows `Object.prototype.toString` finds it.
        (Op::Prototype(kind), name) if name == crate::WellKnown::ToStringTag.property_key() => {
            Value::String(kind.tag().to_owned())
        }
        (Op::Prototype(kind), _) => {
            Value::Native(Native::Temporal(Op::Method(prototype_method(kind, name)?)))
        }
        _ => return None,
    })
}

fn named_kind(name: &str) -> Option<engine::Kind> {
    Some(match name {
        "PlainDate" => engine::Kind::PlainDate,
        "PlainTime" => engine::Kind::PlainTime,
        "PlainDateTime" => engine::Kind::PlainDateTime,
        "PlainYearMonth" => engine::Kind::PlainYearMonth,
        "PlainMonthDay" => engine::Kind::PlainMonthDay,
        "Duration" => engine::Kind::Duration,
        "Instant" => engine::Kind::Instant,
        "ZonedDateTime" => engine::Kind::ZonedDateTime,
        _ => return None,
    })
}

/// Reading a property off a `Temporal` value: a field if it has one, otherwise a method.
///
/// The field table is the engine's, so a name this realm has never heard of cannot answer with a
/// plausible zero -- `date.hour` is `undefined` because `get` says a `PlainDate` has no such field,
/// not because nobody wrote an arm for it.
pub fn instance_property(value: &engine::TemporalValue, name: &str) -> Option<Value> {
    // The tag every `Temporal` type carries, which is how `Object.prototype.toString` tells one
    // from an ordinary object: `[object Temporal.PlainDate]` rather than `[object Object]`.
    if name == crate::WellKnown::ToStringTag.property_key() {
        return Some(Value::String(value.kind().tag().to_owned()));
    }
    if let Some(field) = value.get(name) {
        return Some(match field {
            engine::Field::Number(number) => Value::Number(number),
            engine::Field::Integer128(integer) => {
                Value::BigInt(Rc::new(num_bigint::BigInt::from(integer)))
            }
            engine::Field::Text(text) => Value::String(text),
            engine::Field::Flag(flag) => Value::Boolean(flag),
            engine::Field::Undefined => Value::Undefined,
        });
    }
    Some(Value::Native(Native::Temporal(Op::Method(
        prototype_method(value.kind(), name)?,
    ))))
}

/// Which methods each of the eight types answers to.
///
/// Spelled out per type rather than shared, because the language keeps them apart on purpose: a
/// `PlainMonthDay` has no arithmetic, a `Duration` has no `compare` on the instance, and answering
/// a method the receiver cannot perform would turn a `TypeError` into a wrong answer.
fn prototype_method(kind: engine::Kind, name: &str) -> Option<Method> {
    let shared = match name {
        "toString" => Some(Method::ToString),
        "toJSON" => Some(Method::ToJson),
        "toLocaleString" => Some(Method::ToLocaleString),
        "valueOf" => Some(Method::ValueOf),
        "equals" if kind != engine::Kind::Duration => Some(Method::Equals),
        "with" => Some(Method::With),
        _ => None,
    };
    if shared.is_some() {
        return shared;
    }
    let arithmetic = match name {
        "add" => Some(Method::Add),
        "subtract" => Some(Method::Subtract),
        "until" => Some(Method::Until),
        "since" => Some(Method::Since),
        _ => None,
    };
    match kind {
        engine::Kind::PlainDate => match name {
            "toPlainDateTime" => Some(Method::ToPlainDateTime),
            "toPlainYearMonth" => Some(Method::ToPlainYearMonth),
            "toPlainMonthDay" => Some(Method::ToPlainMonthDay),
            "toZonedDateTime" => Some(Method::ToZonedDateTime),
            "withCalendar" => Some(Method::WithCalendar),
            _ => arithmetic,
        },
        engine::Kind::PlainTime => match name {
            "round" => Some(Method::Round),
            _ => arithmetic,
        },
        engine::Kind::PlainDateTime => match name {
            "round" => Some(Method::Round),
            "toPlainDate" => Some(Method::ToPlainDate),
            "toPlainTime" => Some(Method::ToPlainTime),
            "toZonedDateTime" => Some(Method::ToZonedDateTime),
            "withCalendar" => Some(Method::WithCalendar),
            "withPlainTime" => Some(Method::WithPlainTime),
            _ => arithmetic,
        },
        engine::Kind::PlainYearMonth => match name {
            "toPlainDate" => Some(Method::ToPlainDate),
            _ => arithmetic,
        },
        engine::Kind::PlainMonthDay => match name {
            "toPlainDate" => Some(Method::ToPlainDate),
            _ => None,
        },
        engine::Kind::Duration => match name {
            "round" => Some(Method::Round),
            "total" => Some(Method::Total),
            "negated" => Some(Method::Negated),
            "abs" => Some(Method::Abs),
            _ => arithmetic,
        },
        engine::Kind::Instant => match name {
            "round" => Some(Method::Round),
            "toZonedDateTimeISO" => Some(Method::ToZonedDateTime),
            _ => arithmetic,
        },
        engine::Kind::ZonedDateTime => match name {
            "round" => Some(Method::Round),
            "startOfDay" => Some(Method::StartOfDay),
            "toInstant" => Some(Method::ToInstant),
            "toPlainDate" => Some(Method::ToPlainDate),
            "toPlainTime" => Some(Method::ToPlainTime),
            "toPlainDateTime" => Some(Method::ToPlainDateTime),
            "withCalendar" => Some(Method::WithCalendar),
            "withTimeZone" => Some(Method::WithTimeZone),
            "withPlainTime" => Some(Method::WithPlainTime),
            _ => arithmetic,
        },
    }
}

/// An engine failure, already classified, becoming the error the program catches.
fn thrown(error: engine::Error) -> ExecutionError {
    ExecutionError::TemporalFailure {
        kind: match error.kind {
            engine::ErrorKind::Type => ErrorKind::Type,
            engine::ErrorKind::Range => ErrorKind::Range,
            engine::ErrorKind::Syntax => ErrorKind::Syntax,
        },
        message: error.message,
    }
}

fn type_error(message: &str) -> ExecutionError {
    ExecutionError::TypeErrorMessage {
        message: message.to_owned(),
    }
}

fn wrap(value: engine::TemporalValue) -> Value {
    Value::Temporal(Rc::new(value))
}

/// The property names an options bag is read for, in the order the specification reads them.
///
/// The order is observable: every one of these can be a getter, and Test262 checks which runs
/// first. Alphabetical is what the specification's own option-reading steps come out as, and
/// keeping the list in one place is what stops two call sites from disagreeing about it.
const OPTION_KEYS: [&str; 11] = [
    "calendarName",
    "disambiguation",
    "fractionalSecondDigits",
    "largestUnit",
    "offset",
    "overflow",
    "relativeTo",
    "roundingIncrement",
    "roundingMode",
    "smallestUnit",
    "timeZoneName",
];

/// The calendar and wall-clock field names, in the order the specification reads them.
const FIELD_KEYS: [&str; 15] = [
    "calendar",
    "day",
    "era",
    "eraYear",
    "hour",
    "microsecond",
    "millisecond",
    "minute",
    "month",
    "monthCode",
    "nanosecond",
    "offset",
    "second",
    "timeZone",
    "year",
];

const DURATION_KEYS: [&str; 10] = [
    "days",
    "hours",
    "microseconds",
    "milliseconds",
    "minutes",
    "months",
    "nanoseconds",
    "seconds",
    "weeks",
    "years",
];

impl<'host, H: HostOps + ?Sized> Vm<'host, H> {
    /// `new Temporal.X(...)`.
    pub(crate) fn temporal_construct(
        &mut self,
        kind: engine::Kind,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let mut number = |index: usize, default: f64| -> Result<f64, ExecutionError> {
            match arguments.get(index) {
                None | Some(Value::Undefined) => Ok(default),
                value => self.as_number_argument(value, instruction),
            }
        };
        let built = match kind {
            engine::Kind::PlainDate => {
                let (year, month, day) = (number(0, 0.0)?, number(1, 0.0)?, number(2, 0.0)?);
                let calendar = self.temporal_calendar_argument(arguments.get(3), instruction)?;
                engine::plain_date(year, month, day, calendar.as_deref())
            }
            engine::Kind::PlainTime => engine::plain_time(
                number(0, 0.0)?,
                number(1, 0.0)?,
                number(2, 0.0)?,
                number(3, 0.0)?,
                number(4, 0.0)?,
                number(5, 0.0)?,
            ),
            engine::Kind::PlainDateTime => {
                let fields = [
                    number(0, 0.0)?,
                    number(1, 0.0)?,
                    number(2, 0.0)?,
                    number(3, 0.0)?,
                    number(4, 0.0)?,
                    number(5, 0.0)?,
                    number(6, 0.0)?,
                    number(7, 0.0)?,
                    number(8, 0.0)?,
                ];
                let calendar = self.temporal_calendar_argument(arguments.get(9), instruction)?;
                engine::plain_date_time(
                    fields[0],
                    fields[1],
                    fields[2],
                    fields[3],
                    fields[4],
                    fields[5],
                    fields[6],
                    fields[7],
                    fields[8],
                    calendar.as_deref(),
                )
            }
            engine::Kind::PlainYearMonth => {
                let (year, month) = (number(0, 0.0)?, number(1, 0.0)?);
                let calendar = self.temporal_calendar_argument(arguments.get(2), instruction)?;
                let day = match arguments.get(3) {
                    None | Some(Value::Undefined) => None,
                    value => Some(self.as_number_argument(value, instruction)?),
                };
                engine::plain_year_month(year, month, day, calendar.as_deref())
            }
            engine::Kind::PlainMonthDay => {
                let (month, day) = (number(0, 0.0)?, number(1, 0.0)?);
                let calendar = self.temporal_calendar_argument(arguments.get(2), instruction)?;
                let year = match arguments.get(3) {
                    None | Some(Value::Undefined) => None,
                    value => Some(self.as_number_argument(value, instruction)?),
                };
                engine::plain_month_day(month, day, year, calendar.as_deref())
            }
            engine::Kind::Duration => {
                let mut fields = engine::DurationFields::default();
                let slots: [&mut Option<f64>; 10] = [
                    &mut fields.years,
                    &mut fields.months,
                    &mut fields.weeks,
                    &mut fields.days,
                    &mut fields.hours,
                    &mut fields.minutes,
                    &mut fields.seconds,
                    &mut fields.milliseconds,
                    &mut fields.microseconds,
                    &mut fields.nanoseconds,
                ];
                for (index, slot) in slots.into_iter().enumerate() {
                    *slot = Some(match arguments.get(index) {
                        None | Some(Value::Undefined) => 0.0,
                        value => self.as_number_argument(value, instruction)?,
                    });
                }
                engine::duration(&fields)
            }
            engine::Kind::Instant => {
                let nanoseconds = self.temporal_big_int_argument(arguments.first(), instruction)?;
                engine::instant(nanoseconds)
            }
            engine::Kind::ZonedDateTime => {
                let nanoseconds = self.temporal_big_int_argument(arguments.first(), instruction)?;
                let Some(zone) = self.temporal_string_argument(arguments.get(1), instruction)?
                else {
                    return Err(type_error("a ZonedDateTime needs a time zone"));
                };
                let calendar = self.temporal_calendar_argument(arguments.get(2), instruction)?;
                engine::zoned_date_time(nanoseconds, &zone, calendar.as_deref())
            }
        };
        built.map(wrap).map_err(thrown)
    }

    /// Everything reached through a `Native::Temporal` that is not a constructor.
    pub(crate) fn temporal_dispatch(
        &mut self,
        op: Op,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match op {
            // A namespace is not callable, and neither is a prototype.
            Op::Namespace | Op::Now | Op::Prototype(_) => Err(ExecutionError::NotCallable {
                instruction,
                value: "Temporal namespace",
            }),
            // `Temporal.PlainDate(...)` without `new` is a TypeError: the language refuses it
            // outright rather than treating the call form as a conversion the way `Date` does.
            Op::Constructor(kind) => Err(type_error(&format!(
                "Temporal.{} requires new",
                short_name(kind)
            ))),
            Op::NowTimeZoneId => Ok(Value::String(engine::now::time_zone_id().to_owned())),
            Op::NowRead(kind) => {
                let zone = self.temporal_string_argument(arguments.first(), instruction)?;
                // The realm's clock, not the host's: the same virtual instant `Date.now()` reads,
                // which is what makes two reads with no timer between them agree (ADR 0015).
                let epoch_nanoseconds = self.realm.now().as_nanos() as i128;
                engine::now::read(kind, epoch_nanoseconds, zone.as_deref())
                    .map(wrap)
                    .map_err(thrown)
            }
            Op::From(kind) => {
                let options = self.temporal_options(arguments.get(1), instruction)?;
                let value = arguments.first().cloned().unwrap_or(Value::Undefined);
                self.coerce_temporal(kind, &value, &options, instruction)
                    .map(wrap)
            }
            Op::Compare(kind) => {
                let options = if kind == engine::Kind::Duration {
                    self.temporal_options(arguments.get(2), instruction)?
                } else {
                    engine::Options::default()
                };
                let first = arguments.first().cloned().unwrap_or(Value::Undefined);
                let second = arguments.get(1).cloned().unwrap_or(Value::Undefined);
                let a =
                    self.coerce_temporal(kind, &first, &engine::Options::default(), instruction)?;
                let b =
                    self.coerce_temporal(kind, &second, &engine::Options::default(), instruction)?;
                let ordering = if kind == engine::Kind::Duration {
                    engine::compare_durations(&a, &b, &options)
                } else {
                    engine::compare(&a, &b)
                };
                ordering
                    .map(|order| Value::Number(f64::from(order)))
                    .map_err(thrown)
            }
            Op::Method(method) => self.temporal_method(method, receiver, arguments, instruction),
        }
    }

    fn temporal_method(
        &mut self,
        method: Method,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Temporal(value) = receiver else {
            return Err(type_error("this is not a Temporal value"));
        };
        let value = Rc::clone(value);
        let kind = value.kind();
        match method {
            // Every `Temporal` type refuses `valueOf`, and refusing is the point: `date1 < date2`
            // would otherwise compare two `NaN`s and answer false in both directions.
            Method::ValueOf => Err(type_error(
                "a Temporal value cannot be converted to a primitive; use compare or equals",
            )),
            Method::ToString | Method::ToLocaleString | Method::ToJson => {
                let options = if matches!(method, Method::ToString) {
                    self.temporal_options(arguments.first(), instruction)?
                } else {
                    engine::Options::default()
                };
                value
                    .to_temporal_string(&options)
                    .map(Value::String)
                    .map_err(thrown)
            }
            Method::With => {
                let Some(item) = arguments.first() else {
                    return Err(type_error("with needs a property bag"));
                };
                if !matches!(item, Value::Object(_)) {
                    return Err(type_error("with needs a property bag"));
                }
                if kind == engine::Kind::Duration {
                    let fields = self.temporal_duration_fields(item, instruction)?;
                    return value.duration_with(&fields).map(wrap).map_err(thrown);
                }
                let fields = self.temporal_fields(item, instruction)?;
                let options = self.temporal_options(arguments.get(1), instruction)?;
                value.with(&fields, &options).map(wrap).map_err(thrown)
            }
            Method::Add | Method::Subtract => {
                let operand = arguments.first().cloned().unwrap_or(Value::Undefined);
                let options = self.temporal_options(arguments.get(1), instruction)?;
                let duration = self.coerce_temporal(
                    engine::Kind::Duration,
                    &operand,
                    &engine::Options::default(),
                    instruction,
                )?;
                let shifted = if matches!(method, Method::Add) {
                    value.add(&duration, &options)
                } else {
                    value.subtract(&duration, &options)
                };
                shifted.map(wrap).map_err(thrown)
            }
            Method::Until | Method::Since => {
                let operand = arguments.first().cloned().unwrap_or(Value::Undefined);
                let options = self.temporal_options(arguments.get(1), instruction)?;
                let other =
                    self.coerce_temporal(kind, &operand, &engine::Options::default(), instruction)?;
                let difference = if matches!(method, Method::Until) {
                    value.until(&other, &options)
                } else {
                    value.since(&other, &options)
                };
                difference.map(wrap).map_err(thrown)
            }
            // `round` takes a bare unit string as shorthand for `{ smallestUnit: it }`, which is
            // how nearly every call in the wild spells it.
            Method::Round | Method::Total => {
                let options = match arguments.first() {
                    Some(Value::String(unit)) => {
                        let mut options = engine::Options::default();
                        if matches!(method, Method::Total) {
                            options.unit = Some(unit.clone());
                        } else {
                            options.smallest_unit = Some(unit.clone());
                        }
                        options
                    }
                    None | Some(Value::Undefined) => {
                        return Err(type_error("round needs a unit"));
                    }
                    other => {
                        let mut options = self.temporal_options(other, instruction)?;
                        if matches!(method, Method::Total) {
                            options.unit = self.temporal_option_string(
                                other.expect("checked above"),
                                "unit",
                                instruction,
                            )?;
                        }
                        options
                    }
                };
                if matches!(method, Method::Total) {
                    value.total(&options).map(Value::Number).map_err(thrown)
                } else {
                    value.round(&options).map(wrap).map_err(thrown)
                }
            }
            Method::Equals => {
                let operand = arguments.first().cloned().unwrap_or(Value::Undefined);
                let other =
                    self.coerce_temporal(kind, &operand, &engine::Options::default(), instruction)?;
                value.equals(&other).map(Value::Boolean).map_err(thrown)
            }
            Method::Negated => value.negated().map(wrap).map_err(thrown),
            Method::Abs => value.abs().map(wrap).map_err(thrown),
            Method::StartOfDay => value.start_of_day().map(wrap).map_err(thrown),
            Method::WithCalendar => {
                let Some(calendar) =
                    self.temporal_string_argument(arguments.first(), instruction)?
                else {
                    return Err(type_error("withCalendar needs a calendar"));
                };
                value.with_calendar(&calendar).map(wrap).map_err(thrown)
            }
            Method::WithTimeZone => {
                let Some(zone) = self.temporal_string_argument(arguments.first(), instruction)?
                else {
                    return Err(type_error("withTimeZone needs a time zone"));
                };
                value.with_time_zone(&zone).map(wrap).map_err(thrown)
            }
            Method::WithPlainTime => {
                let time = match arguments.first() {
                    None | Some(Value::Undefined) => None,
                    Some(argument) => Some(self.coerce_temporal(
                        engine::Kind::PlainTime,
                        argument,
                        &engine::Options::default(),
                        instruction,
                    )?),
                };
                value
                    .with_plain_time(time.as_ref())
                    .map(wrap)
                    .map_err(thrown)
            }
            Method::ToZonedDateTime => {
                let (zone, time) = match arguments.first() {
                    Some(Value::String(zone)) => (Some(zone.clone()), None),
                    Some(item @ Value::Object(_)) => {
                        let zone = match self.read_property(item, "timeZone", instruction)? {
                            Value::Undefined => None,
                            other => self.temporal_string_argument(Some(&other), instruction)?,
                        };
                        let time = match self.read_property(item, "plainTime", instruction)? {
                            Value::Undefined => None,
                            other => Some(self.coerce_temporal(
                                engine::Kind::PlainTime,
                                &other,
                                &engine::Options::default(),
                                instruction,
                            )?),
                        };
                        (zone, time)
                    }
                    other => (self.temporal_string_argument(other, instruction)?, None),
                };
                let Some(zone) = zone else {
                    return Err(type_error("toZonedDateTime needs a time zone"));
                };
                let options = self.temporal_options(arguments.get(1), instruction)?;
                value
                    .to_zoned_date_time(&zone, time.as_ref(), &options)
                    .map(wrap)
                    .map_err(thrown)
            }
            // `toPlainDate` means two different things: on a date-and-time it drops the time, and
            // on a year-month or a month-day it supplies the component that was missing.
            Method::ToPlainDate
                if matches!(
                    kind,
                    engine::Kind::PlainYearMonth | engine::Kind::PlainMonthDay
                ) =>
            {
                let Some(item) = arguments
                    .first()
                    .filter(|item| matches!(item, Value::Object(_)))
                else {
                    return Err(type_error("toPlainDate needs a property bag"));
                };
                let fields = self.temporal_fields(item, instruction)?;
                value.to_plain_date_with(&fields).map(wrap).map_err(thrown)
            }
            Method::ToPlainDateTime if kind == engine::Kind::PlainDate => {
                let supplement = match arguments.first() {
                    None | Some(Value::Undefined) => None,
                    Some(item) => Some(self.temporal_time_supplement(item, instruction)?),
                };
                value
                    .to_kind(engine::Kind::PlainDateTime, supplement.as_ref())
                    .map(wrap)
                    .map_err(thrown)
            }
            Method::ToPlainDate => value
                .to_kind(engine::Kind::PlainDate, None)
                .map(wrap)
                .map_err(thrown),
            Method::ToPlainTime => value
                .to_kind(engine::Kind::PlainTime, None)
                .map(wrap)
                .map_err(thrown),
            Method::ToPlainDateTime => value
                .to_kind(engine::Kind::PlainDateTime, None)
                .map(wrap)
                .map_err(thrown),
            Method::ToPlainYearMonth => value
                .to_kind(engine::Kind::PlainYearMonth, None)
                .map(wrap)
                .map_err(thrown),
            Method::ToPlainMonthDay => value
                .to_kind(engine::Kind::PlainMonthDay, None)
                .map(wrap)
                .map_err(thrown),
            Method::ToInstant => value
                .to_kind(engine::Kind::Instant, None)
                .map(wrap)
                .map_err(thrown),
        }
    }

    /// `ToTemporalX`: an existing value of that type, an ISO string, or a property bag.
    ///
    /// `pub(crate)` because `Intl.DurationFormat.prototype.format` takes a `ToTemporalDuration`
    /// argument: the conversion is the same one, and a second copy of it in `intl.rs` would be a
    /// second place for the property-bag read order to drift.
    pub(crate) fn coerce_temporal(
        &mut self,
        kind: engine::Kind,
        value: &Value,
        options: &engine::Options,
        instruction: usize,
    ) -> Result<engine::TemporalValue, ExecutionError> {
        match value {
            Value::Temporal(existing) if existing.kind() == kind => Ok((**existing).clone()),
            // A wider value narrows: a `PlainDateTime` is a `PlainDate` with a time attached, and
            // asking for the date is a projection rather than a conversion that can be wrong.
            Value::Temporal(existing) => existing.to_kind(kind, None).map_err(thrown),
            Value::String(text) => {
                if kind == engine::Kind::ZonedDateTime {
                    engine::parse_zoned_date_time(text, options).map_err(thrown)
                } else {
                    engine::parse(kind, text).map_err(thrown)
                }
            }
            Value::Object(_) => {
                if kind == engine::Kind::Duration {
                    let fields = self.temporal_duration_fields(value, instruction)?;
                    return engine::duration_from_fields(&fields).map_err(thrown);
                }
                let fields = self.temporal_fields(value, instruction)?;
                engine::from_fields(kind, &fields, options).map_err(thrown)
            }
            other => Err(type_error(&format!(
                "a {} is not a Temporal.{}",
                other.type_name(),
                short_name(kind)
            ))),
        }
    }

    /// The time half of a property bag, for `plainDate.toPlainDateTime(time)`.
    fn temporal_time_supplement(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<engine::Fields, ExecutionError> {
        if let Value::Temporal(existing) = value
            && existing.kind() == engine::Kind::PlainTime
        {
            let mut fields = engine::Fields::default();
            for name in [
                "hour",
                "minute",
                "second",
                "millisecond",
                "microsecond",
                "nanosecond",
            ] {
                if let Some(engine::Field::Number(number)) = existing.get(name) {
                    set_time_field(&mut fields, name, number);
                }
            }
            return Ok(fields);
        }
        self.temporal_fields(value, instruction)
    }

    /// Every calendar and wall-clock field the bag carries, read in the specified order.
    fn temporal_fields(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<engine::Fields, ExecutionError> {
        let mut fields = engine::Fields::default();
        for name in FIELD_KEYS {
            let read = self.read_property(value, name, instruction)?;
            if matches!(read, Value::Undefined) {
                continue;
            }
            match name {
                "calendar" => {
                    fields.calendar = self.temporal_string_argument(Some(&read), instruction)?
                }
                "timeZone" => {
                    fields.time_zone = self.temporal_string_argument(Some(&read), instruction)?
                }
                "offset" => {
                    fields.offset = self.temporal_string_argument(Some(&read), instruction)?
                }
                "era" => fields.era = self.temporal_string_argument(Some(&read), instruction)?,
                "monthCode" => {
                    fields.month_code = self.temporal_string_argument(Some(&read), instruction)?;
                }
                _ => {
                    let number = self.as_number_argument(Some(&read), instruction)?;
                    match name {
                        "eraYear" => fields.era_year = Some(number),
                        "year" => fields.year = Some(number),
                        "month" => fields.month = Some(number),
                        "day" => fields.day = Some(number),
                        _ => set_time_field(&mut fields, name, number),
                    }
                }
            }
        }
        Ok(fields)
    }

    fn temporal_duration_fields(
        &mut self,
        value: &Value,
        instruction: usize,
    ) -> Result<engine::DurationFields, ExecutionError> {
        let mut fields = engine::DurationFields::default();
        for name in DURATION_KEYS {
            let read = self.read_property(value, name, instruction)?;
            if matches!(read, Value::Undefined) {
                continue;
            }
            let number = self.as_number_argument(Some(&read), instruction)?;
            match name {
                "years" => fields.years = Some(number),
                "months" => fields.months = Some(number),
                "weeks" => fields.weeks = Some(number),
                "days" => fields.days = Some(number),
                "hours" => fields.hours = Some(number),
                "minutes" => fields.minutes = Some(number),
                "seconds" => fields.seconds = Some(number),
                "milliseconds" => fields.milliseconds = Some(number),
                "microseconds" => fields.microseconds = Some(number),
                _ => fields.nanoseconds = Some(number),
            }
        }
        Ok(fields)
    }

    /// `GetOptionsObject` followed by every option read this realm knows about.
    ///
    /// Reading them all, rather than only the ones the operation uses, is deliberate: the reads
    /// are observable and the specification's own option-reading steps run for every option the
    /// operation *could* take. Reading too few would hide a getter a test is counting.
    fn temporal_options(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<engine::Options, ExecutionError> {
        let mut options = engine::Options::default();
        let Some(value) = value else {
            return Ok(options);
        };
        match value {
            Value::Undefined => return Ok(options),
            Value::Object(_) | Value::Function(_) | Value::Array(_) => {}
            _ => return Err(type_error("options must be an object")),
        }
        for name in OPTION_KEYS {
            let read = self.read_property(value, name, instruction)?;
            if matches!(read, Value::Undefined) {
                continue;
            }
            match name {
                "roundingIncrement" => {
                    options.rounding_increment =
                        Some(self.as_number_argument(Some(&read), instruction)?);
                }
                "fractionalSecondDigits" => {
                    options.fractional_second_digits = Some(match &read {
                        Value::String(text) if text == "auto" => None,
                        _ => {
                            let digits = self.as_number_argument(Some(&read), instruction)?;
                            if !(0.0..=9.0).contains(&digits.trunc()) {
                                return Err(ExecutionError::TemporalFailure {
                                    kind: ErrorKind::Range,
                                    message: "fractionalSecondDigits must be between 0 and 9"
                                        .to_owned(),
                                });
                            }
                            Some(digits.trunc() as u8)
                        }
                    });
                }
                "relativeTo" => {
                    // A `relativeTo` is a date or a zoned date, and which one it is decides
                    // whether a day is always 24 hours -- so it is coerced here rather than
                    // guessed at by the engine.
                    let anchor = match &read {
                        Value::Temporal(existing) => (**existing).clone(),
                        other => self.coerce_temporal(
                            engine::Kind::PlainDate,
                            other,
                            &engine::Options::default(),
                            instruction,
                        )?,
                    };
                    options.relative_to = Some(anchor);
                }
                _ => {
                    let text = self
                        .temporal_string_argument(Some(&read), instruction)?
                        .unwrap_or_default();
                    match name {
                        "calendarName" => options.calendar_name = Some(text),
                        "disambiguation" => options.disambiguation = Some(text),
                        "largestUnit" => options.largest_unit = Some(text),
                        "offset" => options.offset_conflict = Some(text),
                        "overflow" => options.overflow = Some(text),
                        "roundingMode" => options.rounding_mode = Some(text),
                        "smallestUnit" => options.smallest_unit = Some(text),
                        _ => options.time_zone_name = Some(text),
                    }
                }
            }
        }
        Ok(options)
    }

    /// One named option off a bag, for the operations whose option is not in the shared list.
    fn temporal_option_string(
        &mut self,
        value: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<Option<String>, ExecutionError> {
        let read = self.read_property(value, name, instruction)?;
        self.temporal_string_argument(Some(&read), instruction)
    }

    /// `ToString`, with `undefined` staying absent rather than becoming the word.
    fn temporal_string_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<Option<String>, ExecutionError> {
        let Some(value) = value else { return Ok(None) };
        if matches!(value, Value::Undefined) {
            return Ok(None);
        }
        if matches!(value, Value::Symbol(_)) {
            return Err(ExecutionError::SymbolIsNotAString);
        }
        let primitive = self.as_primitive(value, true, instruction)?;
        if matches!(primitive, Value::Symbol(_)) {
            return Err(ExecutionError::SymbolIsNotAString);
        }
        Ok(Some(primitive.to_display_string()))
    }

    /// The `calendar` argument a constructor takes, which is a string or nothing at all.
    fn temporal_calendar_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<Option<String>, ExecutionError> {
        self.temporal_string_argument(value, instruction)
    }

    /// The `epochNanoseconds` a constructor takes, which is a `BigInt` and nothing else.
    fn temporal_big_int_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<i128, ExecutionError> {
        let integer = self.as_big_int_argument(value, instruction)?;
        // Beyond `i128` there is no representable instant at all, so a refusal here is the same
        // RangeError the engine would raise one step later.
        num_traits::ToPrimitive::to_i128(&integer).ok_or(ExecutionError::TemporalFailure {
            kind: ErrorKind::Range,
            message: "epochNanoseconds is outside the representable range".to_owned(),
        })
    }
}

fn set_time_field(fields: &mut engine::Fields, name: &str, number: f64) {
    match name {
        "hour" => fields.hour = Some(number),
        "minute" => fields.minute = Some(number),
        "second" => fields.second = Some(number),
        "millisecond" => fields.millisecond = Some(number),
        "microsecond" => fields.microsecond = Some(number),
        "nanosecond" => fields.nanosecond = Some(number),
        _ => {}
    }
}

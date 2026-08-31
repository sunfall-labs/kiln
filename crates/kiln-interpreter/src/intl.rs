//! `Intl`, wired to the linked internationalization engine.
//!
//! This is glue, not semantics -- the same division `temporal.rs` makes. Every question that has
//! a CLDR answer (what a language tag canonicalizes to, where the group separators fall, which
//! plural category a number takes, how two strings collate, where a word ends) is answered by
//! `kiln-intl`, which answers it with ICU4X: ADR 0025's linked-engine rule, named for this feature
//! by ADR 0029 §4.
//!
//! What lives here is the part that is genuinely the language's. ECMA-402 is unusually specific
//! about *how options are read* -- which properties are got, in what order, which coercions run,
//! and whether a rejected value is a `TypeError` or a `RangeError` -- and a very large share of
//! the conformance corpus tests nothing else. That ordering is observable through getters and
//! proxies, so it has to happen on the machine, here, rather than inside an adapter that only
//! sees the values that came out.

use std::cell::RefCell;
use std::rc::Rc;

use kiln_host_abi::HostOps;
use kiln_intl as engine;

use crate::{ArrayData, ErrorKind, ExecutionError, Native, ObjectData, Value, Vm, WellKnown};

/// Which `Intl` service a constructor, prototype or instance belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Collator,
    DateTimeFormat,
    DisplayNames,
    DurationFormat,
    ListFormat,
    Locale,
    NumberFormat,
    PluralRules,
    RelativeTimeFormat,
    Segmenter,
    /// The `Segments` object `Intl.Segmenter.prototype.segment` returns.
    ///
    /// Not a service the language can name -- there is no `Intl.Segments` -- but it has a
    /// prototype with methods on it and instances that answer to them, which is exactly what a
    /// `Service` is here. Keeping it in the same enum is what lets the brand check, the prototype
    /// lookup and the method table stay one mechanism.
    Segments,
}

impl Service {
    /// The name the language spells it with, which is also its `Symbol.toStringTag`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Collator => "Collator",
            Self::DateTimeFormat => "DateTimeFormat",
            Self::DisplayNames => "DisplayNames",
            Self::DurationFormat => "DurationFormat",
            Self::ListFormat => "ListFormat",
            Self::Locale => "Locale",
            Self::NumberFormat => "NumberFormat",
            Self::PluralRules => "PluralRules",
            Self::RelativeTimeFormat => "RelativeTimeFormat",
            Self::Segmenter => "Segmenter",
            Self::Segments => "Segments",
        }
    }

    /// The tag `Object.prototype.toString` reports.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Collator => "Intl.Collator",
            Self::DateTimeFormat => "Intl.DateTimeFormat",
            Self::DisplayNames => "Intl.DisplayNames",
            Self::DurationFormat => "Intl.DurationFormat",
            Self::ListFormat => "Intl.ListFormat",
            Self::Locale => "Intl.Locale",
            Self::NumberFormat => "Intl.NumberFormat",
            Self::PluralRules => "Intl.PluralRules",
            Self::RelativeTimeFormat => "Intl.RelativeTimeFormat",
            Self::Segmenter => "Intl.Segmenter",
            // `%Segments.prototype%` carries no `Symbol.toStringTag`, so a `Segments` reports the
            // bare `[object Object]` an ordinary object does.
            Self::Segments => "Object",
        }
    }
}

/// What a `Native::Intl` stands for.
///
/// One `Native` variant with a payload rather than forty flat ones, for the reason
/// `Native::Temporal` gives: the six services answer to overlapping method names and the receiver
/// decides what a name *means*, so the variant only has to say which name it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// The `Intl` namespace itself.
    Namespace,
    /// `Intl.getCanonicalLocales`.
    GetCanonicalLocales,
    /// `Intl.supportedValuesOf`.
    SupportedValuesOf,
    /// `Intl.NumberFormat` and its five siblings.
    Constructor(Service),
    /// `Intl.NumberFormat.prototype` and its five siblings.
    Prototype(Service),
    /// `Intl.NumberFormat.supportedLocalesOf` and its siblings.
    SupportedLocalesOf(Service),
    /// A prototype method. The service is the *prototype* it was taken from rather than the
    /// receiver it is called on, which is what makes the brand check possible:
    /// `Intl.NumberFormat.prototype.format.call(collator)` has to be a `TypeError`, and only a
    /// method that remembers which prototype it came off can say so.
    Method(Service, Method),
    /// A locale-sensitive method that lives on another prototype.
    Adapter(Adapter),
}

/// A method of `Number`, `String` or `Date` that ECMA-402 redefines as a call into `Intl`.
///
/// `(1234.5).toLocaleString(l, o)` is specified as `new Intl.NumberFormat(l, o).format(1234.5)`,
/// and the corpus checks the equivalence in both directions: the same string out, and the same
/// exception -- with the same constructor -- for every options bag the service rejects. The only
/// way to keep that true is for the two to be one implementation, which is why these live here
/// rather than beside `Number.prototype.toFixed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    /// `Number.prototype.toLocaleString`.
    NumberToLocaleString,
    /// `String.prototype.localeCompare`.
    StringLocaleCompare,
    /// `Date.prototype.toLocaleString`.
    DateToLocaleString,
    /// `Date.prototype.toLocaleDateString`.
    DateToLocaleDateString,
    /// `Date.prototype.toLocaleTimeString`.
    DateToLocaleTimeString,
    /// `String.prototype.toLocaleLowerCase`.
    StringToLocaleLowerCase,
    /// `String.prototype.toLocaleUpperCase`.
    StringToLocaleUpperCase,
}

impl Adapter {
    /// The name the method reports through `.name`.
    const fn name(self) -> &'static str {
        match self {
            Self::NumberToLocaleString | Self::DateToLocaleString => "toLocaleString",
            Self::StringLocaleCompare => "localeCompare",
            Self::DateToLocaleDateString => "toLocaleDateString",
            Self::DateToLocaleTimeString => "toLocaleTimeString",
            Self::StringToLocaleLowerCase => "toLocaleLowerCase",
            Self::StringToLocaleUpperCase => "toLocaleUpperCase",
        }
    }
}

/// A prototype method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    ResolvedOptions,
    Of,
    Format,
    FormatToParts,
    FormatRange,
    FormatRangeToParts,
    Select,
    SelectRange,
    Compare,
    Segment,
    Containing,
    SegmentIterator,
    Maximize,
    Minimize,
    LocaleToString,
}

/// A built `Intl` object: the engine's formatter, held opaquely.
///
/// Its own `Value` kind rather than an object with reserved slots, for the reason `Value::Date`
/// and `Value::Temporal` are: the state is a Rust value the engine owns, and round-tripping it
/// through JavaScript-visible slots would mean rebuilding the formatter on every call and losing
/// the brand check that tells `Intl.NumberFormat.prototype.format` it was handed one.
#[derive(Debug)]
pub enum IntlValue {
    Collator(engine::collator::Collator),
    DateTimeFormat(engine::datetime::DateTimeFormat),
    DisplayNames(engine::displaynames::DisplayNames),
    DurationFormat(engine::duration::DurationFormat),
    ListFormat(engine::list::ListFormat),
    /// The canonical tag. `Intl.Locale` has no formatter behind it -- it *is* the tag.
    Locale(String),
    NumberFormat(engine::number::NumberFormat),
    PluralRules(engine::plurals::PluralRules),
    RelativeTimeFormat(engine::relative::RelativeTimeFormat),
    Segmenter(engine::segmenter::Segmenter),
    Segments(engine::segmenter::Segments),
}

/// Identity, not equality: two formatters built from the same options are still two objects, and
/// `===` has to say so. The same rule `RegExpData` follows.
impl PartialEq for IntlValue {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl IntlValue {
    /// Which service this is, without matching on the payload at every call site.
    pub const fn service(&self) -> Service {
        match self {
            Self::Collator(_) => Service::Collator,
            Self::DateTimeFormat(_) => Service::DateTimeFormat,
            Self::DisplayNames(_) => Service::DisplayNames,
            Self::DurationFormat(_) => Service::DurationFormat,
            Self::ListFormat(_) => Service::ListFormat,
            Self::Locale(_) => Service::Locale,
            Self::NumberFormat(_) => Service::NumberFormat,
            Self::PluralRules(_) => Service::PluralRules,
            Self::RelativeTimeFormat(_) => Service::RelativeTimeFormat,
            Self::Segmenter(_) => Service::Segmenter,
            Self::Segments(_) => Service::Segments,
        }
    }
}

/// The name an `Intl` built-in reports through `.name`.
pub fn js_name(op: Op) -> String {
    match op {
        Op::Namespace => "Intl".to_owned(),
        Op::GetCanonicalLocales => "getCanonicalLocales".to_owned(),
        Op::SupportedValuesOf => "supportedValuesOf".to_owned(),
        Op::Constructor(service) => service.name().to_owned(),
        Op::Prototype(_) => String::new(),
        Op::SupportedLocalesOf(_) => "supportedLocalesOf".to_owned(),
        Op::Adapter(adapter) => adapter.name().to_owned(),
        Op::Method(_, method) => match method {
            Method::ResolvedOptions => "resolvedOptions".to_owned(),
            Method::Of => "of".to_owned(),
            Method::Format => "format".to_owned(),
            Method::FormatToParts => "formatToParts".to_owned(),
            Method::FormatRange => "formatRange".to_owned(),
            Method::FormatRangeToParts => "formatRangeToParts".to_owned(),
            Method::Select => "select".to_owned(),
            Method::SelectRange => "selectRange".to_owned(),
            Method::Compare => "compare".to_owned(),
            Method::Segment => "segment".to_owned(),
            Method::Containing => "containing".to_owned(),
            Method::SegmentIterator => "[Symbol.iterator]".to_owned(),
            Method::Maximize => "maximize".to_owned(),
            Method::Minimize => "minimize".to_owned(),
            Method::LocaleToString => "toString".to_owned(),
        },
    }
}

/// `Intl` is a namespace; everything else under it is callable or a prototype.
pub const fn namespace_name(op: Op) -> Option<&'static str> {
    match op {
        Op::Namespace => Some("Intl"),
        _ => None,
    }
}

/// The declared parameter count, or `None` for something that is not callable.
pub const fn length(op: Op) -> Option<f64> {
    match op {
        Op::Namespace | Op::Prototype(_) => None,
        // `localeCompare` declares the string it compares against; the other four declare nothing,
        // because their two arguments are the `locales` and `options` the specification marks
        // optional and leaves out of the length.
        Op::Adapter(Adapter::StringLocaleCompare) => Some(1.0),
        Op::Adapter(_) => Some(0.0),
        Op::GetCanonicalLocales | Op::SupportedLocalesOf(_) | Op::SupportedValuesOf => Some(1.0),
        Op::Constructor(Service::Locale) => Some(1.0),
        Op::Constructor(Service::DisplayNames) => Some(2.0),
        Op::Constructor(_) => Some(0.0),
        // `Intl.RelativeTimeFormat` is the one service whose `format` takes two arguments -- a
        // value and a unit -- so the arity depends on the prototype the method came off.
        Op::Method(Service::RelativeTimeFormat, Method::Format | Method::FormatToParts) => {
            Some(2.0)
        }
        Op::Method(
            _,
            Method::Compare
            | Method::FormatRange
            | Method::FormatRangeToParts
            | Method::SelectRange,
        ) => Some(2.0),
        Op::Method(
            _,
            Method::Format
            | Method::FormatToParts
            | Method::Select
            | Method::Segment
            | Method::Containing
            | Method::Of,
        ) => Some(1.0),
        Op::Method(_, _) => Some(0.0),
    }
}

/// A static property of the `Intl` namespace, a constructor or a prototype.
pub fn static_property(op: Op, name: &str) -> Option<Value> {
    const SERVICES: &[Service] = &[
        Service::Collator,
        Service::DateTimeFormat,
        Service::DisplayNames,
        Service::DurationFormat,
        Service::ListFormat,
        Service::Locale,
        Service::NumberFormat,
        Service::PluralRules,
        Service::RelativeTimeFormat,
        Service::Segmenter,
    ];
    match (op, name) {
        (Op::Namespace, "getCanonicalLocales") => {
            Some(Value::Native(Native::Intl(Op::GetCanonicalLocales)))
        }
        (Op::Namespace, "supportedValuesOf") => {
            Some(Value::Native(Native::Intl(Op::SupportedValuesOf)))
        }
        (Op::Namespace, _) => SERVICES
            .iter()
            .find(|service| service.name() == name)
            .map(|service| Value::Native(Native::Intl(Op::Constructor(*service)))),
        (Op::Constructor(service), "prototype") => {
            Some(Value::Native(Native::Intl(Op::Prototype(service))))
        }
        // `Intl.Locale` has no locale negotiation to do, so it has no `supportedLocalesOf`.
        (Op::Constructor(service), "supportedLocalesOf")
            if !matches!(service, Service::Locale | Service::Segments) =>
        {
            Some(Value::Native(Native::Intl(Op::SupportedLocalesOf(service))))
        }
        // `%Segments.prototype%` is not a constructor's prototype -- no `Intl.Segments` exists --
        // so it is the one prototype here with no `constructor` on it.
        (Op::Prototype(service), "constructor") if service != Service::Segments => {
            Some(Value::Native(Native::Intl(Op::Constructor(service))))
        }
        (Op::Prototype(service), _) => prototype_method(service, name),
        _ => None,
    }
}

/// The methods a service's prototype answers to.
///
/// One table shared by the prototype and by an instance, so the two cannot disagree about what a
/// service has -- the arrangement `regexp_method` and `date_method` already use.
pub fn prototype_method(service: Service, name: &str) -> Option<Value> {
    let method = match (service, name) {
        (_, "resolvedOptions") if !matches!(service, Service::Locale | Service::Segments) => {
            Method::ResolvedOptions
        }
        (Service::DisplayNames, "of") => Method::Of,
        (Service::RelativeTimeFormat, "format") => Method::Format,
        (Service::RelativeTimeFormat, "formatToParts") => Method::FormatToParts,
        (
            Service::NumberFormat
            | Service::ListFormat
            | Service::DateTimeFormat
            | Service::DurationFormat,
            "format",
        ) => Method::Format,
        (
            Service::NumberFormat
            | Service::ListFormat
            | Service::DateTimeFormat
            | Service::DurationFormat,
            "formatToParts",
        ) => Method::FormatToParts,
        (Service::DateTimeFormat, "formatRange") => Method::FormatRange,
        (Service::DateTimeFormat, "formatRangeToParts") => Method::FormatRangeToParts,
        (Service::PluralRules, "select") => Method::Select,
        (Service::PluralRules, "selectRange") => Method::SelectRange,
        (Service::Collator, "compare") => Method::Compare,
        (Service::Segmenter, "segment") => Method::Segment,
        (Service::Segments, "containing") => Method::Containing,
        (Service::Segments, name) if name == WellKnown::Iterator.property_key() => {
            Method::SegmentIterator
        }
        (Service::Locale, "maximize") => Method::Maximize,
        (Service::Locale, "minimize") => Method::Minimize,
        (Service::Locale, "toString") => Method::LocaleToString,
        _ => return None,
    };
    Some(Value::Native(Native::Intl(Op::Method(service, method))))
}

/// What an instance answers to: its prototype's methods, plus `Intl.Locale`'s data accessors.
///
/// `Intl.Locale`'s `language`, `region`, `calendar` and the rest are accessor properties in the
/// specification and plain reads here, which is the arrangement every other built-in getter in
/// this interpreter already uses (`RegExp.prototype.flags`, `Map.prototype.size`).
pub fn instance_property(value: &IntlValue, name: &str) -> Option<Value> {
    if let IntlValue::Locale(tag) = value
        && let Ok(parts) = engine::locale::parts(tag)
        && let Some((_, setting)) = parts.into_iter().find(|(key, _)| *key == name)
    {
        return Some(setting_to_value(&setting));
    }
    prototype_method(value.service(), name)
}

/// One `resolvedOptions()` entry as a JavaScript value.
fn setting_to_value(setting: &engine::Setting) -> Value {
    match setting {
        engine::Setting::String(text) => Value::String(text.clone()),
        engine::Setting::Number(number) => Value::Number(*number),
        engine::Setting::Bool(flag) => Value::Boolean(*flag),
        engine::Setting::Strings(items) => Value::Array(ArrayData::new(
            items
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        )),
        engine::Setting::Undefined => Value::Undefined,
    }
}

/// An engine refusal, carrying the error constructor the engine chose.
fn failure(error: engine::Error) -> ExecutionError {
    ExecutionError::IntlFailure {
        kind: match error.kind {
            engine::ErrorKind::Range => ErrorKind::Range,
            engine::ErrorKind::Type => ErrorKind::Type,
        },
        message: error.message,
    }
}

fn range(message: impl Into<String>) -> ExecutionError {
    ExecutionError::IntlFailure {
        kind: ErrorKind::Range,
        message: message.into(),
    }
}

fn type_error(message: impl Into<String>) -> ExecutionError {
    ExecutionError::IntlFailure {
        kind: ErrorKind::Type,
        message: message.into(),
    }
}

impl<'host, H: HostOps + ?Sized> Vm<'host, H> {
    /// Every `Intl` call, whether it arrived as a function, a method or a constructor.
    pub(crate) fn intl_dispatch(
        &mut self,
        op: Op,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match op {
            Op::GetCanonicalLocales => {
                let tags = self.canonicalize_locale_list(arguments.first(), instruction)?;
                Ok(Value::Array(ArrayData::new(
                    tags.into_iter().map(Value::String).collect(),
                )))
            }
            Op::SupportedLocalesOf(_) => {
                let tags = self.canonicalize_locale_list(arguments.first(), instruction)?;
                // The second argument is an options bag whose only relevant key is
                // `localeMatcher`, which is validated and then makes no difference -- see
                // `kiln_intl::locale::LOCALE_MATCHERS` for why one matcher answers for both.
                self.intl_locale_matcher(arguments.get(1), instruction)?;
                Ok(Value::Array(ArrayData::new(
                    engine::locale::supported_locales(&tags)
                        .into_iter()
                        .map(Value::String)
                        .collect(),
                )))
            }
            // `Intl.NumberFormat(...)` and `Intl.Collator(...)` without `new` build an object
            // anyway. That is not an oversight in the specification: those two predate the rest
            // and their legacy call semantics are still normative, while every constructor added
            // since is `new`-only and a plain call is a TypeError.
            Op::SupportedValuesOf => {
                let key = self.intl_string_argument(arguments.first(), instruction)?;
                let values = supported_values(&key)?;
                Ok(Value::Array(ArrayData::new(
                    values.into_iter().map(Value::String).collect(),
                )))
            }
            Op::Constructor(
                service @ (Service::NumberFormat | Service::Collator | Service::DateTimeFormat),
            ) => self.intl_construct(service, arguments, instruction),
            Op::Constructor(service) => Err(type_error(format!(
                "Constructor Intl.{} requires 'new'",
                service.name()
            ))),
            Op::Method(service, method) => {
                self.intl_method(service, method, receiver, arguments, instruction)
            }
            Op::Adapter(adapter) => self.intl_adapter(adapter, receiver, arguments, instruction),
            Op::Namespace | Op::Prototype(_) => Ok(Value::Undefined),
        }
    }

    /// `Number.prototype.toLocaleString` and its four siblings.
    ///
    /// Each one is the specification's own two steps -- construct the service with the arguments,
    /// then run it on the receiver -- and the construction goes through the same
    /// [`Self::intl_construct`] the `Intl` constructor uses, so the option reads, their order and
    /// their refusals are one implementation rather than two that agree today.
    fn intl_adapter(
        &mut self,
        adapter: Adapter,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        match adapter {
            Adapter::NumberToLocaleString => {
                // The brand check first: `Number.prototype.toLocaleString.call("5")` is a
                // TypeError rather than a formatted five.
                let number = this_number_value(receiver)?;
                let built = self.intl_construct(
                    Service::NumberFormat,
                    &[argument(arguments, 0), argument(arguments, 1)],
                    instruction,
                )?;
                let Value::Intl(value) = &built else {
                    return Err(type_error("Intl.NumberFormat did not build a formatter"));
                };
                let IntlValue::NumberFormat(formatter) = value.as_ref() else {
                    return Err(type_error("Intl.NumberFormat did not build a formatter"));
                };
                Ok(Value::String(
                    formatter
                        .format(&engine::number::Numeric::Double(number))
                        .map_err(failure)?,
                ))
            }
            Adapter::StringLocaleCompare => {
                if receiver.is_nullish() {
                    return Err(type_error(
                        "String.prototype.localeCompare called on null or undefined",
                    ));
                }
                // Both strings are coerced before the collator is built, which is observable: a
                // `toString` that throws does so before an invalid `locales` is noticed.
                let left = self.intl_string_argument(Some(receiver), instruction)?;
                let right = self.intl_string_argument(arguments.first(), instruction)?;
                let built = self.intl_construct(
                    Service::Collator,
                    &[argument(arguments, 1), argument(arguments, 2)],
                    instruction,
                )?;
                let Value::Intl(value) = &built else {
                    return Err(type_error("Intl.Collator did not build a collator"));
                };
                let IntlValue::Collator(collator) = value.as_ref() else {
                    return Err(type_error("Intl.Collator did not build a collator"));
                };
                Ok(Value::Number(
                    match collator.compare(&utf16(&left), &utf16(&right)) {
                        std::cmp::Ordering::Less => -1.0,
                        std::cmp::Ordering::Equal => 0.0,
                        std::cmp::Ordering::Greater => 1.0,
                    },
                ))
            }
            Adapter::StringToLocaleLowerCase | Adapter::StringToLocaleUpperCase => {
                if receiver.is_nullish() {
                    return Err(type_error(
                        "String.prototype case mapping called on null or undefined",
                    ));
                }
                let text = self.intl_string_argument(Some(receiver), instruction)?;
                // The locale list is validated even though this build maps case the same way in
                // every locale: `"x".toLocaleLowerCase("i")` is a `RangeError`, and swallowing a
                // malformed tag would make a program that mistyped one believe it worked.
                //
                // The mapping itself is the locale-independent one. Turkish dotless `i`, Azeri and
                // the Lithuanian retained dot are conditional mappings that need CLDR's
                // special-casing data, which this build does not link; producing the default
                // mapping for those three locales is the specification's own fallback, and it is
                // the same string `toLowerCase` gives.
                self.canonicalize_locale_list(arguments.first(), instruction)?;
                Ok(Value::String(
                    if matches!(adapter, Adapter::StringToLocaleLowerCase) {
                        text.to_lowercase()
                    } else {
                        text.to_uppercase()
                    },
                ))
            }
            Adapter::DateToLocaleString
            | Adapter::DateToLocaleDateString
            | Adapter::DateToLocaleTimeString => {
                let Value::Date(instant) = receiver else {
                    return Err(type_error("this is not a Date object."));
                };
                let time = instant.get();
                // Before any option is read: an invalid date is the string, not a formatting
                // attempt, so a throwing getter on the options bag never runs for one.
                if !time.is_finite() {
                    return Ok(Value::String("Invalid Date".to_owned()));
                }
                let (wants_date, wants_time) = match adapter {
                    Adapter::DateToLocaleDateString => (true, false),
                    Adapter::DateToLocaleTimeString => (false, true),
                    _ => (true, true),
                };
                // The locale list is canonicalized before the options bag is read, which is the
                // specification's order and is observable: an invalid tag is refused before a
                // getter on `options` can run.
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let mut options = self.intl_date_time_options(arguments.get(1), instruction)?;
                // `ToDateTimeOptions`'s two refusals: a date-only method may not be handed a time
                // style, and a time-only method may not be handed a date style.
                if !wants_time && options.time_style.is_some() {
                    return Err(type_error("toLocaleDateString cannot take a timeStyle"));
                }
                if !wants_date && options.date_style.is_some() {
                    return Err(type_error("toLocaleTimeString cannot take a dateStyle"));
                }
                if needs_date_time_defaults(&options, wants_date, wants_time) {
                    let numeric = || Some("numeric".to_owned());
                    if wants_date {
                        options.year = numeric();
                        options.month = numeric();
                        options.day = numeric();
                    }
                    if wants_time {
                        options.hour = numeric();
                        options.minute = numeric();
                        options.second = numeric();
                    }
                }
                let formatter =
                    engine::datetime::DateTimeFormat::new(&locales, options).map_err(failure)?;
                Ok(Value::String(formatter.format(time).map_err(failure)?))
            }
        }
    }

    /// `new Intl.X(locales, options)`.
    pub(crate) fn intl_construct(
        &mut self,
        service: Service,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let built = match service {
            Service::Locale => {
                // `Intl.Locale` takes a tag rather than a locale *list*: the first argument is one
                // string, and a non-string that is not already a Locale is a TypeError.
                let tag = match arguments.first() {
                    Some(Value::Intl(existing)) => match existing.as_ref() {
                        IntlValue::Locale(tag) => tag.clone(),
                        _ => return Err(type_error("Intl.Locale expects a locale or a string")),
                    },
                    Some(Value::String(text)) => text.clone(),
                    Some(value @ Value::Object(_)) => {
                        let primitive = self.as_primitive(value, true, instruction)?;
                        primitive.to_display_string()
                    }
                    _ => return Err(type_error("Intl.Locale expects a locale or a string")),
                };
                IntlValue::Locale(engine::locale::canonicalize(&tag).map_err(failure)?)
            }
            Service::NumberFormat => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let options = self.intl_number_options(arguments.get(1), instruction)?;
                IntlValue::NumberFormat(
                    engine::number::NumberFormat::new(&locales, options).map_err(failure)?,
                )
            }
            Service::PluralRules => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let bag = self.intl_options_object(arguments.get(1), instruction)?;
                self.intl_locale_matcher(Some(&bag), instruction)?;
                let kind = self
                    .intl_option(&bag, "type", engine::plurals::TYPES, instruction)?
                    .unwrap_or_else(|| "cardinal".to_owned());
                // Read between `type` and the digit options, which is where the specification
                // puts them and where the option-order tests expect the getters to fire. They are
                // not idle: `notation: "compact"` changes which digit defaults apply, and the
                // rounded digits are what the plural rules run over.
                let notation = self
                    .intl_option(&bag, "notation", engine::number::NOTATIONS, instruction)?
                    .unwrap_or_else(|| "standard".to_owned());
                let compact_display = self
                    .intl_option(
                        &bag,
                        "compactDisplay",
                        engine::number::COMPACT_DISPLAYS,
                        instruction,
                    )?
                    .unwrap_or_else(|| "short".to_owned());
                let mut options = self.intl_digit_options(&bag, instruction)?;
                options.notation = notation;
                options.compact_display = compact_display;
                IntlValue::PluralRules(
                    engine::plurals::PluralRules::new(&locales, &kind, options).map_err(failure)?,
                )
            }
            Service::Collator => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let bag = self.intl_options_object(arguments.get(1), instruction)?;
                // The read order is the specification's, and it is observable: a getter on
                // `usage` runs before one on `localeMatcher`, and the corpus checks that.
                let usage = self
                    .intl_option(&bag, "usage", engine::collator::USAGES, instruction)?
                    .unwrap_or_else(|| "sort".to_owned());
                self.intl_locale_matcher(Some(&bag), instruction)?;
                let collation = self.intl_unicode_type(&bag, "collation", instruction)?;
                let numeric = self.intl_boolean_option(&bag, "numeric", instruction)?;
                let case_first = self.intl_option(
                    &bag,
                    "caseFirst",
                    engine::collator::CASE_FIRSTS,
                    instruction,
                )?;
                let sensitivity = self.intl_option(
                    &bag,
                    "sensitivity",
                    engine::collator::SENSITIVITIES,
                    instruction,
                )?;
                let ignore_punctuation =
                    self.intl_boolean_option(&bag, "ignorePunctuation", instruction)?;
                IntlValue::Collator(
                    engine::collator::Collator::new(
                        &locales,
                        engine::collator::Options {
                            usage,
                            collation,
                            numeric,
                            case_first,
                            sensitivity,
                            ignore_punctuation,
                        },
                    )
                    .map_err(failure)?,
                )
            }
            Service::DateTimeFormat => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let options = self.intl_date_time_options(arguments.get(1), instruction)?;
                IntlValue::DateTimeFormat(
                    engine::datetime::DateTimeFormat::new(&locales, options).map_err(failure)?,
                )
            }
            Service::RelativeTimeFormat => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let bag = self.intl_options_object(arguments.get(1), instruction)?;
                self.intl_locale_matcher(Some(&bag), instruction)?;
                let numbering_system =
                    self.intl_unicode_type(&bag, "numberingSystem", instruction)?;
                let style =
                    self.intl_option(&bag, "style", engine::relative::STYLES, instruction)?;
                let numeric =
                    self.intl_option(&bag, "numeric", engine::relative::NUMERICS, instruction)?;
                IntlValue::RelativeTimeFormat(
                    engine::relative::RelativeTimeFormat::new(
                        &locales,
                        engine::relative::Options {
                            numbering_system,
                            style,
                            numeric,
                        },
                    )
                    .map_err(failure)?,
                )
            }
            Service::DisplayNames => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                // `GetOptionsObject` rather than `CoerceOptionsToObject`: `Intl.DisplayNames` is
                // the one constructor whose options argument is required, so a missing or
                // primitive bag is a TypeError rather than an empty one.
                let bag = match arguments.get(1) {
                    Some(value)
                        if !matches!(value, Value::Null)
                            && matches!(value.type_of(), "object" | "function") =>
                    {
                        value.clone()
                    }
                    _ => return Err(type_error("options must be an object")),
                };
                self.intl_locale_matcher(Some(&bag), instruction)?;
                let style =
                    self.intl_option(&bag, "style", engine::displaynames::STYLES, instruction)?;
                let kind = self
                    .intl_option(&bag, "type", engine::displaynames::TYPES, instruction)?
                    .ok_or_else(|| type_error("type must be provided"))?;
                let fallback = self.intl_option(
                    &bag,
                    "fallback",
                    engine::displaynames::FALLBACKS,
                    instruction,
                )?;
                let language_display = self.intl_option(
                    &bag,
                    "languageDisplay",
                    engine::displaynames::LANGUAGE_DISPLAYS,
                    instruction,
                )?;
                IntlValue::DisplayNames(
                    engine::displaynames::DisplayNames::new(
                        &locales,
                        engine::displaynames::Options {
                            style,
                            kind,
                            fallback,
                            language_display,
                        },
                    )
                    .map_err(failure)?,
                )
            }
            Service::DurationFormat => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let options = self.intl_duration_options(arguments.get(1), instruction)?;
                IntlValue::DurationFormat(
                    engine::duration::DurationFormat::new(&locales, options).map_err(failure)?,
                )
            }
            Service::ListFormat => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let bag = self.intl_options_object(arguments.get(1), instruction)?;
                self.intl_locale_matcher(Some(&bag), instruction)?;
                let kind = self
                    .intl_option(&bag, "type", engine::list::TYPES, instruction)?
                    .unwrap_or_else(|| "conjunction".to_owned());
                let style = self
                    .intl_option(&bag, "style", engine::list::STYLES, instruction)?
                    .unwrap_or_else(|| "long".to_owned());
                IntlValue::ListFormat(
                    engine::list::ListFormat::new(&locales, &kind, &style).map_err(failure)?,
                )
            }
            // Unreachable: there is no `Intl.Segments`, so nothing produces this constructor.
            Service::Segments => {
                return Err(type_error("Intl.Segments is not a constructor"));
            }
            Service::Segmenter => {
                let locales = self.canonicalize_locale_list(arguments.first(), instruction)?;
                let bag = self.intl_options_object(arguments.get(1), instruction)?;
                self.intl_locale_matcher(Some(&bag), instruction)?;
                let granularity = self
                    .intl_option(
                        &bag,
                        "granularity",
                        engine::segmenter::GRANULARITIES,
                        instruction,
                    )?
                    .unwrap_or_else(|| "grapheme".to_owned());
                IntlValue::Segmenter(
                    engine::segmenter::Segmenter::new(&locales, &granularity).map_err(failure)?,
                )
            }
        };
        Ok(Value::Intl(Rc::new(built)))
    }

    fn intl_method(
        &mut self,
        service: Service,
        method: Method,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Intl(value) = receiver else {
            return Err(type_error("this is not an Intl object"));
        };
        // The brand check, before anything is read off the arguments: a method taken from one
        // service's prototype and called on another's instance is a TypeError even when both
        // services happen to answer to the name.
        if value.service() != service {
            return Err(type_error(format!(
                "Intl.{}.prototype.{} called on an Intl.{}",
                service.name(),
                js_name(Op::Method(service, method)),
                value.service().name()
            )));
        }
        match (method, value.as_ref()) {
            (Method::ResolvedOptions, _) => {
                let settings = match value.as_ref() {
                    IntlValue::NumberFormat(f) => f.resolved_options(),
                    IntlValue::PluralRules(r) => r.resolved_options(),
                    IntlValue::Collator(c) => c.resolved_options(),
                    IntlValue::DateTimeFormat(d) => d.resolved_options(),
                    IntlValue::DisplayNames(d) => d.resolved_options(),
                    IntlValue::DurationFormat(d) => d.resolved_options(),
                    IntlValue::RelativeTimeFormat(r) => r.resolved_options(),
                    IntlValue::ListFormat(l) => l.resolved_options(),
                    IntlValue::Segmenter(s) => s.resolved_options(),
                    IntlValue::Locale(_) | IntlValue::Segments(_) => {
                        return Err(type_error("this Intl object has no resolvedOptions"));
                    }
                };
                let mut data = ObjectData::default();
                for (name, setting) in settings {
                    // A property whose value is `undefined` is still created: the specification
                    // distinguishes "absent" from "present and undefined", and `Intl.Locale`'s
                    // accessors rely on the difference.
                    data.set(name, setting_to_value(&setting));
                }
                Ok(Value::Object(Rc::new(RefCell::new(data))))
            }
            (Method::Format, IntlValue::NumberFormat(formatter)) => {
                let numeric = self.intl_numeric_argument(arguments.first(), instruction)?;
                Ok(Value::String(formatter.format(&numeric).map_err(failure)?))
            }
            (Method::FormatToParts, IntlValue::NumberFormat(formatter)) => {
                let numeric = self.intl_numeric_argument(arguments.first(), instruction)?;
                let parts = formatter.format_to_parts(&numeric).map_err(failure)?;
                Ok(parts_array(&parts, "value"))
            }
            (Method::Format, IntlValue::ListFormat(formatter)) => {
                let items = self.intl_string_list(arguments.first(), instruction)?;
                Ok(Value::String(formatter.format(&items)))
            }
            (Method::FormatToParts, IntlValue::ListFormat(formatter)) => {
                let items = self.intl_string_list(arguments.first(), instruction)?;
                Ok(parts_array(&formatter.format_to_parts(&items), "value"))
            }
            (Method::Format, IntlValue::DateTimeFormat(formatter)) => {
                let (shape, at) =
                    self.intl_date_time_argument(arguments.first(), formatter, instruction)?;
                let formatted = match shape {
                    Some(shape) => formatter.format_shape(shape, at),
                    None => formatter.format(at),
                };
                Ok(Value::String(formatted.map_err(failure)?))
            }
            (Method::FormatToParts, IntlValue::DateTimeFormat(formatter)) => {
                let (shape, at) =
                    self.intl_date_time_argument(arguments.first(), formatter, instruction)?;
                let parts = match shape {
                    Some(shape) => formatter.format_shape_to_parts(shape, at),
                    None => formatter.format_to_parts(at),
                }
                .map_err(failure)?;
                Ok(parts_array(&parts, "value"))
            }
            (Method::FormatRange | Method::FormatRangeToParts, IntlValue::DateTimeFormat(f)) => {
                // Both ends are required: `formatRange(x)` is a TypeError rather than a single
                // date, which is the one place a date argument may not be `undefined`.
                if arguments
                    .first()
                    .is_none_or(|value| matches!(value, Value::Undefined))
                    || arguments
                        .get(1)
                        .is_none_or(|value| matches!(value, Value::Undefined))
                {
                    return Err(type_error("formatRange requires two dates"));
                }
                let (start_shape, start) =
                    self.intl_date_time_argument(arguments.first(), f, instruction)?;
                let (end_shape, end) =
                    self.intl_date_time_argument(arguments.get(1), f, instruction)?;
                // The two ends have to be the same kind of thing: a range from a `PlainDate` to an
                // `Instant` has no meaning the interval patterns can express, and the
                // specification refuses it rather than converting one end into the other.
                if start_shape != end_shape {
                    return Err(type_error(
                        "formatRange needs both ends to be the same kind of date",
                    ));
                }
                if method == Method::FormatRange {
                    return Ok(Value::String(
                        match start_shape {
                            Some(shape) => f.format_shape_range(shape, start, end),
                            None => f.format_range(start, end),
                        }
                        .map_err(failure)?,
                    ));
                }
                let parts = match start_shape {
                    Some(shape) => f.format_shape_range_to_parts(shape, start, end),
                    None => f.format_range_to_parts(start, end),
                }
                .map_err(failure)?;
                Ok(Value::Array(ArrayData::new(
                    parts
                        .into_iter()
                        .map(|(kind, source, text)| {
                            let mut data = ObjectData::default();
                            data.set("type", Value::String(kind.to_owned()));
                            data.set("value", Value::String(text));
                            data.set("source", Value::String(source.to_owned()));
                            Value::Object(Rc::new(RefCell::new(data)))
                        })
                        .collect(),
                )))
            }
            (Method::Format | Method::FormatToParts, IntlValue::DurationFormat(formatter)) => {
                let duration = self.intl_duration_argument(arguments.first(), instruction)?;
                if method == Method::Format {
                    return Ok(Value::String(formatter.format(&duration).map_err(failure)?));
                }
                let parts = formatter.format_to_parts(&duration).map_err(failure)?;
                Ok(Value::Array(ArrayData::new(
                    parts
                        .into_iter()
                        .map(|(kind, text, unit)| {
                            let mut data = ObjectData::default();
                            data.set("type", Value::String(kind.to_owned()));
                            data.set("value", Value::String(text));
                            // The `unit` field exists only on the runs that belong to one: a list
                            // separator is not a nanosecond's punctuation.
                            if let Some(unit) = unit {
                                data.set("unit", Value::String(unit.to_owned()));
                            }
                            Value::Object(Rc::new(RefCell::new(data)))
                        })
                        .collect(),
                )))
            }
            (Method::Of, IntlValue::DisplayNames(names)) => {
                let code = self.intl_string_argument(arguments.first(), instruction)?;
                Ok(match names.of(&code).map_err(failure)? {
                    Some(name) => Value::String(name),
                    None => Value::Undefined,
                })
            }
            (Method::Format | Method::FormatToParts, IntlValue::RelativeTimeFormat(f)) => {
                let value = self.as_number_argument(arguments.first(), instruction)?;
                let spelled = self.intl_string_argument(arguments.get(1), instruction)?;
                if !value.is_finite() {
                    return Err(range(format!("value is not finite: {value}")));
                }
                let unit = engine::relative::canonical_unit(&spelled)
                    .ok_or_else(|| range(format!("invalid unit: {spelled}")))?;
                if method == Method::Format {
                    return Ok(Value::String(f.format(value, unit).map_err(failure)?));
                }
                let parts = f.format_to_parts(value, unit).map_err(failure)?;
                Ok(Value::Array(ArrayData::new(
                    parts
                        .into_iter()
                        .map(|(kind, text, numeric)| {
                            let mut data = ObjectData::default();
                            data.set("type", Value::String(kind.to_owned()));
                            data.set("value", Value::String(text));
                            if numeric {
                                data.set("unit", Value::String(unit.to_owned()));
                            }
                            Value::Object(Rc::new(RefCell::new(data)))
                        })
                        .collect(),
                )))
            }
            (Method::Select, IntlValue::PluralRules(rules)) => {
                let number = self.as_number_argument(arguments.first(), instruction)?;
                Ok(Value::String(
                    rules
                        .select(&engine::number::Numeric::Double(number))
                        .map_err(failure)?
                        .to_owned(),
                ))
            }
            (Method::SelectRange, IntlValue::PluralRules(rules)) => {
                // Both ends are required and neither may be NaN: `selectRange(1)` is a TypeError
                // and `selectRange(NaN, 2)` a RangeError, which is a distinction the corpus makes.
                for index in 0..2 {
                    if arguments
                        .get(index)
                        .is_none_or(|v| matches!(v, Value::Undefined))
                    {
                        return Err(type_error("selectRange requires two values"));
                    }
                }
                let start = self.as_number_argument(arguments.first(), instruction)?;
                let end = self.as_number_argument(arguments.get(1), instruction)?;
                if start.is_nan() || end.is_nan() {
                    return Err(range("selectRange was given a value that is not a number"));
                }
                Ok(Value::String(
                    rules
                        .select_range(
                            &engine::number::Numeric::Double(start),
                            &engine::number::Numeric::Double(end),
                        )
                        .map_err(failure)?
                        .to_owned(),
                ))
            }
            (Method::Compare, IntlValue::Collator(collator)) => {
                let left = self.intl_string_argument(arguments.first(), instruction)?;
                let right = self.intl_string_argument(arguments.get(1), instruction)?;
                let ordering = collator.compare(&utf16(&left), &utf16(&right));
                Ok(Value::Number(match ordering {
                    std::cmp::Ordering::Less => -1.0,
                    std::cmp::Ordering::Equal => 0.0,
                    std::cmp::Ordering::Greater => 1.0,
                }))
            }
            (Method::Segment, IntlValue::Segmenter(segmenter)) => {
                let text = self.intl_string_argument(arguments.first(), instruction)?;
                // A `Segments` object rather than an array of records: the language's `Segments`
                // is iterable *and* answers `containing(index)`, and only an object with its own
                // prototype has somewhere for the second half to live.
                Ok(Value::Intl(Rc::new(IntlValue::Segments(
                    engine::segmenter::Segments::new(segmenter, &text),
                ))))
            }
            (Method::Containing, IntlValue::Segments(segments)) => {
                let index = self.as_number_argument(arguments.first(), instruction)?;
                // `ToIntegerOrInfinity`, then the bounds check. An index outside the string is
                // `undefined` rather than an error: "which segment holds code unit -1" has a
                // legitimate answer of "none".
                let index = if index.is_nan() { 0.0 } else { index.trunc() };
                if index < 0.0 || index >= segments.len() as f64 {
                    return Ok(Value::Undefined);
                }
                Ok(match segments.containing(index as usize) {
                    Some(segment) => segment_record(segments, segment),
                    None => Value::Undefined,
                })
            }
            (Method::SegmentIterator, IntlValue::Segments(segments)) => {
                // The segments as an array, iterated by the array iterator. The specification's
                // `%SegmentIterator%` walks the string lazily; this walks a list that was already
                // computed, which is the same sequence of records in the same order -- and the
                // list is what `containing` needs anyway, so computing it twice would be the only
                // difference.
                let items: Vec<Value> = segments
                    .list()
                    .iter()
                    .map(|segment| segment_record(segments, segment))
                    .collect();
                let array = Value::Array(ArrayData::new(items));
                let method =
                    self.read_property(&array, WellKnown::Iterator.property_key(), instruction)?;
                self.call_value(&method, array, Vec::new(), instruction)
            }
            (Method::Maximize, IntlValue::Locale(tag)) => Ok(Value::Intl(Rc::new(
                IntlValue::Locale(engine::locale::maximize(tag).map_err(failure)?),
            ))),
            (Method::Minimize, IntlValue::Locale(tag)) => Ok(Value::Intl(Rc::new(
                IntlValue::Locale(engine::locale::minimize(tag).map_err(failure)?),
            ))),
            (Method::LocaleToString, IntlValue::Locale(tag)) => Ok(Value::String(tag.clone())),
            // The brand check: `Intl.NumberFormat.prototype.format.call(collator)` is a TypeError
            // rather than a formatted anything.
            (method, value) => Err(type_error(format!(
                "Intl.{}.prototype.{} called on the wrong kind of object",
                value.service().name(),
                js_name(Op::Method(service, method))
            ))),
        }
    }

    /// `CanonicalizeLocaleList`: the JavaScript half, which is observable.
    ///
    /// Written out rather than delegated because every step of it can run user code -- a getter
    /// on `length`, a proxy `has` trap, a `toString` on an element -- and the order those run in
    /// is what a large part of the corpus is checking.
    fn canonicalize_locale_list(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<Vec<String>, ExecutionError> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        if matches!(value, Value::Undefined) {
            return Ok(Vec::new());
        }
        // A bare string is treated as a one-element list, and so is a `Intl.Locale`.
        let elements: Vec<Value> = match value {
            Value::String(_) | Value::Intl(_) => vec![value.clone()],
            Value::Array(array) => array.borrow().clone(),
            Value::Undefined | Value::Null => {
                return Err(type_error("locales is not an object"));
            }
            _ => {
                let length = self.read_property(value, "length", instruction)?;
                let length = self.as_number_argument(Some(&length), instruction)?;
                if !length.is_finite() || length < 0.0 {
                    Vec::new()
                } else {
                    let mut out = Vec::new();
                    for index in 0..(length as u64) {
                        let key = index.to_string();
                        let element = self.read_property(value, &key, instruction)?;
                        if matches!(element, Value::Undefined) {
                            continue;
                        }
                        out.push(element);
                    }
                    out
                }
            }
        };

        let mut seen: Vec<String> = Vec::new();
        for element in elements {
            let tag = match &element {
                Value::Intl(value) => match value.as_ref() {
                    IntlValue::Locale(tag) => tag.clone(),
                    _ => return Err(type_error("a locale list element is not a string")),
                },
                Value::String(text) => text.clone(),
                Value::Object(_) | Value::Array(_) => {
                    let primitive = self.as_primitive(&element, true, instruction)?;
                    primitive.to_display_string()
                }
                _ => {
                    return Err(type_error(
                        "a locale list element is not a string or object",
                    ));
                }
            };
            let canonical = engine::locale::canonicalize(&tag).map_err(failure)?;
            if !seen.contains(&canonical) {
                seen.push(canonical);
            }
        }
        Ok(seen)
    }

    /// `CoerceOptionsToObject`/`GetOptionsObject`: `undefined` becomes an empty bag.
    fn intl_options_object(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let _ = instruction;
        match value {
            None | Some(Value::Undefined) => Ok(empty_options()),
            Some(Value::Null) => Err(type_error("options is null")),
            Some(value) => Ok(value.clone()),
        }
    }

    /// `GetOption(options, name, string, values, undefined)`.
    fn intl_option(
        &mut self,
        options: &Value,
        name: &str,
        values: &[&str],
        instruction: usize,
    ) -> Result<Option<String>, ExecutionError> {
        let read = self.read_property(options, name, instruction)?;
        if matches!(read, Value::Undefined) {
            return Ok(None);
        }
        let text = self.intl_string_argument(Some(&read), instruction)?;
        if !values.contains(&text.as_str()) {
            return Err(range(format!(
                "value {text} out of range for option {name}"
            )));
        }
        Ok(Some(text))
    }

    /// `GetOption(options, name, boolean, empty, undefined)`.
    fn intl_boolean_option(
        &mut self,
        options: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<Option<bool>, ExecutionError> {
        let read = self.read_property(options, name, instruction)?;
        if matches!(read, Value::Undefined) {
            return Ok(None);
        }
        Ok(Some(read.is_truthy()))
    }

    /// `localeMatcher`, read and validated for its side effect on ordering and nothing else.
    fn intl_locale_matcher(
        &mut self,
        options: Option<&Value>,
        instruction: usize,
    ) -> Result<(), ExecutionError> {
        let bag = self.intl_options_object(options, instruction)?;
        self.intl_option(
            &bag,
            "localeMatcher",
            engine::locale::LOCALE_MATCHERS,
            instruction,
        )?;
        Ok(())
    }

    /// An option whose value has to be a well-formed Unicode extension `type`.
    ///
    /// `collation` and `numberingSystem` are not chosen from a fixed list -- any 3-to-8 character
    /// alphanumeric subtag is *well formed*, and whether there is data for it is a separate
    /// question the adapter answers later. A value that is not well formed is a `RangeError` here,
    /// before it can reach the engine.
    fn intl_unicode_type(
        &mut self,
        options: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<Option<String>, ExecutionError> {
        let read = self.read_property(options, name, instruction)?;
        if matches!(read, Value::Undefined) {
            return Ok(None);
        }
        let text = self.intl_string_argument(Some(&read), instruction)?;
        let well_formed = !text.is_empty()
            && text.split('-').all(|part| {
                (3..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
            });
        if !well_formed {
            return Err(range(format!(
                "value {text} out of range for option {name}"
            )));
        }
        Ok(Some(text))
    }

    /// `GetNumberOption(options, name, minimum, maximum, fallback)`.
    fn intl_number_option(
        &mut self,
        options: &Value,
        name: &str,
        minimum: f64,
        maximum: f64,
        fallback: f64,
        instruction: usize,
    ) -> Result<f64, ExecutionError> {
        let read = self.read_property(options, name, instruction)?;
        if matches!(read, Value::Undefined) {
            return Ok(fallback);
        }
        let number = self.as_number_argument(Some(&read), instruction)?;
        if number.is_nan() || number < minimum || number > maximum {
            return Err(range(format!(
                "value {number} out of range for option {name}"
            )));
        }
        Ok(number.floor())
    }

    /// A digit option read with `Get` rather than `GetNumberOption`, and coerced here.
    ///
    /// The four `minimum`/`maximum` digit options are *got* in one order and *coerced* in another,
    /// and both orders are observable through getters, so the read and the coercion are separate
    /// steps rather than one helper.
    fn intl_raw_number(
        &mut self,
        options: &Value,
        name: &str,
        instruction: usize,
    ) -> Result<Option<f64>, ExecutionError> {
        let read = self.read_property(options, name, instruction)?;
        if matches!(read, Value::Undefined) {
            return Ok(None);
        }
        Ok(Some(self.as_number_argument(Some(&read), instruction)?))
    }

    /// The digit options `Intl.NumberFormat` and `Intl.PluralRules` share.
    fn intl_digit_options(
        &mut self,
        bag: &Value,
        instruction: usize,
    ) -> Result<engine::number::Options, ExecutionError> {
        let minimum_integer_digits =
            self.intl_number_option(bag, "minimumIntegerDigits", 1.0, 21.0, 1.0, instruction)?;
        let minimum_fraction_digits =
            self.intl_raw_number(bag, "minimumFractionDigits", instruction)?;
        let maximum_fraction_digits =
            self.intl_raw_number(bag, "maximumFractionDigits", instruction)?;
        let minimum_significant_digits =
            self.intl_raw_number(bag, "minimumSignificantDigits", instruction)?;
        let maximum_significant_digits =
            self.intl_raw_number(bag, "maximumSignificantDigits", instruction)?;
        let rounding_increment =
            self.intl_number_option(bag, "roundingIncrement", 1.0, 5000.0, 1.0, instruction)?;
        let rounding_mode = self
            .intl_option(
                bag,
                "roundingMode",
                engine::number::ROUNDING_MODES,
                instruction,
            )?
            .unwrap_or_else(|| "halfExpand".to_owned());
        let rounding_priority = self
            .intl_option(
                bag,
                "roundingPriority",
                engine::number::ROUNDING_PRIORITIES,
                instruction,
            )?
            .unwrap_or_else(|| "auto".to_owned());
        let trailing_zero_display = self
            .intl_option(
                bag,
                "trailingZeroDisplay",
                engine::number::TRAILING_ZERO_DISPLAYS,
                instruction,
            )?
            .unwrap_or_else(|| "auto".to_owned());
        Ok(engine::number::Options {
            minimum_integer_digits,
            minimum_fraction_digits,
            maximum_fraction_digits,
            minimum_significant_digits,
            maximum_significant_digits,
            rounding_increment,
            rounding_mode,
            rounding_priority,
            trailing_zero_display,
            ..engine::number::Options::default()
        })
    }

    /// `InitializeNumberFormat`'s option reads, in the specification's order.
    fn intl_number_options(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<engine::number::Options, ExecutionError> {
        let bag = self.intl_options_object(value, instruction)?;
        self.intl_locale_matcher(Some(&bag), instruction)?;
        let numbering_system = self.intl_unicode_type(&bag, "numberingSystem", instruction)?;
        let style = self
            .intl_option(&bag, "style", engine::number::STYLES, instruction)?
            .unwrap_or_else(|| "decimal".to_owned());
        let currency = {
            let read = self.read_property(&bag, "currency", instruction)?;
            if matches!(read, Value::Undefined) {
                None
            } else {
                let text = self.intl_string_argument(Some(&read), instruction)?;
                // A currency code is three letters and nothing else. This is a *syntax* check, so
                // it is a RangeError whether or not there is data for the code.
                if text.len() != 3 || !text.chars().all(|c| c.is_ascii_alphabetic()) {
                    return Err(range(format!("invalid currency code: {text}")));
                }
                Some(text.to_ascii_uppercase())
            }
        };
        if style == "currency" && currency.is_none() {
            return Err(type_error(
                "currency must be provided when style is \"currency\"",
            ));
        }
        let currency_display = self
            .intl_option(
                &bag,
                "currencyDisplay",
                engine::number::CURRENCY_DISPLAYS,
                instruction,
            )?
            .unwrap_or_else(|| "symbol".to_owned());
        let currency_sign = self
            .intl_option(
                &bag,
                "currencySign",
                engine::number::CURRENCY_SIGNS,
                instruction,
            )?
            .unwrap_or_else(|| "standard".to_owned());
        let unit = {
            let read = self.read_property(&bag, "unit", instruction)?;
            if matches!(read, Value::Undefined) {
                None
            } else {
                Some(self.intl_string_argument(Some(&read), instruction)?)
            }
        };
        if style == "unit" && unit.is_none() {
            return Err(type_error("unit must be provided when style is \"unit\""));
        }
        let unit_display = self
            .intl_option(
                &bag,
                "unitDisplay",
                engine::number::UNIT_DISPLAYS,
                instruction,
            )?
            .unwrap_or_else(|| "short".to_owned());

        let mut options = self.intl_digit_options(&bag, instruction)?;
        options.numbering_system = numbering_system;
        options.style = style;
        options.currency = currency;
        options.currency_display = currency_display;
        options.currency_sign = currency_sign;
        options.unit = unit;
        options.unit_display = unit_display;

        options.notation = self
            .intl_option(&bag, "notation", engine::number::NOTATIONS, instruction)?
            .unwrap_or_else(|| "standard".to_owned());
        options.compact_display = self
            .intl_option(
                &bag,
                "compactDisplay",
                engine::number::COMPACT_DISPLAYS,
                instruction,
            )?
            .unwrap_or_else(|| "short".to_owned());
        // `useGrouping` is the one option with three possible types, so it cannot go through
        // `GetOption`: `false` means never, `true` and `"always"` mean always, a string is checked
        // against the vocabulary, and `undefined` means the locale decides.
        let grouping = self.read_property(&bag, "useGrouping", instruction)?;
        options.use_grouping = match &grouping {
            Value::Undefined => engine::number::Grouping::Auto,
            Value::Boolean(false) => engine::number::Grouping::Never,
            Value::Boolean(true) => engine::number::Grouping::Always,
            _ => {
                let text = self.intl_string_argument(Some(&grouping), instruction)?;
                match text.as_str() {
                    "min2" => engine::number::Grouping::Min2,
                    "auto" => engine::number::Grouping::Auto,
                    "always" | "true" => engine::number::Grouping::Always,
                    "false" => engine::number::Grouping::Never,
                    _ => {
                        return Err(range(format!(
                            "value {text} out of range for option useGrouping"
                        )));
                    }
                }
            }
        };
        options.sign_display = self
            .intl_option(
                &bag,
                "signDisplay",
                engine::number::SIGN_DISPLAYS,
                instruction,
            )?
            .unwrap_or_else(|| "auto".to_owned());
        Ok(options)
    }

    /// `CreateDurationFormat`'s option reads, in the specification's order.
    ///
    /// `GetDurationUnitOptions` is written out here rather than delegated because it interleaves
    /// reads with decisions: the *default* a unit's `Display` option falls back to depends on what
    /// the unit's own style read returned, and both reads can run user code. An adapter that only
    /// saw the finished values could not put the getters in the right order, and the corpus checks
    /// the order.
    fn intl_duration_options(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<engine::duration::Options, ExecutionError> {
        use engine::duration as df;

        // `GetOptionsObject`, not `CoerceOptionsToObject`: a primitive options bag is a TypeError.
        let bag = match value {
            None | Some(Value::Undefined) => empty_options(),
            Some(value)
                if !matches!(value, Value::Null)
                    && matches!(value.type_of(), "object" | "function") =>
            {
                value.clone()
            }
            _ => return Err(type_error("options must be an object")),
        };
        self.intl_option(
            &bag,
            "localeMatcher",
            engine::locale::LOCALE_MATCHERS,
            instruction,
        )?;
        let numbering_system = self.intl_unicode_type(&bag, "numberingSystem", instruction)?;
        let style = self
            .intl_option(&bag, "style", df::STYLES, instruction)?
            .unwrap_or_else(|| "short".to_owned());

        let mut units = Vec::with_capacity(df::UNITS.len());
        // `prevStyle`, in the specification's internal vocabulary. `"fractional"` is a style the
        // language cannot write and never reports, and it is exactly what makes a sub-second unit
        // fold into the second before it, so it has to be tracked apart from what is reported.
        let mut previous = String::new();
        for unit in df::UNITS {
            let asked = self.intl_option(&bag, unit, df::styles_for(unit), instruction)?;
            let mut display_default = "always";
            let mut settled = match asked {
                Some(style) => style,
                None if style == "digital" => {
                    if !matches!(*unit, "hours" | "minutes" | "seconds") {
                        display_default = "auto";
                    }
                    df::digital_default(unit).to_owned()
                }
                None if matches!(previous.as_str(), "fractional" | "numeric" | "2-digit") => {
                    if !matches!(*unit, "minutes" | "seconds") {
                        display_default = "auto";
                    }
                    "numeric".to_owned()
                }
                None => {
                    display_default = "auto";
                    style.clone()
                }
            };
            // A `numeric` sub-second unit is a *fractional* one: it has no field of its own and
            // becomes digits after the second's decimal point.
            if settled == "numeric"
                && matches!(*unit, "milliseconds" | "microseconds" | "nanoseconds")
            {
                settled = "fractional".to_owned();
                display_default = "auto";
            }
            let display = self
                .intl_option(&bag, &format!("{unit}Display"), df::DISPLAYS, instruction)?
                .unwrap_or_else(|| display_default.to_owned());
            if display == "always" && settled == "fractional" {
                return Err(range(format!(
                    "{unit} cannot be displayed always while it is a fraction of a second"
                )));
            }
            if previous == "fractional" && settled != "fractional" {
                return Err(range(format!(
                    "{unit} must be numeric because the unit before it is a fraction"
                )));
            }
            if matches!(previous.as_str(), "numeric" | "2-digit") {
                if !matches!(settled.as_str(), "fractional" | "numeric" | "2-digit") {
                    return Err(range(format!(
                        "{unit} cannot be {settled} after a numeric unit"
                    )));
                }
                if matches!(*unit, "minutes" | "seconds") {
                    settled = "2-digit".to_owned();
                }
            }
            previous.clone_from(&settled);
            // What the adapter and `resolvedOptions()` see. The internal `"fractional"` is spelled
            // `"numeric"` everywhere outside this loop, which is the specification's own mapping.
            let reported = if settled == "fractional" {
                "numeric".to_owned()
            } else {
                settled
            };
            units.push((reported, display));
        }

        let fractional_digits = {
            let read = self.read_property(&bag, "fractionalDigits", instruction)?;
            if matches!(read, Value::Undefined) {
                None
            } else {
                let number = self.as_number_argument(Some(&read), instruction)?;
                if number.is_nan() || !(0.0..=9.0).contains(&number) {
                    return Err(range(format!(
                        "value {number} out of range for option fractionalDigits"
                    )));
                }
                Some(number.floor() as u8)
            }
        };

        Ok(df::Options {
            numbering_system,
            style,
            units,
            fractional_digits,
        })
    }

    /// `ToTemporalDuration`, then the ten magnitudes and the one sign the formatter wants.
    ///
    /// The conversion is `temporal.rs`'s rather than a second copy: a duration argument is a
    /// `Temporal.Duration`, an ISO 8601 duration string or a property bag, and which of the three
    /// it is -- and which of them is a `TypeError` rather than a `RangeError` -- is settled there
    /// for every other caller already.
    fn intl_duration_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<engine::duration::Value, ExecutionError> {
        let argument = value.cloned().unwrap_or(Value::Undefined);
        let duration = self.coerce_temporal(
            kiln_temporal::Kind::Duration,
            &argument,
            &kiln_temporal::Options::default(),
            instruction,
        )?;
        let components = kiln_temporal::duration_components(&duration)
            .ok_or_else(|| type_error("this is not a Temporal.Duration"))?;
        // Every component of a valid duration has the same direction, so one flag answers for all
        // ten and each magnitude travels unsigned.
        let negative = components.iter().any(|amount| *amount < 0);
        let mut units = [0u128; 10];
        for (slot, amount) in units.iter_mut().zip(components) {
            *slot = amount.unsigned_abs();
        }
        Ok(engine::duration::Value { negative, units })
    }

    /// `ToDateTimeFormattable`, followed by the part of `HandleDateTimeValue` that is the
    /// machine's: which shape the value is, and where on the timeline it sits.
    ///
    /// A `Temporal` object is passed through as itself rather than coerced. That is the whole
    /// point of the operation: `ToNumber(plainDate)` is `NaN`, so a formatter that did not
    /// recognise the object would refuse every `Temporal` value with a `RangeError` about a
    /// non-finite date -- a wrong diagnosis of a value it simply had not been taught to read.
    fn intl_date_time_argument(
        &mut self,
        value: Option<&Value>,
        formatter: &engine::datetime::DateTimeFormat,
        instruction: usize,
    ) -> Result<(Option<engine::datetime::Shape>, f64), ExecutionError> {
        use engine::datetime::Shape;

        let Some(Value::Temporal(temporal)) = value else {
            return Ok((None, self.intl_instant_argument(value, instruction)?));
        };
        let shape = match temporal.kind() {
            kiln_temporal::Kind::Instant => Shape::Instant,
            kiln_temporal::Kind::PlainDate => Shape::PlainDate,
            kiln_temporal::Kind::PlainDateTime => Shape::PlainDateTime,
            kiln_temporal::Kind::PlainYearMonth => Shape::PlainYearMonth,
            kiln_temporal::Kind::PlainMonthDay => Shape::PlainMonthDay,
            kiln_temporal::Kind::PlainTime => Shape::PlainTime,
            // A `ZonedDateTime` carries its own zone, and formatting it in the formatter's would
            // silently answer a different question; the specification says to call its own
            // `toLocaleString` instead.
            kiln_temporal::Kind::ZonedDateTime => {
                return Err(type_error(
                    "Intl.DateTimeFormat does not format a Temporal.ZonedDateTime; \
                     use its own toLocaleString",
                ));
            }
            kiln_temporal::Kind::Duration => {
                return Err(type_error(
                    "Intl.DateTimeFormat does not format a Temporal.Duration",
                ));
            }
        };
        // A value in one calendar formatted by a formatter in another would be a different date
        // with the same digits. `iso8601` is the exception the specification makes, because it is
        // the calendar a `Temporal` value has when its program never chose one.
        if let Some(kiln_temporal::Field::Text(calendar)) = temporal.get("calendarId")
            && calendar != "iso8601"
            && calendar != formatter.calendar()
        {
            return Err(range(format!(
                "a Temporal value in the {calendar} calendar cannot be formatted by a formatter \
                 in the {} calendar",
                formatter.calendar()
            )));
        }
        let at = kiln_temporal::format_instant(temporal)
            .ok_or_else(|| type_error("this Temporal value has no instant to format"))?;
        Ok((Some(shape), at))
    }

    /// `ToIntlMathematicalValue`: a Number, a BigInt or a numeric string, kept apart.
    fn intl_numeric_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<engine::number::Numeric, ExecutionError> {
        match value {
            Some(Value::BigInt(integer)) => Ok(engine::number::Numeric::Exact(integer.to_string())),
            Some(Value::String(text)) => {
                // A string keeps every digit it was written with rather than going through a
                // double, which is the whole reason the specification widened this argument.
                let trimmed = text.trim();
                if trimmed
                    .strip_prefix(['+', '-'])
                    .unwrap_or(trimmed)
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.')
                    && trimmed.chars().any(|c| c.is_ascii_digit())
                {
                    Ok(engine::number::Numeric::Exact(trimmed.to_owned()))
                } else {
                    let number = self.as_number_argument(value, instruction)?;
                    Ok(engine::number::Numeric::Double(number))
                }
            }
            _ => Ok(engine::number::Numeric::Double(
                self.as_number_argument(value, instruction)?,
            )),
        }
    }

    /// `StringListFromIterable`.
    ///
    /// The iterator protocol rather than a `length` walk, because the difference is observable:
    /// the argument is an *iterable*, so a user `Symbol.iterator` runs, its `next` runs once per
    /// element, and a throw from either has to propagate out of `format`. Reading indices off it
    /// would produce the right answer for an array and silently skip a program's own iterator.
    fn intl_string_list(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<Vec<String>, ExecutionError> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        if matches!(value, Value::Undefined) {
            return Ok(Vec::new());
        }
        let method = self.read_property(value, WellKnown::Iterator.property_key(), instruction)?;
        if method.is_nullish() {
            return Err(type_error("the list is not iterable"));
        }
        let iterator = self.call_value(&method, value.clone(), Vec::new(), instruction)?;
        let next = self.read_property(&iterator, "next", instruction)?;
        let mut items = Vec::new();
        loop {
            let step = self.call_value(&next, iterator.clone(), Vec::new(), instruction)?;
            if self.read_property(&step, "done", instruction)?.is_truthy() {
                return Ok(items);
            }
            let element = self.read_property(&step, "value", instruction)?;
            let Value::String(text) = element else {
                // Not `ToString`: a non-string element is a TypeError, so that a number in a list
                // is refused rather than quietly spelled out in whatever locale happens to apply.
                return Err(type_error("a list element is not a string"));
            };
            items.push(text);
        }
    }

    /// `CreateDateTimeFormat`'s option reads, in the specification's order.
    ///
    /// Written out rather than folded into a loop because the order is the thing being tested:
    /// `constructor-options-order.js` asserts the exact sequence of gets and `toString` calls, and
    /// each of the eleven component options is a separate `GetOption` whose coercion can run user
    /// code.
    fn intl_date_time_options(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<engine::datetime::Options, ExecutionError> {
        use engine::datetime as dtf;

        let bag = self.intl_options_object(value, instruction)?;
        self.intl_option(
            &bag,
            "localeMatcher",
            engine::locale::LOCALE_MATCHERS,
            instruction,
        )?;
        let calendar = self.intl_unicode_type(&bag, "calendar", instruction)?;
        let numbering_system = self.intl_unicode_type(&bag, "numberingSystem", instruction)?;
        let hour12 = self.intl_boolean_option(&bag, "hour12", instruction)?;
        let hour_cycle = self.intl_option(&bag, "hourCycle", dtf::HOUR_CYCLES, instruction)?;
        // The specification drops `hourCycle` outright once `hour12` has spoken, so a program
        // that sets both does not get to see the loser reported back.
        let hour_cycle = if hour12.is_some() { None } else { hour_cycle };

        let time_zone = {
            let read = self.read_property(&bag, "timeZone", instruction)?;
            if matches!(read, Value::Undefined) {
                // `DefaultTimeZone()`. ADR 0015 fixed it at UTC and nothing here asks the host.
                dtf::canonical_time_zone("UTC")
            } else {
                let text = self.intl_string_argument(Some(&read), instruction)?;
                match offset_time_zone(&text) {
                    Some(seconds) => Some(dtf::TimeZoneChoice::Offset(seconds)),
                    None => Some(
                        dtf::canonical_time_zone(&text)
                            .ok_or_else(|| range(format!("invalid time zone: {text}")))?,
                    ),
                }
            }
        };

        let weekday = self.intl_option(&bag, "weekday", dtf::TEXT_WIDTHS, instruction)?;
        let era = self.intl_option(&bag, "era", dtf::TEXT_WIDTHS, instruction)?;
        let year = self.intl_option(&bag, "year", dtf::NUMERIC_WIDTHS, instruction)?;
        let month = self.intl_option(&bag, "month", dtf::MONTH_WIDTHS, instruction)?;
        let day = self.intl_option(&bag, "day", dtf::NUMERIC_WIDTHS, instruction)?;
        let day_period = self.intl_option(&bag, "dayPeriod", dtf::TEXT_WIDTHS, instruction)?;
        let hour = self.intl_option(&bag, "hour", dtf::NUMERIC_WIDTHS, instruction)?;
        let minute = self.intl_option(&bag, "minute", dtf::NUMERIC_WIDTHS, instruction)?;
        let second = self.intl_option(&bag, "second", dtf::NUMERIC_WIDTHS, instruction)?;
        let fractional_second_digits = {
            let read = self.read_property(&bag, "fractionalSecondDigits", instruction)?;
            if matches!(read, Value::Undefined) {
                None
            } else {
                let number = self.as_number_argument(Some(&read), instruction)?;
                if number.is_nan() || !(1.0..=3.0).contains(&number) {
                    return Err(range(format!(
                        "value {number} out of range for option fractionalSecondDigits"
                    )));
                }
                Some(number.floor() as u8)
            }
        };
        let time_zone_name =
            self.intl_option(&bag, "timeZoneName", dtf::ZONE_NAMES, instruction)?;
        self.intl_option(&bag, "formatMatcher", dtf::FORMAT_MATCHERS, instruction)?;
        let date_style = self.intl_option(&bag, "dateStyle", dtf::STYLES, instruction)?;
        let time_style = self.intl_option(&bag, "timeStyle", dtf::STYLES, instruction)?;

        let options = dtf::Options {
            calendar,
            numbering_system,
            hour12,
            hour_cycle,
            time_zone,
            weekday,
            era,
            year,
            month,
            day,
            day_period,
            hour,
            minute,
            second,
            fractional_second_digits,
            time_zone_name,
            date_style,
            time_style,
        };
        if (options.date_style.is_some() || options.time_style.is_some())
            && options.has_components()
        {
            return Err(type_error(
                "dateStyle and timeStyle cannot be combined with explicit date-time components",
            ));
        }
        Ok(options)
    }

    /// The date argument of `format`, `formatToParts` and the range pair.
    ///
    /// `undefined` is the current instant, which is the realm's virtual clock rather than the
    /// host's (ADR 0015), so an unqualified `format()` reports the epoch. Everything else is
    /// `ToNumber` followed by `TimeClip`, and a value outside the representable range is a
    /// `RangeError` rather than a formatted approximation.
    fn intl_instant_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<f64, ExecutionError> {
        let milliseconds = match value {
            None | Some(Value::Undefined) => self.realm.now().as_secs_f64() * 1_000.0,
            Some(value) => self.as_number_argument(Some(value), instruction)?,
        };
        if !milliseconds.is_finite() || milliseconds.abs() > 8.64e15 {
            return Err(range(format!("date value is not finite: {milliseconds}")));
        }
        Ok(milliseconds.trunc() + 0.0)
    }

    /// `ToString`, with `undefined` becoming the word rather than staying absent.
    fn intl_string_argument(
        &mut self,
        value: Option<&Value>,
        instruction: usize,
    ) -> Result<String, ExecutionError> {
        let Some(value) = value else {
            return Ok("undefined".to_owned());
        };
        if matches!(value, Value::Symbol(_)) {
            return Err(ExecutionError::SymbolIsNotAString);
        }
        let primitive = self.as_primitive(value, true, instruction)?;
        if matches!(primitive, Value::Symbol(_)) {
            return Err(ExecutionError::SymbolIsNotAString);
        }
        Ok(primitive.to_display_string())
    }
}

/// One segment, as the `{ segment, index, input }` record the language hands out.
///
/// `isWordLike` is *absent* rather than `false` for a grapheme or sentence segmenter: the
/// specification only creates the property for `granularity: "word"`, and a program that tests
/// `"isWordLike" in record` can tell the difference.
fn segment_record(
    segments: &engine::segmenter::Segments,
    segment: &engine::segmenter::Segment,
) -> Value {
    let mut data = ObjectData::default();
    data.set("segment", Value::String(segments.text_of(segment)));
    data.set("index", Value::Number(segment.start as f64));
    data.set("input", Value::String(segments.input().to_owned()));
    if let Some(word_like) = segment.word_like {
        data.set("isWordLike", Value::Boolean(word_like));
    }
    Value::Object(Rc::new(RefCell::new(data)))
}

/// A `formatToParts` result: an array of `{ type, value }` objects.
fn parts_array(parts: &[(&'static str, String)], value_key: &str) -> Value {
    Value::Array(ArrayData::new(
        parts
            .iter()
            .map(|(kind, text)| {
                let mut data = ObjectData::default();
                data.set("type", Value::String((*kind).to_owned()));
                data.set(value_key, Value::String(text.clone()));
                Value::Object(Rc::new(RefCell::new(data)))
            })
            .collect(),
    ))
}

/// The empty options bag `GetOptionsObject` and `CoerceOptionsToObject` create for a missing one.
///
/// `OrdinaryObjectCreate(null)`, and the null matters: a program that puts `maximumFractionDigits`
/// on `Object.prototype` must not change what `(1.23).toLocaleString("en")` produces, and an
/// ordinary empty object would inherit it.
fn empty_options() -> Value {
    let mut data = ObjectData::default();
    data.set_prototype(Value::Null);
    Value::Object(Rc::new(RefCell::new(data)))
}

/// `thisNumberValue`: the Number a `Number.prototype` method is running on.
///
/// A `Number` *object* answers too -- `Number.prototype.toLocaleString.call(new Number(5))` is
/// "5", not a TypeError -- because the specification reads the wrapper's boxed primitive rather
/// than requiring a primitive receiver.
fn this_number_value(receiver: &Value) -> Result<f64, ExecutionError> {
    match receiver {
        Value::Number(value) => Ok(*value),
        Value::Object(object) => match object.borrow().primitive() {
            Some(Value::Number(value)) => Ok(*value),
            _ => Err(type_error(
                "Number.prototype.toLocaleString called on a value that is not a number",
            )),
        },
        other => Err(type_error(format!(
            "Number.prototype.toLocaleString called on {}",
            other.to_display_string()
        ))),
    }
}

/// One positional argument, with the absent ones spelled `undefined` the way a call does.
fn argument(arguments: &[Value], index: usize) -> Value {
    arguments.get(index).cloned().unwrap_or(Value::Undefined)
}

/// `ToDateTimeOptions`' `needDefaults`: whether the request named nothing the method cares about.
///
/// Only the fields the method is *for* count. `toLocaleTimeString(undefined, { year: "numeric" })`
/// still defaults its hour, minute and second, because a year is not a time -- and it keeps the
/// year too, since the operation adds defaults rather than replacing what it was given.
fn needs_date_time_defaults(
    options: &engine::datetime::Options,
    wants_date: bool,
    wants_time: bool,
) -> bool {
    let date_named = options.weekday.is_some()
        || options.year.is_some()
        || options.month.is_some()
        || options.day.is_some();
    let time_named = options.day_period.is_some()
        || options.hour.is_some()
        || options.minute.is_some()
        || options.second.is_some()
        || options.fractional_second_digits.is_some();
    if wants_date && date_named {
        return false;
    }
    if wants_time && time_named {
        return false;
    }
    // A style is a complete request on its own, and a method that takes both accepts either one
    // as evidence that the caller has said what they want.
    if wants_date && wants_time && (options.date_style.is_some() || options.time_style.is_some()) {
        return false;
    }
    if wants_date && !wants_time && options.date_style.is_some() {
        return false;
    }
    if wants_time && !wants_date && options.time_style.is_some() {
        return false;
    }
    true
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `IsTimeZoneOffsetString`, in the shape ECMA-402 restricts it to.
///
/// The grammar is `UTCOffset` without sub-minute precision: a sign, two hour digits in 00..23,
/// and optionally two minute digits in 00..59 with or without a colon. Everything else -- a
/// one-digit hour, a seconds field, a Unicode minus sign, a fraction -- is a `RangeError`, which
/// is what makes `+15:59:00` a rejection rather than a zone.
fn offset_time_zone(text: &str) -> Option<i32> {
    let mut characters = text.chars();
    let sign = match characters.next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let rest: Vec<char> = characters.collect();
    let digits = |slice: &[char]| -> Option<i32> {
        if slice.len() == 2 && slice.iter().all(char::is_ascii_digit) {
            slice.iter().collect::<String>().parse().ok()
        } else {
            None
        }
    };
    let (hours, minutes) = match rest.len() {
        2 => (digits(&rest)?, 0),
        4 => (digits(&rest[..2])?, digits(&rest[2..])?),
        5 if rest[2] == ':' => (digits(&rest[..2])?, digits(&rest[3..])?),
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// `Intl.supportedValuesOf`.
///
/// Four of the six lists are things this build can enumerate. `currency` and `unit` are empty on
/// purpose rather than by omission: the specification defines each as the values "for which the
/// implementation provides the functionality of `Intl.NumberFormat`", and this build refuses
/// `style: "currency"` and `style: "unit"` outright (see `kiln_intl::number`), so claiming a
/// currency it cannot format would be the wrong answer rather than the missing one.
fn supported_values(key: &str) -> Result<Vec<String>, ExecutionError> {
    match key {
        "calendar" => Ok(engine::datetime::available_calendars()),
        "collation" => Ok(engine::collator::available_collations()),
        "currency" | "unit" => Ok(Vec::new()),
        "numberingSystem" => Ok(engine::number::available_numbering_systems()),
        "timeZone" => Ok(engine::datetime::available_time_zones()),
        _ => Err(range(format!("invalid key for supportedValuesOf: {key}"))),
    }
}

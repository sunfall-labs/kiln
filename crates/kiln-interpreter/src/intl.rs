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
    ListFormat,
    Locale,
    NumberFormat,
    PluralRules,
    Segmenter,
}

impl Service {
    /// The name the language spells it with, which is also its `Symbol.toStringTag`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Collator => "Collator",
            Self::ListFormat => "ListFormat",
            Self::Locale => "Locale",
            Self::NumberFormat => "NumberFormat",
            Self::PluralRules => "PluralRules",
            Self::Segmenter => "Segmenter",
        }
    }

    /// The tag `Object.prototype.toString` reports.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Collator => "Intl.Collator",
            Self::ListFormat => "Intl.ListFormat",
            Self::Locale => "Intl.Locale",
            Self::NumberFormat => "Intl.NumberFormat",
            Self::PluralRules => "Intl.PluralRules",
            Self::Segmenter => "Intl.Segmenter",
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
    /// `Intl.NumberFormat` and its five siblings.
    Constructor(Service),
    /// `Intl.NumberFormat.prototype` and its five siblings.
    Prototype(Service),
    /// `Intl.NumberFormat.supportedLocalesOf` and its siblings.
    SupportedLocalesOf(Service),
    /// A prototype method, dispatched on the receiver rather than on the constructor.
    Method(Method),
}

/// A prototype method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    ResolvedOptions,
    Format,
    FormatToParts,
    Select,
    Compare,
    Segment,
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
    ListFormat(engine::list::ListFormat),
    /// The canonical tag. `Intl.Locale` has no formatter behind it -- it *is* the tag.
    Locale(String),
    NumberFormat(engine::number::NumberFormat),
    PluralRules(engine::plurals::PluralRules),
    Segmenter(engine::segmenter::Segmenter),
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
            Self::ListFormat(_) => Service::ListFormat,
            Self::Locale(_) => Service::Locale,
            Self::NumberFormat(_) => Service::NumberFormat,
            Self::PluralRules(_) => Service::PluralRules,
            Self::Segmenter(_) => Service::Segmenter,
        }
    }
}

/// The name an `Intl` built-in reports through `.name`.
pub fn js_name(op: Op) -> String {
    match op {
        Op::Namespace => "Intl".to_owned(),
        Op::GetCanonicalLocales => "getCanonicalLocales".to_owned(),
        Op::Constructor(service) => service.name().to_owned(),
        Op::Prototype(_) => String::new(),
        Op::SupportedLocalesOf(_) => "supportedLocalesOf".to_owned(),
        Op::Method(method) => match method {
            Method::ResolvedOptions => "resolvedOptions".to_owned(),
            Method::Format => "format".to_owned(),
            Method::FormatToParts => "formatToParts".to_owned(),
            Method::Select => "select".to_owned(),
            Method::Compare => "compare".to_owned(),
            Method::Segment => "segment".to_owned(),
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
        Op::GetCanonicalLocales | Op::SupportedLocalesOf(_) => Some(1.0),
        Op::Constructor(Service::Locale) => Some(1.0),
        Op::Constructor(_) => Some(0.0),
        Op::Method(Method::Compare) => Some(2.0),
        Op::Method(Method::Format | Method::FormatToParts | Method::Select | Method::Segment) => {
            Some(1.0)
        }
        Op::Method(_) => Some(0.0),
    }
}

/// A static property of the `Intl` namespace, a constructor or a prototype.
pub fn static_property(op: Op, name: &str) -> Option<Value> {
    const SERVICES: &[Service] = &[
        Service::Collator,
        Service::ListFormat,
        Service::Locale,
        Service::NumberFormat,
        Service::PluralRules,
        Service::Segmenter,
    ];
    match (op, name) {
        (Op::Namespace, "getCanonicalLocales") => {
            Some(Value::Native(Native::Intl(Op::GetCanonicalLocales)))
        }
        (Op::Namespace, _) => SERVICES
            .iter()
            .find(|service| service.name() == name)
            .map(|service| Value::Native(Native::Intl(Op::Constructor(*service)))),
        (Op::Constructor(service), "prototype") => {
            Some(Value::Native(Native::Intl(Op::Prototype(service))))
        }
        // `Intl.Locale` has no locale negotiation to do, so it has no `supportedLocalesOf`.
        (Op::Constructor(service), "supportedLocalesOf") if service != Service::Locale => {
            Some(Value::Native(Native::Intl(Op::SupportedLocalesOf(service))))
        }
        (Op::Prototype(service), "constructor") => {
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
        (_, "resolvedOptions") if service != Service::Locale => Method::ResolvedOptions,
        (Service::NumberFormat | Service::ListFormat, "format") => Method::Format,
        (Service::NumberFormat | Service::ListFormat, "formatToParts") => Method::FormatToParts,
        (Service::PluralRules, "select") => Method::Select,
        (Service::Collator, "compare") => Method::Compare,
        (Service::Segmenter, "segment") => Method::Segment,
        (Service::Locale, "maximize") => Method::Maximize,
        (Service::Locale, "minimize") => Method::Minimize,
        (Service::Locale, "toString") => Method::LocaleToString,
        _ => return None,
    };
    Some(Value::Native(Native::Intl(Op::Method(method))))
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
            Op::Constructor(service @ (Service::NumberFormat | Service::Collator)) => {
                self.intl_construct(service, arguments, instruction)
            }
            Op::Constructor(service) => Err(type_error(format!(
                "Constructor Intl.{} requires 'new'",
                service.name()
            ))),
            Op::Method(method) => self.intl_method(method, receiver, arguments, instruction),
            Op::Namespace | Op::Prototype(_) => Ok(Value::Undefined),
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
                let options = self.intl_digit_options(&bag, instruction)?;
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
        method: Method,
        receiver: &Value,
        arguments: &[Value],
        instruction: usize,
    ) -> Result<Value, ExecutionError> {
        let Value::Intl(value) = receiver else {
            return Err(type_error("this is not an Intl object"));
        };
        match (method, value.as_ref()) {
            (Method::ResolvedOptions, _) => {
                let settings = match value.as_ref() {
                    IntlValue::NumberFormat(f) => f.resolved_options(),
                    IntlValue::PluralRules(r) => r.resolved_options(),
                    IntlValue::Collator(c) => c.resolved_options(),
                    IntlValue::ListFormat(l) => l.resolved_options(),
                    IntlValue::Segmenter(s) => s.resolved_options(),
                    IntlValue::Locale(_) => {
                        return Err(type_error("Intl.Locale has no resolvedOptions"));
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
            (Method::Select, IntlValue::PluralRules(rules)) => {
                let number = self.as_number_argument(arguments.first(), instruction)?;
                Ok(Value::String(
                    rules
                        .select(&engine::number::Numeric::Double(number))
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
                let units = utf16(&text);
                let segments = segmenter.segments(&units);
                // The language's `Segments` object is iterable and random-access. This build
                // answers the iteration, which is what `for (const s of segmenter.segment(x))`
                // needs; `containing()` is not here yet and is absent rather than approximated.
                let items = segments
                    .into_iter()
                    .map(|segment| {
                        let mut data = ObjectData::default();
                        data.set(
                            "segment",
                            Value::String(String::from_utf16_lossy(
                                &units[segment.start..segment.end],
                            )),
                        );
                        data.set("index", Value::Number(segment.start as f64));
                        data.set("input", Value::String(text.clone()));
                        if let Some(word_like) = segment.word_like {
                            data.set("isWordLike", Value::Boolean(word_like));
                        }
                        Value::Object(Rc::new(RefCell::new(data)))
                    })
                    .collect();
                Ok(Value::Array(ArrayData::new(items)))
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
                js_name(Op::Method(method))
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
            None | Some(Value::Undefined) => {
                Ok(Value::Object(Rc::new(RefCell::new(ObjectData::default()))))
            }
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

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

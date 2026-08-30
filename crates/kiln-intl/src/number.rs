//! `Intl.NumberFormat`.
//!
//! The split of responsibility here is the one the crate documentation describes, and it is worth
//! being explicit about because it looks like duplication and is not. ECMAScript specifies its
//! *numeric* behaviour algorithmically: `SetNumberFormatDigitOptions`, `ToRawFixed`,
//! `ToRawPrecision`, the nine rounding modes and the rounding increments are exact spec text over
//! a decimal value and are identical in every locale. Those are implemented here, against the
//! specification. What comes from ICU4X is everything that is genuinely CLDR-shaped: which glyphs
//! the digits are, where the group separators fall, what the separators and the minus sign are
//! spelled as, and -- through `writeable`'s part annotations -- where each run begins and ends.
//!
//! Doing the rounding twice, once here and once in the library, is how the two disagree at `0.5`.
//! Doing the grouping twice is how a build produces `1,00,000` for `en` in one place and
//! `100,000` in another. Each job is done once, in the layer that owns it.

use fixed_decimal::{
    Decimal, RoundingIncrement, SignDisplay, SignedRoundingMode, UnsignedRoundingMode,
};
use icu_decimal::DecimalFormatter;
use icu_decimal::options::{DecimalFormatterOptions, GroupingStrategy};

use crate::locale::{RelevantKey, Resolution, resolve};
use crate::{Error, Setting};

/// The accepted values of `style`. `currency` and `unit` are accepted by the constructor and
/// refused by `format`; see [`NumberFormat::format`].
pub const STYLES: &[&str] = &["decimal", "percent", "currency", "unit"];
/// The accepted values of `notation`.
pub const NOTATIONS: &[&str] = &["standard", "scientific", "engineering", "compact"];
/// The accepted values of `compactDisplay`.
pub const COMPACT_DISPLAYS: &[&str] = &["short", "long"];
/// The accepted values of `signDisplay`.
pub const SIGN_DISPLAYS: &[&str] = &["auto", "never", "always", "exceptZero", "negative"];
/// The accepted values of `currencyDisplay`.
pub const CURRENCY_DISPLAYS: &[&str] = &["code", "symbol", "narrowSymbol", "name"];
/// The accepted values of `currencySign`.
pub const CURRENCY_SIGNS: &[&str] = &["standard", "accounting"];
/// The accepted values of `unitDisplay`.
pub const UNIT_DISPLAYS: &[&str] = &["short", "narrow", "long"];
/// The accepted values of `roundingMode`.
pub const ROUNDING_MODES: &[&str] = &[
    "ceil",
    "floor",
    "expand",
    "trunc",
    "halfCeil",
    "halfFloor",
    "halfExpand",
    "halfTrunc",
    "halfEven",
];
/// The accepted values of `roundingPriority`.
pub const ROUNDING_PRIORITIES: &[&str] = &["auto", "morePrecision", "lessPrecision"];
/// The accepted values of `trailingZeroDisplay`.
pub const TRAILING_ZERO_DISPLAYS: &[&str] = &["auto", "stripIfInteger"];
/// The accepted string values of `useGrouping`. `true`, `false` and `undefined` are also accepted,
/// which is why the option is not read with a plain `GetOption`.
pub const USE_GROUPINGS: &[&str] = &["min2", "auto", "always"];

/// The rounding increments ECMA-402 permits: `{1, 2, 5, 25} x 10^k`, and nothing else.
///
/// The list is not a range because the increment has to divide a power of ten evenly for the
/// rounded value to still be representable in the fraction digits that were asked for.
pub const ROUNDING_INCREMENTS: &[u32] = &[
    1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000,
];

/// What `useGrouping` settled on. It is not a string option because `false` is a legal value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grouping {
    /// Group only when the integer part has at least two digits beyond the first group.
    Min2,
    /// Group as the locale ordinarily does.
    Auto,
    /// Group even where the locale ordinarily would not.
    Always,
    /// Never group.
    Never,
}

/// The number to format.
///
/// ECMA-402 calls this an "Intl mathematical value", and it is deliberately wider than a Number:
/// `format` accepts a BigInt and a string, and a string is read as a decimal literal at full
/// precision rather than through a Number. Keeping the two apart at the Seam is what stops a
/// 30-digit BigInt from being rounded to 17 digits on the way in.
#[derive(Clone, Debug, PartialEq)]
pub enum Numeric {
    /// A Number.
    Double(f64),
    /// A BigInt or a numeric string, carried as the exact decimal digits it was written with.
    Exact(String),
}

/// The options a caller has already read off the JavaScript options object, in spec order.
///
/// Every field is what `GetOption`/`GetNumberOption` produced, not what the constructor settled
/// on: the string options have been checked against their vocabularies by the caller (so the
/// `RangeError` is thrown while reading, in the order the corpus tests), and the numeric ones are
/// the raw `ToNumber` results, because the range checks on those happen inside
/// `SetNumberFormatDigitOptions` rather than at read time.
#[derive(Clone, Debug)]
pub struct Options {
    /// `numberingSystem`, already checked for `type` syntax.
    pub numbering_system: Option<String>,
    /// `style`.
    pub style: String,
    /// `currency`, a well-formed currency code.
    pub currency: Option<String>,
    /// `currencyDisplay`.
    pub currency_display: String,
    /// `currencySign`.
    pub currency_sign: String,
    /// `unit`, a well-formed sanctioned unit identifier.
    pub unit: Option<String>,
    /// `unitDisplay`.
    pub unit_display: String,
    /// `minimumIntegerDigits`, already range-checked to 1..=21 by `GetNumberOption`.
    pub minimum_integer_digits: f64,
    /// `minimumFractionDigits`, `None` when absent.
    pub minimum_fraction_digits: Option<f64>,
    /// `maximumFractionDigits`, `None` when absent.
    pub maximum_fraction_digits: Option<f64>,
    /// `minimumSignificantDigits`, `None` when absent.
    pub minimum_significant_digits: Option<f64>,
    /// `maximumSignificantDigits`, `None` when absent.
    pub maximum_significant_digits: Option<f64>,
    /// `roundingIncrement`, already range-checked to 1..=5000 by `GetNumberOption`.
    pub rounding_increment: f64,
    /// `roundingMode`.
    pub rounding_mode: String,
    /// `roundingPriority`.
    pub rounding_priority: String,
    /// `trailingZeroDisplay`.
    pub trailing_zero_display: String,
    /// `notation`.
    pub notation: String,
    /// `compactDisplay`.
    pub compact_display: String,
    /// `useGrouping`, after its three-way coercion.
    pub use_grouping: Grouping,
    /// `signDisplay`.
    pub sign_display: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            numbering_system: None,
            style: "decimal".into(),
            currency: None,
            currency_display: "symbol".into(),
            currency_sign: "standard".into(),
            unit: None,
            unit_display: "short".into(),
            minimum_integer_digits: 1.0,
            minimum_fraction_digits: None,
            maximum_fraction_digits: None,
            minimum_significant_digits: None,
            maximum_significant_digits: None,
            rounding_increment: 1.0,
            rounding_mode: "halfExpand".into(),
            rounding_priority: "auto".into(),
            trailing_zero_display: "auto".into(),
            notation: "standard".into(),
            compact_display: "short".into(),
            use_grouping: Grouping::Auto,
            sign_display: "auto".into(),
        }
    }
}

/// Which of the digit limits actually decides the rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoundingType {
    SignificantDigits,
    FractionDigits,
    MorePrecision,
    LessPrecision,
}

#[derive(Clone, Copy, Debug)]
struct Digits {
    min_integer: u8,
    min_fraction: u8,
    max_fraction: u8,
    min_significant: u8,
    max_significant: u8,
    rounding_type: RoundingType,
    /// True when the caller asked for fraction or significant digits explicitly, which is what
    /// decides whether `resolvedOptions()` reports them.
    reports_fraction: bool,
    reports_significant: bool,
}

/// A built number formatter.
///
/// `Debug` is written out rather than derived because `DecimalFormatter` has no useful one and a
/// caller printing this wants the resolved settings, not the data structure behind them.
pub struct NumberFormat {
    resolution: Resolution,
    formatter: DecimalFormatter,
    options: Options,
    digits: Digits,
    rounding_increment: u32,
}

impl core::fmt::Debug for NumberFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NumberFormat")
            .field("locale", &self.resolution.locale)
            .field("style", &self.options.style)
            .finish_non_exhaustive()
    }
}

/// `DefaultNumberOption`: coerce, range-check and truncate one numeric option.
fn default_number_option(
    value: Option<f64>,
    minimum: f64,
    maximum: f64,
    name: &str,
) -> Result<Option<f64>, Error> {
    let Some(value) = value else { return Ok(None) };
    if value.is_nan() || value < minimum || value > maximum {
        return Err(Error::range(format!("{name} value is out of range.")));
    }
    Ok(Some(value.floor()))
}

/// `SetNumberFormatDigitOptions`, which is where most of `NumberFormat`'s error surface lives.
fn resolve_digits(options: &Options) -> Result<Digits, Error> {
    // The per-style fraction-digit defaults. `currency` would take its digits from the currency's
    // own minor-unit count, which this build does not have; it is refused at `format` time rather
    // than given a plausible default here.
    let (min_fraction_default, max_fraction_default) = match options.style.as_str() {
        "percent" => (0u8, 0u8),
        _ => (0u8, 3u8),
    };

    let has_significant = options.minimum_significant_digits.is_some()
        || options.maximum_significant_digits.is_some();
    let has_fraction =
        options.minimum_fraction_digits.is_some() || options.maximum_fraction_digits.is_some();
    let compact = options.notation == "compact";

    let mut need_significant = true;
    let mut need_fraction = true;
    if options.rounding_priority == "auto" {
        need_significant = has_significant;
        if has_significant || (!has_fraction && compact) {
            need_fraction = false;
        }
    }

    let (mut min_significant, mut max_significant) = (1u8, 21u8);
    if need_significant && has_significant {
        {
            let mnsd = default_number_option(
                options.minimum_significant_digits,
                1.0,
                21.0,
                "minimumSignificantDigits",
            )?
            .unwrap_or(1.0);
            let mxsd = default_number_option(
                options.maximum_significant_digits,
                mnsd,
                21.0,
                "maximumSignificantDigits",
            )?
            .unwrap_or(21.0);
            min_significant = mnsd as u8;
            max_significant = mxsd as u8;
        }
    }

    let (mut min_fraction, mut max_fraction) = (min_fraction_default, max_fraction_default);
    if need_fraction && has_fraction {
        {
            let mnfd = default_number_option(
                options.minimum_fraction_digits,
                0.0,
                100.0,
                "minimumFractionDigits",
            )?;
            let mxfd = default_number_option(
                options.maximum_fraction_digits,
                0.0,
                100.0,
                "maximumFractionDigits",
            )?;
            let (mnfd, mxfd) = match (mnfd, mxfd) {
                (None, Some(mxfd)) => (f64::from(min_fraction_default).min(mxfd), mxfd),
                (Some(mnfd), None) => (mnfd, f64::from(max_fraction_default).max(mnfd)),
                (Some(mnfd), Some(mxfd)) => {
                    if mnfd > mxfd {
                        return Err(Error::range(
                            "minimumFractionDigits is greater than maximumFractionDigits"
                                .to_string(),
                        ));
                    }
                    (mnfd, mxfd)
                }
                (None, None) => unreachable!("has_fraction implies one of the two is present"),
            };
            min_fraction = mnfd as u8;
            max_fraction = mxfd as u8;
        }
    }

    let rounding_type = if need_significant || need_fraction {
        match options.rounding_priority.as_str() {
            "morePrecision" => RoundingType::MorePrecision,
            "lessPrecision" => RoundingType::LessPrecision,
            _ if has_significant => RoundingType::SignificantDigits,
            _ => RoundingType::FractionDigits,
        }
    } else {
        // Compact notation with no digit options at all: the specification switches to
        // more-precision between "no fraction digits" and "two significant digits", which is what
        // makes `1234` compact to `1.2K` rather than `1K`.
        min_fraction = 0;
        max_fraction = 0;
        min_significant = 1;
        max_significant = 2;
        RoundingType::MorePrecision
    };

    if options.rounding_increment != 1.0 {
        if rounding_type != RoundingType::FractionDigits {
            return Err(Error::type_error(
                "roundingIncrement can only be used with fraction-digit rounding".to_string(),
            ));
        }
        if max_fraction != min_fraction {
            return Err(Error::range(
                "roundingIncrement requires minimumFractionDigits to equal maximumFractionDigits"
                    .to_string(),
            ));
        }
    }

    Ok(Digits {
        min_integer: options.minimum_integer_digits as u8,
        min_fraction,
        max_fraction,
        min_significant,
        max_significant,
        rounding_type,
        reports_fraction: need_fraction || !need_significant,
        reports_significant: need_significant,
    })
}

/// `GetUnsignedRoundingMode` folded into `fixed_decimal`'s signed/unsigned split.
///
/// The two libraries agree exactly here, which is why the mapping is a table rather than an
/// implementation: `ceil`/`floor`/`halfCeil`/`halfFloor` are the sign-aware modes and the other
/// five are the sign-blind ones.
fn rounding_mode(name: &str) -> SignedRoundingMode {
    match name {
        "ceil" => SignedRoundingMode::Ceil,
        "floor" => SignedRoundingMode::Floor,
        "halfCeil" => SignedRoundingMode::HalfCeil,
        "halfFloor" => SignedRoundingMode::HalfFloor,
        "expand" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::Expand),
        "trunc" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::Trunc),
        "halfTrunc" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfTrunc),
        "halfEven" => SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfEven),
        _ => SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    }
}

/// Split an ECMA-402 rounding increment into the `{1,2,5,25}` factor and the power of ten it
/// multiplies, which is the shape `fixed_decimal` takes it in.
fn split_increment(increment: u32) -> Option<(RoundingIncrement, i16)> {
    let mut value = increment;
    let mut power = 0i16;
    while value.is_multiple_of(10) && value > 1 {
        value /= 10;
        power += 1;
    }
    let factor = match value {
        1 => RoundingIncrement::MultiplesOf1,
        2 => RoundingIncrement::MultiplesOf2,
        5 => RoundingIncrement::MultiplesOf5,
        25 => RoundingIncrement::MultiplesOf25,
        _ => return None,
    };
    Some((factor, power))
}

/// The digits a Number is rounded from: its shortest round-tripping decimal spelling.
///
/// This is deliberately *not* the exact mathematical value, and the difference is observable.
/// The double nearest `1.0005` is `1.000499999999999945...`, so rounding the exact value to three
/// fraction digits gives `1.000` -- which is what `(1.0005).toFixed(3)` answers, and is not what
/// `Intl.NumberFormat` answers. Every engine rounds `Intl` output from the same digits
/// `Number::toString` would produce, the corpus is written against that, and Rust's `Display` for
/// `f64` is the same shortest-round-trip spelling with no exponent, so it is the right input here.
fn shortest_decimal(value: f64) -> String {
    format!("{value}")
}

impl NumberFormat {
    /// Build a formatter, resolving the locale and the digit options.
    pub fn new(requested: &[String], options: Options) -> Result<Self, Error> {
        let digits = resolve_digits(&options)?;
        let increment = options.rounding_increment as u32;
        if !ROUNDING_INCREMENTS.contains(&increment) {
            return Err(Error::range(format!(
                "roundingIncrement value is out of range: {increment}"
            )));
        }

        let keys = [RelevantKey::new("nu").with_option(options.numbering_system.clone())];
        let resolution = resolve(requested, &keys, |key, value| {
            key == "nu" && crate::number::numbering_system_is_supported(value)
        });

        let mut formatter_options = DecimalFormatterOptions::default();
        formatter_options.grouping_strategy = Some(match options.use_grouping {
            Grouping::Min2 => GroupingStrategy::Min2,
            Grouping::Auto => GroupingStrategy::Auto,
            Grouping::Always => GroupingStrategy::Always,
            Grouping::Never => GroupingStrategy::Never,
        });
        let formatter = DecimalFormatter::try_new(resolution.icu().into(), formatter_options)
            .map_err(|_| Error::range("no number formatting data for the resolved locale"))?;

        Ok(Self {
            resolution,
            formatter,
            options,
            digits,
            rounding_increment: increment,
        })
    }

    /// The tag `resolvedOptions().locale` reports.
    pub fn locale(&self) -> &str {
        &self.resolution.locale
    }

    /// The value after the digit options have been applied, before any locale rendering.
    ///
    /// `Intl.PluralRules` needs exactly this and nothing after it: a plural category is decided by
    /// the rounded digits, not by the glyphs they are drawn with. Crate-private because `Decimal`
    /// is an ICU4X type and the Seam does not carry those.
    pub(crate) fn rounded(&self, value: &Numeric) -> Result<Decimal, Error> {
        let text = match value {
            Numeric::Double(number) if !number.is_finite() => {
                return Err(Error::range("a non-finite number has no plural category"));
            }
            Numeric::Double(number) => shortest_decimal(*number),
            Numeric::Exact(text) => text.clone(),
        };
        let mut decimal = Decimal::try_from_str(&text)
            .map_err(|_| Error::range(format!("cannot read {text} as a number")))?;
        if self.options.style == "percent" {
            decimal.multiply_pow10(2);
        }
        self.round(&mut decimal);
        decimal.trim_start();
        decimal.pad_start(i16::from(self.digits.min_integer));
        Ok(decimal)
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        let mut out = vec![
            ("locale", Setting::string(&self.resolution.locale)),
            (
                "numberingSystem",
                Setting::string(self.resolution.value("nu").unwrap_or("latn")),
            ),
            ("style", Setting::string(&self.options.style)),
        ];
        if let Some(currency) = &self.options.currency {
            out.push(("currency", Setting::string(currency)));
            out.push((
                "currencyDisplay",
                Setting::string(&self.options.currency_display),
            ));
            out.push(("currencySign", Setting::string(&self.options.currency_sign)));
        }
        if let Some(unit) = &self.options.unit {
            out.push(("unit", Setting::string(unit)));
            out.push(("unitDisplay", Setting::string(&self.options.unit_display)));
        }
        out.push((
            "minimumIntegerDigits",
            Setting::Number(f64::from(self.digits.min_integer)),
        ));
        if self.digits.reports_fraction {
            out.push((
                "minimumFractionDigits",
                Setting::Number(f64::from(self.digits.min_fraction)),
            ));
            out.push((
                "maximumFractionDigits",
                Setting::Number(f64::from(self.digits.max_fraction)),
            ));
        }
        if self.digits.reports_significant {
            out.push((
                "minimumSignificantDigits",
                Setting::Number(f64::from(self.digits.min_significant)),
            ));
            out.push((
                "maximumSignificantDigits",
                Setting::Number(f64::from(self.digits.max_significant)),
            ));
        }
        out.push((
            "useGrouping",
            match self.options.use_grouping {
                Grouping::Never => Setting::Bool(false),
                Grouping::Min2 => Setting::string("min2"),
                Grouping::Auto => Setting::string("auto"),
                Grouping::Always => Setting::string("always"),
            },
        ));
        out.push(("notation", Setting::string(&self.options.notation)));
        if self.options.notation == "compact" {
            out.push((
                "compactDisplay",
                Setting::string(&self.options.compact_display),
            ));
        }
        out.push(("signDisplay", Setting::string(&self.options.sign_display)));
        out.push((
            "roundingIncrement",
            Setting::Number(f64::from(self.rounding_increment)),
        ));
        out.push(("roundingMode", Setting::string(&self.options.rounding_mode)));
        out.push((
            "roundingPriority",
            Setting::string(&self.options.rounding_priority),
        ));
        out.push((
            "trailingZeroDisplay",
            Setting::string(&self.options.trailing_zero_display),
        ));
        out
    }

    /// Round `value` and render it, or refuse.
    ///
    /// `currency`, `unit` and every notation but `standard` are refused rather than approximated.
    /// The pattern that wraps a formatted number in a currency symbol, a unit name or a compact
    /// suffix is CLDR data that `icu_decimal` 2.3 does not carry, and inventing `"$" + number`
    /// would be right for `en-US` and wrong for the locales that put the symbol elsewhere, spell
    /// it differently or space it differently -- the wrong-answer failure mode rather than the
    /// missing-answer one. The refusal names the reason so a caller can report it as unsupported.
    pub fn format(&self, value: &Numeric) -> Result<String, Error> {
        let mut parts = String::new();
        for (_, text) in self.format_to_parts(value)? {
            parts.push_str(&text);
        }
        Ok(parts)
    }

    /// `formatToParts()`: the same rendering, split into the runs ICU4X annotated.
    pub fn format_to_parts(&self, value: &Numeric) -> Result<Vec<(&'static str, String)>, Error> {
        self.refuse_unsupported()?;

        let decimal = match value {
            Numeric::Double(number) if number.is_nan() => {
                // `NaN` has no sign for this purpose -- `format(-NaN)` is `format(NaN)` -- so the
                // only mark that can precede it is an unconditional `+`.
                let mut out = Vec::new();
                if self.options.sign_display == "always" {
                    out.push(("plusSign", "+".to_string()));
                }
                out.push(("nan", nan_text().to_string()));
                return Ok(out);
            }
            Numeric::Double(number) if number.is_infinite() => {
                let negative = number.is_sign_negative();
                let mut out = Vec::new();
                match self.options.sign_display.as_str() {
                    "never" => {}
                    // An infinity is not zero, so `exceptZero` marks it exactly as `always` does.
                    "always" | "exceptZero" => out.push(if negative {
                        ("minusSign", "-".to_string())
                    } else {
                        ("plusSign", "+".to_string())
                    }),
                    _ if negative => out.push(("minusSign", "-".to_string())),
                    _ => {}
                }
                out.push(("infinity", "\u{221e}".to_string()));
                return Ok(out);
            }
            Numeric::Double(number) => shortest_decimal(*number),
            Numeric::Exact(text) => text.clone(),
        };
        let mut decimal = Decimal::try_from_str(&decimal)
            .map_err(|_| Error::range(format!("cannot format {decimal} as a number")))?;

        if self.options.style == "percent" {
            decimal.multiply_pow10(2);
        }
        self.round(&mut decimal);
        // `multiply_pow10` for `percent` shifts the whole digit range, so the leading zero of
        // `0.256` becomes a leading zero of `25.6`; trimming before padding is what stops a
        // percentage coming out as `026%`.
        decimal.trim_start();
        decimal.pad_start(i16::from(self.digits.min_integer));
        decimal.apply_sign_display(match self.options.sign_display.as_str() {
            "never" => SignDisplay::Never,
            "always" => SignDisplay::Always,
            "exceptZero" => SignDisplay::ExceptZero,
            "negative" => SignDisplay::Negative,
            _ => SignDisplay::Auto,
        });

        let mut parts = collect_parts(&self.formatter, &decimal);
        if self.options.style == "percent" {
            // `en`'s percent pattern is `{0}%` with no space, which is also the CLDR root pattern.
            // Locales that space or reposition it are wrong here; that is the same `icu_decimal`
            // data gap the currency refusal is about, and it is recorded rather than hidden.
            parts.push(("percentSign", "%".to_string()));
        }
        Ok(parts)
    }

    /// `format`'s and `formatToParts`'s shared refusal.
    fn refuse_unsupported(&self) -> Result<(), Error> {
        let unsupported = if self.options.style == "currency" {
            Some("style: \"currency\"")
        } else if self.options.style == "unit" {
            Some("style: \"unit\"")
        } else if self.options.notation != "standard" {
            Some("a notation other than \"standard\"")
        } else {
            None
        };
        match unsupported {
            Some(what) => Err(Error::type_error(format!(
                "Intl.NumberFormat does not yet have the locale data for {what}"
            ))),
            None => Ok(()),
        }
    }

    fn round(&self, decimal: &mut Decimal) {
        let mode = rounding_mode(&self.options.rounding_mode);
        match self.digits.rounding_type {
            RoundingType::SignificantDigits => self.round_significant(decimal, mode),
            RoundingType::FractionDigits => self.round_fraction(decimal, mode),
            // `morePrecision` keeps whichever of the two limits leaves more digits, and
            // `lessPrecision` the other; the specification computes both roundings and compares
            // them rather than comparing the limits, because which one is more precise depends on
            // the magnitude of the value.
            RoundingType::MorePrecision | RoundingType::LessPrecision => {
                let mut significant = decimal.clone();
                self.round_significant(&mut significant, mode);
                let mut fraction = decimal.clone();
                self.round_fraction(&mut fraction, mode);
                let more_precise =
                    significant.nonzero_magnitude_end() <= fraction.nonzero_magnitude_end();
                let take_significant =
                    (self.digits.rounding_type == RoundingType::MorePrecision) == more_precise;
                *decimal = if take_significant {
                    significant
                } else {
                    fraction
                };
            }
        }
        if self.options.trailing_zero_display == "stripIfInteger" {
            decimal.trim_end_if_integer();
        }
    }

    fn round_significant(&self, decimal: &mut Decimal, mode: SignedRoundingMode) {
        if decimal.is_zero() {
            decimal.pad_end(-(i16::from(self.digits.min_significant) - 1));
            return;
        }
        let start = decimal.nonzero_magnitude_start();
        decimal.round_with_mode(start - i16::from(self.digits.max_significant) + 1, mode);
        // Rounding `9.99` to two significant digits gives `10`, whose most significant digit has
        // moved; the padding has to be computed from where the digits ended up, not where they
        // started.
        let start = decimal.nonzero_magnitude_start();
        decimal.pad_end(start - i16::from(self.digits.min_significant) + 1);
        decimal.trim_end();
        let start = decimal.nonzero_magnitude_start();
        decimal.pad_end(start - i16::from(self.digits.min_significant) + 1);
    }

    fn round_fraction(&self, decimal: &mut Decimal, mode: SignedRoundingMode) {
        let position = -i16::from(self.digits.max_fraction);
        match split_increment(self.rounding_increment) {
            Some((factor, power)) if self.rounding_increment != 1 => {
                decimal.round_with_mode_and_increment(position + power, mode, factor);
            }
            _ => decimal.round_with_mode(position, mode),
        }
        decimal.pad_end(-i16::from(self.digits.min_fraction));
    }
}

/// CLDR's root spelling of "not a number".
///
/// `icu_decimal` 2.3's `DecimalSymbols` carries the separators and the sign affixes and not this,
/// so it is the inherited root value rather than the resolved locale's. Most locales inherit it
/// unchanged; Russian, Arabic, Persian and Finnish do not, and those are wrong here. Recorded
/// rather than papered over -- a locale-correct answer needs data this build does not have.
fn nan_text() -> &'static str {
    "NaN"
}

/// Is `value` a numbering system whose digits ICU4X actually has?
///
/// Asked by observation rather than from a table: format a number whose digits are all distinct
/// under the candidate system and under `latn`, and see whether the glyphs differ. A numbering
/// system with no data falls back to `latn` inside ICU4X, so "the output did not change" is
/// exactly "there is no data", with no list to keep in step with the bundle.
pub fn numbering_system_is_supported(value: &str) -> bool {
    if value == "latn" {
        return true;
    }
    let Ok(tag) = format!("und-u-nu-{value}").parse::<icu_locale_core::Locale>() else {
        return false;
    };
    let Ok(candidate) = DecimalFormatter::try_new((&tag).into(), Default::default()) else {
        return false;
    };
    let Ok(latn) = DecimalFormatter::try_new(Default::default(), Default::default()) else {
        return false;
    };
    let probe = Decimal::from(9876543210i64);
    candidate.format_to_string(&probe) != latn.format_to_string(&probe)
}

/// Render a decimal and keep ICU4X's own run annotations.
fn collect_parts(formatter: &DecimalFormatter, decimal: &Decimal) -> Vec<(&'static str, String)> {
    use writeable::Writeable;
    let mut sink = crate::parts::Collector::new(|part| match part {
        icu_decimal::parts::INTEGER => "integer",
        icu_decimal::parts::FRACTION => "fraction",
        icu_decimal::parts::GROUP => "group",
        icu_decimal::parts::DECIMAL => "decimal",
        icu_decimal::parts::MINUS_SIGN => "minusSign",
        icu_decimal::parts::PLUS_SIGN => "plusSign",
        // ICU4X annotated a run this build has no name for. `literal` is the specification's own
        // catch-all and is the honest answer for text that is present but unclassified.
        _ => "literal",
    });
    let _ = formatter.format(decimal).write_to_parts(&mut sink);
    sink.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(locale: &str, options: Options, value: f64) -> String {
        NumberFormat::new(&[locale.to_string()], options)
            .expect("the options resolve")
            .format(&Numeric::Double(value))
            .expect("the value formats")
    }

    fn digits(min_fraction: Option<f64>, max_fraction: Option<f64>) -> Options {
        Options {
            minimum_fraction_digits: min_fraction,
            maximum_fraction_digits: max_fraction,
            ..Options::default()
        }
    }

    #[test]
    fn grouping_and_separators_come_from_the_locale() {
        assert_eq!(
            format("en-US", Options::default(), 1234567.891),
            "1,234,567.891"
        );
        assert_eq!(
            format("de-DE", Options::default(), 1234567.891),
            "1.234.567,891"
        );
        // Indian grouping is 3-2-2, which is exactly the sort of thing a hand-rolled grouper gets
        // wrong and CLDR gets right.
        assert_eq!(format("en-IN", Options::default(), 1234567.0), "12,34,567");
    }

    #[test]
    fn the_numbering_system_changes_the_digits() {
        assert_eq!(
            format("th-u-nu-thai", Options::default(), 1234.0),
            "\u{e51},\u{e52}\u{e53}\u{e54}"
        );
        // Arabic-Indic digits *and* U+066C as the group separator: a pairing a hand-written
        // formatter has no reason to keep together, and one `node` agrees with exactly.
        assert_eq!(
            format("ar-EG", Options::default(), 1234.0),
            "\u{661}\u{66c}\u{662}\u{663}\u{664}"
        );
    }

    #[test]
    fn the_default_maximum_of_three_fraction_digits_rounds_half_away_from_zero() {
        assert_eq!(format("en-US", Options::default(), 1.0005), "1.001");
        assert_eq!(format("en-US", Options::default(), -1.0005), "-1.001");
        assert_eq!(format("en-US", Options::default(), 1.0004), "1");
    }

    #[test]
    fn fraction_digits_pad_and_truncate() {
        assert_eq!(format("en-US", digits(Some(2.0), Some(2.0)), 1.5), "1.50");
        assert_eq!(format("en-US", digits(Some(0.0), Some(0.0)), 1.5), "2");
        assert_eq!(
            format("en-US", digits(None, Some(5.0)), 1.23456789),
            "1.23457"
        );
        assert_eq!(format("en-US", digits(Some(4.0), None), 1.5), "1.5000");
    }

    #[test]
    fn significant_digits_win_over_fraction_digits_when_both_are_given() {
        let options = Options {
            minimum_significant_digits: Some(3.0),
            maximum_significant_digits: Some(3.0),
            maximum_fraction_digits: Some(0.0),
            ..Options::default()
        };
        // `auto` priority makes the significant digits decide, so the fraction limit is ignored.
        assert_eq!(format("en-US", options, 1.23456), "1.23");
    }

    #[test]
    fn significant_digits_pad_and_round() {
        let sig = |min: f64, max: f64| Options {
            minimum_significant_digits: Some(min),
            maximum_significant_digits: Some(max),
            ..Options::default()
        };
        assert_eq!(format("en-US", sig(1.0, 3.0), 1.23456), "1.23");
        assert_eq!(format("en-US", sig(5.0, 5.0), 1.2), "1.2000");
        assert_eq!(format("en-US", sig(1.0, 2.0), 9.99), "10");
        assert_eq!(format("en-US", sig(1.0, 3.0), 0.0), "0");
        assert_eq!(format("en-US", sig(3.0, 3.0), 0.0), "0.00");
        assert_eq!(format("en-US", sig(1.0, 2.0), 123456.0), "120,000");
    }

    #[test]
    fn every_rounding_mode_is_the_one_the_language_names() {
        // The pair 2.5 / -2.5 separates all nine, which is why the table is written out.
        for (mode, positive, negative) in [
            ("ceil", "3", "-2"),
            ("floor", "2", "-3"),
            ("expand", "3", "-3"),
            ("trunc", "2", "-2"),
            ("halfCeil", "3", "-2"),
            ("halfFloor", "2", "-3"),
            ("halfExpand", "3", "-3"),
            ("halfTrunc", "2", "-2"),
            ("halfEven", "2", "-2"),
        ] {
            let options = || Options {
                maximum_fraction_digits: Some(0.0),
                minimum_fraction_digits: Some(0.0),
                rounding_mode: mode.to_string(),
                ..Options::default()
            };
            assert_eq!(format("en-US", options(), 2.5), positive, "{mode} of 2.5");
            assert_eq!(format("en-US", options(), -2.5), negative, "{mode} of -2.5");
        }
    }

    #[test]
    fn the_rounding_increment_snaps_to_a_multiple() {
        let options = |increment: f64, fraction: f64| Options {
            rounding_increment: increment,
            minimum_fraction_digits: Some(fraction),
            maximum_fraction_digits: Some(fraction),
            ..Options::default()
        };
        assert_eq!(format("en-US", options(5.0, 2.0), 1.22), "1.20");
        assert_eq!(format("en-US", options(5.0, 2.0), 1.23), "1.25");
        assert_eq!(format("en-US", options(25.0, 2.0), 1.1), "1.00");
        assert_eq!(format("en-US", options(25.0, 2.0), 1.2), "1.25");
    }

    #[test]
    fn sign_display_decides_what_appears_before_the_digits() {
        let with = |name: &str, value: f64| {
            format(
                "en-US",
                Options {
                    sign_display: name.into(),
                    ..Options::default()
                },
                value,
            )
        };
        assert_eq!(with("auto", 1.0), "1");
        assert_eq!(with("always", 1.0), "+1");
        assert_eq!(with("always", 0.0), "+0");
        assert_eq!(with("exceptZero", 0.0), "0");
        assert_eq!(with("exceptZero", 1.0), "+1");
        assert_eq!(with("never", -1.0), "1");
        assert_eq!(with("negative", -0.0), "0");
        assert_eq!(with("negative", -1.0), "-1");
    }

    #[test]
    fn use_grouping_is_the_locale_strategy_rather_than_a_separator_search() {
        let with = |grouping: Grouping, value: f64| {
            format(
                "en-US",
                Options {
                    use_grouping: grouping,
                    ..Options::default()
                },
                value,
            )
        };
        assert_eq!(with(Grouping::Never, 1234567.0), "1234567");
        assert_eq!(with(Grouping::Auto, 1234.0), "1,234");
        assert_eq!(with(Grouping::Min2, 1234.0), "1234");
        assert_eq!(with(Grouping::Min2, 12345.0), "12,345");
    }

    #[test]
    fn minimum_integer_digits_pads_the_left() {
        let options = Options {
            minimum_integer_digits: 5.0,
            ..Options::default()
        };
        assert_eq!(format("en-US", options, 12.0), "00,012");
    }

    #[test]
    fn percent_scales_by_a_hundred_and_defaults_to_no_fraction_digits() {
        let options = Options {
            style: "percent".into(),
            ..Options::default()
        };
        assert_eq!(format("en-US", options, 0.256), "26%");
    }

    #[test]
    fn trailing_zero_display_strips_only_a_whole_number() {
        let options = |value: f64| {
            format(
                "en-US",
                Options {
                    minimum_fraction_digits: Some(2.0),
                    maximum_fraction_digits: Some(2.0),
                    trailing_zero_display: "stripIfInteger".into(),
                    ..Options::default()
                },
                value,
            )
        };
        assert_eq!(options(1.0), "1");
        assert_eq!(options(1.5), "1.50");
    }

    #[test]
    fn not_a_number_and_infinity_are_words_rather_than_digits() {
        let f = NumberFormat::new(&["en-US".into()], Options::default()).unwrap();
        assert_eq!(f.format(&Numeric::Double(f64::NAN)).unwrap(), "NaN");
        assert_eq!(
            f.format(&Numeric::Double(f64::INFINITY)).unwrap(),
            "\u{221e}"
        );
        assert_eq!(
            f.format(&Numeric::Double(f64::NEG_INFINITY)).unwrap(),
            "-\u{221e}"
        );
    }

    #[test]
    fn sign_display_still_applies_to_a_value_with_no_digits() {
        // Every expectation here is `node`'s. The asymmetry is real: `NaN` takes a `+` under
        // `always` and nothing under `exceptZero`, while an infinity takes one under both.
        let with = |sign: &str, value: f64| {
            format(
                "en-US",
                Options {
                    sign_display: sign.into(),
                    ..Options::default()
                },
                value,
            )
        };
        assert_eq!(with("always", f64::NAN), "+NaN");
        assert_eq!(with("always", -f64::NAN), "+NaN");
        assert_eq!(with("exceptZero", f64::NAN), "NaN");
        assert_eq!(with("always", f64::INFINITY), "+\u{221e}");
        assert_eq!(with("always", f64::NEG_INFINITY), "-\u{221e}");
        assert_eq!(with("never", f64::NEG_INFINITY), "\u{221e}");
        assert_eq!(with("exceptZero", f64::INFINITY), "+\u{221e}");
        assert_eq!(with("negative", f64::INFINITY), "\u{221e}");
        assert_eq!(with("negative", f64::NEG_INFINITY), "-\u{221e}");
    }

    #[test]
    fn an_exact_value_is_not_routed_through_a_double() {
        // The point of the `Exact` arm: a BigInt with more digits than an f64 can hold keeps them.
        let f = NumberFormat::new(
            &["en-US".into()],
            Options {
                maximum_fraction_digits: Some(0.0),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(
            f.format(&Numeric::Exact("123456789012345678901234567890".into()))
                .unwrap(),
            "123,456,789,012,345,678,901,234,567,890"
        );
    }

    #[test]
    fn format_to_parts_reports_icus_own_runs() {
        let f = NumberFormat::new(&["en-US".into()], Options::default()).unwrap();
        assert_eq!(
            f.format_to_parts(&Numeric::Double(-1234.5)).unwrap(),
            vec![
                ("minusSign", "-".to_string()),
                ("integer", "1".to_string()),
                ("group", ",".to_string()),
                ("integer", "234".to_string()),
                ("decimal", ".".to_string()),
                ("fraction", "5".to_string()),
            ]
        );
    }

    #[test]
    fn an_out_of_range_digit_option_is_a_range_error() {
        let bad = |options: Options| {
            NumberFormat::new(&["en-US".into()], options)
                .expect_err("should be rejected")
                .kind
        };
        assert_eq!(bad(digits(Some(3.0), Some(1.0))), crate::ErrorKind::Range);
        assert_eq!(bad(digits(Some(-1.0), None)), crate::ErrorKind::Range);
        assert_eq!(bad(digits(None, Some(101.0))), crate::ErrorKind::Range);
        assert_eq!(
            bad(Options {
                minimum_significant_digits: Some(0.0),
                ..Options::default()
            }),
            crate::ErrorKind::Range
        );
        assert_eq!(
            bad(Options {
                rounding_increment: 3.0,
                ..Options::default()
            }),
            crate::ErrorKind::Range
        );
    }

    #[test]
    fn a_rounding_increment_outside_fraction_rounding_is_a_type_error() {
        // The one place in `SetNumberFormatDigitOptions` that throws a TypeError rather than a
        // RangeError, which the corpus checks specifically.
        let error = NumberFormat::new(
            &["en-US".into()],
            Options {
                rounding_increment: 5.0,
                minimum_significant_digits: Some(2.0),
                ..Options::default()
            },
        )
        .expect_err("should be rejected");
        assert_eq!(error.kind, crate::ErrorKind::Type);
    }

    #[test]
    fn currency_and_unit_are_refused_rather_than_approximated() {
        for style in ["currency", "unit"] {
            let f = NumberFormat::new(
                &["en-US".into()],
                Options {
                    style: style.into(),
                    currency: Some("USD".into()),
                    unit: Some("meter".into()),
                    ..Options::default()
                },
            )
            .expect("the constructor accepts the style");
            f.format(&Numeric::Double(1.0)).expect_err("format refuses");
        }
    }

    #[test]
    fn an_unsupported_numbering_system_falls_back_rather_than_being_reported() {
        assert!(numbering_system_is_supported("thai"));
        assert!(numbering_system_is_supported("arab"));
        assert!(numbering_system_is_supported("latn"));
        assert!(!numbering_system_is_supported("nonesuch"));
        // Algorithmic systems have no digit data, so they are not usable by a number formatter.
        assert!(!numbering_system_is_supported("roman"));
    }
}

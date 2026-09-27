//! `Intl.PluralRules`.
//!
//! Which plural form a number takes is a per-language rule expression over the *formatted* digits,
//! not over the number: English distinguishes `1 item` from `1.0 items`, and Polish sorts 2, 22 and
//! 25 into three different categories. That is why the digit options live here at all -- the rules
//! run on what the number would look like after rounding, so the rounding has to happen first.

use crate::number::{NumberFormat, Numeric, Options};
use crate::{Error, Setting};
use icu_plurals::{
    PluralCategory, PluralRuleType, PluralRules as IcuPluralRules, PluralRulesOptions,
    PluralRulesWithRanges,
};

/// The accepted values of `type`.
pub const TYPES: &[&str] = &["cardinal", "ordinal"];

/// A built plural-rule selector.
pub struct PluralRules {
    rules: IcuPluralRules,
    /// The same rules with CLDR's *range* table behind them. A range is not the category of its
    /// ends: `1-2` is `other` in English while both ends are not, and Russian sorts `0-2` into
    /// `few` although neither end is, so the pairing is data rather than a rule over two answers.
    ranges: PluralRulesWithRanges<IcuPluralRules>,
    /// The rounding half. `Intl.PluralRules` takes the same digit options as `Intl.NumberFormat`
    /// and for the same reason, so it borrows the implementation rather than growing a second one
    /// that would round `2.5` differently.
    rounding: NumberFormat,
    kind: String,
}

impl core::fmt::Debug for PluralRules {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PluralRules")
            .field("type", &self.kind)
            .finish_non_exhaustive()
    }
}

fn category_name(category: PluralCategory) -> &'static str {
    match category {
        PluralCategory::Zero => "zero",
        PluralCategory::One => "one",
        PluralCategory::Two => "two",
        PluralCategory::Few => "few",
        PluralCategory::Many => "many",
        PluralCategory::Other => "other",
    }
}

impl PluralRules {
    /// Build a selector. `options` carries the same digit options `Intl.NumberFormat` takes.
    pub fn new(requested: &[String], kind: &str, options: Options) -> Result<Self, Error> {
        let rounding = NumberFormat::new(requested, options)?;
        let locale: icu_locale_core::Locale = rounding
            .locale()
            .parse()
            .map_err(|_| Error::range("the resolved locale does not parse"))?;
        let plural_type = if kind == "ordinal" {
            PluralRuleType::Ordinal
        } else {
            PluralRuleType::Cardinal
        };
        let rules = IcuPluralRules::try_new(
            (&locale).into(),
            PluralRulesOptions::default().with_type(plural_type),
        )
        .map_err(|_| Error::range("no plural data for the resolved locale"))?;
        let ranges = if kind == "ordinal" {
            PluralRulesWithRanges::try_new_ordinal((&locale).into())
        } else {
            PluralRulesWithRanges::try_new_cardinal((&locale).into())
        }
        .map_err(|_| Error::range("no plural range data for the resolved locale"))?;
        Ok(Self {
            rules,
            ranges,
            rounding,
            kind: kind.to_string(),
        })
    }

    /// `select(value)`: the plural category the rounded value falls into.
    pub fn select(&self, value: &Numeric) -> Result<&'static str, Error> {
        let rounded = self.rounding.rounded(value)?;
        Ok(category_name(self.rules.category_for(&rounded)))
    }

    /// `selectRange(start, end)`: the category a range of two values falls into.
    pub fn select_range(&self, start: &Numeric, end: &Numeric) -> Result<&'static str, Error> {
        let start = self.rounding.rounded(start)?;
        let end = self.rounding.rounded(end)?;
        Ok(category_name(self.ranges.category_for_range(&start, &end)))
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        let mut out = vec![
            ("locale", Setting::string(self.rounding.locale())),
            ("type", Setting::string(&self.kind)),
        ];
        // The digit half is `Intl.NumberFormat`'s, minus the properties that describe a number's
        // *appearance* rather than its rounding -- a plural category has no grouping or sign.
        for (name, value) in self.rounding.resolved_options() {
            if matches!(
                name,
                "minimumIntegerDigits"
                    | "minimumFractionDigits"
                    | "maximumFractionDigits"
                    | "minimumSignificantDigits"
                    | "maximumSignificantDigits"
            ) {
                out.push((name, value));
            }
        }
        // `pluralCategories` is created *between* the digit options and the rounding options,
        // which the `resolvedOptions()` property-order tests check exactly.
        let categories: Vec<String> = self
            .rules
            .categories()
            .map(|c| category_name(c).to_string())
            .collect();
        out.push(("pluralCategories", Setting::Strings(categories)));
        for (name, value) in self.rounding.resolved_options() {
            if matches!(
                name,
                "roundingIncrement" | "roundingMode" | "roundingPriority" | "trailingZeroDisplay"
            ) {
                out.push((name, value));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(locale: &str, kind: &str, value: f64) -> &'static str {
        PluralRules::new(&[locale.to_string()], kind, Options::default())
            .expect("the rules build")
            .select(&Numeric::Double(value))
            .expect("the value selects")
    }

    #[test]
    fn english_cardinals_split_one_from_everything_else() {
        assert_eq!(select("en-US", "cardinal", 1.0), "one");
        assert_eq!(select("en-US", "cardinal", 0.0), "other");
        assert_eq!(select("en-US", "cardinal", 2.0), "other");
    }

    #[test]
    fn english_ordinals_have_four_categories() {
        assert_eq!(select("en-US", "ordinal", 1.0), "one");
        assert_eq!(select("en-US", "ordinal", 2.0), "two");
        assert_eq!(select("en-US", "ordinal", 3.0), "few");
        assert_eq!(select("en-US", "ordinal", 4.0), "other");
        assert_eq!(select("en-US", "ordinal", 11.0), "other");
        assert_eq!(select("en-US", "ordinal", 21.0), "one");
    }

    #[test]
    fn polish_sorts_a_number_into_one_of_four() {
        // The reason a second implementation is not worth writing: this is a rule expression over
        // the operands, and getting 22 and 25 into different buckets is the whole point.
        assert_eq!(select("pl", "cardinal", 1.0), "one");
        assert_eq!(select("pl", "cardinal", 2.0), "few");
        assert_eq!(select("pl", "cardinal", 5.0), "many");
        assert_eq!(select("pl", "cardinal", 22.0), "few");
        assert_eq!(select("pl", "cardinal", 25.0), "many");
        assert_eq!(select("pl", "cardinal", 1.5), "other");
    }

    #[test]
    fn the_category_follows_the_rounded_digits_rather_than_the_number() {
        // `1.0` is `other` in English and `1` is `one`; a formatter that forced two fraction
        // digits therefore makes every whole number `other`. That is the reason the digit options
        // are on `PluralRules` at all.
        let rules = PluralRules::new(
            &["en-US".to_string()],
            "cardinal",
            Options {
                minimum_fraction_digits: Some(2.0),
                ..Options::default()
            },
        )
        .unwrap();
        assert_eq!(rules.select(&Numeric::Double(1.0)).unwrap(), "other");
        let plain =
            PluralRules::new(&["en-US".to_string()], "cardinal", Options::default()).unwrap();
        assert_eq!(plain.select(&Numeric::Double(1.0)).unwrap(), "one");
    }

    #[test]
    fn a_range_has_its_own_category_rather_than_its_ends() {
        let english =
            PluralRules::new(&["en-US".to_string()], "cardinal", Options::default()).unwrap();
        let range = |a: f64, b: f64| {
            english
                .select_range(&Numeric::Double(a), &Numeric::Double(b))
                .unwrap()
        };
        assert_eq!(range(1.0, 2.0), "other");
        // The two cases the conformance corpus pins. A range whose ends land in the same category
        // still goes through the range table rather than short-circuiting: the specification's
        // equal-formatted-string shortcut was removed, so `200-200` is the `(other, other)` entry.
        assert_eq!(range(200.0, 200.0), "other");
        assert_eq!(range(102.0, 201.0), "other");
        let russian =
            PluralRules::new(&["ru".to_string()], "cardinal", Options::default()).unwrap();
        // Neither end is `few`; the range is. That is CLDR's range table and not a rule over the
        // two answers.
        assert_eq!(
            russian
                .select_range(&Numeric::Double(0.0), &Numeric::Double(2.0))
                .unwrap(),
            "few"
        );
    }

    #[test]
    fn resolved_options_report_the_categories_the_locale_actually_uses() {
        let rules = PluralRules::new(&["pl".to_string()], "cardinal", Options::default()).unwrap();
        let resolved = rules.resolved_options();
        let categories = resolved
            .iter()
            .find(|(k, _)| *k == "pluralCategories")
            .unwrap();
        assert_eq!(
            categories.1,
            Setting::Strings(vec![
                "one".into(),
                "few".into(),
                "many".into(),
                "other".into()
            ])
        );
    }
}

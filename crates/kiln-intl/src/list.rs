//! `Intl.ListFormat`.
//!
//! Joining a list is not `join(", ")` with a special case for the last element. Spanish turns
//! `y` into `e` before a word starting with `i`, Hebrew changes the conjunction before the final
//! element, and the two-element pattern is frequently not the same as the many-element one. All of
//! that is CLDR pattern data, so all of it comes from ICU4X; what this module adds is the mapping
//! from the language's `type`/`style` pair onto ICU4X's constructor/length pair.

use icu_list::ListFormatter;
use icu_list::options::{ListFormatterOptions, ListLength};

use crate::locale::{Resolution, resolve};
use crate::{Error, Setting};

/// The accepted values of `type`.
pub const TYPES: &[&str] = &["conjunction", "disjunction", "unit"];
/// The accepted values of `style`.
pub const STYLES: &[&str] = &["long", "short", "narrow"];

/// A built list formatter.
pub struct ListFormat {
    formatter: ListFormatter,
    resolution: Resolution,
    kind: String,
    style: String,
}

impl core::fmt::Debug for ListFormat {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ListFormat")
            .field("locale", &self.resolution.locale)
            .field("type", &self.kind)
            .field("style", &self.style)
            .finish_non_exhaustive()
    }
}

impl ListFormat {
    /// Build a list formatter. `kind` is `type` and `style` is `style`.
    pub fn new(requested: &[String], kind: &str, style: &str) -> Result<Self, Error> {
        let resolution = resolve(requested, &[], |_, _| false);
        let length = match style {
            "short" => ListLength::Short,
            "narrow" => ListLength::Narrow,
            _ => ListLength::Wide,
        };
        let options = ListFormatterOptions::default().with_length(length);
        let preferences = resolution.icu().into();
        let formatter = match kind {
            "disjunction" => ListFormatter::try_new_or(preferences, options),
            "unit" => ListFormatter::try_new_unit(preferences, options),
            _ => ListFormatter::try_new_and(preferences, options),
        }
        .map_err(|_| Error::range("no list formatting data for the resolved locale"))?;
        Ok(Self {
            formatter,
            resolution,
            kind: kind.to_string(),
            style: style.to_string(),
        })
    }

    /// `format(list)`.
    pub fn format(&self, items: &[String]) -> String {
        self.formatter.format_to_string(items.iter())
    }

    /// `formatToParts(list)`, using ICU4X's own element/literal annotations.
    pub fn format_to_parts(&self, items: &[String]) -> Vec<(&'static str, String)> {
        use writeable::Writeable;
        let mut sink = crate::parts::Collector::new(|part| match part {
            icu_list::parts::ELEMENT => "element",
            _ => "literal",
        });
        let _ = self
            .formatter
            .format(items.iter())
            .write_to_parts(&mut sink);
        sink.finish()
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        vec![
            ("locale", Setting::string(&self.resolution.locale)),
            ("type", Setting::string(&self.kind)),
            ("style", Setting::string(&self.style)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(locale: &str, kind: &str, style: &str, items: &[&str]) -> String {
        let items: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        ListFormat::new(&[locale.to_string()], kind, style)
            .expect("the formatter builds")
            .format(&items)
    }

    #[test]
    fn the_conjunction_is_the_locales_and_not_a_comma() {
        assert_eq!(
            format("en-US", "conjunction", "long", &["a", "b", "c"]),
            "a, b, and c"
        );
        assert_eq!(
            format("de", "conjunction", "long", &["a", "b", "c"]),
            "a, b und c"
        );
    }

    #[test]
    fn spanish_changes_the_conjunction_before_an_i_sound() {
        // The reason this is data rather than a join: `y` becomes `e` before `Italia`, and no
        // amount of string concatenation discovers that.
        assert_eq!(
            format("es", "conjunction", "long", &["Espa\u{f1}a", "Suiza"]),
            "Espa\u{f1}a y Suiza"
        );
        assert_eq!(
            format(
                "es",
                "conjunction",
                "long",
                &["Espa\u{f1}a", "Suiza", "Italia"]
            ),
            "Espa\u{f1}a, Suiza e Italia"
        );
    }

    #[test]
    fn the_two_element_pattern_is_not_the_many_element_one() {
        assert_eq!(
            format("en-US", "conjunction", "long", &["a", "b"]),
            "a and b"
        );
        assert_eq!(
            format("en-US", "disjunction", "long", &["a", "b"]),
            "a or b"
        );
        assert_eq!(
            format("en-US", "disjunction", "long", &["a", "b", "c"]),
            "a, b, or c"
        );
    }

    #[test]
    fn an_empty_or_single_list_has_no_connective_at_all() {
        assert_eq!(format("en-US", "conjunction", "long", &[]), "");
        assert_eq!(format("en-US", "conjunction", "long", &["only"]), "only");
    }

    #[test]
    fn format_to_parts_separates_the_elements_from_the_text_between_them() {
        let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let parts = ListFormat::new(&["en-US".into()], "conjunction", "long")
            .unwrap()
            .format_to_parts(&items);
        assert_eq!(
            parts,
            vec![
                ("element", "a".to_string()),
                ("literal", ", ".to_string()),
                ("element", "b".to_string()),
                ("literal", ", and ".to_string()),
                ("element", "c".to_string()),
            ]
        );
    }
}

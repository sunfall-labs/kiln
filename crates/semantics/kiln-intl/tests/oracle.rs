//! A differential check of formatted output against `node`'s full ICU.
//!
//! The cases are chosen rather than generated: each one is a place where a hand-rolled formatter
//! and a real CLDR one diverge -- Indian 3-2-2 grouping, Arabic digits paired with their own
//! separator, Polish and Spanish declining to group a four-digit number at all -- so a regression
//! in the adapter shows up as a wrong *answer* here rather than as a missing feature elsewhere.
//!
//! The expectations are recorded as literals so the suite runs with no `node` present. Where
//! `node`'s CLDR is newer than the bundle `kiln_intl::ENGINE_IDENTITY` names, a legitimate
//! data-version difference appears here as a failure; the resolution is to recheck against the
//! oracle and move the literal, never to loosen the assertion.

use kiln_intl::number::{NumberFormat, Numeric, Options};

fn format(locale: &str, value: f64) -> String {
    NumberFormat::new(&[locale.to_string()], Options::default())
        .expect("options resolve")
        .format(&Numeric::Double(value))
        .expect("value formats")
}

#[test]
fn formatted_numbers_agree_with_the_oracle() {
    // Each expectation is `new Intl.NumberFormat(<locale>).format(<value>)` under node 24.
    for (locale, value, expected) in [
        ("en-US", 1234567.891, "1,234,567.891"),
        ("de-DE", 1234567.891, "1.234.567,891"),
        // A narrow no-break space, not a space.
        ("fr-FR", 1234567.891, "1\u{202f}234\u{202f}567,891"),
        // 3-2-2 grouping rather than 3-3-3.
        ("en-IN", 1234567.0, "12,34,567"),
        ("hi-IN", 1234567.0, "12,34,567"),
        // Arabic-Indic digits with U+066C between them.
        ("ar-EG", 1234.0, "\u{661}\u{66c}\u{662}\u{663}\u{664}"),
        // Thai digits with an ordinary comma.
        ("th-u-nu-thai", 1234.0, "\u{e51},\u{e52}\u{e53}\u{e54}"),
        ("ja-JP", 1234.5, "1,234.5"),
        // `minimumGroupingDigits` is 2 here, so four digits are not grouped at all.
        ("es-ES", 1234.5, "1234,5"),
        ("pl", 1234.5, "1234,5"),
        ("ru", 1234.5, "1\u{a0}234,5"),
        // Rounded from the shortest round-trip digits, not from the exact value of the double.
        ("en-US", 1.0005, "1.001"),
        ("en-US", 1234.5678, "1,234.568"),
        ("en-US", -0.0, "-0"),
    ] {
        assert_eq!(format(locale, value), expected, "{locale} {value}");
    }
}

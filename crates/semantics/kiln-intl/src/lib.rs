//! `Intl`'s locale, number, collation and segmentation semantics, behind a Kiln-shaped Seam.
//!
//! Internationalization is the other place, after regular expressions and calendars, where the
//! honest answer is a linked engine. A locale identifier is a grammar with an alias table behind
//! it; a collation order is a 30,000-entry weight table plus a normalizer; a plural category is a
//! per-language rule expression. None of that is a page of code, and a second implementation of it
//! would agree with CLDR on `en-US` and disagree somewhere in the Serbian plural rules or the
//! Danish collation, which is a *wrong answer* rather than a refusal -- the failure mode ADR 0025
//! rejected for `regress` and ADR 0029 §4 names ICU4X to avoid here.
//!
//! What this crate adds on top of ICU4X is the Seam. No `icu_*` type appears in the public
//! interface: a tag arrives and leaves as a `String`, an option arrives as the string the language
//! spells it with, a formatter is an opaque struct, and every resolved value comes back as a
//! [`Setting`] -- a string, a number or a boolean. That is what keeps ICU4X out of HIR, IR, the
//! protocol records and the embedding ABI (ADR 0029's adoption rule, condition 2), and it is what
//! makes the deletion test meaningful: removing ICU4X leaves this interface standing and its
//! implementation missing.
//!
//! # What this crate deliberately does *not* take from ICU4X
//!
//! ECMAScript's *numeric* behaviour is specified algorithmically rather than by data.
//! `ToRawPrecision`, `ToRawFixed`, the rounding modes and the rounding increment are exact spec
//! text over a decimal string, and they are the same in every locale. Those are implemented here,
//! against the specification, because deferring them to a library that has its own rounding model
//! is how the two disagree at `0.5`. ICU4X supplies what is genuinely CLDR-shaped: the digits, the
//! separators, the grouping positions, the plural rules, the collation weights and the segment
//! boundaries.
//!
//! # The realm's locale is not the host's
//!
//! ADR 0015 gave the realm one clock (virtual, starting at the epoch) and one timezone (UTC),
//! because a program that asks the host what time it is is not reproducible. A locale is the same
//! shape of problem and gets the same answer: [`locale::DEFAULT`] is a constant, nothing here
//! reads `LANG`, `LC_ALL` or any other ambient setting, and a build that formats a number produces
//! the same bytes on every machine. ADR 0029 §4 says the same thing from the other side -- "host
//! time, timezone and locale data remain explicit inputs".

pub mod casemap;
pub mod collator;
pub mod datetime;
pub mod displaynames;
pub mod duration;
pub mod list;
pub mod locale;
pub mod normalize;
pub mod number;
mod parts;
pub mod plurals;
pub mod relative;
pub mod segmenter;

/// Which JavaScript error a rejected input should become.
///
/// ECMA-402 is unusually specific about this, and the distinction is load-bearing: an option whose
/// *value* is out of range is a `RangeError`, while one whose *type* is wrong is a `TypeError`,
/// and a large share of the conformance corpus tests nothing but which of the two comes out. The
/// adapter therefore names the error rather than leaving the caller to guess from the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// A `RangeError`: the value was the right type and not an accepted one.
    Range,
    /// A `TypeError`: the value could not be used at all.
    Type,
}

/// A rejected input, carrying the error the language should throw and the message it should carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Which JavaScript error constructor to use.
    pub kind: ErrorKind,
    /// The message text, already phrased for a JavaScript reader.
    pub message: String,
}

impl Error {
    /// A `RangeError`.
    pub fn range(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Range,
            message: message.into(),
        }
    }

    /// A `TypeError`.
    pub fn type_error(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Type,
            message: message.into(),
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.message)
    }
}

/// One entry of a `resolvedOptions()` record.
///
/// The record is a JavaScript object whose values are strings, numbers and booleans and nothing
/// else, so the Seam can carry it without ever naming a `Value`: the caller walks the list in
/// order -- which is the order the specification requires the properties to be created in -- and
/// builds the object in its own representation.
#[derive(Clone, Debug, PartialEq)]
pub enum Setting {
    /// A string-valued option, already canonical.
    String(String),
    /// A numeric option. Always an integer in practice, but the language sees a Number.
    Number(f64),
    /// A boolean option.
    Bool(bool),
    /// A list-valued option -- `pluralCategories` and nothing else so far. The language sees an
    /// Array, so the caller has to know it is building one rather than a string.
    Strings(Vec<String>),
    /// The property is present with the value `undefined`, which is not the same as absent.
    Undefined,
}

impl Setting {
    /// Convenience for the common case of a `&'static str`.
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }
}

/// The CLDR bundle and engine versions the formatters in this crate answer from.
///
/// ADR 0029's adoption rule, condition 4: a data bundle that can change observable output has to
/// participate in the identity of whatever it is baked into, or a cached artifact outlives the
/// data that produced it. `compiled_data` fixes the bundle at build time precisely so that this
/// string is knowable; the runtime-identity computation folds it in the way it already folds the
/// number provider's format and parse versions.
///
/// `icu_experimental` is versioned separately because it is pre-1.0 and moves on its own line;
/// `Intl.RelativeTimeFormat`, `Intl.DisplayNames` and `Intl.DurationFormat` answer from its bundle
/// rather than the 2.3 one, so a build that could not say which would not be reproducible either.
///
/// The CLDR version here is the one `icu_*_data` 2.3 was generated from. It is *not* necessarily
/// the version the differential oracle (`node`) was built against, so a spelled-out currency name
/// or a newly-added locale can legitimately differ between the two; that is a data-version
/// difference to be recorded, not a bug to be chased.
pub const ENGINE_IDENTITY: &str = "icu4x=2.3;icu4x-experimental=0.6;cldr=48.2.1;icu=release-78.1rc";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_names_the_javascript_constructor_rather_than_implying_it() {
        assert_eq!(Error::range("bad").kind, ErrorKind::Range);
        assert_eq!(Error::type_error("bad").kind, ErrorKind::Type);
    }

    #[test]
    fn the_engine_identity_is_stated_rather_than_discovered() {
        // A conformance run that cannot say which CLDR it measured is not reproducible. If this
        // fails after a dependency bump, the identity string is what has to move, not the test.
        assert!(ENGINE_IDENTITY.contains("cldr="));
    }
}

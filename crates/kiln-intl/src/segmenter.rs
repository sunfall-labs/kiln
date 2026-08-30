//! `Intl.Segmenter`.
//!
//! A grapheme cluster is not a code point and a word is not a run of letters. `e` followed by a
//! combining acute is one grapheme, a family emoji is one grapheme built from seven code points,
//! and Thai and Japanese are written without spaces so their word boundaries come from a
//! dictionary rather than from punctuation. UAX #29 plus that dictionary is what ICU4X supplies
//! here.
//!
//! Boundaries cross this Seam as UTF-16 code-unit indices, for the reason ADR 0022 gives: the
//! language indexes strings in code units, so an index in any other unit would have to be
//! converted by the caller and would be wrong for exactly the strings segmentation is interesting
//! on.

use icu_segmenter::options::{SentenceBreakInvariantOptions, WordBreakInvariantOptions};
use icu_segmenter::{GraphemeClusterSegmenter, SentenceSegmenter, WordSegmenter};

use crate::locale::{Resolution, resolve};
use crate::{Error, Setting};

/// The accepted values of `granularity`.
pub const GRANULARITIES: &[&str] = &["grapheme", "word", "sentence"];

/// One segment of a segmented string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// Where the segment starts, in UTF-16 code units.
    pub start: usize,
    /// Where the segment ends, in UTF-16 code units.
    pub end: usize,
    /// `true` when the segment is word-like, `false` when it is not, and `None` for a granularity
    /// where the property does not exist -- which is not the same as `false`, because the language
    /// leaves `isWordLike` off the object entirely for grapheme and sentence segmenters.
    pub word_like: Option<bool>,
}

/// A built segmenter.
#[derive(Debug)]
pub struct Segmenter {
    resolution: Resolution,
    granularity: String,
}

impl Segmenter {
    /// Build a segmenter.
    pub fn new(requested: &[String], granularity: &str) -> Result<Self, Error> {
        if !GRANULARITIES.contains(&granularity) {
            return Err(Error::range(format!("invalid granularity: {granularity}")));
        }
        let resolution = resolve(requested, &[], |_, _| false);
        Ok(Self {
            resolution,
            granularity: granularity.to_string(),
        })
    }

    /// `segment(string)`: every segment of the string, in order.
    ///
    /// The whole list rather than an iterator because the language's `Segments` object is
    /// random-access -- `containing(index)` may be asked for any index, in any order -- and a
    /// re-scan per query would turn a linear walk into a quadratic one.
    pub fn segments(&self, text: &[u16]) -> Vec<Segment> {
        let boundaries: Vec<(usize, Option<bool>)> = match self.granularity.as_str() {
            "grapheme" => GraphemeClusterSegmenter::new()
                .segment_utf16(text)
                .map(|index| (index, None))
                .collect(),
            "sentence" => SentenceSegmenter::new(SentenceBreakInvariantOptions::default())
                .segment_utf16(text)
                .map(|index| (index, None))
                .collect(),
            _ => {
                let segmenter = WordSegmenter::new_auto(WordBreakInvariantOptions::default());
                let mut iterator = segmenter.segment_utf16(text);
                let mut out = Vec::new();
                while let Some(index) = iterator.next() {
                    // `is_word_like` describes the segment that *ends* at this boundary, which is
                    // why it is recorded against the boundary and shifted when the pairs are made.
                    out.push((index, Some(iterator.is_word_like())));
                }
                out
            }
        };

        boundaries
            .windows(2)
            .map(|pair| Segment {
                start: pair[0].0,
                end: pair[1].0,
                word_like: pair[1].1,
            })
            .collect()
    }

    /// `resolvedOptions()`, in the order the specification creates the properties.
    pub fn resolved_options(&self) -> Vec<(&'static str, Setting)> {
        vec![
            ("locale", Setting::string(&self.resolution.locale)),
            ("granularity", Setting::string(&self.granularity)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(granularity: &str, text: &str) -> Vec<String> {
        let units: Vec<u16> = text.encode_utf16().collect();
        Segmenter::new(&["en-US".into()], granularity)
            .expect("the segmenter builds")
            .segments(&units)
            .into_iter()
            .map(|s| String::from_utf16_lossy(&units[s.start..s.end]))
            .collect()
    }

    #[test]
    fn a_grapheme_is_not_a_code_point() {
        // `e` + combining acute is one grapheme made of two code points; the family emoji is one
        // grapheme made of seven. Counting code points gets both wrong.
        assert_eq!(segment("grapheme", "e\u{301}"), vec!["e\u{301}"]);
        assert_eq!(
            segment("grapheme", "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"),
            vec!["\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"]
        );
        assert_eq!(segment("grapheme", "abc"), vec!["a", "b", "c"]);
    }

    #[test]
    fn a_surrogate_pair_is_one_grapheme_and_two_code_units() {
        let text = "\u{1f600}";
        let units: Vec<u16> = text.encode_utf16().collect();
        assert_eq!(units.len(), 2);
        let segments = Segmenter::new(&["en-US".into()], "grapheme")
            .unwrap()
            .segments(&units);
        // One segment spanning both code units: the indices are in the units the language counts
        // in, so `end` is 2 rather than 1.
        assert_eq!(
            segments,
            vec![Segment {
                start: 0,
                end: 2,
                word_like: None
            }]
        );
    }

    #[test]
    fn word_segmentation_separates_words_from_the_text_between_them() {
        assert_eq!(
            segment("word", "Hello, world!"),
            vec!["Hello", ",", " ", "world", "!"]
        );
    }

    #[test]
    fn word_like_distinguishes_a_word_from_the_punctuation_beside_it() {
        let units: Vec<u16> = "Hi, 42!".encode_utf16().collect();
        let segments = Segmenter::new(&["en-US".into()], "word")
            .unwrap()
            .segments(&units);
        let word_like: Vec<Option<bool>> = segments.iter().map(|s| s.word_like).collect();
        assert_eq!(
            word_like,
            vec![
                Some(true),
                Some(false),
                Some(false),
                Some(true),
                Some(false)
            ]
        );
    }

    #[test]
    fn a_script_without_spaces_still_has_word_boundaries() {
        // Thai is written without spaces, so the boundaries come from a dictionary. A splitter
        // that looked for whitespace would return the whole string and be silently wrong.
        let segments = segment(
            "word",
            "\u{e2a}\u{e27}\u{e31}\u{e2a}\u{e14}\u{e35}\u{e04}\u{e23}\u{e31}\u{e1a}",
        );
        assert!(
            segments.len() > 1,
            "expected Thai to split, got {segments:?}"
        );
    }

    #[test]
    fn sentence_segmentation_keeps_an_abbreviation_with_its_sentence() {
        let segments = segment("sentence", "Hello world. Goodbye.");
        assert_eq!(segments, vec!["Hello world. ", "Goodbye."]);
    }

    #[test]
    fn an_empty_string_has_no_segments() {
        assert!(segment("grapheme", "").is_empty());
        assert!(segment("word", "").is_empty());
    }

    #[test]
    fn a_granularity_the_language_does_not_have_is_a_range_error() {
        let error = Segmenter::new(&["en-US".into()], "line").expect_err("should be rejected");
        assert_eq!(error.kind, crate::ErrorKind::Range);
    }
}

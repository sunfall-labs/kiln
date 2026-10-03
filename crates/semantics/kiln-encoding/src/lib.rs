//! The WHATWG `TextDecoder` core: label resolution and decoding, owned here once.
//!
//! The interpreter calls this directly and the native provider calls it through
//! `kiln_text_decoder_label` / `kiln_text_decode` (ADR 0060). Neither engine may carry its own
//! label table, BOM rule or error text, so the two cannot drift apart.

use encoding_rs::{Decoder as RsDecoder, DecoderResult, Encoding};

/// The label `new TextDecoder()` resolves when it is given none.
pub const DEFAULT_LABEL: &str = "utf-8";

/// `TextDecoderOptions.fatal`, as the flag word the provider ABI carries.
pub const FLAG_FATAL: u32 = 1;
/// `TextDecoderOptions.ignoreBOM`, as the flag word the provider ABI carries.
pub const FLAG_IGNORE_BOM: u32 = 2;
/// Shared TypeError text for malformed input, also used to preflight native output buffers.
pub const INVALID_DATA_MESSAGE: &str = "The encoded data was not valid for encoding";

/// Why a `TextDecoder` operation failed. The variant picks the JavaScript error class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// `RangeError`: the label names no encoding.
    InvalidLabel(String),
    /// `TypeError`: `fatal` was set and the input was not valid for the encoding.
    InvalidData,
}

/// One WHATWG decoder's incremental state. The owning JavaScript `TextDecoder` keeps this state
/// in its own engine object; this type contains no JavaScript values or realm references.
#[derive(Debug)]
pub struct StreamingDecoder {
    encoding: &'static Encoding,
    decoder: RsDecoder,
    fatal: bool,
    ignore_bom: bool,
}

impl StreamingDecoder {
    /// Resolve and construct a decoder once, preserving BOM and malformed-input policy across
    /// every `decode` call on that object.
    pub fn new(label: &str, fatal: bool, ignore_bom: bool) -> Result<Self, Error> {
        let encoding = resolve(label)?;
        let decoder = Self::fresh_decoder(encoding, ignore_bom);
        Ok(Self {
            encoding,
            decoder,
            fatal,
            ignore_bom,
        })
    }

    /// Decode one segment. `stream=false` marks the end of this sequence so the codec flushes
    /// incomplete bytes and resets itself for the next independent sequence.
    pub fn decode(&mut self, bytes: &[u8], stream: bool) -> Result<String, Error> {
        let last = !stream;
        let mut output = String::new();
        let result = if self.fatal {
            output.reserve(
                self.decoder
                    .max_utf8_buffer_length_without_replacement(bytes.len())
                    .ok_or(Error::InvalidData)?,
            );
            let (result, read) =
                self.decoder
                    .decode_to_string_without_replacement(bytes, &mut output, last);
            debug_assert!(read <= bytes.len());
            match result {
                DecoderResult::InputEmpty => Ok(output),
                DecoderResult::OutputFull => Err(Error::InvalidData),
                DecoderResult::Malformed(_, _) => Err(Error::InvalidData),
            }
        } else {
            output.reserve(
                self.decoder
                    .max_utf8_buffer_length(bytes.len())
                    .ok_or(Error::InvalidData)?,
            );
            let (result, read, _) = self.decoder.decode_to_string(bytes, &mut output, last);
            debug_assert!(read <= bytes.len());
            match result {
                encoding_rs::CoderResult::InputEmpty => Ok(output),
                encoding_rs::CoderResult::OutputFull => Err(Error::InvalidData),
            }
        };
        // encoding_rs marks a decoder finished after `last=true`. TextDecoder starts a fresh
        // independent sequence on the next decode(), so restore its initial BOM/codec state
        // after every final call and after fatal errors.
        if !stream || result.is_err() {
            self.decoder = Self::fresh_decoder(self.encoding, self.ignore_bom);
        }
        result
    }

    /// A safe output-buffer bound for one input segment in the current decoder state.
    ///
    /// The native provider uses this before mutating the decoder so a caller can retry with a
    /// larger buffer without submitting the same bytes twice. The fatal error message is
    /// included because malformed input is written into that same output buffer.
    pub fn max_output_buffer_length(&self, input_bytes: usize) -> Option<usize> {
        let output = if self.fatal {
            self.decoder
                .max_utf8_buffer_length_without_replacement(input_bytes)?
        } else {
            self.decoder.max_utf8_buffer_length(input_bytes)?
        };
        Some(if self.fatal {
            output.max(INVALID_DATA_MESSAGE.len())
        } else {
            output
        })
    }

    fn fresh_decoder(encoding: &'static Encoding, ignore_bom: bool) -> RsDecoder {
        if ignore_bom {
            encoding.new_decoder_without_bom_handling()
        } else {
            // `new_decoder` also allows BOMs to override UTF-8/UTF-16 selection. TextDecoder
            // has already selected its encoding from the label; it only strips an initial BOM.
            encoding.new_decoder_with_bom_removal()
        }
    }

    /// Canonical encoding label, retained so adapters do not need a second resolution path.
    pub fn encoding_name(&self) -> &'static str {
        self.encoding.name()
    }
}

impl Error {
    /// The exact message both engines throw.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidLabel(label) => {
                format!("The encoding label provided ('{label}') is invalid")
            }
            Self::InvalidData => INVALID_DATA_MESSAGE.to_owned(),
        }
    }
}

/// Resolve a WHATWG label, trimming it first. `replacement` and the legacy labels that map to it
/// resolve like any other.
pub fn resolve(label: &str) -> Result<&'static Encoding, Error> {
    Encoding::for_label(label.trim().as_bytes())
        .ok_or_else(|| Error::InvalidLabel(label.to_owned()))
}

/// The canonical lowercase name `TextDecoder.prototype.encoding` reports for a label.
pub fn canonical_name(label: &str) -> Result<String, Error> {
    Ok(resolve(label)?.name().to_ascii_lowercase())
}

/// `TextDecoder.prototype.decode` for a decoder whose `encoding` attribute is `encoding_name`
/// (an unresolvable name falls back to UTF-8, as an attribute that was never overwritten cannot
/// produce one).
pub fn decode(
    encoding_name: &str,
    bytes: &[u8],
    fatal: bool,
    ignore_bom: bool,
) -> Result<String, Error> {
    let encoding = Encoding::for_label(encoding_name.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    let (decoded, had_errors) = if ignore_bom {
        encoding.decode_without_bom_handling(bytes)
    } else {
        encoding.decode_with_bom_removal(bytes)
    };
    if fatal && had_errors {
        return Err(Error::InvalidData);
    }
    Ok(decoded.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_resolve_to_canonical_lowercase_names() {
        assert_eq!(canonical_name(" UTF8 ").unwrap(), "utf-8");
        assert_eq!(canonical_name("latin1").unwrap(), "windows-1252");
        assert_eq!(canonical_name("unicode-1-1-utf-8").unwrap(), "utf-8");
        assert_eq!(canonical_name("ucs-2").unwrap(), "utf-16le");
        assert_eq!(canonical_name("sjis").unwrap(), "shift_jis");
        assert_eq!(canonical_name("replacement").unwrap(), "replacement");
        assert_eq!(
            canonical_name("nope").unwrap_err().message(),
            "The encoding label provided ('nope') is invalid"
        );
    }

    #[test]
    fn bom_and_fatal_follow_the_options() {
        let with_bom = [0xEF, 0xBB, 0xBF, b'a'];
        assert_eq!(decode("utf-8", &with_bom, false, false).unwrap(), "a");
        assert_eq!(
            decode("utf-8", &with_bom, false, true).unwrap(),
            "\u{FEFF}a"
        );
        assert_eq!(decode("utf-8", &[0xFF], false, false).unwrap(), "\u{FFFD}");
        assert_eq!(
            decode("utf-8", &[0xFF], true, false),
            Err(Error::InvalidData)
        );
        assert_eq!(
            decode("utf-16le", &[0xFF, 0xFE, b'a', 0], false, false).unwrap(),
            "a"
        );
    }

    #[test]
    fn streaming_retains_utf8_bom_and_partial_character_state() {
        let mut decoder = StreamingDecoder::new("utf-8", false, false).unwrap();
        assert_eq!(decoder.decode(&[0xEF], true).unwrap(), "");
        assert_eq!(decoder.decode(&[0xBB, 0xBF, b'A'], true).unwrap(), "A");
        assert_eq!(decoder.decode(&[0xF0, 0x9F], true).unwrap(), "");
        assert_eq!(decoder.decode(&[0x98], true).unwrap(), "");
        assert_eq!(decoder.decode(&[0x80], false).unwrap(), "😀");
        assert_eq!(decoder.decode(b"again", false).unwrap(), "again");
    }

    #[test]
    fn streaming_preserves_legacy_shift_state_between_segments() {
        let mut decoder = StreamingDecoder::new("iso-2022-jp", false, false).unwrap();
        assert_eq!(decoder.decode(&[0x1B, b'$'], true).unwrap(), "");
        assert_eq!(decoder.decode(&[b'B', 0x24], true).unwrap(), "");
        assert_eq!(
            decoder.decode(&[0x22, 0x1B, b'(', b'B'], false).unwrap(),
            "あ"
        );
    }

    #[test]
    fn streaming_fatal_flush_rejects_then_resets_for_next_sequence() {
        let mut decoder = StreamingDecoder::new("utf-8", true, false).unwrap();
        assert_eq!(
            decoder.decode(&[0xff, 0xfe], false),
            Err(Error::InvalidData)
        );
        assert_eq!(decoder.decode(&[0xE2], true).unwrap(), "");
        assert_eq!(decoder.decode(&[], false), Err(Error::InvalidData));
        assert_eq!(decoder.decode(b"A", false).unwrap(), "A");
    }
}

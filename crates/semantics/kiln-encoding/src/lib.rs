//! The WHATWG `TextDecoder` core: label resolution and decoding, owned here once.
//!
//! The interpreter calls this directly and the native provider calls it through
//! `kiln_text_decoder_label` / `kiln_text_decode` (ADR 0060). Neither engine may carry its own
//! label table, BOM rule or error text, so the two cannot drift apart.

use encoding_rs::Encoding;

/// The label `new TextDecoder()` resolves when it is given none.
pub const DEFAULT_LABEL: &str = "utf-8";

/// `TextDecoderOptions.fatal`, as the flag word the provider ABI carries.
pub const FLAG_FATAL: u32 = 1;
/// `TextDecoderOptions.ignoreBOM`, as the flag word the provider ABI carries.
pub const FLAG_IGNORE_BOM: u32 = 2;

/// Why a `TextDecoder` operation failed. The variant picks the JavaScript error class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Error {
    /// `RangeError`: the label names no encoding.
    InvalidLabel(String),
    /// `TypeError`: `fatal` was set and the input was not valid for the encoding.
    InvalidData,
}

impl Error {
    /// The exact message both engines throw.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidLabel(label) => {
                format!("The encoding label provided ('{label}') is invalid")
            }
            Self::InvalidData => "The encoded data was not valid for encoding".to_owned(),
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
}

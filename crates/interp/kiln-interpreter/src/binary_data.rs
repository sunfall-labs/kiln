use super::*;

/// Which element type a view reads its bytes as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedKind {
    Int8,
    Uint8,
    /// `Uint8ClampedArray`: out-of-range writes saturate instead of wrapping, which is what makes
    /// it the right type for pixel data and the wrong one for anything arithmetic.
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    /// `Float16Array`, and the `DataView` pair that reads the same two bytes.
    ///
    /// Sixteen bits of float have no Rust type on this toolchain, so the conversion is spelled
    /// out here rather than borrowed. It is not a rounding of `Float32`: half has five bits of
    /// exponent and ten of mantissa, so its subnormals begin where nothing else's do, and a
    /// value that overflows it becomes an infinity rather than the nearest finite number.
    Float16,
    Float32,
    Float64,
    /// `BigInt64Array` and `BigUint64Array`, whose elements are `BigInt`s rather than numbers.
    ///
    /// Not a formatting choice: sixty-four bits of integer do not fit in an `f64`, so a view
    /// that decoded them to one would hand back the *nearest double* -- exactly the silent
    /// rounding the BigInt type exists to prevent, and undetectable from inside the program
    /// because the wrong answer is still a number.
    BigInt64,
    BigUint64,
}

impl TypedKind {
    /// How many bytes one element occupies.
    pub(super) fn width(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 | Self::Float16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }

    /// Whether this kind's elements are BigInts rather than Numbers.
    ///
    /// The specification calls this a view's *content type*, and it gates far more than
    /// decoding: a Number assigned into a `BigInt64Array` is a `TypeError` rather than a
    /// rounded integer, and `set` refuses a source whose content type differs instead of
    /// converting between the two. Two numeric towers that never cross implicitly is the
    /// whole reason the language has two.
    pub(super) fn is_big(self) -> bool {
        matches!(self, Self::BigInt64 | Self::BigUint64)
    }

    /// The element half of the view's name: `Int8Array` without the `Array`.
    ///
    /// What a `DataView` accessor is called after -- `getInt8`, `setFloat64` -- so that the two
    /// spellings cannot drift from one another.
    pub(super) fn element_name(self) -> &'static str {
        self.name()
            .strip_suffix("Array")
            .expect("every typed-array name ends in Array")
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Int8 => "Int8Array",
            Self::Uint8 => "Uint8Array",
            Self::Uint8Clamped => "Uint8ClampedArray",
            Self::Int16 => "Int16Array",
            Self::Uint16 => "Uint16Array",
            Self::Int32 => "Int32Array",
            Self::Uint32 => "Uint32Array",
            Self::Float16 => "Float16Array",
            Self::Float32 => "Float32Array",
            Self::Float64 => "Float64Array",
            Self::BigInt64 => "BigInt64Array",
            Self::BigUint64 => "BigUint64Array",
        }
    }

    /// One element decoded from `bytes`, least-significant byte first.
    ///
    /// Split out of the view that owns the buffer because a `DataView` reads the same eight
    /// element types out of the same bytes and differs only in the *order* it is handed them.
    /// A second copy of these conversions could disagree with this one about what a `Float32`
    /// is, and a disagreement between two readers of one buffer is unfindable from the outside.
    pub(super) fn decode(self, bytes: &[u8]) -> Value {
        match self {
            Self::Int8 => Value::Number(f64::from(bytes[0] as i8)),
            Self::Uint8 | Self::Uint8Clamped => Value::Number(f64::from(bytes[0])),
            Self::Int16 => Value::Number(f64::from(i16::from_le_bytes([bytes[0], bytes[1]]))),
            Self::Uint16 => Value::Number(f64::from(u16::from_le_bytes([bytes[0], bytes[1]]))),
            Self::Int32 => Value::Number(f64::from(i32::from_le_bytes(
                bytes.try_into().expect("four bytes"),
            ))),
            Self::Uint32 => Value::Number(f64::from(u32::from_le_bytes(
                bytes.try_into().expect("four bytes"),
            ))),
            Self::Float16 => Value::Number(f64::from(half_to_f32(u16::from_le_bytes([
                bytes[0], bytes[1],
            ])))),
            Self::Float32 => Value::Number(f64::from(f32::from_le_bytes(
                bytes.try_into().expect("four bytes"),
            ))),
            Self::Float64 => {
                Value::Number(f64::from_le_bytes(bytes.try_into().expect("eight bytes")))
            }
            // The same eight bytes the `DataView` pair reads, and read the same way, so a
            // buffer written through `setBigInt64` and read through a `BigInt64Array` agrees
            // with itself.
            Self::BigInt64 => Value::BigInt(Rc::new(num_bigint::BigInt::from(i64::from_le_bytes(
                bytes.try_into().expect("eight bytes"),
            )))),
            Self::BigUint64 => Value::BigInt(Rc::new(num_bigint::BigInt::from(
                u64::from_le_bytes(bytes.try_into().expect("eight bytes")),
            ))),
        }
    }

    /// `value` written into `slot`, least-significant byte first.
    ///
    /// `value` has already been through [`coerce_element`], which is where a write that crosses
    /// between the two numeric towers is refused and where a program's `valueOf` has had its
    /// say. What is left here is the *narrowing*: every integer kind but the clamped one wraps,
    /// taking the value modulo 2^n, which is why `u8[0] = 256` is 0 and `big[0] = 2n ** 64n`
    /// is 0n.
    pub(super) fn encode(self, value: &Value, slot: &mut [u8]) {
        // Split off first rather than reached through `as_number`, because these two are the
        // kinds for which a number is not an element at all.
        if let Self::BigInt64 | Self::BigUint64 = self {
            let raw = match value {
                Value::BigInt(value) => wrapping_u64(value),
                // `coerce_element` admits nothing else for these kinds; zero rather than a
                // panic so a future caller that forgets it cannot take the process down.
                _ => 0,
            };
            slot.copy_from_slice(&raw.to_le_bytes());
            return;
        }
        let value = value.as_number();
        match self {
            // Every integer type but the clamped one *wraps*: the value is taken modulo 2^n,
            // which is what `ToInt32` and its siblings do and is why `u8[0] = 256` is 0.
            Self::Int8 => slot[0] = to_int32(value) as u8,
            Self::Uint8 => slot[0] = to_uint32(value) as u8,
            Self::Uint8Clamped => {
                slot[0] = if value.is_nan() {
                    0
                } else {
                    // Saturating, and rounding halves to even, which is what the specification
                    // says and what keeps pixel arithmetic from drifting.
                    let rounded = if (value.fract().abs() - 0.5).abs() < f64::EPSILON {
                        let down = value.floor();
                        if (down as i64) % 2 == 0 {
                            down
                        } else {
                            down + 1.0
                        }
                    } else {
                        value.round()
                    };
                    rounded.clamp(0.0, 255.0) as u8
                };
            }
            Self::Int16 => slot.copy_from_slice(&(to_int32(value) as i16).to_le_bytes()),
            Self::Uint16 => slot.copy_from_slice(&(to_uint32(value) as u16).to_le_bytes()),
            Self::Int32 => slot.copy_from_slice(&to_int32(value).to_le_bytes()),
            Self::Uint32 => slot.copy_from_slice(&to_uint32(value).to_le_bytes()),
            Self::Float16 => slot.copy_from_slice(&f64_to_half(value).to_le_bytes()),
            #[allow(clippy::cast_possible_truncation)]
            Self::Float32 => slot.copy_from_slice(&(value as f32).to_le_bytes()),
            Self::Float64 => slot.copy_from_slice(&value.to_le_bytes()),
            Self::BigInt64 | Self::BigUint64 => unreachable!("handled above"),
        }
    }
}

/// An IEEE-754 binary16 bit pattern as the number it stands for.
///
/// Spelled out rather than borrowed because this toolchain has no `f16`. Half has five bits of
/// exponent and ten of mantissa, so the three cases are the ones any float format has: a zero
/// exponent is a subnormal -- or a zero -- scaled by 2^-24, an exponent of all ones is an
/// infinity or a NaN, and everything between is the ordinary `1.m x 2^(e-15)`.
pub(super) fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0_f32 } else { -1.0 };
    let exponent = (bits >> 10) & 0x1f;
    let mantissa = f32::from(bits & 0x3ff);
    match exponent {
        // Subnormal, and zero with it: no implicit leading one, and the smallest step half can
        // take is 2^-24.
        0 => sign * mantissa * (2.0_f32).powi(-24),
        // Every exponent bit set is an infinity when the mantissa is empty and a NaN when it is
        // not. Which NaN is not observable -- the language has one -- so the sign is dropped.
        0x1f => {
            if mantissa == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + mantissa / 1024.0) * (2.0_f32).powi(i32::from(exponent) - 15),
    }
}

/// A number as the nearest binary16 bit pattern, rounding halves to even.
///
/// `ToFloat16`, in one rounding step from the number the language actually holds.
///
/// One step, and that is the whole point of not going through `f32`. Two roundings are not one:
/// `2.980232238769532e-8` is a hair above half of the smallest subnormal half and rounds *up* to
/// it, but rounding it to binary32 first lands exactly on the halfway point, where round-to-even
/// then takes it down to zero. Every value between two adjacent binary32 neighbours of a half
/// boundary has the same problem.
///
/// Not a truncation of the binary64 pattern either: half's subnormals begin where nothing else's
/// do, so a value too small for a normal half still has bits to keep, and a value too large
/// becomes an *infinity* rather than the largest finite half.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(super) fn f64_to_half(value: f64) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 48) & 0x8000) as u16;
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & 0x000f_ffff_ffff_ffff;
    // NaN keeps a non-empty mantissa so it does not come back as an infinity; which NaN it is
    // is not observable.
    if exponent == 0x7ff {
        return sign | 0x7c00 | u16::from(mantissa != 0) << 9;
    }
    // The unbiased exponent, rebiased for half. Above half's range is an infinity, which is what
    // the specification's `ToFloat16` produces rather than the largest finite value.
    let unbiased = exponent - 1023 + 15;
    if unbiased >= 0x1f {
        return sign | 0x7c00;
    }
    if unbiased <= 0 {
        // Subnormal, and zero with it. The implicit leading one comes back for the shift; below
        // this the shift would be wider than the word and the answer is a signed zero anyway.
        if unbiased < -20 {
            return sign;
        }
        let with_leading = if exponent == 0 {
            // A binary64 subnormal is far below half's smallest, so it is already zero above --
            // but a zero itself arrives here and has no implicit one.
            mantissa
        } else {
            mantissa | 0x0010_0000_0000_0000
        };
        let shift = (43 - unbiased) as u32;
        let kept = with_leading >> shift;
        let remainder = with_leading & ((1_u64 << shift) - 1);
        let half = 1_u64 << (shift - 1);
        let rounded = if remainder > half || (remainder == half && kept & 1 == 1) {
            kept + 1
        } else {
            kept
        };
        return sign | rounded as u16;
    }
    let kept = mantissa >> 42;
    let remainder = mantissa & 0x3ff_ffff_ffff;
    let mut raw = ((unbiased as u64) << 10) | kept;
    if remainder > 0x200_0000_0000 || (remainder == 0x200_0000_0000 && kept & 1 == 1) {
        // A carry out of the mantissa lands in the exponent, which is what makes 65520 round up
        // to an infinity rather than to the largest finite half.
        raw += 1;
    }
    sign | raw as u16
}

/// What a decoder raises when the text is not what it claims to be.
///
/// A `SyntaxError` rather than a `TypeError`, because the argument is the right *kind* of thing
/// and the wrong *spelling* of one -- the same distinction `JSON.parse` draws.
pub(super) fn byte_text_refused(base64: bool) -> ExecutionError {
    ExecutionError::SyntaxErrorMessage {
        message: if base64 {
            "invalid base64 string".to_owned()
        } else {
            "invalid hexadecimal string".to_owned()
        },
    }
}

/// What a *concrete* view's constructor or prototype publishes that the abstract one does not.
///
/// One function for both halves so that a static and a method cannot end up on the wrong object,
/// and so the instance read, the table and the own-key walk agree about which names exist.
pub(super) fn byte_text_method(kind: TypedKind, name: &str, statics: bool) -> Option<Value> {
    if kind != TypedKind::Uint8 {
        return None;
    }
    let found = [
        ByteText::FromBase64,
        ByteText::FromHex,
        ByteText::ToBase64,
        ByteText::ToHex,
        ByteText::SetFromBase64,
        ByteText::SetFromHex,
    ]
    .into_iter()
    .find(|conversion| conversion.key() == name && conversion.is_static() == statics)?;
    Some(Value::Native(Native::Uint8Text(found), RealmId::PRIMARY))
}

/// The methods a concrete view's prototype publishes, as a value rather than an option, for the
/// tables that answer `undefined` for a name they do not have.
pub(super) fn typed_array_for_method(kind: TypedKind, name: &str) -> Value {
    byte_text_method(kind, name, false).unwrap_or(Value::Undefined)
}

/// One of the six conversions between a `Uint8Array` and a text spelling of its bytes.
///
/// Defined on `Uint8Array` alone rather than on every view, because bytes are the only element
/// type a text encoding of a buffer can mean: there is no agreed spelling of an `Int32Array` in
/// base64, and a view whose elements are wider than a byte would have to invent a byte order to
/// have one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByteText {
    FromBase64,
    FromHex,
    ToBase64,
    ToHex,
    SetFromBase64,
    SetFromHex,
}

impl ByteText {
    /// The name this conversion answers to, which is also where it lives: the two that build a
    /// view are statics on the constructor and the four that read or fill one are methods.
    pub(super) const fn key(self) -> &'static str {
        match self {
            Self::FromBase64 => "fromBase64",
            Self::FromHex => "fromHex",
            Self::ToBase64 => "toBase64",
            Self::ToHex => "toHex",
            Self::SetFromBase64 => "setFromBase64",
            Self::SetFromHex => "setFromHex",
        }
    }

    pub(super) const fn is_static(self) -> bool {
        matches!(self, Self::FromBase64 | Self::FromHex)
    }
}

/// What to do with a base64 string whose final chunk is not four characters long.
///
/// Three answers rather than a boolean because they are genuinely three: `loose` decodes what is
/// there, `strict` refuses it, and `stop-before-partial` decodes everything before it and
/// reports how far it got -- which is the one a program streaming a base64 document needs, since
/// the next read will carry the rest of that chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LastChunk {
    Loose,
    Strict,
    StopBeforePartial,
}

/// What a decode produced: how much of the string it consumed, the bytes it made, and the
/// refusal it stopped at.
///
/// The three travel together because `setFromBase64` reports all three and in that order: the
/// bytes decoded before an illegal character are *written* and the refusal is raised afterwards,
/// so a decode that both produced output and failed is an ordinary outcome rather than a
/// contradiction.
pub(super) struct Decoded {
    pub(super) read: usize,
    pub(super) bytes: Vec<u8>,
    pub(super) refused: bool,
}

/// The value a base64 digit stands for, in whichever alphabet was asked for.
///
/// The two differ in exactly two characters, and mixing them is a refusal rather than a
/// fallback: `+` and `/` are not base64url, `-` and `_` are not base64, and a decoder that
/// accepted both would silently accept a string neither encoder produces.
const fn base64_value(character: u8, url: bool) -> Option<u8> {
    Some(match character {
        b'A'..=b'Z' => character - b'A',
        b'a'..=b'z' => character - b'a' + 26,
        b'0'..=b'9' => character - b'0' + 52,
        b'+' if !url => 62,
        b'/' if !url => 63,
        b'-' if url => 62,
        b'_' if url => 63,
        _ => return None,
    })
}

/// The digit a six-bit value is written as.
const fn base64_digit(value: u8, url: bool) -> u8 {
    match value {
        0..=25 => b'A' + value,
        26..=51 => b'a' + value - 26,
        52..=61 => b'0' + value - 52,
        62 => {
            if url {
                b'-'
            } else {
                b'+'
            }
        }
        _ => {
            if url {
                b'_'
            } else {
                b'/'
            }
        }
    }
}

/// The five characters `FromBase64` skips wherever they appear.
///
/// ASCII whitespace and nothing else: a non-breaking space, a thin space and a line separator
/// are all refusals, which is what stops a string pasted out of a rendered document from
/// decoding to something subtly different from what it displays.
const fn is_base64_whitespace(character: u8) -> bool {
    matches!(character, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

/// `DecodeBase64Chunk`: two, three or four digits as the one, two or three bytes they carry.
///
/// A short chunk has bits left over -- four in a two-digit chunk, two in a three-digit one --
/// and `strict` is the mode that refuses a chunk whose leftover bits are not zero. They are
/// unreachable through any encoder, so a string carrying them was not produced by one, and
/// accepting it means two different strings decode to the same bytes.
pub(super) fn decode_base64_chunk(chunk: &[u8], strict: bool) -> Option<Vec<u8>> {
    let mut packed: u32 = 0;
    for digit in chunk {
        packed = (packed << 6) | u32::from(*digit);
    }
    match chunk.len() {
        2 => {
            if strict && packed & 0xf != 0 {
                return None;
            }
            #[expect(clippy::cast_possible_truncation)]
            Some(vec![(packed >> 4) as u8])
        }
        3 => {
            if strict && packed & 0x3 != 0 {
                return None;
            }
            #[expect(clippy::cast_possible_truncation)]
            Some(vec![(packed >> 10) as u8, (packed >> 2) as u8])
        }
        #[expect(clippy::cast_possible_truncation)]
        _ => Some(vec![
            (packed >> 16) as u8,
            (packed >> 8) as u8,
            packed as u8,
        ]),
    }
}

/// `FromBase64(string, alphabet, lastChunkHandling, maxLength)`.
///
/// Bounded by `limit` rather than decoding everything and truncating, because the bound is
/// observable twice over: `setFromBase64` reports how much of the string it consumed, and a
/// string whose *next* chunk would not fit is left unread rather than partly written. That is
/// also why trailing garbage past a filled target is not a refusal -- the decoder stopped
/// before reaching it, and a character it never looked at cannot be illegal.
pub(super) fn from_base64(text: &str, url: bool, handling: LastChunk, limit: usize) -> Decoded {
    let units: Vec<u8> = text
        .chars()
        // A character outside ASCII cannot be a digit, a pad or whitespace, so it is refused --
        // and it is folded to one byte here so the walk is over positions the caller counts in.
        .map(|character| u8::try_from(character as u32).unwrap_or(0xff))
        .collect();
    let length = units.len();
    let mut read = 0;
    let mut bytes: Vec<u8> = Vec::new();
    let mut chunk: Vec<u8> = Vec::new();
    let mut index = 0;
    if limit == 0 {
        return Decoded {
            read,
            bytes,
            refused: false,
        };
    }
    let skip = |mut at: usize| {
        while at < length && is_base64_whitespace(units[at]) {
            at += 1;
        }
        at
    };
    let refuse = |read: usize, bytes: Vec<u8>| Decoded {
        read,
        bytes,
        refused: true,
    };
    loop {
        index = skip(index);
        if index == length {
            if chunk.is_empty() {
                return Decoded {
                    read: length,
                    bytes,
                    refused: false,
                };
            }
            // A chunk with nothing after it. `stop-before-partial` leaves it for the next read,
            // `strict` refuses it because a complete document pads its last chunk, and `loose`
            // takes what is there -- unless it is a single digit, which carries no whole byte.
            return match handling {
                LastChunk::StopBeforePartial => Decoded {
                    read,
                    bytes,
                    refused: false,
                },
                LastChunk::Strict => refuse(read, bytes),
                LastChunk::Loose if chunk.len() == 1 => refuse(read, bytes),
                LastChunk::Loose => {
                    if limit - bytes.len() < chunk.len() - 1 {
                        return Decoded {
                            read,
                            bytes,
                            refused: false,
                        };
                    }
                    match decode_base64_chunk(&chunk, false) {
                        Some(decoded) => {
                            bytes.extend(decoded);
                            Decoded {
                                read: length,
                                bytes,
                                refused: false,
                            }
                        }
                        None => refuse(read, bytes),
                    }
                }
            };
        }
        let unit = units[index];
        index += 1;
        if unit == b'=' {
            // Padding closes the string: a chunk of two digits needs two pads and one of three
            // needs one, and anything after them -- another pad included -- is a refusal.
            if chunk.len() < 2 {
                return refuse(read, bytes);
            }
            index = skip(index);
            if chunk.len() == 2 {
                if index == length {
                    return match handling {
                        LastChunk::StopBeforePartial => Decoded {
                            read,
                            bytes,
                            refused: false,
                        },
                        _ => refuse(read, bytes),
                    };
                }
                if units[index] != b'=' {
                    return refuse(read, bytes);
                }
                index = skip(index + 1);
            }
            if index != length {
                return refuse(read, bytes);
            }
            if limit - bytes.len() < chunk.len() - 1 {
                return Decoded {
                    read,
                    bytes,
                    refused: false,
                };
            }
            return match decode_base64_chunk(&chunk, handling == LastChunk::Strict) {
                Some(decoded) => {
                    bytes.extend(decoded);
                    Decoded {
                        read: length,
                        bytes,
                        refused: false,
                    }
                }
                None => refuse(read, bytes),
            };
        }
        let Some(value) = base64_value(unit, url) else {
            return refuse(read, bytes);
        };
        chunk.push(value);
        if chunk.len() == 4 {
            // A chunk that would not fit is left *unread* rather than partly written, which is
            // the difference between a bounded decode and a decode-then-truncate: a program
            // filling a five-byte target from an eight-character string is told it consumed
            // four characters, so the next read starts where this one stopped.
            if limit - bytes.len() < 3 {
                return Decoded {
                    read,
                    bytes,
                    refused: false,
                };
            }
            bytes.extend(decode_base64_chunk(&chunk, false).unwrap_or_default());
            chunk.clear();
            read = index;
            if bytes.len() == limit {
                return Decoded {
                    read,
                    bytes,
                    refused: false,
                };
            }
        }
    }
}

/// `FromHex(string, maxLength)`.
///
/// No whitespace and no partial byte: hex has no chunk shorter than two digits, so an odd length
/// is refused before anything is read rather than after the last whole pair.
pub(super) fn from_hex(text: &str, limit: usize) -> Decoded {
    let units: Vec<char> = text.chars().collect();
    let mut bytes = Vec::new();
    if !units.len().is_multiple_of(2) {
        return Decoded {
            read: 0,
            bytes,
            refused: true,
        };
    }
    let mut read = 0;
    while read < units.len() && bytes.len() < limit {
        let high = units[read].to_digit(16);
        let low = units[read + 1].to_digit(16);
        // `to_digit` admits exactly the sixteen ASCII hexits in either case, which is the set
        // the specification names.
        let (Some(high), Some(low)) = (high, low) else {
            return Decoded {
                read,
                bytes,
                refused: true,
            };
        };
        #[expect(clippy::cast_possible_truncation)]
        bytes.push(((high << 4) | low) as u8);
        read += 2;
    }
    Decoded {
        read,
        bytes,
        refused: false,
    }
}

/// The base64 spelling of some bytes, padded unless the caller asked otherwise.
pub(crate) fn to_base64_text(bytes: &[u8], url: bool, omit_padding: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let packed = u32::from(group[0]) << 16
            | u32::from(group.get(1).copied().unwrap_or(0)) << 8
            | u32::from(group.get(2).copied().unwrap_or(0));
        #[expect(clippy::cast_possible_truncation)]
        let digits = [
            base64_digit((packed >> 18) as u8 & 0x3f, url),
            base64_digit((packed >> 12) as u8 & 0x3f, url),
            base64_digit((packed >> 6) as u8 & 0x3f, url),
            base64_digit(packed as u8 & 0x3f, url),
        ];
        // A group of one byte spells two digits and a group of two spells three; the rest is
        // padding, which carries no bits and which a program may ask to have left off.
        let kept = group.len() + 1;
        for digit in &digits[..kept] {
            out.push(char::from(*digit));
        }
        if !omit_padding {
            for _ in kept..4 {
                out.push('=');
            }
        }
    }
    out
}

/// The hex spelling of some bytes, in lower case as the specification writes it.
pub(crate) fn to_hex_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0xf), 16).unwrap_or('0'));
    }
    out
}

/// `value` converted the way an assignment through an element of `kind` converts it, or the
/// refusal the specification makes instead.
///
/// The two numeric towers do not mix, and this is the seam where they would: a Number written
/// into a `BigInt64Array` is a `TypeError` rather than a rounded integer, and a BigInt written
/// into a `Uint8Array` is one rather than its low byte. Easy to lose here precisely because
/// both ends of the write are bytes, and losing it turns a throw the program relies on into a
/// plausible value.
///
/// Takes a *primitive*: reducing an object to one calls the program's code, which only the
/// machine can do, so callers that can reach user code coerce first.
pub(super) fn coerce_element(kind: TypedKind, value: &Value) -> Result<Value, ExecutionError> {
    if !kind.is_big() {
        return match value {
            Value::Symbol(_) => Err(ExecutionError::SymbolIsNotANumber),
            Value::BigInt(_) => Err(ExecutionError::BigIntIsNotANumber),
            other => Ok(Value::Number(other.as_number())),
        };
    }
    // Already the right tower: kept as it is rather than rebuilt, so the common case does not
    // copy a bignum's digits.
    if matches!(value, Value::BigInt(_)) {
        return Ok(value.clone());
    }
    Ok(Value::BigInt(Rc::new(match value {
        Value::Boolean(flag) => num_bigint::BigInt::from(u8::from(*flag)),
        // A string that is not an integer literal is a `SyntaxError` rather than a NaN: this
        // type has no NaN to become, which is the same answer `BigInt("x")` gives.
        Value::String(text) => match text
            .to_scalar_string()
            .map_err(|_| ExecutionError::InvalidBigIntString)?
            .trim()
        {
            "" => num_bigint::BigInt::from(0),
            trimmed => trimmed
                .parse::<num_bigint::BigInt>()
                .map_err(|_| ExecutionError::InvalidBigIntString)?,
        },
        _ => return Err(ExecutionError::NotABigInt),
    })))
}

/// The bytes behind an `ArrayBuffer` or a `SharedArrayBuffer`, and the ceiling a resizable or
/// growable one may move within.
///
/// `max_byte_length` is the whole difference between the language's two kinds of buffer: `None`
/// is a fixed-length buffer, and `Some(max)` one created with `{ maxByteLength: max }`, whose
/// `resize` moves its length anywhere in `0..=max`. The ceiling is *not* reserved up front --
/// the bytes grow on demand -- because a program may legally declare a ceiling far larger than
/// it ever uses, and reserving eagerly would turn a declaration into an allocation.
#[derive(Debug)]
pub struct BufferData {
    /// The bytes themselves.
    ///
    /// A `RefCell` for an ordinary buffer and an `Arc<Mutex<..>>` for a shared one -- see
    /// [`Bytes`]. The distinction exists because `$262.agent.broadcast` hands a
    /// `SharedArrayBuffer`'s bytes to a *second thread*, and the value graph around them is
    /// `Rc`-based and cannot go. Every reader still spells the access `borrow`/`borrow_mut`,
    /// because reading a buffer's bytes is one operation whoever holds them.
    pub(super) bytes: Bytes,
    pub(super) max_byte_length: Option<usize>,
    /// Whether this is a `SharedArrayBuffer` rather than an `ArrayBuffer`.
    ///
    /// A flag rather than a second value type, because the bytes are the same bytes and every
    /// view reads them the same way. What the flag decides is which *constructor* claims the
    /// buffer -- and what the ceiling means: a shared buffer only ever grows, so no view over
    /// one can be stranded and no shrink can drop bytes another holder is still reading. That
    /// last part is the whole reason the language has a second buffer type, and it is exactly
    /// what a single type with a `grow` alias would lose.
    pub(super) shared: bool,
    /// Whether the bytes have been handed to somebody else and this buffer no longer owns them.
    ///
    /// A flag rather than emptying `bytes`, because "detached" and "zero-length" are different
    /// states that the language distinguishes everywhere: a zero-length buffer can still be
    /// resized and sliced, and a detached one throws. `Cell`, because detaching is something a
    /// program does to a buffer it only holds a shared reference to -- `transfer` is called on
    /// a buffer that any number of views are already pointing at, and every one of them has to
    /// see it happen.
    pub(super) detached: std::cell::Cell<bool>,
    /// Whether the bytes can never change again.
    ///
    /// Not a `Cell`: a buffer is *made* immutable and never becomes one, because the promise is
    /// only worth anything if it cannot be withdrawn -- a handle that read the flag and then
    /// found a write is exactly what the type exists to rule out. Nothing can detach one either,
    /// for the same reason: detaching is the largest write there is.
    pub(super) immutable: bool,
    /// The properties a program put on the buffer by name. The bytes are reached only through a
    /// view, so a buffer has no exotic keys either and every name lands here.
    pub(super) properties: RefCell<ObjectData>,
}

/// Identity, not contents: two buffers holding the same bytes are still two buffers, and a
/// resize through one must not be able to make the program think it holds the other.
impl PartialEq for BufferData {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl BufferData {
    /// A buffer of `length` zero bytes whose length can never change.
    pub(super) fn fixed(length: usize) -> Rc<Self> {
        Self::owned(vec![0; length])
    }

    /// A fixed buffer that adopts already-owned bytes without another allocation or copy.
    pub(super) fn owned(bytes: Vec<u8>) -> Rc<Self> {
        Rc::new(Self {
            bytes: Bytes::new(false, bytes),
            max_byte_length: None,
            shared: false,
            detached: std::cell::Cell::new(false),
            immutable: false,
            properties: RefCell::new(ObjectData::default()),
        })
    }

    /// `IsDetachedBuffer`. A shared buffer can never be one: nothing can take bytes away from a
    /// holder that has no way to learn it happened, which is the same reason it has no `resize`.
    pub(super) fn detached(&self) -> bool {
        self.detached.get()
    }

    /// The length the language reports, which is zero once the bytes are gone.
    ///
    /// Zero rather than the length the buffer had, because every accessor the specification
    /// defines on a detached buffer answers zero and every view over one reports itself empty.
    /// Reporting the old length here would make a stranded view claim to address bytes that no
    /// longer exist -- and this is the one place that would have to be remembered in a dozen.
    pub(super) fn byte_length(&self) -> usize {
        if self.detached.get() {
            return 0;
        }
        self.bytes.borrow().len()
    }

    /// `DetachArrayBuffer`: give up the bytes and hand them back.
    ///
    /// The bytes are *moved out* rather than copied, because that is what makes `transfer` a
    /// transfer: a program moves a large buffer across an ownership boundary expecting the cost
    /// to be a pointer, and a copy here would make it a memcpy of the whole block.
    pub(super) fn detach(&self) -> Vec<u8> {
        self.detached.set(true);
        std::mem::take(&mut *self.bytes.borrow_mut())
    }

    /// Move a resizable buffer's length within its ceiling.
    ///
    /// A shrink *forgets* the bytes it drops rather than hiding them: the specification says a
    /// resize hands back a fresh block whose new bytes are zero, so shrinking and growing again
    /// must not resurrect what was there before -- which is exactly the stale read a view over
    /// the old length would otherwise be able to perform.
    pub(super) fn resize(&self, length: usize) -> Result<(), ExecutionError> {
        if self.shared {
            return Err(ExecutionError::NotABuffer);
        }
        if self.immutable {
            return Err(ExecutionError::BufferImmutable);
        }
        if self.detached.get() {
            return Err(ExecutionError::BufferDetached);
        }
        let Some(max) = self.max_byte_length else {
            return Err(ExecutionError::BufferNotResizable);
        };
        if length > max {
            return Err(ExecutionError::BufferLengthOutOfRange {
                length: length as f64,
            });
        }
        self.bytes.borrow_mut().resize(length, 0);
        Ok(())
    }

    /// Move a growable shared buffer's length up within its ceiling.
    ///
    /// Only up. A shared buffer is reachable from more than one holder by construction, and a
    /// shrink would pull bytes out from under a holder that has no way to learn it happened --
    /// which is why the specification gives the shared type `grow` instead of `resize` and
    /// makes a request below the current length a `RangeError` rather than a no-op. Growing to
    /// exactly the current length is allowed and does nothing, which is what makes `grow` safe
    /// to call in a loop that is racing another holder.
    pub(super) fn grow(&self, length: usize) -> Result<(), ExecutionError> {
        if !self.shared {
            return Err(ExecutionError::NotABuffer);
        }
        let Some(max) = self.max_byte_length else {
            return Err(ExecutionError::BufferNotGrowable);
        };
        if length > max || length < self.byte_length() {
            return Err(ExecutionError::BufferLengthOutOfRange {
                length: length as f64,
            });
        }
        self.bytes.borrow_mut().resize(length, 0);
        Ok(())
    }
}

/// A typed array: a window onto a byte buffer.
#[derive(Debug)]
pub struct TypedArrayData {
    pub(super) kind: TypedKind,
    /// Shared, so two views over one buffer see each other's writes.
    pub(super) buffer: Rc<BufferData>,
    /// Where the window starts, in bytes.
    pub(super) offset: usize,
    /// How long the window is, in elements -- or `None` for a *length-tracking* view, which is
    /// what a view created without an explicit length over a resizable buffer is. Such a view
    /// has no length of its own: it reports whatever the buffer holds right now, so a `resize`
    /// through any handle is visible through it immediately instead of leaving it addressing
    /// bytes that are no longer there.
    pub(super) length: Option<usize>,
    /// The properties that are not elements.
    ///
    /// An integer-indexed exotic object answers `[[Get]]`, `[[Set]]` and the rest itself for
    /// every *canonical numeric index string*, and is an ordinary object for every other name --
    /// so `u8.constructor = C` and `u8["01"] = 1` are ordinary property writes that have to go
    /// somewhere. Which names those are is [`canonical_index_name`], not "is it an index":
    /// `"1.5"` and `"-0"` reach no element and are still element keys, so they are discarded
    /// rather than stored here.
    pub(super) properties: RefCell<ObjectData>,
}

impl PartialEq for TypedArrayData {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl TypedArrayData {
    /// The raw bytes covered by this view.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        if self.out_of_bounds() {
            return Vec::new();
        }
        let length = self.len() * self.kind.width();
        self.buffer.bytes.borrow()[self.offset..self.offset + length].to_vec()
    }

    /// Writes raw bytes into this typed array's buffer window at the given byte offset.
    pub(crate) fn write_bytes(&self, offset: usize, src: &[u8]) -> usize {
        if self.out_of_bounds() {
            return 0;
        }
        let byte_len = self.len() * self.kind.width();
        if offset >= byte_len {
            return 0;
        }
        let available = byte_len - offset;
        let to_write = available.min(src.len());
        let at = self.offset + offset;
        let mut buffer = self.buffer.bytes.borrow_mut();
        if at + to_write <= buffer.len() {
            buffer[at..at + to_write].copy_from_slice(&src[..to_write]);
            to_write
        } else {
            0
        }
    }

    /// Whether the window no longer fits in the buffer.
    ///
    /// Two ways to become true. A resizable buffer can shrink under a view that was already
    /// there; the specification does not truncate such a view to the bytes that survive -- it
    /// treats it as empty -- because the surviving prefix is not what the program asked for,
    /// and handing it back would be a stale read dressed up as a short one. And a buffer can be
    /// detached, which strands *every* view over it however it was sized: a zero-length view
    /// over a detached buffer is out of bounds too, which is why detachment is asked about
    /// directly rather than inferred from the byte length having gone to zero.
    pub(super) fn out_of_bounds(&self) -> bool {
        if self.buffer.detached() {
            return true;
        }
        let total = self.buffer.byte_length();
        match self.length {
            None => self.offset > total,
            Some(length) => self.offset > total || self.offset + length * self.kind.width() > total,
        }
    }

    /// How many elements the view currently has.
    pub(super) fn len(&self) -> usize {
        if self.out_of_bounds() {
            return 0;
        }
        match self.length {
            None => (self.buffer.byte_length() - self.offset) / self.kind.width(),
            Some(length) => length,
        }
    }

    /// The element at `index`, or `None` past the end.
    ///
    /// A `Value` rather than an `f64`, because two of the eleven element kinds hold BigInts:
    /// what a view reads is decided by its kind, not by a single numeric representation every
    /// kind has to fit into.
    pub(super) fn get(&self, index: usize) -> Option<Value> {
        if index >= self.len() {
            return None;
        }
        let width = self.kind.width();
        let at = self.offset + index * width;
        let buffer = self.buffer.bytes.borrow();
        let bytes = buffer.get(at..at + width)?;
        Some(self.kind.decode(bytes))
    }

    /// Write `value` at `index`, ignoring a write past the end as the specification does.
    ///
    /// The conversion happens *before* the bounds check, which is the specification's order and
    /// is observable: `new BigInt64Array(0)[9] = 1` throws even though nothing is written, and
    /// a `valueOf` that resizes the buffer runs even when the slot it was aimed at is gone.
    pub(super) fn set(&self, index: usize, value: &Value) -> Result<(), ExecutionError> {
        // The one place a view writes, so the one place immutability has to be checked: every
        // element assignment, `fill`, `copyWithin`, `set`, `sort` and `setFromBase64` reaches
        // the buffer through here, and a check per method would be a dozen chances to forget.
        if self.buffer.immutable {
            return Err(ExecutionError::BufferImmutable);
        }
        let value = coerce_element(self.kind, value)?;
        if index >= self.len() {
            return Ok(());
        }
        let width = self.kind.width();
        let at = self.offset + index * width;
        let mut buffer = self.buffer.bytes.borrow_mut();
        let Some(slot) = buffer.get_mut(at..at + width) else {
            return Ok(());
        };
        self.kind.encode(&value, slot);
        Ok(())
    }

    /// Every element, each in the numeric tower its kind belongs to.
    pub(super) fn items(&self) -> Vec<Value> {
        (0..self.len())
            .map(|index| self.get(index).unwrap_or(Value::Undefined))
            .collect()
    }
}

/// A `DataView`: a window onto a byte buffer whose element type is chosen at each access.
///
/// Not a [`TypedArrayData`] of some element kind, because a `DataView` *has* no element kind:
/// every read and write names its own type and its own byte order, which is the whole reason
/// the type exists. A typed array reads the platform's order and has no way to say otherwise.
#[derive(Debug)]
pub struct DataViewData {
    /// Shared, so a write through this view is visible through every other view of the buffer.
    pub(super) buffer: Rc<BufferData>,
    /// Where the window starts, in bytes.
    pub(super) offset: usize,
    /// How long the window is, in bytes -- or `None` for a *length-tracking* view, which is what
    /// a view created without an explicit length over a resizable buffer is. The same
    /// distinction [`TypedArrayData`] draws, and for the same reason.
    pub(super) length: Option<usize>,
    /// The properties a program put on the view by name. A `DataView` is an ordinary object in
    /// every respect except the buffer it wraps: it has no exotic keys at all, so every name
    /// lands here.
    pub(super) properties: RefCell<ObjectData>,
}

impl PartialEq for DataViewData {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl DataViewData {
    /// Whether a shrink -- or a detach, which strands every view at once -- has left the
    /// window outside its buffer.
    pub(super) fn out_of_bounds(&self) -> bool {
        if self.buffer.detached() {
            return true;
        }
        let total = self.buffer.byte_length();
        match self.length {
            None => self.offset > total,
            Some(length) => self.offset > total || self.offset + length > total,
        }
    }

    /// How many bytes the view spans right now, or `None` once it is out of bounds.
    ///
    /// `None` rather than zero, because the specification does not treat a stranded view as an
    /// empty one: every accessor on it is a `TypeError`. Collapsing the two would let a program
    /// carry on reading zeroes from a buffer that no longer holds what it asked for.
    pub(super) fn byte_length(&self) -> Option<usize> {
        if self.out_of_bounds() {
            return None;
        }
        Some(match self.length {
            None => self.buffer.byte_length() - self.offset,
            Some(length) => length,
        })
    }
}

/// Where in the buffer element `index` of `kind` begins, refusing anything the view cannot reach.
///
/// The two refusals are deliberately different errors: a window a resize has stranded is a
/// `TypeError`, because there is nothing to read *through*, while an index past the end of a
/// window that is still valid is the ordinary `RangeError` for asking for byte 8 of four bytes.
pub(super) fn data_view_slot(
    view: &DataViewData,
    index: usize,
    width: usize,
) -> Result<usize, ExecutionError> {
    let Some(size) = view.byte_length() else {
        return Err(ExecutionError::DataViewOutOfBounds);
    };
    match index.checked_add(width) {
        Some(end) if end <= size => Ok(view.offset + index),
        _ => Err(ExecutionError::DataViewOutOfRange),
    }
}

/// Reorder the bytes of one element for a `DataView` access.
///
/// The buffer always holds an element least-significant byte first, because that is the order a
/// typed array wrote it in; an access that did not ask for little-endian wants the other one.
/// One function, used by the read and by the write, so the two can never disagree about which
/// direction the reversal goes -- a disagreement that round-trips perfectly through the same
/// view and only shows when the bytes meet something outside the program.
pub(super) fn order_bytes(bytes: &mut [u8], little_endian: bool) {
    if !little_endian {
        bytes.reverse();
    }
}

/// The low 64 bits of a BigInt, two's complement.
///
/// `ToBigInt64` and `ToBigUint64` *wrap* rather than refuse, exactly as `ToInt32` does for the
/// narrower element types, which is what makes `setBigUint64(0, -1n)` store all ones.
pub(super) fn wrapping_u64(value: &num_bigint::BigInt) -> u64 {
    let (sign, digits) = value.to_u64_digits();
    let magnitude = digits.first().copied().unwrap_or(0);
    if matches!(sign, num_bigint::Sign::Minus) {
        magnitude.wrapping_neg()
    } else {
        magnitude
    }
}

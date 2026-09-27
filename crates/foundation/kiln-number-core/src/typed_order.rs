//! The default order of `%TypedArray%.prototype.sort` (`TypedArraySortCompare` with no
//! comparator, ECMA-262 §23.2.4.7), owned once for both engines (ADR 0060, shared semantic core).
//!
//! Numbers ascend, `-0` sorts before `+0`, and every `NaN` sorts last; integers and BigInts
//! ascend as integers. The interpreter sorts its buffers with [`sort_elements`] directly and the
//! native backend reaches the same function through the provider (`kiln_typed_array_sort`), one
//! crossing per sort; the parallel sort's default order is [`compare_default`].
//!
//! [`sort_elements`] maps each element to a `u64` key whose unsigned order is the default order,
//! sorts the keys, and writes them back. Equal keys are identical elements, so the result is the
//! one a stable sort would give -- except that every `NaN` is written back as the canonical quiet
//! `NaN`, which the specification allows (`SetValueInBuffer` picks a `NaN`'s bits).

use core::cmp::Ordering;

/// The element type of a view, in the order `kiln_typed_kind` numbers the kinds the native
/// backend has, with `Float16` (the interpreter's only) beside the other floats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementKind {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float16,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl ElementKind {
    /// The kind a native `kiln_typed_kind` names (0..=10; the native backend has no `Float16`).
    pub fn from_native(kind: i32) -> Option<Self> {
        Some(match kind {
            0 => Self::Int8,
            1 => Self::Uint8,
            2 => Self::Uint8Clamped,
            3 => Self::Int16,
            4 => Self::Uint16,
            5 => Self::Int32,
            6 => Self::Uint32,
            7 => Self::Float32,
            8 => Self::Float64,
            9 => Self::BigInt64,
            10 => Self::BigUint64,
            _ => return None,
        })
    }

    /// Bytes per element.
    pub const fn width(self) -> usize {
        match self {
            Self::Int8 | Self::Uint8 | Self::Uint8Clamped => 1,
            Self::Int16 | Self::Uint16 | Self::Float16 => 2,
            Self::Int32 | Self::Uint32 | Self::Float32 => 4,
            Self::Float64 | Self::BigInt64 | Self::BigUint64 => 8,
        }
    }

    /// The sign bit and the value mask of one element, within a `u64`.
    const fn sign_and_mask(self) -> (u64, u64) {
        match self.width() {
            1 => (0x80, 0xff),
            2 => (0x8000, 0xffff),
            4 => (0x8000_0000, 0xffff_ffff),
            _ => (1 << 63, u64::MAX),
        }
    }

    const fn class(self) -> Class {
        match self {
            Self::Int8 | Self::Int16 | Self::Int32 | Self::BigInt64 => Class::Signed,
            Self::Uint8 | Self::Uint8Clamped | Self::Uint16 | Self::Uint32 | Self::BigUint64 => {
                Class::Unsigned
            }
            Self::Float16 => Class::Float {
                infinity: 0x7c00,
                quiet_nan: 0x7e00,
            },
            Self::Float32 => Class::Float {
                infinity: 0x7f80_0000,
                quiet_nan: 0x7fc0_0000,
            },
            Self::Float64 => Class::Float {
                infinity: 0x7ff0_0000_0000_0000,
                quiet_nan: 0x7ff8_0000_0000_0000,
            },
        }
    }
}

#[derive(Clone, Copy)]
enum Class {
    Signed,
    Unsigned,
    /// IEEE 754 binary floats: the bits of `+Infinity` and of the canonical quiet `NaN`.
    Float {
        infinity: u64,
        quiet_nan: u64,
    },
}

/// The default order of two numbers: ascending, `-0` before `+0`, `NaN` after everything (and
/// equal to another `NaN`). Integer kinds widen to `f64` exactly, so this is their order too.
pub fn compare_default(x: f64, y: f64) -> Ordering {
    match (x.is_nan(), y.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ if x < y => Ordering::Less,
        _ if x > y => Ordering::Greater,
        _ if x == 0.0 && y == 0.0 => x.is_sign_positive().cmp(&y.is_sign_positive()),
        _ => Ordering::Equal,
    }
}

/// Why [`sort_elements`] did not sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortError {
    /// `bytes` is not a whole number of elements of the kind.
    RaggedBytes,
    /// `keys` holds fewer slots than `bytes` holds elements.
    ShortKeys,
}

/// Sort the elements of `kind` stored little-endian in `bytes` into the default order, in place.
///
/// `keys` is caller-owned scratch of at least one slot per element (this crate allocates
/// nothing); its contents afterwards are unspecified.
pub fn sort_elements(
    bytes: &mut [u8],
    kind: ElementKind,
    keys: &mut [u64],
) -> Result<(), SortError> {
    let width = kind.width();
    if !bytes.len().is_multiple_of(width) {
        return Err(SortError::RaggedBytes);
    }
    let count = bytes.len().checked_div(width).unwrap_or(0);
    let Some(keys) = keys.get_mut(..count) else {
        return Err(SortError::ShortKeys);
    };
    let (sign, mask) = kind.sign_and_mask();
    let class = kind.class();
    for (key, element) in keys.iter_mut().zip(bytes.chunks_exact(width)) {
        *key = to_key(read_le(element), class, sign, mask);
    }
    keys.sort_unstable();
    for (key, element) in keys.iter().zip(bytes.chunks_exact_mut(width)) {
        write_le(element, from_key(*key, class, sign, mask));
    }
    Ok(())
}

/// `raw` (the element's bits, zero-extended) as a key whose unsigned order is the default order.
fn to_key(raw: u64, class: Class, sign: u64, mask: u64) -> u64 {
    match class {
        Class::Unsigned => raw,
        Class::Signed => raw ^ sign,
        // Every `NaN` has magnitude bits above `+Infinity`'s; it takes the one key past all others.
        Class::Float { infinity, .. } if raw & !sign > infinity => u64::MAX,
        // A negative float's bits grow with its magnitude, so they are inverted; a positive one's
        // (and `+0`'s) sit above every negative one's (and `-0`'s) once the sign bit is set.
        Class::Float { .. } if raw & sign != 0 => !raw & mask,
        Class::Float { .. } => raw | sign,
    }
}

fn from_key(key: u64, class: Class, sign: u64, mask: u64) -> u64 {
    match class {
        Class::Unsigned => key,
        Class::Signed => key ^ sign,
        Class::Float { quiet_nan, .. } if key == u64::MAX => quiet_nan,
        Class::Float { .. } if key & sign != 0 => key & !sign,
        Class::Float { .. } => !key & mask,
    }
}

fn read_le(element: &[u8]) -> u64 {
    element
        .iter()
        .rev()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte))
}

fn write_le(element: &mut [u8], value: u64) {
    let mut value = value;
    for byte in element.iter_mut() {
        *byte = value.to_le_bytes()[0];
        value >>= 8;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec;
    use std::vec::Vec;

    /// Each element of `bytes`, `width` bytes little-endian, zero-extended.
    fn elements(bytes: &[u8], width: usize) -> Vec<u64> {
        bytes.chunks(width).map(read_le).collect()
    }

    fn sorted_f64(values: &[f64]) -> Vec<f64> {
        let mut bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut keys = vec![0_u64; values.len()];
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::Float64, &mut keys),
            Ok(())
        );
        elements(&bytes, 8)
            .into_iter()
            .map(f64::from_bits)
            .collect()
    }

    #[test]
    fn floats_ascend_with_negative_zero_first_and_nan_last() {
        let out = sorted_f64(&[
            3.0,
            f64::NAN,
            -0.0,
            0.0,
            -f64::INFINITY,
            -2.5,
            f64::INFINITY,
            0.0,
            -0.0,
        ]);
        let bits: Vec<u64> = out.iter().map(|v| v.to_bits()).collect();
        let expected: Vec<u64> = [
            -f64::INFINITY,
            -2.5,
            -0.0,
            -0.0,
            0.0,
            0.0,
            3.0,
            f64::INFINITY,
        ]
        .iter()
        .map(|v| v.to_bits())
        .collect();
        assert_eq!(bits.get(..8), Some(expected.as_slice()));
        assert!(out.get(8).is_some_and(|v| v.is_nan()));
    }

    #[test]
    fn the_key_order_is_compare_default() {
        let values = [
            -f64::INFINITY,
            -1e300,
            -1.0,
            -f64::MIN_POSITIVE,
            -5e-324,
            -0.0,
            0.0,
            5e-324,
            1.0,
            1e300,
            f64::INFINITY,
        ];
        let sign = 1_u64 << 63;
        let class = ElementKind::Float64.class();
        for x in values {
            for y in values {
                let keys = to_key(x.to_bits(), class, sign, u64::MAX).cmp(&to_key(
                    y.to_bits(),
                    class,
                    sign,
                    u64::MAX,
                ));
                assert_eq!(keys, compare_default(x, y), "{x} vs {y}");
            }
        }
    }

    #[test]
    fn integers_and_narrow_floats_sort_by_value() {
        let mut bytes: Vec<u8> = [5_i16, -3, 0, i16::MIN, i16::MAX, -1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut keys = vec![0_u64; 6];
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::Int16, &mut keys),
            Ok(())
        );
        let out: Vec<i16> = elements(&bytes, 2)
            .into_iter()
            .map(|v| v as u16 as i16)
            .collect();
        assert_eq!(out, [i16::MIN, -3, -1, 0, 5, i16::MAX]);

        let mut bytes: Vec<u8> = [2.0_f32, f32::NAN, -0.0, -1.0, 0.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut keys = vec![0_u64; 5];
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::Float32, &mut keys),
            Ok(())
        );
        let out = elements(&bytes, 4);
        let expected = [
            (-1.0_f32).to_bits(),
            (-0.0_f32).to_bits(),
            0,
            2.0_f32.to_bits(),
            0x7fc0_0000,
        ];
        assert_eq!(out, expected.map(u64::from));

        // Float16: -2, -0, +0, 1, NaN.
        let mut bytes: Vec<u8> = [0x3c00_u16, 0x7c01, 0x0000, 0x8000, 0xc000]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut keys = vec![0_u64; 5];
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::Float16, &mut keys),
            Ok(())
        );
        assert_eq!(
            elements(&bytes, 2),
            [0xc000, 0x8000, 0x0000, 0x3c00, 0x7e00]
        );

        let mut bytes: Vec<u8> = [u64::MAX, 0, 7]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut keys = vec![0_u64; 3];
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::BigUint64, &mut keys),
            Ok(())
        );
        assert_eq!(bytes.get(..8), Some(0_u64.to_le_bytes().as_slice()));
        let mut bytes: Vec<u8> = [i64::MAX, i64::MIN, -1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::BigInt64, &mut keys),
            Ok(())
        );
        assert_eq!(bytes.get(..8), Some(i64::MIN.to_le_bytes().as_slice()));
    }

    #[test]
    fn short_scratch_and_ragged_bytes_sort_nothing() {
        let mut bytes = [1_u8, 0, 0];
        let mut keys = [0_u64; 1];
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::Uint16, &mut keys),
            Err(SortError::RaggedBytes)
        );
        let mut bytes = [3_u8, 1];
        assert_eq!(
            sort_elements(&mut bytes, ElementKind::Uint8, &mut keys),
            Err(SortError::ShortKeys)
        );
        assert_eq!(bytes, [3, 1]);
    }
}

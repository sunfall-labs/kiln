use super::INVALID_ARGUMENT;
use alloc::boxed::Box;
use num_bigint::BigInt;
use num_traits::{FromPrimitive, Pow, Signed, ToPrimitive, Zero};

/// An operation the language defines but this cannot perform: division by zero, or an
/// exponent no machine could represent. The caller turns it into a RangeError.
const OUT_OF_RANGE: i32 = -2;

/// The binary operators, numbered. Stable only within one generated unit, like every other
/// operation code in the emitted runtime.
const ADD: u32 = 1;
const SUBTRACT: u32 = 2;
const MULTIPLY: u32 = 3;
const DIVIDE: u32 = 4;
const REMAINDER: u32 = 5;
const EXPONENTIATE: u32 = 6;
const BITWISE_AND: u32 = 7;
const BITWISE_OR: u32 = 8;
const BITWISE_XOR: u32 = 9;
const SHIFT_LEFT: u32 = 10;
const SHIFT_RIGHT: u32 = 11;

/// How far `<<` may widen a BigInt, in bits.
///
/// The language puts no bound on the type -- an implementation is expected to run out of
/// memory eventually -- but "out of memory" is not an outcome a program can catch, and
/// `1n << (2n ** 64n)` is a count no allocation could ever satisfy. The caller turns this
/// refusal into the `RangeError` every engine raises there.
const MAX_SHIFT: u64 = 1 << 24;

/// Parse a digit string in `radix`, with an optional leading sign.
///
/// # Safety
///
/// `text` must name a readable span of `len` bytes when `len` is nonzero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_parse(
    text: *const u8,
    len: usize,
    radix: u32,
    out: *mut *mut BigInt,
) -> i32 {
    if out.is_null() || !(2..=36).contains(&radix) {
        return INVALID_ARGUMENT;
    }
    if len == 0 {
        return INVALID_ARGUMENT;
    }
    if text.is_null() || len > isize::MAX as usize {
        return INVALID_ARGUMENT;
    }
    // SAFETY: checked above and guaranteed by the caller contract.
    let bytes = unsafe { core::slice::from_raw_parts(text, len) };
    let Ok(digits) = core::str::from_utf8(bytes) else {
        return INVALID_ARGUMENT;
    };
    let Some(parsed) = BigInt::parse_bytes(digits.trim().as_bytes(), radix) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: `out` is a writable slot by the caller contract.
    unsafe { out.write(Box::into_raw(Box::new(parsed))) };
    0
}

/// `BigInt(n)`, which requires an integral finite number: rounding here would be the one thing
/// a BigInt exists to prevent.
///
/// # Safety
///
/// `out` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_from_double(value: f64, out: *mut *mut BigInt) -> i32 {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // `trunc` rather than `fract`: `num_traits` brings its own `fract` into scope here and
    // the two disagree about nothing except which one the compiler picks.
    if !value.is_finite() || value != libm_trunc(value) {
        return OUT_OF_RANGE;
    }
    let Some(converted) = BigInt::from_f64(value) else {
        return OUT_OF_RANGE;
    };
    // SAFETY: `out` is a writable slot by the caller contract.
    unsafe { out.write(Box::into_raw(Box::new(converted))) };
    0
}

/// # Safety
///
/// `handle` must have come from one of the constructors above and must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_free(handle: *mut BigInt) {
    if handle.is_null() {
        return;
    }
    // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw`.
    drop(unsafe { Box::from_raw(handle) });
}

/// The nearest double, which is what a comparison against a number needs -- and is lossy by
/// definition, which is why nothing else uses it.
///
/// # Safety
///
/// `handle` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_to_double(handle: *const BigInt) -> f64 {
    if handle.is_null() {
        return f64::NAN;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    unsafe { &*handle }.to_f64().unwrap_or(f64::NAN)
}

/// The exact value as a 128-bit integer, split into a signed top half and an unsigned bottom
/// half because C99 has no portable one -- the same pair `kiln_temporal_now` takes.
///
/// Exact or nothing: [`OUT_OF_RANGE`] when the value does not fit, never a saturated or
/// truncated one. This is what `Temporal.Instant.fromEpochNanoseconds` needs, and an instant
/// off by 2^128 nanoseconds would be a wrong answer that prints rather than a caught error.
///
/// # Safety
///
/// `handle` must be live and both out-parameters writable slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_to_i128(
    handle: *const BigInt,
    out_high: *mut i64,
    out_low: *mut u64,
) -> i32 {
    if handle.is_null() || out_high.is_null() || out_low.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    let Some(value) = unsafe { &*handle }.to_i128() else {
        return OUT_OF_RANGE;
    };
    // SAFETY: both out-parameters are writable slots by the caller contract.
    unsafe {
        out_high.write((value >> 64) as i64);
        out_low.write(value as u64);
    }
    0
}

/// Whether the value is zero, which is the whole of a BigInt's truthiness.
///
/// # Safety
///
/// `handle` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_is_zero(handle: *const BigInt) -> i32 {
    if handle.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    i32::from(unsafe { &*handle }.is_zero())
}

/// `-1`, `0` or `1`, or `-2` when either handle is null.
///
/// # Safety
///
/// Both handles must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_compare(left: *const BigInt, right: *const BigInt) -> i32 {
    if left.is_null() || right.is_null() {
        return OUT_OF_RANGE;
    }
    // SAFETY: the caller contract guarantees both handles are live.
    let (left, right) = unsafe { (&*left, &*right) };
    match left.cmp(right) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// The value written in `radix`, as UTF-8 without a terminator, or the required length as a
/// negative number when the buffer is too small.
///
/// # Safety
///
/// `handle` must be live and `buffer` must be writable for `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_to_string(
    handle: *const BigInt,
    radix: u32,
    buffer: *mut u8,
    capacity: usize,
) -> i32 {
    if handle.is_null() || !(2..=36).contains(&radix) {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    let text = unsafe { &*handle }.to_str_radix(radix);
    let bytes = text.as_bytes();
    let Ok(length) = i32::try_from(bytes.len()) else {
        return INVALID_ARGUMENT;
    };
    if bytes.len() > capacity || buffer.is_null() {
        // Negative, so the caller can size a buffer and ask again rather than guessing.
        return length.saturating_neg();
    }
    // SAFETY: the length is within the capacity the caller promised.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
    length
}

/// One binary operation.
///
/// # Safety
///
/// Both handles must be live and `out` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_binary(
    operation: u32,
    left: *const BigInt,
    right: *const BigInt,
    out: *mut *mut BigInt,
) -> i32 {
    if left.is_null() || right.is_null() || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees both handles are live.
    let (left, right) = unsafe { (&*left, &*right) };
    let produced = match operation {
        ADD => left + right,
        SUBTRACT => left - right,
        MULTIPLY => left * right,
        // Division truncates toward zero and by zero is a RangeError, neither of which
        // matches what a number would do.
        DIVIDE | REMAINDER => {
            if right.is_zero() {
                return OUT_OF_RANGE;
            }
            if operation == DIVIDE {
                left / right
            } else {
                left % right
            }
        }
        EXPONENTIATE => {
            if right.is_negative() {
                return OUT_OF_RANGE;
            }
            let Some(exponent) = right.to_u32() else {
                return OUT_OF_RANGE;
            };
            Pow::pow(left.clone(), exponent)
        }
        BITWISE_AND => left & right,
        BITWISE_OR => left | right,
        BITWISE_XOR => left ^ right,
        // One operation with a sign rather than two: a negative count shifts the other way,
        // so `1n << -1n` is `1n >> 1n`. A widening shift past the bound is refused before
        // anything is allocated; a narrowing one is clamped to the value's own width, since
        // shifting further only converges on 0 or -1.
        SHIFT_LEFT | SHIFT_RIGHT => {
            let negative = right.is_negative();
            let widening = (operation == SHIFT_LEFT) != negative;
            let count = if negative {
                -right.clone()
            } else {
                right.clone()
            };
            match count.to_u64() {
                Some(count) if widening => {
                    if count > MAX_SHIFT {
                        return OUT_OF_RANGE;
                    }
                    left.clone() << usize::try_from(count).unwrap_or(usize::MAX)
                }
                Some(count) => {
                    let clamped = count.min(left.bits().saturating_add(1));
                    left.clone() >> usize::try_from(clamped).unwrap_or(usize::MAX)
                }
                None if widening => return OUT_OF_RANGE,
                None => left.clone() >> usize::MAX,
            }
        }
        _ => return INVALID_ARGUMENT,
    };
    // SAFETY: `out` is a writable slot by the caller contract.
    unsafe { out.write(Box::into_raw(Box::new(produced))) };
    0
}

/// `trunc` without pulling in a math library: a double's integral part.
fn libm_trunc(value: f64) -> f64 {
    // `as i64` would saturate for anything large, which is exactly the case that matters.
    if value.abs() >= 9_007_199_254_740_992.0 {
        return value;
    }
    #[expect(clippy::cast_possible_truncation)]
    let whole = value as i64;
    #[expect(clippy::cast_precision_loss)]
    let whole = whole as f64;
    whole
}

/// `-x`, which is the only unary operation a BigInt has that is not a comparison.
///
/// # Safety
///
/// `handle` must be live and `out` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_bigint_negate(handle: *const BigInt, out: *mut *mut BigInt) -> i32 {
    if handle.is_null() || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    let negated = -(unsafe { &*handle }).clone();
    // SAFETY: `out` is a writable slot by the caller contract.
    unsafe { out.write(Box::into_raw(Box::new(negated))) };
    0
}

macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (bigint, kiln_bigint_parse, bigint_parse) => {
            const _: unsafe extern "C" fn(*const u8, usize, u32, *mut *mut BigInt) -> i32 = kiln_bigint_parse;
        };
        (bigint, kiln_bigint_from_double, bigint_from_double) => {
            const _: unsafe extern "C" fn(f64, *mut *mut BigInt) -> i32 = kiln_bigint_from_double;
        };
        (bigint, kiln_bigint_free, bigint_free) => {
            const _: unsafe extern "C" fn(*mut BigInt) = kiln_bigint_free;
        };
        (bigint, kiln_bigint_to_double, bigint_to_double) => {
            const _: unsafe extern "C" fn(*const BigInt) -> f64 = kiln_bigint_to_double;
        };
        (bigint, kiln_bigint_to_string, bigint_to_string) => {
            const _: unsafe extern "C" fn(*const BigInt, u32, *mut u8, usize) -> i32 = kiln_bigint_to_string;
        };
        (bigint, kiln_bigint_binary, bigint_binary) => {
            const _: unsafe extern "C" fn(u32, *const BigInt, *const BigInt, *mut *mut BigInt) -> i32 = kiln_bigint_binary;
        };
        (bigint, kiln_bigint_negate, bigint_negate) => {
            const _: unsafe extern "C" fn(*const BigInt, *mut *mut BigInt) -> i32 = kiln_bigint_negate;
        };
        (bigint, kiln_bigint_compare, bigint_compare) => {
            const _: unsafe extern "C" fn(*const BigInt, *const BigInt) -> i32 = kiln_bigint_compare;
        };
        (bigint, kiln_bigint_is_zero, bigint_is_zero) => {
            const _: unsafe extern "C" fn(*const BigInt) -> i32 = kiln_bigint_is_zero;
        };
        (bigint, kiln_bigint_to_i128, bigint_to_i128) => {
            const _: unsafe extern "C" fn(*const BigInt, *mut i64, *mut u64) -> i32 = kiln_bigint_to_i128;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

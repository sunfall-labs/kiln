//! Private C ABI adapter for the semantics a generated artifact cannot compute for itself.
//!
//! Three of those so far: ECMAScript number conversion, whose shortest spelling no printf
//! conversion can express; regular expressions, which are an engine rather than a function; and
//! arbitrary-precision integers, which are exactly specified and still no easier to get right
//! twice (ADR 0025). All three are here for the same reason -- one implementation, shared with the
//! interpreter, rather than two that agree until they do not.
//!
//! The exported names are link-time implementation details of generated artifacts, not additions
//! to the public embedding ABI.
//!
//! The crate is still called `kiln-native-number-ffi`, which is now half a name. Renaming it moves
//! a packaging contract -- the manifest field, the archive filename, the environment variable --
//! and that is `kiln-native`'s to move; see NOTES.md.
#![no_std]
#![deny(
    unsafe_op_in_unsafe_fn,
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

#[cfg(any(feature = "regexp", feature = "bigint"))]
extern crate alloc;

#[cfg(not(test))]
use core::panic::PanicInfo;

const INVALID_ARGUMENT: i32 = -1;

/// The allocator the regexp engine runs on.
///
/// `malloc` and `free`, because the generated artifact this is linked into already has them and a
/// second allocator would be a second heap in one process. Alignment above `max_align_t` goes
/// through `posix_memalign`, which is the only part `malloc` cannot promise.
#[cfg(all(not(test), any(feature = "regexp", feature = "bigint")))]
mod heap {
    use core::alloc::{GlobalAlloc, Layout};

    unsafe extern "C" {
        fn malloc(size: usize) -> *mut core::ffi::c_void;
        fn free(pointer: *mut core::ffi::c_void);
        fn posix_memalign(
            out: *mut *mut core::ffi::c_void,
            alignment: usize,
            size: usize,
        ) -> core::ffi::c_int;
    }

    /// What `malloc` already guarantees on every target this compiles for.
    const MALLOC_ALIGNMENT: usize = 16;

    struct CHeap;

    // SAFETY: every allocation is returned by `malloc` or `posix_memalign` and freed by `free`,
    // which is the contract both halves of libc's allocator keep.
    unsafe impl GlobalAlloc for CHeap {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if layout.align() <= MALLOC_ALIGNMENT {
                // SAFETY: a nonzero size is guaranteed by the `Layout` contract.
                return unsafe { malloc(layout.size()) }.cast::<u8>();
            }
            let mut out = core::ptr::null_mut();
            // SAFETY: `out` is a valid slot for the returned pointer.
            let status = unsafe { posix_memalign(&raw mut out, layout.align(), layout.size()) };
            if status == 0 {
                out.cast::<u8>()
            } else {
                core::ptr::null_mut()
            }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
            // SAFETY: the pointer came from `alloc` above and is freed exactly once.
            unsafe { free(pointer.cast::<core::ffi::c_void>()) };
        }
    }

    #[global_allocator]
    static HEAP: CHeap = CHeap;
}

/// Format `value` using ECMAScript `Number::toString` spelling.
///
/// On success the function returns the byte length excluding a trailing NUL and writes that NUL.
/// A null output, zero capacity, or a buffer without room for both the bytes and terminator returns
/// `-1` without modifying the output.
///
/// # Safety
///
/// When `buffer` is non-null, it must name storage valid for writes of `capacity` bytes for the
/// duration of this call. The storage must not overlap memory used by this function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_number_to_js_string(
    value: f64,
    buffer: *mut u8,
    capacity: usize,
) -> i32 {
    if buffer.is_null() || capacity == 0 {
        return INVALID_ARGUMENT;
    }

    let mut formatted = [0_u8; kiln_number_core::MAX_FORMATTED_LEN];
    let Ok(length) = kiln_number_core::format_number(value, &mut formatted) else {
        return INVALID_ARGUMENT;
    };
    let Some(required) = length.checked_add(1) else {
        return INVALID_ARGUMENT;
    };
    if capacity < required {
        return INVALID_ARGUMENT;
    }
    let Some(bytes) = formatted.get(..length) else {
        return INVALID_ARGUMENT;
    };
    let Ok(result) = i32::try_from(length) else {
        return INVALID_ARGUMENT;
    };

    // SAFETY: the caller contract guarantees `capacity` writable bytes, the check above proves
    // `length + 1 <= capacity`, and `formatted` is disjoint stack storage.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, length);
        buffer.add(length).write(0);
    }
    result
}

/// Parse an ECMAScript `StringNumericLiteral` from a byte span.
///
/// `(NULL, 0)` is the empty string and produces positive zero. A null pointer with a nonzero length,
/// a span too large for Rust's slice contract, invalid UTF-8, or malformed input produces `NaN`.
///
/// # Safety
///
/// When `bytes` is non-null and `length` is nonzero, it must name readable storage of exactly
/// `length` bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_string_to_number(bytes: *const u8, length: usize) -> f64 {
    if length == 0 {
        return 0.0;
    }
    if bytes.is_null() || length > isize::MAX as usize {
        return f64::NAN;
    }

    // SAFETY: the caller contract supplies `length` readable bytes, null was rejected, `u8` has
    // alignment one, and the explicit bound satisfies `slice::from_raw_parts`' size requirement.
    let input = unsafe { core::slice::from_raw_parts(bytes, length) };
    kiln_number_core::string_to_number(input)
}

#[cfg(feature = "regexp")]
mod regexp {
    use super::INVALID_ARGUMENT;

    /// A compiled pattern, and the last match it produced.
    ///
    /// The match is kept because a capture group's *name* is only reachable through one -- `regress`
    /// clones the names into each match rather than exposing them on the pattern. Keeping it also
    /// means the C side reads a result with two calls instead of marshalling a variable-length
    /// structure across the boundary.
    pub struct KilnRegex {
        compiled: regress::Regex,
        last: Option<regress::Match>,
    }

    /// Compile `pattern` with `flags`.
    ///
    /// Returns 0 and writes the handle on success, and -1 for a pattern the engine rejects -- which
    /// the caller turns into a SyntaxError, because that is what `new RegExp` with the same text
    /// produces.
    ///
    /// # Safety
    ///
    /// `pattern` and `flags` must name readable UTF-8 spans of the given lengths, and `out_handle`
    /// must be a writable slot. The handle belongs to the caller and is released by
    /// [`kiln_regexp_free`].
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_compile(
        pattern: *const u8,
        pattern_len: usize,
        flags: *const u8,
        flags_len: usize,
        out_handle: *mut *mut KilnRegex,
    ) -> i32 {
        if out_handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees both spans are readable for their lengths; a null
        // pointer with a zero length is the empty span, which `from_raw_parts` requires be dangling
        // rather than null.
        let (pattern, flags) = unsafe { (span(pattern, pattern_len), span(flags, flags_len)) };
        let (Some(pattern), Some(flags)) = (pattern, flags) else {
            return INVALID_ARGUMENT;
        };
        let (Ok(pattern), Ok(flags)) = (core::str::from_utf8(pattern), core::str::from_utf8(flags))
        else {
            return INVALID_ARGUMENT;
        };
        let Ok(compiled) = regress::Regex::with_flags(pattern, flags) else {
            return INVALID_ARGUMENT;
        };
        let handle = alloc::boxed::Box::new(KilnRegex {
            compiled,
            last: None,
        });
        // SAFETY: `out_handle` is a writable slot by the caller contract.
        unsafe { out_handle.write(alloc::boxed::Box::into_raw(handle)) };
        0
    }

    /// Release a handle from [`kiln_regexp_compile`].
    ///
    /// # Safety
    ///
    /// `handle` must have come from [`kiln_regexp_compile`] and must not be used again.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_free(handle: *mut KilnRegex) {
        if handle.is_null() {
            return;
        }
        // SAFETY: the caller contract guarantees the pointer came from `Box::into_raw` above.
        drop(unsafe { alloc::boxed::Box::from_raw(handle) });
    }

    /// Search `subject` from `start`, both counted in UTF-16 code units.
    ///
    /// Returns 1 for a match, 0 for none, and -1 for an invalid argument. On a match, up to
    /// `out_capacity` pairs of `(start, end)` offsets are written to `out_ranges`, one pair per group
    /// beginning with the whole match; a group that did not participate writes `(-1, -1)`.
    ///
    /// The offsets are UTF-16 code units because that is what the language counts in (ADR 0022), and
    /// matching over UTF-16 is what makes them so without a conversion.
    ///
    /// # Safety
    ///
    /// `handle` must be live, `subject` must name a readable span of `subject_len` units, and
    /// `out_ranges` must be writable for `out_capacity * 2` values.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_search(
        handle: *mut KilnRegex,
        subject: *const u16,
        subject_len: usize,
        start: usize,
        out_ranges: *mut i64,
        out_capacity: usize,
    ) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live and uniquely borrowed here.
        let state = unsafe { &mut *handle };
        if start > subject_len {
            state.last = None;
            return 0;
        }
        // SAFETY: the caller contract guarantees the span is readable for its length.
        let Some(units) = (unsafe { units(subject, subject_len) }) else {
            return INVALID_ARGUMENT;
        };
        let Some(found) = state.compiled.find_from_utf16(units, start).next() else {
            state.last = None;
            return 0;
        };
        let mut written = 0usize;
        for group in found.groups() {
            if written >= out_capacity || out_ranges.is_null() {
                break;
            }
            let (from, to) = match group {
                Some(range) => (as_offset(range.start), as_offset(range.end)),
                // Not zero: a group that did not participate is `undefined`, which is a different
                // answer from one that matched nothing.
                None => (-1, -1),
            };
            // SAFETY: `written` is below `out_capacity`, and the caller promised twice that many
            // writable values.
            unsafe {
                out_ranges.add(written.wrapping_mul(2)).write(from);
                out_ranges
                    .add(written.wrapping_mul(2).wrapping_add(1))
                    .write(to);
            }
            written = written.wrapping_add(1);
        }
        state.last = Some(found);
        1
    }

    /// How many groups the last match reported, including the whole match as group zero.
    ///
    /// # Safety
    ///
    /// `handle` must be live.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_group_count(handle: *const KilnRegex) -> usize {
        if handle.is_null() {
            return 0;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let state = unsafe { &*handle };
        state
            .last
            .as_ref()
            .map_or(0, |found| found.groups().count())
    }

    /// The name of capture group `index` from the last match, as UTF-8 without a terminator.
    ///
    /// Returns the byte length written, 0 for a group with no name, and -1 when there is no last match
    /// or the buffer is too small.
    ///
    /// # Safety
    ///
    /// `handle` must be live and `buffer` must be writable for `capacity` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn kiln_regexp_group_name(
        handle: *const KilnRegex,
        index: usize,
        buffer: *mut u8,
        capacity: usize,
    ) -> i32 {
        if handle.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the caller contract guarantees the handle is live.
        let state = unsafe { &*handle };
        let Some(found) = state.last.as_ref() else {
            return INVALID_ARGUMENT;
        };
        // `named_groups` counts from the first capture group, where the caller counts from the whole
        // match -- so group zero has no name by construction and the rest are shifted by one.
        let Some(wanted) = index.checked_sub(1) else {
            return 0;
        };
        let Some((name, _)) = found.named_groups().nth(wanted) else {
            return 0;
        };
        let bytes = name.as_bytes();
        let Ok(length) = i32::try_from(bytes.len()) else {
            return INVALID_ARGUMENT;
        };
        if bytes.len() > capacity || buffer.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: the length is within the capacity the caller promised, and `name` is disjoint from
        // the caller's buffer.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        length
    }

    /// A byte span, or `None` when the pointer contract is violated.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` bytes.
    unsafe fn span<'a>(pointer: *const u8, length: usize) -> Option<&'a [u8]> {
        if length == 0 {
            return Some(&[]);
        }
        if pointer.is_null() || length > isize::MAX as usize {
            return None;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }

    /// The same, for the UTF-16 subject.
    ///
    /// # Safety
    ///
    /// A non-null `pointer` must be readable for `length` units.
    unsafe fn units<'a>(pointer: *const u16, length: usize) -> Option<&'a [u16]> {
        if length == 0 {
            return Some(&[]);
        }
        if pointer.is_null() || length > isize::MAX as usize / 2 {
            return None;
        }
        // SAFETY: checked above and guaranteed by the caller contract.
        Some(unsafe { core::slice::from_raw_parts(pointer, length) })
    }

    /// An offset that fits the boundary type, saturating rather than wrapping.
    fn as_offset(value: usize) -> i64 {
        i64::try_from(value).unwrap_or(i64::MAX)
    }
}

/// Arbitrary-precision integers, exactly as the interpreter holds them.
///
/// A BigInt exists to be exact, so the one thing this must never do is lose a digit. Handing the
/// work to `num-bigint` -- the crate the interpreter already links -- is what makes "the two
/// engines agree" a property of the build rather than of the test suite.
//
// The crate denies `arithmetic_side_effects` because a fixed-width overflow is a silent wrong
// answer. Arbitrary-precision arithmetic has no such failure -- it grows instead -- and the two
// operations that *can* fail, division by zero and an exponent no machine can hold, are checked
// before they run.
#[expect(clippy::arithmetic_side_effects)]
#[cfg(feature = "bigint")]
mod bigint {
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
}

// A no-std static library must define a panic personality. All reachable core and adapter paths
// are written and linted not to panic; spinning is an unreachable backstop that avoids unwinding
// across C or terminating an embedding process by policy.
#[cfg(not(test))]
#[panic_handler]
fn panic(_information: &PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// Precompiled `core` retains an unreachable personality reference even when this final archive is
// built with `panic=abort`. Supplying the symbol keeps the C link self-contained; no reachable path
// unwinds through it, and the provider's exported-symbol policy will keep it private in a DSO.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub extern "C" fn rust_eh_personality() {}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    #[test]
    fn formatter_translates_capacity_and_nul_contracts_without_partial_writes() {
        let mut exact = [b'X'; 6];
        // SAFETY: `exact` is writable for its full reported capacity.
        let length = unsafe { kiln_number_to_js_string(1.234, exact.as_mut_ptr(), exact.len()) };
        assert_eq!(length, 5);
        assert_eq!(&exact, b"1.234\0");

        let mut short = [b'X'; 5];
        // SAFETY: `short` is writable for its full reported capacity.
        let length = unsafe { kiln_number_to_js_string(1.234, short.as_mut_ptr(), short.len()) };
        assert_eq!(length, INVALID_ARGUMENT);
        assert_eq!(&short, b"XXXXX");

        // SAFETY: a null pointer is explicitly accepted as an invalid argument and never read.
        assert_eq!(
            unsafe { kiln_number_to_js_string(1.0, core::ptr::null_mut(), 0) },
            INVALID_ARGUMENT
        );
    }

    #[test]
    fn parser_maps_null_empty_and_bounded_bytes_to_the_existing_contract() {
        // SAFETY: `(NULL, 0)` is explicitly the empty span.
        let empty = unsafe { kiln_string_to_number(core::ptr::null(), 0) };
        assert_eq!(empty.to_bits(), 0.0_f64.to_bits());

        // SAFETY: null with nonzero length is rejected before it can be read.
        assert!(unsafe { kiln_string_to_number(core::ptr::null(), 1) }.is_nan());

        let text = b"0x20000000000003";
        // SAFETY: `text` is readable for its reported length.
        let parsed = unsafe { kiln_string_to_number(text.as_ptr(), text.len()) };
        assert_eq!(parsed, 9_007_199_254_740_996.0);

        let malformed = [0xc0, 0xaf];
        // SAFETY: `malformed` is readable for its reported length.
        assert!(unsafe { kiln_string_to_number(malformed.as_ptr(), malformed.len()) }.is_nan());
    }
}

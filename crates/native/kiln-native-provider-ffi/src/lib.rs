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
//! The crate is called `kiln-native-provider-ffi` (renamed from `kiln-native-number-ffi` per ADR 0025).
//! It supplies the private Rust runtime engines and HostOps capabilities linked into native executables.
#![deny(
    unsafe_op_in_unsafe_fn,
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

#[cfg(any(feature = "regexp", feature = "bigint", feature = "json"))]
extern crate alloc;

#[cfg(any(feature = "realm", feature = "regexp", feature = "json"))]
use kiln_string::StringView;

const INVALID_ARGUMENT: i32 = -1;
#[cfg(feature = "realm")]
const RUNTIME_ERROR: i32 = -8;

/// Canonical borrowed JavaScript string view used by the native provider seams.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct KilnString {
    pub unit_len: u32,
    pub flags: u32,
    pub data: *const core::ffi::c_void,
}

const _: () = {
    assert!(core::mem::size_of::<KilnString>() == 16);
    assert!(core::mem::offset_of!(KilnString, unit_len) == 0);
    assert!(core::mem::offset_of!(KilnString, flags) == 4);
    assert!(core::mem::offset_of!(KilnString, data) == 8);
};

/// Validate a string view once at a coarse provider boundary.
#[cfg(any(feature = "realm", feature = "regexp", feature = "json"))]
pub(crate) unsafe fn validate_string<'a>(view: *const KilnString) -> Option<StringView<'a>> {
    if view.is_null() {
        return None;
    }
    // SAFETY: null was rejected; the caller owns the view for this call.
    let view = unsafe { &*view };
    let len = view.unit_len as usize;
    match view.flags {
        0 => {
            if len == 0 {
                return Some(StringView::Latin1(&[]));
            }
            if view.data.is_null() || len > isize::MAX as usize {
                return None;
            }
            // SAFETY: validated by the private C ABI contract.
            Some(StringView::Latin1(unsafe {
                core::slice::from_raw_parts(view.data.cast(), len)
            }))
        }
        1 => {
            if len == 0 {
                return Some(StringView::Utf16(&[]));
            }
            if view.data.is_null() || (view.data as usize & 1) != 0 || len > isize::MAX as usize / 2
            {
                return None;
            }
            // SAFETY: validated by the private C ABI contract.
            Some(StringView::Utf16(unsafe {
                core::slice::from_raw_parts(view.data.cast(), len)
            }))
        }
        _ => None,
    }
}

#[cfg(all(test, any(feature = "realm", feature = "regexp", feature = "json")))]
mod string_view_tests {
    use super::{KilnString, StringView, validate_string};
    use core::ffi::c_void;

    #[test]
    fn validates_flags_nulls_and_utf16_alignment() {
        let bytes = [b'x'];
        let unknown_flags = KilnString {
            unit_len: 1,
            flags: 2,
            data: bytes.as_ptr().cast(),
        };
        assert!(unsafe { validate_string(&unknown_flags) }.is_none());

        let null_nonempty = KilnString {
            unit_len: 1,
            flags: 0,
            data: core::ptr::null(),
        };
        assert!(unsafe { validate_string(&null_nonempty) }.is_none());

        let units = [0x61_u16];
        let misaligned = KilnString {
            unit_len: 1,
            flags: 1,
            data: units.as_ptr().cast::<u8>().wrapping_add(1).cast::<c_void>(),
        };
        assert!(unsafe { validate_string(&misaligned) }.is_none());

        let empty_latin1 = KilnString {
            unit_len: 0,
            flags: 0,
            data: core::ptr::null(),
        };
        assert_eq!(
            unsafe { validate_string(&empty_latin1) },
            Some(StringView::Latin1(&[]))
        );
        let empty_utf16 = KilnString {
            unit_len: 0,
            flags: 1,
            data: core::ptr::null(),
        };
        assert_eq!(
            unsafe { validate_string(&empty_utf16) },
            Some(StringView::Utf16(&[]))
        );
    }
}

#[cfg(feature = "ffi")]
mod ffi;

/// The private native Realm ABI.
///
/// Handles are process-unique integers rather than Rust pointers. The owning thread keeps the
/// actual runtime in thread-local storage, so a stale or cross-thread handle is rejected without
/// dereferencing caller-controlled memory. No Tokio or Rust layout crosses the C seam.
#[cfg(feature = "realm")]
#[path = "provider/realm.rs"]
mod realm;

/// Cross-realm ports, `node:worker_threads` and shared memory for native realms (realms issue 06).
#[cfg(feature = "realm")]
#[path = "provider/workers.rs"]
mod workers;

/// The allocator the regexp engine runs on.
///
/// `malloc` and `free`, because the generated artifact this is linked into already has them and a
/// second allocator would be a second heap in one process. Alignment above `max_align_t` goes
/// through `posix_memalign`, which is the only part `malloc` cannot promise.
#[cfg(all(not(test), any(feature = "regexp", feature = "bigint")))]
#[path = "provider/heap.rs"]
mod heap;

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

/// Sort the `count` elements of `kiln_typed_kind` `kind` at `data` into `%TypedArray%.prototype.sort`'s
/// default order, in place, on the calling thread: the shared order of
/// `kiln_number_core::typed_order`, which the interpreter runs directly. One crossing per sort
/// (ADR 0060); the sequential `sort` reaches it through the `__kilnTypedArraySort` primitive.
///
/// Returns 0 when sorted and `-1`, with nothing written, for a null `data`, an unknown kind, or
/// scratch the allocator could not provide.
///
/// # Safety
///
/// When `count` is nonzero, `data` must name `count` elements of the kind, valid for reads and
/// writes for the duration of the call, that nothing else touches during it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_typed_array_sort(data: *mut u8, count: usize, kind: i32) -> i32 {
    use kiln_number_core::typed_order::{ElementKind, sort_elements};
    let Some(kind) = ElementKind::from_native(kind) else {
        return INVALID_ARGUMENT;
    };
    if count < 2 {
        return 0;
    }
    let Some(length) = count.checked_mul(kind.width()) else {
        return INVALID_ARGUMENT;
    };
    if data.is_null() || length > isize::MAX as usize {
        return INVALID_ARGUMENT;
    }
    let mut keys = Vec::new();
    if keys.try_reserve_exact(count).is_err() {
        return INVALID_ARGUMENT;
    }
    keys.resize(count, 0_u64);
    // SAFETY: the caller's contract supplies `length` readable and writable bytes that nothing
    // else touches during the call; null and the slice size bound were rejected above.
    let bytes = unsafe { core::slice::from_raw_parts_mut(data, length) };
    match sort_elements(bytes, kind, &mut keys) {
        Ok(()) => 0,
        Err(_) => INVALID_ARGUMENT,
    }
}

macro_rules! assert_root_provider_export {
    ($(($group:ident, $name:ident, $signature:ident);)*) => {
        $(assert_root_provider_export!($group, $name, $signature);)*
    };
    (root, kiln_number_to_js_string, number_to_js_string) => {
        const _: unsafe extern "C" fn(f64, *mut u8, usize) -> i32 = kiln_number_to_js_string;
    };
    (root, kiln_string_to_number, string_to_number) => {
        const _: unsafe extern "C" fn(*const u8, usize) -> f64 = kiln_string_to_number;
    };
    (date, kiln_date_parse, date_parse) => {
        const _: unsafe extern "C" fn(*const KilnString) -> f64 = kiln_date_parse;
    };
    (root, kiln_typed_array_sort, typed_array_sort) => {
        const _: unsafe extern "C" fn(*mut u8, usize, i32) -> i32 = kiln_typed_array_sort;
    };
    ($group:ident, $name:ident, $signature:ident) => {};
}

kiln_native_provider_contract::provider_abi_registry!(assert_root_provider_export);

#[path = "provider/date.rs"]
mod date;
use date::kiln_date_parse;

// Keep every optional provider variant link-compatible with the one private ABI. Generated C is
// deliberately program-independent in several places, so even a path removed by the linker can
// leave an undefined reference that must resolve before section collection runs. These refusals
// are the same scheme the Realm uses for an archive built without `realm-http`: capability
// selection proves the path unreachable, while a selector bug fails closed instead of jumping
// through a missing symbol.
#[cfg(not(feature = "regexp"))]
#[path = "provider/regexp_stubs.rs"]
mod regexp_stubs;

#[cfg(not(feature = "bigint"))]
#[path = "provider/bigint_stubs.rs"]
mod bigint_stubs;

#[cfg(not(feature = "temporal"))]
#[path = "provider/temporal_stubs.rs"]
mod temporal_stubs;

#[cfg(not(feature = "json"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_json_parse(
    _source: *const KilnString,
    _out_nodes: *mut *const core::ffi::c_void,
    _out_count: *mut usize,
    _out_text: *mut KilnString,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "intl"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_string_normalize(
    _units: *const u16,
    _count: u64,
    _form: u32,
    _out: *mut u16,
    _capacity: u64,
    _out_len: *mut u64,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "intl"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_intl_string_case(
    _units: *const u16,
    _count: u64,
    _locale: *const u8,
    _locale_len: u64,
    _uppercase: u32,
    _out: *mut u16,
    _capacity: u64,
    _out_len: *mut u64,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "intl"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_intl_collate(
    _left: *const u16,
    _left_len: u64,
    _right: *const u16,
    _right_len: u64,
    _spec: *const u8,
    _spec_len: u64,
    _out_order: *mut i32,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "intl"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_intl_segment(
    _bytes: *const u8,
    _len: usize,
    _granularity: u32,
    _out_records: *mut *const u32,
    _out_count: *mut usize,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "intl"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_intl_datetime_format(
    _spec: *const u8,
    _spec_len: u64,
    _epoch_milliseconds: f64,
    _operation: u32,
    _out: *mut *const u8,
    _out_len: *mut u64,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "parallel"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_parallel_map(
    _in: *const core::ffi::c_void,
    _count: usize,
    _out: *mut core::ffi::c_void,
    _chunk: Option<
        unsafe extern "C" fn(
            *const core::ffi::c_void,
            *mut core::ffi::c_void,
            usize,
            usize,
            *mut core::ffi::c_void,
        ),
    >,
    _context: *mut core::ffi::c_void,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "parallel"))]
/// Without the pool every call runs on the calling thread.
#[unsafe(no_mangle)]
pub extern "C" fn kiln_parallel_threads() -> usize {
    1
}

#[cfg(not(feature = "parallel"))]
/// # Safety
///
/// This disabled feature stub does not dereference its arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_parallel_sort(
    _data: *mut core::ffi::c_void,
    _count: usize,
    _kind: i32,
    _order: i32,
) -> i32 {
    INVALID_ARGUMENT
}

#[cfg(not(feature = "ffi"))]
#[path = "provider/ffi_stubs.rs"]
mod ffi_stubs;

#[cfg(feature = "regexp")]
#[path = "provider/regexp.rs"]
mod regexp;

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
#[path = "provider/bigint.rs"]
mod bigint;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
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

/// `Temporal`, over the same `kiln-temporal` adapter the interpreter links (ADR 0025, ADR 0029 §4).
///
/// The boundary keeps the discipline the regexp and BigInt providers set: the value is an opaque
/// handle, and nothing that is not a scalar crosses. That is cheaper here than anywhere else,
/// because `kiln-temporal` was already built with a scalar seam -- fields come back by *name* as
/// numbers, strings and flags, and options arrive as strings -- so one generic reader serves every
/// field of all eight types and the C side needs no per-field code.
///
/// The instant is always an argument, never read here. ADR 0015 makes the realm's clock virtual and
/// its zone UTC, and a provider that could reach the host's clock would be able to break that
/// invariant without anything failing to compile.
#[cfg(feature = "temporal")]
#[path = "provider/temporal.rs"]
mod temporal;

/// `JSON.parse`'s reader, as a tape (ADR 0025's shape: nothing but scalars and byte ranges cross).
///
/// The grammar lives in `kiln-json`, which the interpreter links directly; this archive links the
/// same crate, so a generated program and the interpreter cannot disagree about what parses. The
/// emitted C walks the tape once and does only what must touch its heap: make strings, arrays and
/// objects, and define properties.
///
/// There is no handle and nothing to free. The tape and text live in the crate's per-thread
/// scratch, which a parse overwrites in place, so a call hands back pointers into it that stay
/// valid until the next parse on the same thread. One parse is in flight per thread by
/// construction -- the C walk allocates but never parses -- and the crate asserts it.
#[cfg(feature = "json")]
#[path = "provider/json.rs"]
mod json;

/// ICU4X's Unicode segmentation engine behind a scalar-only provider seam.
///
/// The JavaScript-visible `Intl.Segmenter` object is compiled from TypeScript. This function does
/// only the part that belongs in Rust: turn UTF-8 text into UAX #29 boundary records. Each record
/// is four `u32`s: start and end UTF-16 indices, followed by an is-word-like presence bit and its
/// boolean value. The thread-local result remains valid until this thread calls again.
#[cfg(feature = "intl")]
#[path = "provider/intl.rs"]
mod intl;

/// `parallelMap`'s fork-join (ADR 0058, application 2).
///
/// The emitted C hands over the element and result arrays as opaque pointers, a chunk worker
/// over an index range, and a context pointer; Rust owns the thread pool, how the range is
/// split, and the floor below which splitting is not worth a wake-up. The elements are never
/// read here: they are the program's NaN-boxed values, and only the worker knows how to read
/// them, so nothing is copied or unboxed across the boundary and the worker runs against the
/// program's own arrays.
///
/// What a worker may do is decided in `kiln-backend-c` (`parallel.rs`): a body that reads its
/// `double` parameters and returns a `double`, reaching no host, heap or thread-local realm
/// state. That is what makes it safe to call on a thread that owns no realm.
#[cfg(feature = "parallel")]
#[path = "provider/parallel.rs"]
mod parallel;

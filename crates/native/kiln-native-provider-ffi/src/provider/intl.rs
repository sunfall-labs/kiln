use super::INVALID_ARGUMENT;
use kiln_intl::casemap::map_case_utf16;
use kiln_intl::segmenter::Segmenter;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    static RECORDS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

/// Segment one UTF-8 string. `granularity` is 0 for grapheme, 1 for word and 2 for sentence.
///
/// # Safety
///
/// `bytes` must be readable for `len` bytes (null is accepted only for an empty string), and
/// both output arguments must be writable slots. The returned pointer is borrowed until the
/// next call on this thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_intl_segment(
    bytes: *const u8,
    len: usize,
    granularity: u32,
    out_records: *mut *const u32,
    out_count: *mut usize,
) -> i32 {
    if (bytes.is_null() && len != 0) || out_records.is_null() || out_count.is_null() {
        return INVALID_ARGUMENT;
    }
    let bytes = if len == 0 {
        &[]
    } else {
        // SAFETY: the caller contract guarantees this readable span.
        unsafe { core::slice::from_raw_parts(bytes, len) }
    };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return INVALID_ARGUMENT;
    };
    let granularity = match granularity {
        0 => "grapheme",
        1 => "word",
        2 => "sentence",
        _ => return INVALID_ARGUMENT,
    };
    let segmented = catch_unwind(AssertUnwindSafe(|| {
        let segmenter = Segmenter::new(&["en-US".to_owned()], granularity)
            .expect("the provider passes a declared granularity");
        let units: Vec<u16> = text.encode_utf16().collect();
        segmenter.segments(&units)
    }));
    let Ok(segments) = segmented else {
        return -2;
    };
    RECORDS.with(|scratch| {
        let mut records = scratch.borrow_mut();
        records.clear();
        let Some(capacity) = segments.len().checked_mul(4) else {
            return -2;
        };
        records.reserve(capacity);
        for segment in segments {
            let (present, word_like) = match segment.word_like {
                None => (0, 0),
                Some(value) => (1, u32::from(value)),
            };
            let (Ok(start), Ok(end)) = (u32::try_from(segment.start), u32::try_from(segment.end))
            else {
                return -2;
            };
            records.extend_from_slice(&[start, end, present, word_like]);
        }
        // SAFETY: both are writable slots by the caller contract. The RefCell's vector owns
        // the returned storage after this borrow ends and until the next provider call.
        unsafe {
            out_records.write(records.as_ptr());
            out_count.write(records.len() / 4);
        }
        0
    })
}

/// Copy `answer` into `out`, reporting its length either way: the caller sizes a retry
/// from `out_len` when `capacity` was short, the same protocol as `kiln_string_case`.
unsafe fn spill_units(answer: &[u16], out: *mut u16, capacity: u64, out_len: *mut u64) -> i32 {
    unsafe { out_len.write(answer.len() as u64) };
    if capacity < answer.len() as u64 {
        return INVALID_ARGUMENT;
    }
    if !answer.is_empty() {
        if out.is_null() {
            return INVALID_ARGUMENT;
        }
        unsafe { std::ptr::copy_nonoverlapping(answer.as_ptr(), out, answer.len()) };
    }
    0
}

/// Locale-sensitive JavaScript string case mapping. Returns -3 for an invalid locale and -2
/// for a provider panic; the caller owns JavaScript error construction.
///
/// # Safety
/// `units`/`locale` must be readable for their lengths (null only when empty), while `out` and
/// `out_len` must satisfy the usual spill buffer contract.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_intl_string_case(
    units: *const u16,
    count: u64,
    locale: *const u8,
    locale_len: u64,
    uppercase: u32,
    out: *mut u16,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    if out_len.is_null() || (units.is_null() && count != 0) || (locale.is_null() && locale_len != 0) || uppercase > 1 {
        return INVALID_ARGUMENT;
    }
    let (Ok(count), Ok(locale_len)) = (usize::try_from(count), usize::try_from(locale_len)) else {
        return INVALID_ARGUMENT;
    };
    let input = if count == 0 { &[] } else { unsafe { core::slice::from_raw_parts(units, count) } };
    let tag = if locale_len == 0 { &[] } else { unsafe { core::slice::from_raw_parts(locale, locale_len) } };
    let Ok(tag) = core::str::from_utf8(tag) else { return INVALID_ARGUMENT; };
    let mapped = catch_unwind(AssertUnwindSafe(|| {
        map_case_utf16(input, if tag.is_empty() { None } else { Some(tag) }, uppercase != 0)
    }));
    match mapped {
        Ok(Ok(answer)) => unsafe { spill_units(&answer, out, capacity, out_len) },
        Ok(Err(error)) if error.kind == kiln_intl::ErrorKind::Range => -3,
        Ok(Err(_)) => INVALID_ARGUMENT,
        Err(_) => -2,
    }
}

/// `String.prototype.normalize`: `form` is 0 `NFC`, 1 `NFD`, 2 `NFKC`, 3 `NFKD`. The tables
/// are ICU4X's, shared with the interpreter through `kiln_intl::normalize`; the form is
/// validated by the caller, which owns the `RangeError`.
///
/// # Safety
///
/// `units` must be readable for `count` units (null only for an empty string), `out`
/// writable for `capacity` units, and `out_len` a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_string_normalize(
    units: *const u16,
    count: u64,
    form: u32,
    out: *mut u16,
    capacity: u64,
    out_len: *mut u64,
) -> i32 {
    if out_len.is_null() || (units.is_null() && count != 0) {
        return INVALID_ARGUMENT;
    }
    let Ok(count) = usize::try_from(count) else {
        return INVALID_ARGUMENT;
    };
    let form = match form {
        0 => "NFC",
        1 => "NFD",
        2 => "NFKC",
        3 => "NFKD",
        _ => return INVALID_ARGUMENT,
    };
    // SAFETY: the caller contract guarantees this readable span.
    let input = if count == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(units, count) }
    };
    let normalized = catch_unwind(AssertUnwindSafe(|| {
        kiln_intl::normalize::normalize(input, form)
    }));
    let Ok(Some(answer)) = normalized else {
        return -2;
    };
    // SAFETY: `out` and `out_len` are what the caller contract says they are.
    unsafe { spill_units(&answer, out, capacity, out_len) }
}

thread_local! {
    /// The collator the last `kiln_intl_collate` built, keyed by its spec. A sort calls
    /// `localeCompare` O(n log n) times with the same locale and options, and building an
    /// ICU4X collator per call is the whole cost.
    static COLLATOR: RefCell<Option<(Vec<u8>, kiln_intl::collator::Collator)>> =
        const { RefCell::new(None) };
}

/// The collator a `spec` names, or `None` for a locale or option value the collator
/// rejects.
///
/// The spec is seven UTF-8 fields separated by U+001F, each empty when the program did not
/// say: `locale`, `usage`, `sensitivity`, `numeric` (`true`/`false`), `caseFirst`,
/// `ignorePunctuation` (`true`/`false`), `collation`. A flat string rather than a struct
/// because it is also the cache key.
fn build_collator(spec: &str) -> Option<kiln_intl::collator::Collator> {
    let mut fields = spec.split('\u{1f}');
    let mut next = || fields.next().unwrap_or("").to_owned();
    let locale = next();
    let usage = next();
    let sensitivity = next();
    let numeric = next();
    let case_first = next();
    let ignore_punctuation = next();
    let collation = next();
    let flag = |text: String| match text.as_str() {
        "" => Some(None),
        "true" => Some(Some(true)),
        "false" => Some(Some(false)),
        _ => None,
    };
    let options = kiln_intl::collator::Options {
        usage: if usage.is_empty() {
            "sort".to_owned()
        } else {
            usage
        },
        collation: (!collation.is_empty()).then_some(collation),
        numeric: flag(numeric)?,
        case_first: (!case_first.is_empty()).then_some(case_first),
        sensitivity: (!sensitivity.is_empty()).then_some(sensitivity),
        ignore_punctuation: flag(ignore_punctuation)?,
    };
    // The constructor resolves a tag it was handed; the `RangeError` for a malformed one is
    // `CanonicalizeLocaleList`'s, which the interpreter runs first and this runs here.
    let requested: Vec<String> = if locale.is_empty() {
        Vec::new()
    } else {
        vec![kiln_intl::locale::canonicalize(&locale).ok()?]
    };
    if !kiln_intl::collator::USAGES.contains(&options.usage.as_str())
        || options
            .sensitivity
            .as_deref()
            .is_some_and(|s| !kiln_intl::collator::SENSITIVITIES.contains(&s))
        || options
            .case_first
            .as_deref()
            .is_some_and(|c| !kiln_intl::collator::CASE_FIRSTS.contains(&c))
    {
        return None;
    }
    kiln_intl::collator::Collator::new(&requested, options).ok()
}

/// `String.prototype.localeCompare` and `Intl.Collator.prototype.compare`: `out_order` is
/// -1, 0 or 1. Returns -1 for a spec the collator rejects (the caller's `RangeError`) and
/// -2 when the engine failed.
///
/// # Safety
///
/// `left`, `right` and `spec` must be readable for their lengths (null only when the
/// length is zero), and `out_order` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_intl_collate(
    left: *const u16,
    left_len: u64,
    right: *const u16,
    right_len: u64,
    spec: *const u8,
    spec_len: u64,
    out_order: *mut i32,
) -> i32 {
    if out_order.is_null()
        || (left.is_null() && left_len != 0)
        || (right.is_null() && right_len != 0)
        || (spec.is_null() && spec_len != 0)
    {
        return INVALID_ARGUMENT;
    }
    let (Ok(left_len), Ok(right_len), Ok(spec_len)) = (
        usize::try_from(left_len),
        usize::try_from(right_len),
        usize::try_from(spec_len),
    ) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: the caller contract guarantees these readable spans.
    let left = if left_len == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(left, left_len) }
    };
    let right = if right_len == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(right, right_len) }
    };
    let spec = if spec_len == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(spec, spec_len) }
    };
    let Ok(spec_text) = core::str::from_utf8(spec) else {
        return INVALID_ARGUMENT;
    };
    let compared = catch_unwind(AssertUnwindSafe(|| {
        COLLATOR.with(|slot| {
            let mut slot = slot.borrow_mut();
            let cached = matches!(slot.as_ref(), Some((key, _)) if key.as_slice() == spec);
            if !cached {
                let built = build_collator(spec_text)?;
                *slot = Some((spec.to_vec(), built));
            }
            let (_, collator) = slot.as_ref()?;
            Some(collator.compare(left, right))
        })
    }));
    match compared {
        Ok(Some(order)) => {
            let order = match order {
                core::cmp::Ordering::Less => -1,
                core::cmp::Ordering::Equal => 0,
                core::cmp::Ordering::Greater => 1,
            };
            // SAFETY: `out_order` is a writable slot by the caller contract.
            unsafe { out_order.write(order) };
            0
        }
        Ok(None) => -1,
        Err(_) => -2,
    }
}

macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (intl, kiln_intl_segment, intl_segment) => {
            const _: unsafe extern "C" fn(*const u8, usize, u32, *mut *const u32, *mut usize) -> i32 = kiln_intl_segment;
        };
        (intl, kiln_string_normalize, string_normalize) => {
            const _: unsafe extern "C" fn(*const u16, u64, u32, *mut u16, u64, *mut u64) -> i32 = kiln_string_normalize;
        };
        (intl, kiln_intl_collate, intl_collate) => {
            const _: unsafe extern "C" fn(*const u16, u64, *const u16, u64, *const u8, u64, *mut i32) -> i32 = kiln_intl_collate;
        };
        (intl, kiln_intl_string_case, intl_string_case) => {
            const _: unsafe extern "C" fn(*const u16, u64, *const u8, u64, u32, *mut u16, u64, *mut u64) -> i32 = kiln_intl_string_case;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_utf16_grapheme_boundaries() {
        let text = "e\u{301}x😀";
        let mut records = core::ptr::null();
        let mut count = 0;
        let status = unsafe {
            kiln_intl_segment(
                text.as_ptr(),
                text.len(),
                0,
                &raw mut records,
                &raw mut count,
            )
        };
        assert_eq!(status, 0);
        let records = unsafe { core::slice::from_raw_parts(records, count * 4) };
        assert_eq!(records, &[0, 2, 0, 0, 2, 3, 0, 0, 3, 5, 0, 0]);
    }

    #[test]
    fn normalizes_and_reports_the_length_when_short() {
        let ligature: Vec<u16> = "\u{fb01}".encode_utf16().collect();
        let mut out = [0u16; 1];
        let mut len = 0u64;
        let status = unsafe {
            kiln_string_normalize(ligature.as_ptr(), 1, 2, out.as_mut_ptr(), 1, &raw mut len)
        };
        assert_eq!((status, len), (INVALID_ARGUMENT, 2));
        let mut out = [0u16; 2];
        let status = unsafe {
            kiln_string_normalize(ligature.as_ptr(), 1, 2, out.as_mut_ptr(), 2, &raw mut len)
        };
        assert_eq!((status, len, out), (0, 2, [b'f' as u16, b'i' as u16]));
    }

    #[test]
    fn collates_by_spec() {
        let a: Vec<u16> = "a".encode_utf16().collect();
        let big_c: Vec<u16> = "C".encode_utf16().collect();
        let mut order = 9i32;
        let spec = "en\u{1f}\u{1f}base\u{1f}\u{1f}\u{1f}\u{1f}";
        let status = unsafe {
            kiln_intl_collate(
                a.as_ptr(),
                1,
                big_c.as_ptr(),
                1,
                spec.as_ptr(),
                spec.len() as u64,
                &raw mut order,
            )
        };
        assert_eq!((status, order), (0, -1));
        let bad = "not a tag!\u{1f}\u{1f}\u{1f}\u{1f}\u{1f}\u{1f}";
        let status = unsafe {
            kiln_intl_collate(
                a.as_ptr(),
                1,
                big_c.as_ptr(),
                1,
                bad.as_ptr(),
                bad.len() as u64,
                &raw mut order,
            )
        };
        assert_eq!(status, -1);
    }
}

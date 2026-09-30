use super::INVALID_ARGUMENT;

/// A compiled pattern, and the last match it produced.
///
/// The match is kept because a capture group's *name* is only reachable through one -- `regress`
/// clones the names into each match rather than exposing them on the pattern. Keeping it also
/// means the C side reads a result with two calls instead of marshalling a variable-length
/// structure across the boundary.
pub struct KilnRegex {
    compiled: regress::Regex,
    /// The `u`/`v` flag: code-point matching rather than code-unit matching.
    unicode: bool,
    last: Option<regress::Match>,
    latin1_subject: Vec<u16>,
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
    pattern: *const super::KilnString,
    flags: *const super::KilnString,
    out_handle: *mut *mut KilnRegex,
) -> i32 {
    if out_handle.is_null() {
        return INVALID_ARGUMENT;
    }
    // Compile patterns through one scalar string conversion; matching itself stays on the
    // regress UTF-16 API so subject offsets remain JavaScript unit offsets.
    let (Some(pattern), Some(flags)) = (unsafe { super::validate_string(pattern) }, unsafe {
        super::validate_string(flags)
    }) else {
        return INVALID_ARGUMENT;
    };
    let Some(flags) = view_to_flags(flags) else {
        return INVALID_ARGUMENT;
    };
    let unicode = flags.contains('u') || flags.contains('v');
    // Without `u` the pattern is code units like the subject: an astral literal is a pair
    // of surrogates that match a pair in the input one unit at a time. Only under `u` does
    // a surrogate pair in the pattern mean the one code point.
    let pattern = if unicode {
        view_to_unicode(pattern)
    } else {
        view_to_units(pattern)
    };
    let Some(pattern) = pattern else {
        return INVALID_ARGUMENT;
    };
    let Ok(compiled) = regress::Regex::from_unicode(pattern.into_iter(), flags.as_str()) else {
        return INVALID_ARGUMENT;
    };
    let handle = alloc::boxed::Box::new(KilnRegex {
        compiled,
        unicode,
        last: None,
        latin1_subject: Vec::new(),
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
    subject: *const super::KilnString,
    start: usize,
    out_ranges: *mut i64,
    out_capacity: usize,
) -> i32 {
    if handle.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live and uniquely borrowed here.
    let state = unsafe { &mut *handle };
    let Some(subject) = (unsafe { super::validate_string(subject) }) else {
        return INVALID_ARGUMENT;
    };
    let units: &[u16] = match subject {
        super::StringView::Utf16(units) => units,
        super::StringView::Latin1(bytes) => {
            state.latin1_subject.clear();
            state
                .latin1_subject
                .extend(bytes.iter().map(|&byte| u16::from(byte)));
            &state.latin1_subject
        }
    };
    if start > units.len() {
        state.last = None;
        return 0;
    }
    // Without `u` the language matches code units, so a surrogate pair is two
    // characters (`/^.$/.test("\u{1F600}")` is false); with it, one code point.
    let found = if state.unicode {
        state.compiled.find_from_utf16(units, start).next()
    } else {
        state.compiled.find_from_ucs2(units, start).next()
    };
    let Some(found) = found else {
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

/// The number of distinct named captures in the last match.
///
/// # Safety
///
/// `handle` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_named_group_count(handle: *const KilnRegex) -> usize {
    if handle.is_null() {
        return 0;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    let state = unsafe { &*handle };
    state
        .last
        .as_ref()
        .map_or(0, |found| found.named_groups().count())
}

/// Read a named capture by its ordinal among distinct names, keeping the capture's actual value
/// with its name even when unnamed groups precede it. Unmatched named groups write (-1, -1).
///
/// Returns the UTF-8 name length, or -1 for invalid arguments or an unknown index.
///
/// # Safety
///
/// `handle` must be live; `buffer` must be writable for `capacity` bytes; `from`, `to` and
/// `capture` must be writable slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_regexp_named_group_at(
    handle: *const KilnRegex,
    index: usize,
    buffer: *mut u8,
    capacity: usize,
    from: *mut i64,
    to: *mut i64,
    capture: *mut usize,
) -> i32 {
    if handle.is_null() || buffer.is_null() || from.is_null() || to.is_null() || capture.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: the caller contract guarantees the handle is live.
    let state = unsafe { &*handle };
    let Some(found) = state.last.as_ref() else {
        return INVALID_ARGUMENT;
    };
    let Some((name, range)) = found.named_groups().nth(index) else {
        return INVALID_ARGUMENT;
    };
    let bytes = name.as_bytes();
    let Ok(length) = i32::try_from(bytes.len()) else {
        return INVALID_ARGUMENT;
    };
    if bytes.len() > capacity {
        return INVALID_ARGUMENT;
    }
    // SAFETY: `buffer` and the output slots are writable by the caller contract, and the source
    // name is disjoint from the caller's buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
        match range {
            Some(range) => {
                from.write(as_offset(range.start));
                to.write(as_offset(range.end));
                let capture_index = found
                    .captures
                    .iter()
                    .position(|candidate| candidate.as_ref() == Some(&range))
                    .and_then(|index| index.checked_add(1));
                let Some(capture_index) = capture_index else {
                    return INVALID_ARGUMENT;
                };
                capture.write(capture_index);
            }
            None => {
                from.write(-1);
                to.write(-1);
                capture.write(0);
            }
        }
    }
    length
}

fn view_to_unicode(view: super::StringView<'_>) -> Option<Vec<u32>> {
    match view {
        super::StringView::Latin1(bytes) => {
            Some(bytes.iter().map(|&byte| u32::from(byte)).collect())
        }
        super::StringView::Utf16(units) => Some(
            char::decode_utf16(units.iter().copied())
                .map(|result| {
                    result
                        .map(u32::from)
                        .unwrap_or_else(|error| u32::from(error.unpaired_surrogate()))
                })
                .collect(),
        ),
    }
}

/// Every UTF-16 unit as its own value, surrogates included.
fn view_to_units(view: super::StringView<'_>) -> Option<Vec<u32>> {
    match view {
        super::StringView::Latin1(bytes) => {
            Some(bytes.iter().map(|&byte| u32::from(byte)).collect())
        }
        super::StringView::Utf16(units) => {
            Some(units.iter().map(|&unit| u32::from(unit)).collect())
        }
    }
}

fn view_to_flags(view: super::StringView<'_>) -> Option<String> {
    match view {
        super::StringView::Latin1(bytes) => bytes
            .iter()
            .all(|&byte| byte.is_ascii())
            .then(|| String::from_utf8(bytes.to_vec()).ok())
            .flatten(),
        super::StringView::Utf16(units) => units
            .iter()
            .copied()
            .map(u8::try_from)
            .collect::<Result<Vec<_>, _>>()
            .ok()
            .filter(|bytes| bytes.iter().all(u8::is_ascii))
            .and_then(|bytes| String::from_utf8(bytes).ok()),
    }
}

/// An offset that fits the boundary type, saturating rather than wrapping.
fn as_offset(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (regexp, kiln_regexp_compile, regexp_compile) => {
            const _: unsafe extern "C" fn(*const super::KilnString, *const super::KilnString, *mut *mut KilnRegex) -> i32 = kiln_regexp_compile;
        };
        (regexp, kiln_regexp_free, regexp_free) => {
            const _: unsafe extern "C" fn(*mut KilnRegex) = kiln_regexp_free;
        };
        (regexp, kiln_regexp_search, regexp_search) => {
            const _: unsafe extern "C" fn(*mut KilnRegex, *const super::KilnString, usize, *mut i64, usize) -> i32 = kiln_regexp_search;
        };
        (regexp, kiln_regexp_group_count, regexp_group_count) => {
            const _: unsafe extern "C" fn(*const KilnRegex) -> usize = kiln_regexp_group_count;
        };
        (regexp, kiln_regexp_group_name, regexp_group_name) => {
            const _: unsafe extern "C" fn(*const KilnRegex, usize, *mut u8, usize) -> i32 = kiln_regexp_group_name;
        };
        (regexp, kiln_regexp_named_group_count, regexp_named_group_count) => {
            const _: unsafe extern "C" fn(*const KilnRegex) -> usize = kiln_regexp_named_group_count;
        };
        (regexp, kiln_regexp_named_group_at, regexp_named_group_at) => {
            const _: unsafe extern "C" fn(*const KilnRegex, usize, *mut u8, usize, *mut i64, *mut i64, *mut usize) -> i32 = kiln_regexp_named_group_at;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

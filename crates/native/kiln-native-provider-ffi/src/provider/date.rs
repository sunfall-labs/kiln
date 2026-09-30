use super::KilnString;

/// Date string parsing shared with the interpreter. Malformed UTF-16 cannot match any accepted
/// grammar and therefore yields the invalid-date NaN, as required by `Date.parse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_date_parse(source: *const KilnString) -> f64 {
    if source.is_null() {
        return f64::NAN;
    }
    // SAFETY: the caller owns the header and its code-unit buffer for the duration of this call.
    let source = unsafe { &*source };
    let length = source.unit_len as usize;
    if length == 0 {
        return kiln_date_core::parse_date("");
    }
    let mut text = String::new();
    match source.flags {
        0 => {
            if length > isize::MAX as usize || source.data.is_null() {
                return f64::NAN;
            }
            // SAFETY: the ABI contract defines a Latin1 buffer with `unit_len` bytes.
            let bytes = unsafe { core::slice::from_raw_parts(source.data.cast::<u8>(), length) };
            text.reserve(length);
            for byte in bytes {
                text.push(char::from(*byte));
            }
        }
        1 => {
            if length > isize::MAX as usize / 2
                || source.data.is_null()
                || (source.data as usize & 1) != 0
            {
                return f64::NAN;
            }
            // SAFETY: the ABI contract defines an aligned UTF-16 buffer with `unit_len` units.
            let units = unsafe { core::slice::from_raw_parts(source.data.cast::<u16>(), length) };
            text.reserve(length);
            for scalar in char::decode_utf16(units.iter().copied()) {
                let Ok(scalar) = scalar else {
                    return f64::NAN;
                };
                text.push(scalar);
            }
        }
        _ => return f64::NAN,
    }
    kiln_date_core::parse_date(&text)
}

//! Bun-compatible, checker-scoped support primitives.
//!
//! Byte-oriented helpers intentionally operate on source spans. The checker owns source bytes;
//! JavaScript UTF-16 string values remain owned by `kiln-string`.

pub mod strings {
    #[inline]
    pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        index_of(haystack, needle).is_some()
    }
    #[inline]
    pub fn index_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            Some(0)
        } else {
            memchr::memmem::find(haystack, needle)
        }
    }
    #[inline]
    pub fn last_index_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            Some(haystack.len())
        } else {
            memchr::memmem::rfind(haystack, needle)
        }
    }
    #[inline]
    pub fn index_of_char(haystack: &[u8], needle: u8) -> Option<u32> {
        memchr::memchr(needle, haystack).map(|i| i as u32)
    }
    #[inline]
    pub fn index_of_char_usize(haystack: &[u8], needle: u8) -> Option<usize> {
        memchr::memchr(needle, haystack)
    }
    #[inline]
    pub fn last_index_of_char(haystack: &[u8], needle: u8) -> Option<usize> {
        memchr::memrchr(needle, haystack)
    }
    #[inline]
    pub fn contains_char(haystack: &[u8], needle: u8) -> bool {
        index_of_char(haystack, needle).is_some()
    }
    #[inline]
    pub fn count_char(haystack: &[u8], needle: u8) -> usize {
        memchr::memchr_iter(needle, haystack).count()
    }
    pub fn index_of_any(haystack: &[u8], chars: &[u8]) -> Option<usize> {
        match chars {
            [] => None,
            [a] => memchr::memchr(*a, haystack),
            [a, b] => memchr::memchr2(*a, *b, haystack),
            [a, b, c] => memchr::memchr3(*a, *b, *c, haystack),
            _ => chars
                .iter()
                .filter_map(|b| memchr::memchr(*b, haystack))
                .min(),
        }
    }
    pub fn last_index_of_any(haystack: &[u8], chars: &[u8]) -> Option<usize> {
        match chars {
            [] => None,
            [a] => memchr::memrchr(*a, haystack),
            [a, b] => memchr::memrchr2(*a, *b, haystack),
            [a, b, c] => memchr::memrchr3(*a, *b, *c, haystack),
            _ => chars
                .iter()
                .filter_map(|b| memchr::memrchr(*b, haystack))
                .max(),
        }
    }
    pub fn split<'a>(
        input: &'a [u8],
        delimiter: &'a [u8],
    ) -> impl Iterator<Item = &'a [u8]> + Clone {
        assert!(!delimiter.is_empty(), "split delimiter must not be empty");
        Split {
            rest: Some(input),
            delimiter,
        }
    }
    #[derive(Clone)]
    struct Split<'a> {
        rest: Option<&'a [u8]>,
        delimiter: &'a [u8],
    }
    impl<'a> Iterator for Split<'a> {
        type Item = &'a [u8];
        fn next(&mut self) -> Option<Self::Item> {
            let rest = self.rest?;
            if let Some(i) = index_of(rest, self.delimiter) {
                self.rest = Some(&rest[i + self.delimiter.len()..]);
                Some(&rest[..i])
            } else {
                self.rest = None;
                Some(rest)
            }
        }
    }
    pub fn split_once<'a>(input: &'a [u8], delimiter: &[u8]) -> Option<(&'a [u8], &'a [u8])> {
        if delimiter.is_empty() {
            return None;
        }
        let at = index_of(input, delimiter)?;
        Some((&input[..at], &input[at + delimiter.len()..]))
    }
    pub fn split_once_char(input: &[u8], delimiter: u8) -> Option<(&[u8], &[u8])> {
        let at = index_of_char_usize(input, delimiter)?;
        Some((&input[..at], &input[at + 1..]))
    }
    #[derive(Clone)]
    pub struct SplitAny<'a> {
        input: &'a [u8],
        chars: &'a [u8],
        next: Option<usize>,
    }
    impl<'a> Iterator for SplitAny<'a> {
        type Item = &'a [u8];
        fn next(&mut self) -> Option<Self::Item> {
            let start = self.next?;
            if let Some(offset) = self.input[start..]
                .iter()
                .position(|b| self.chars.contains(b))
            {
                let end = start + offset;
                self.next = Some(end + 1);
                Some(&self.input[start..end])
            } else {
                self.next = None;
                Some(&self.input[start..])
            }
        }
    }
    pub fn split_any<'a>(input: &'a [u8], chars: &'a [u8]) -> SplitAny<'a> {
        SplitAny {
            input,
            chars,
            next: Some(0),
        }
    }
    pub fn tokenize_any<'a>(
        input: &'a [u8],
        chars: &'a [u8],
    ) -> impl Iterator<Item = &'a [u8]> + Clone {
        split_any(input, chars).filter(|part| !part.is_empty())
    }
    pub fn copy_lowercase_if_needed<'a>(input: &'a [u8], out: &'a mut [u8]) -> &'a [u8] {
        if !input.iter().any(u8::is_ascii_uppercase) {
            return input;
        }
        for (dst, &src) in out.iter_mut().zip(input) {
            *dst = src.to_ascii_lowercase();
        }
        &out[..input.len()]
    }
    pub fn eql_case_insensitive_ascii_check_length(a: &[u8], b: &[u8]) -> bool {
        a.len() == b.len() && a.eq_ignore_ascii_case(b)
    }
    pub fn without_trailing_slash(path: &[u8]) -> &[u8] {
        let mut end = path.len();
        while end > 1 && matches!(path[end - 1], b'/' | b'\\') {
            end -= 1;
        }
        &path[..end]
    }
    pub fn replace_owned(input: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
        if needle.is_empty() {
            return input.to_vec();
        }
        let mut out = Vec::with_capacity(input.len());
        let mut rest = input;
        while let Some(i) = index_of(rest, needle) {
            out.extend_from_slice(&rest[..i]);
            out.extend_from_slice(replacement);
            rest = &rest[i + needle.len()..];
        }
        out.extend_from_slice(rest);
        out
    }
    pub fn without_utf8_bom(input: &[u8]) -> &[u8] {
        input.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(input)
    }
    pub fn wtf8_byte_sequence_length(first: u8) -> usize {
        match first {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => 1,
        }
    }
    #[derive(Default, Clone, Copy)]
    pub struct Cursor {
        pub i: u32,
        pub width: u8,
        pub c: i32,
    }
    pub struct CodepointIterator<'a> {
        bytes: &'a [u8],
    }
    impl<'a> CodepointIterator<'a> {
        pub const fn init(bytes: &'a [u8]) -> Self {
            Self { bytes }
        }
        #[inline]
        pub fn next(&self, cursor: &mut Cursor) -> bool {
            let pos = (cursor.i as usize).saturating_add(cursor.width as usize);
            let Some(&first) = self.bytes.get(pos) else {
                return false;
            };
            cursor.i = pos as u32;
            if first < 0x80 {
                cursor.c = i32::from(first);
                cursor.width = 1;
                return true;
            }
            match decode_wtf8(&self.bytes[pos..]) {
                Some((cp, width)) => {
                    cursor.c = cp as i32;
                    cursor.width = width as u8;
                }
                None => {
                    cursor.c = 0xfffd;
                    cursor.width = 1;
                }
            }
            true
        }
    }
    /// Decode one valid UTF-8 or WTF-8 sequence, including a three-byte lone surrogate.
    pub fn decode_wtf8(bytes: &[u8]) -> Option<(u32, usize)> {
        let first = *bytes.first()?;
        if first < 0x80 {
            return Some((u32::from(first), 1));
        }
        let continuation = |i: usize| {
            bytes
                .get(i)
                .copied()
                .filter(|b| b & 0xc0 == 0x80)
                .map(|b| u32::from(b & 0x3f))
        };
        match first {
            0xc2..=0xdf => {
                let b1 = continuation(1)?;
                Some(((((first & 0x1f) as u32) << 6) | b1, 2))
            }
            0xe0..=0xef => {
                let raw1 = *bytes.get(1)?;
                let b1 = continuation(1)?;
                let b2 = continuation(2)?;
                if (first == 0xe0 && raw1 < 0xa0) || (first == 0xed && raw1 < 0x80) {
                    return None;
                }
                Some(((((first & 0x0f) as u32) << 12) | (b1 << 6) | b2, 3))
            }
            0xf0..=0xf4 => {
                let raw1 = *bytes.get(1)?;
                let b1 = continuation(1)?;
                let b2 = continuation(2)?;
                let b3 = continuation(3)?;
                if (first == 0xf0 && raw1 < 0x90) || (first == 0xf4 && raw1 > 0x8f) {
                    return None;
                }
                Some((
                    (((first & 7) as u32) << 18) | (b1 << 12) | (b2 << 6) | b3,
                    4,
                ))
            }
            _ => None,
        }
    }
}

pub mod identifier;
pub mod lexer {
    pub type CodePoint = i32;
    pub fn char_and_size(bytes: &[u8], at: usize) -> (CodePoint, usize) {
        let Some(&first) = bytes.get(at) else {
            return (-1, 0);
        };
        if first < 0x80 {
            return (i32::from(first), 1);
        }
        match super::strings::decode_wtf8(&bytes[at..]) {
            Some((cp, width)) => (cp as i32, width),
            None => (0xfffd, 1),
        }
    }
    pub fn peek_unicode_escape(bytes: &[u8], at: usize) -> Option<(CodePoint, usize)> {
        let rest = bytes.get(at..)?;
        if rest.get(0) != Some(&b'\\') || rest.get(1) != Some(&b'u') {
            return None;
        }
        if rest.get(2) == Some(&b'{') {
            let end = rest.get(3..)?.iter().position(|b| *b == b'}')? + 3;
            let digits = rest.get(3..end)?;
            if digits.is_empty() || digits.len() > 6 {
                return None;
            }
            let value = u32::from_str_radix(core::str::from_utf8(digits).ok()?, 16).ok()?;
            (value <= 0x10ffff).then_some((value as i32, end + 1))
        } else {
            let digits = rest.get(2..6)?;
            let value = u32::from_str_radix(core::str::from_utf8(digits).ok()?, 16).ok()?;
            Some((value as i32, 6))
        }
    }
    pub use super::identifier::is_identifier;
    pub fn is_identifier_start(codepoint: u32) -> bool {
        codepoint <= 0x10ffff && super::identifier::is_identifier_start(codepoint as i32)
    }
    pub fn is_identifier_part(codepoint: u32) -> bool {
        codepoint <= 0x10ffff && super::identifier::is_identifier_part(codepoint as i32)
    }
    pub fn is_white_space_single_line(cp: CodePoint) -> bool {
        matches!(cp, 9 | 11 | 12 | 32 | 0xfeff | 0x85 | 0x200b)
            || matches!(
                cp,
                0xa0 | 0x1680 | 0x2000..=0x200a | 0x202f | 0x205f | 0x3000
            )
    }
    pub fn last_char(text: &[u8]) -> (CodePoint, usize) {
        let start = text.iter().rposition(|b| b & 0xc0 != 0x80).unwrap_or(0);
        match char_and_size(text, start) {
            (cp, size) if start + size == text.len() => (cp, start),
            _ => (0xfffd, text.len().saturating_sub(1)),
        }
    }
    pub fn end_of_run(text: &[u8], mut at: usize, pred: impl Fn(CodePoint) -> bool) -> usize {
        loop {
            let (cp, len) = char_and_size(text, at);
            if len == 0 || !pred(cp) {
                return at;
            }
            at += len;
        }
    }
    pub fn starts_with_line_break(text: &[u8]) -> bool {
        matches!(text.first(), Some(b'\n' | b'\r'))
            || text.starts_with(b"\xe2\x80\xa8")
            || text.starts_with(b"\xe2\x80\xa9")
    }
    pub fn is_type_script_identifier_part(cp: CodePoint) -> bool {
        let cp = cp as u32;
        is_identifier_part(cp) || matches!(cp, 0x30fb | 0xff65)
    }
    pub fn scan_identifier_parts(text: &[u8], mut at: usize) -> usize {
        loop {
            at = end_of_run(text, at, is_type_script_identifier_part);
            if text.get(at) != Some(&b'\\') {
                return at;
            }
            match peek_unicode_escape(text, at) {
                Some((cp, len)) if is_type_script_identifier_part(cp) => at += len,
                _ => return at,
            }
        }
    }
}

pub mod fmt {
    use std::fmt::Write;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ParseIntError {
        InvalidCharacter,
        Overflow,
    }

    pub fn parse_f64(bytes: &[u8]) -> Option<f64> {
        if bytes.is_empty() || !bytes.is_ascii() {
            return None;
        }
        if bytes.trim_ascii() != bytes {
            return None;
        }
        let text = core::str::from_utf8(bytes).ok()?;
        let (negative, rest) = text.strip_prefix('-').map_or((false, text), |r| (true, r));
        let (negative, rest) = rest
            .strip_prefix('+')
            .map_or((negative, rest), |r| (false, r));
        match rest.to_ascii_lowercase().as_str() {
            "inf" | "infinity" => Some(if negative {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }),
            "nan" => Some(f64::NAN),
            _ => {
                // Bun's helper accepts a complete decimal spelling. Reject Rust/ECMAScript
                // whitespace trimming and non-decimal prefixes before using Kiln's shared
                // correctly-rounded decimal conversion.
                if text.parse::<f64>().is_err() {
                    return None;
                }
                let n = kiln_number_core::string_to_number(bytes);
                if n.is_nan() { None } else { Some(n) }
            }
        }
    }

    pub fn parse_unsigned<T>(bytes: &[u8], radix: u8) -> Result<T, ParseIntError>
    where
        T: TryFrom<i128> + TryFrom<u128>,
    {
        if bytes.is_empty() || !(2..=36).contains(&radix) {
            return Err(ParseIntError::InvalidCharacter);
        }
        let mut value = 0u128;
        for &b in bytes {
            let digit = match b {
                b'0'..=b'9' => u32::from(b - b'0'),
                b'a'..=b'z' => u32::from(b - b'a') + 10,
                b'A'..=b'Z' => u32::from(b - b'A') + 10,
                _ => return Err(ParseIntError::InvalidCharacter),
            };
            if digit >= u32::from(radix) {
                return Err(ParseIntError::InvalidCharacter);
            }
            value = value
                .checked_mul(u128::from(radix))
                .and_then(|v| v.checked_add(u128::from(digit)))
                .ok_or(ParseIntError::Overflow)?;
        }
        T::try_from(value).map_err(|_| ParseIntError::Overflow)
    }

    pub struct FormatDouble;
    impl FormatDouble {
        pub fn dtoa(buffer: &mut [u8], value: f64) -> &[u8] {
            format_double(buffer, value, false)
        }
        pub fn dtoa_with_negative_zero(buffer: &mut [u8], value: f64) -> &[u8] {
            format_double(buffer, value, true)
        }
    }
    fn format_double(buffer: &mut [u8], value: f64, negative_zero: bool) -> &[u8] {
        if value == 0.0 && value.is_sign_negative() && negative_zero {
            buffer[..2].copy_from_slice(b"-0");
            return &buffer[..2];
        }
        let len = kiln_number_core::format_number(value, buffer)
            .unwrap_or_else(|_| panic!("number buffer must fit ECMAScript output"));
        &buffer[..len]
    }
    pub struct ItoaBuf {
        bytes: [u8; 40],
    }
    impl ItoaBuf {
        pub const fn new() -> Self {
            Self { bytes: [0; 40] }
        }
    }
    pub fn itoa<T: core::fmt::Display>(buf: &mut ItoaBuf, value: T) -> &[u8] {
        let mut temp = String::new();
        write!(&mut temp, "{value}").expect("String formatting cannot fail");
        let n = temp.len().min(buf.bytes.len());
        buf.bytes[..n].copy_from_slice(&temp.as_bytes()[..n]);
        &buf.bytes[..n]
    }
    pub fn digit_count(mut value: usize) -> usize {
        let mut n = 1;
        while value >= 10 {
            value /= 10;
            n += 1;
        }
        n
    }
    pub struct VecWriter<'a>(pub &'a mut Vec<u8>);
    impl Write for VecWriter<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            self.0.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }
}

/// Static, allocation-free byte-key table used by the checker.
pub trait ComptimeStringKey: Copy {
    fn bytes(self) -> &'static [u8];
}
impl<const N: usize> ComptimeStringKey for &'static [u8; N] {
    fn bytes(self) -> &'static [u8] {
        self
    }
}
impl ComptimeStringKey for &'static str {
    fn bytes(self) -> &'static [u8] {
        self.as_bytes()
    }
}

pub struct ComptimeStringMap<T, const N: usize> {
    entries: [(&'static [u8], T); N],
}
impl<T, const N: usize> ComptimeStringMap<T, N> {
    pub const fn new(entries: [(&'static [u8], T); N]) -> Self {
        Self { entries }
    }
    pub fn get(&self, key: &[u8]) -> Option<&T> {
        self.entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }
    pub fn contains_key(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }
    pub fn entries(&self) -> impl Iterator<Item = (&'static [u8], &T)> {
        self.entries.iter().map(|(k, v)| (*k, v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &'static [u8]> + '_ {
        self.entries.iter().map(|(k, _)| *k)
    }
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().map(|(_, v)| v)
    }
    pub fn get_with_eql(
        &self,
        key: &[u8],
        mut eql: impl FnMut(&[u8], &[u8]) -> bool,
    ) -> Option<&T> {
        self.entries
            .iter()
            .find(|(k, _)| eql(k, key))
            .map(|(_, v)| v)
    }
    pub fn get_ascii_case_insensitive(&self, key: &[u8]) -> Option<&T> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }
}
pub struct ComptimeStringSet<const N: usize> {
    keys: [&'static [u8]; N],
}
impl<const N: usize> ComptimeStringSet<N> {
    pub const fn new(keys: [&'static [u8]; N]) -> Self {
        Self { keys }
    }
    pub fn contains(&self, key: &[u8]) -> bool {
        self.keys.contains(&key)
    }
    pub fn iter(&self) -> impl Iterator<Item = &'static [u8]> + Clone {
        self.keys.iter().copied()
    }
    pub fn keys(&self) -> impl Iterator<Item = &'static [u8]> + Clone {
        self.iter()
    }
    pub const fn len(&self) -> usize {
        N
    }
    pub const fn is_empty(&self) -> bool {
        N == 0
    }
}

#[macro_export]
macro_rules! comptime_string_map { ($(#[$attr:meta])* $vis:vis static $name:ident : $ty:ty = { $($key:expr => $value:expr),* $(,)? };) => { $(#[$attr])* $vis static $name: $crate::support::core::ComptimeStringMap<$ty,{0 $(+ { let _ = stringify!($key); 1 })*}> = $crate::support::core::ComptimeStringMap::new([$(( $key, $value )),*]); }; }
#[macro_export]
macro_rules! comptime_string_set { ($(#[$attr:meta])* $vis:vis static $name:ident = { $($key:expr),* $(,)? };) => { $(#[$attr])* $vis static $name: std::sync::LazyLock<$crate::support::core::ComptimeStringSet<{0 $(+ { let _ = stringify!($key); 1 })*}>> = std::sync::LazyLock::new(|| $crate::support::core::ComptimeStringSet::new([$($crate::support::core::ComptimeStringKey::bytes($key)),*])); }; }
pub use crate::{comptime_string_map, comptime_string_set};

#[derive(Clone, Copy)]
pub struct StackCheck {
    stack_limit: usize,
}
impl Default for StackCheck {
    fn default() -> Self {
        Self { stack_limit: 0 }
    }
}
impl StackCheck {
    const THRESHOLD: usize = 128 * 1024;
    pub fn init() -> Self {
        Self {
            stack_limit: stack_limit(),
        }
    }
    pub fn configure_thread() {}
    pub fn is_safe_to_recurse(&self) -> bool {
        stack_pointer().saturating_sub(self.stack_limit) > Self::THRESHOLD
    }
    pub fn remaining(&self) -> usize {
        stack_pointer()
            .saturating_sub(self.stack_limit)
            .saturating_sub(Self::THRESHOLD)
    }
}
#[inline(always)]
fn stack_pointer() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        let p: usize;
        unsafe {
            core::arch::asm!("mov {}, rsp", out(reg) p, options(nomem, nostack, preserves_flags));
        }
        p
    }
    #[cfg(target_arch = "aarch64")]
    {
        let p: usize;
        unsafe {
            core::arch::asm!("mov {}, sp", out(reg) p, options(nomem, nostack, preserves_flags));
        }
        p
    }
    #[cfg(target_arch = "x86")]
    {
        let p: usize;
        unsafe {
            core::arch::asm!("mov {}, esp", out(reg) p, options(nomem, nostack, preserves_flags));
        }
        p
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86")))]
    {
        let local = 0u8;
        (&local as *const u8) as usize
    }
}
#[cfg(any(target_os = "linux", target_os = "android"))]
fn stack_limit() -> usize {
    // SAFETY: pthread attribute APIs initialize/read the provided attr storage and write valid
    // stack bounds for the calling thread. Every initialized attr is destroyed below.
    unsafe {
        let mut attr = core::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
        if libc::pthread_getattr_np(libc::pthread_self(), attr.as_mut_ptr()) != 0 {
            return fallback_stack_limit();
        }
        let mut base = core::ptr::null_mut();
        let mut size = 0usize;
        let result = libc::pthread_attr_getstack(attr.as_ptr(), &mut base, &mut size);
        let _ = libc::pthread_attr_destroy(attr.as_mut_ptr());
        if result == 0 {
            base as usize
        } else {
            fallback_stack_limit()
        }
    }
}
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn stack_limit() -> usize {
    // SAFETY: pthread_get_stackaddr_np and pthread_get_stacksize_np inspect the current live thread.
    unsafe {
        let high = libc::pthread_get_stackaddr_np(libc::pthread_self()) as usize;
        let size = libc::pthread_get_stacksize_np(libc::pthread_self());
        let low = high.saturating_sub(size);
        if low == 0 {
            fallback_stack_limit()
        } else {
            low
        }
    }
}
#[cfg(windows)]
fn stack_limit() -> usize {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }
    let mut low = 0;
    let mut high = 0;
    // SAFETY: both out-pointers refer to initialized writable usize values.
    unsafe {
        GetCurrentThreadStackLimits(&mut low, &mut high);
    }
    let _ = high;
    if low == 0 {
        fallback_stack_limit()
    } else {
        low
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios", windows)))]
fn stack_limit() -> usize {
    fallback_stack_limit()
}
fn fallback_stack_limit() -> usize {
    stack_pointer().saturating_sub(256 * 1024)
}

#[macro_export]
macro_rules! declare_scope {
    ($name:ident,$visibility:ident) => {
        #[allow(non_camel_case_types)]
        pub struct $name;
    };
}
#[macro_export]
macro_rules! scoped_log { ($scope:ty,$($arg:tt)*) => {{ if std::env::var_os("KILN_SEMA_LOG").is_some() { eprintln!($($arg)*); } }}; }
pub use crate::{declare_scope, scoped_log};
